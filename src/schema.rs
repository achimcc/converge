//! Compares this program's wire types with a service's OpenAPI description.
//! It checks names and types. It cannot check behaviour: Radarr's file lists
//! only 200 for the quality update, and the service answers 202.

use serde_json::Value;

use crate::endpoint::{Endpoint, Shape};

const COMPONENTS: &str = "#/components/schemas/";

/// Every finding is one sentence; an empty result means everything matched.
pub fn check(openapi: &Value, endpoints: &[Endpoint], wire: &[schemars::Schema]) -> Vec<String> {
    let mut findings = Vec::new();
    for ep in endpoints {
        let pointer = format!(
            "/paths/{}/{}",
            escape(ep.path),
            ep.method.to_ascii_lowercase()
        );
        let Some(operation) = openapi.pointer(&pointer) else {
            findings.push(format!("{} {} does not exist", ep.method, ep.path));
            continue;
        };
        if let Some(shape) = ep.request {
            let schema = operation.pointer("/requestBody/content/application~1json/schema");
            check_body(
                &mut findings,
                ep,
                "request body",
                schema,
                shape,
                openapi,
                wire,
            );
        }
        if let Some(shape) = ep.response {
            let schema = operation.pointer("/responses/200/content/application~1json/schema");
            check_body(
                &mut findings,
                ep,
                "200 response",
                schema,
                shape,
                openapi,
                wire,
            );
        }
    }
    // A component shared by two endpoints (the list GET and the update PUT)
    // is compared twice; report each finding once, in first-seen order.
    let mut seen = std::collections::HashSet::new();
    findings.retain(|f| seen.insert(f.clone()));
    findings
}

/// JSON pointer escaping (RFC 6901).
fn escape(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

fn check_body(
    findings: &mut Vec<String>,
    ep: &Endpoint,
    what: &str,
    schema: Option<&Value>,
    shape: Shape,
    openapi: &Value,
    wire: &[schemars::Schema],
) {
    let label = format!("{} {} {what}", ep.method, ep.path);
    let Some(schema) = schema else {
        findings.push(format!("{label} has no application/json schema"));
        return;
    };
    let (component, reference, kind) = match shape {
        Shape::One(c) => (c, schema.get("$ref"), "one"),
        Shape::List(c) => {
            let is_array = schema.get("type").and_then(Value::as_str) == Some("array");
            let reference = if is_array {
                schema.pointer("/items/$ref")
            } else {
                None
            };
            (c, reference, "a list of")
        }
    };
    let expected = format!("{COMPONENTS}{component}");
    if reference.and_then(Value::as_str) != Some(expected.as_str()) {
        findings.push(format!("{label} is not {kind} {component}"));
        return;
    }
    let Some(ours) = wire
        .iter()
        .map(schemars::Schema::as_value)
        .find(|s| s.get("title").and_then(Value::as_str) == Some(component))
    else {
        findings.push(format!(
            "{label} uses {component}, but this program has no wire type of that name"
        ));
        return;
    };
    compare(findings, component, ours, ours.get("$defs"), openapi);
}

fn compare(
    findings: &mut Vec<String>,
    component: &str,
    ours: &Value,
    defs: Option<&Value>,
    openapi: &Value,
) {
    let Some(theirs) = openapi
        .pointer(&format!(
            "/components/schemas/{}/properties",
            escape(component)
        ))
        .and_then(Value::as_object)
    else {
        findings.push(format!(
            "component {component} does not exist or has no properties"
        ));
        return;
    };
    // A wire type without properties would compare nothing and pass.
    let Some(our_properties) = ours
        .get("properties")
        .and_then(Value::as_object)
        .filter(|p| !p.is_empty())
    else {
        findings.push(format!("wire type {component} declares no properties"));
        return;
    };
    for (name, our) in our_properties {
        let at = format!("{component}.{name}");
        let Some(their) = theirs.get(name) else {
            findings.push(format!("{at}: not in the OpenAPI description"));
            continue;
        };
        let (our_type, our_nullable) = rust_type(our);
        if their.get("nullable").and_then(Value::as_bool) == Some(true) && !our_nullable {
            findings.push(format!("{at}: nullable there, but not an Option here"));
        }
        let their_ref = their.get("$ref").and_then(Value::as_str);
        match (our_type, their_ref) {
            (RustType::Ref(our_name), Some(their_ref)) => {
                let their_name = their_ref.strip_prefix(COMPONENTS).unwrap_or(their_ref);
                if our_name != their_name {
                    findings.push(format!(
                        "{at}: refers to {their_name} there, {our_name} here"
                    ));
                } else if let Some(nested) = defs.and_then(|d| d.get(&our_name)) {
                    compare(findings, &our_name, nested, defs, openapi);
                } else {
                    findings.push(format!("{at}: wire type {our_name} is not defined"));
                }
            }
            (RustType::Plain(our_name), None) => {
                let their_type = their
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("untyped");
                if their_type != our_name {
                    findings.push(format!("{at}: {their_type} there, {our_name} here"));
                } else if our_name == "array" {
                    compare_list(findings, &at, our, their, defs, openapi);
                }
            }
            (ours, theirs) => findings.push(format!(
                "{at}: {} here, {} there",
                ours.describe(),
                theirs.map_or("a plain type".to_string(), |r| format!(
                    "a reference to {r}"
                ))
            )),
        }
    }
}

/// Follows a list into its elements. Without this, `items: Vec<T>` would match
/// any array and the fields of `T` would never be compared.
fn compare_list(
    findings: &mut Vec<String>,
    at: &str,
    ours: &Value,
    theirs: &Value,
    defs: Option<&Value>,
    openapi: &Value,
) {
    let our_element = ours
        .pointer("/items/$ref")
        .and_then(Value::as_str)
        .map(|r| r.trim_start_matches("#/$defs/"));
    let their_element = theirs
        .pointer("/items/$ref")
        .and_then(Value::as_str)
        .map(|r| r.strip_prefix(COMPONENTS).unwrap_or(r));
    match (our_element, their_element) {
        // A list of plain values on both sides: nothing further to follow.
        (None, None) => {}
        (Some(ours), Some(theirs)) if ours == theirs => match defs.and_then(|d| d.get(ours)) {
            Some(nested) => compare(findings, ours, nested, defs, openapi),
            None => findings.push(format!("{at}: wire type {ours} is not defined")),
        },
        (ours, theirs) => findings.push(format!(
            "{at}: a list of {} there, of {} here",
            theirs.unwrap_or("plain values"),
            ours.unwrap_or("plain values")
        )),
    }
}

enum RustType {
    Plain(String),
    Ref(String),
    Unknown,
}

impl RustType {
    fn describe(&self) -> String {
        match self {
            RustType::Plain(t) => format!("plain {t}"),
            RustType::Ref(r) => format!("a reference to {r}"),
            RustType::Unknown => "an unrecognised schema".to_string(),
        }
    }
}

/// The base type of a schemars property and whether it admits null.
fn rust_type(v: &Value) -> (RustType, bool) {
    if let Some(r) = v.get("$ref").and_then(Value::as_str) {
        return (
            RustType::Ref(r.trim_start_matches("#/$defs/").to_string()),
            false,
        );
    }
    // `Option<Struct>` is rendered as anyOf [ {$ref}, {type: null} ].
    if let Some(any) = v.get("anyOf").and_then(Value::as_array) {
        let is_null = |x: &Value| x.get("type").and_then(Value::as_str) == Some("null");
        let nullable = any.iter().any(is_null);
        let inner = any.iter().find(|x| !is_null(x));
        return (
            inner.map_or(RustType::Unknown, |i| rust_type(i).0),
            nullable,
        );
    }
    match v.get("type") {
        Some(Value::String(t)) => (RustType::Plain(t.clone()), false),
        Some(Value::Array(types)) => {
            let names: Vec<&str> = types.iter().filter_map(Value::as_str).collect();
            let nullable = names.contains(&"null");
            let base = names.into_iter().find(|t| *t != "null");
            (
                base.map_or(RustType::Unknown, |t| RustType::Plain(t.to_string())),
                nullable,
            )
        }
        _ => (RustType::Unknown, false),
    }
}
