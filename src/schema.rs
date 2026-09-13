//! Compares this program's wire types -- and the field paths of a spec --
//! with a service's OpenAPI description. It checks names, types and enums. It
//! cannot check behaviour: Radarr's file lists only 200 for the quality
//! update, and the service answers 202.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::{
    endpoint::{Endpoint, Shape},
    paths,
};

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
    dedup(findings)
}

/// Checks the dotted paths of a spec against `component`: every segment a
/// property, every value of the property's type, inside its enum, `null`
/// only where the property is nullable.
pub fn check_paths(
    openapi: &Value,
    component: &str,
    desired: &BTreeMap<String, Value>,
) -> Vec<String> {
    let mut findings = Vec::new();
    for (path, value) in desired {
        let label = format!("{component}.{path}");
        let Some(segments) = paths::segments(path) else {
            findings.push(format!("{label}: not a path"));
            continue;
        };
        let mut current = component.to_string();
        for (i, segment) in segments.iter().enumerate() {
            let Some(properties) = properties_of(openapi, &current) else {
                findings.push(format!("{label}: component {current} does not exist"));
                break;
            };
            let Some(property) = properties.get(*segment) else {
                findings.push(format!("{label}: {current} has no property {segment}"));
                break;
            };
            let property = describe(openapi, property);
            if i + 1 == segments.len() {
                check_value(&mut findings, &label, &property, value, openapi);
            } else if let Kind::Ref(next) = property.kind {
                current = next;
            } else {
                findings.push(format!("{label}: {segment} is not an object"));
                break;
            }
        }
    }
    dedup(findings)
}

/// Checks a JSON list of objects (a spec's triggers, say) against
/// `component`, element by element.
pub fn check_objects(openapi: &Value, component: &str, list: &Value) -> Vec<String> {
    let mut findings = Vec::new();
    let Some(items) = list.as_array() else {
        return vec![format!("{component}: the spec has no list")];
    };
    for (i, item) in items.iter().enumerate() {
        let label = format!("{component}[{i}]");
        match item.as_object() {
            Some(object) => check_object(&mut findings, &label, component, object, openapi),
            None => findings.push(format!("{label}: not an object")),
        }
    }
    dedup(findings)
}

fn dedup(mut findings: Vec<String>) -> Vec<String> {
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

fn properties_of<'a>(
    openapi: &'a Value,
    component: &str,
) -> Option<&'a serde_json::Map<String, Value>> {
    openapi
        .pointer(&format!(
            "/components/schemas/{}/properties",
            escape(component)
        ))
        .and_then(Value::as_object)
}

/// The component a schema points to, written as `$ref` (Radarr) or as
/// `allOf: [{$ref}]` (Jellyfin).
fn reference(schema: &Value) -> Option<&str> {
    schema
        .get("$ref")
        .or_else(|| schema.pointer("/allOf/0/$ref"))
        .and_then(Value::as_str)
        .map(|r| r.strip_prefix(COMPONENTS).unwrap_or(r))
}

enum Kind {
    /// An object component.
    Ref(String),
    /// A JSON type: boolean, integer, number, string, array, object.
    Plain(String),
    /// One of several JSON types (Trailarr's `UpdateSetting.value`:
    /// integer, string or boolean).
    AnyOf(Vec<String>),
    Untyped,
}

fn is_null_type(schema: &Value) -> bool {
    schema.get("type").and_then(Value::as_str) == Some("null")
}

/// What an OpenAPI property is, with references to enum components resolved.
struct Property<'a> {
    kind: Kind,
    nullable: bool,
    enumeration: Option<&'a Vec<Value>>,
    items: Option<&'a Value>,
}

fn describe<'a>(openapi: &'a Value, property: &'a Value) -> Property<'a> {
    let nullable = property.get("nullable").and_then(Value::as_bool) == Some(true);
    let items = property.get("items");
    // OpenAPI 3.1 (FastAPI, so Trailarr) writes a nullable property as
    // `anyOf: [<schema>, {type: null}]` instead of `nullable: true`.
    if let Some(any) = property.get("anyOf").and_then(Value::as_array) {
        let nullable = nullable || any.iter().any(is_null_type);
        let others: Vec<&'a Value> = any.iter().filter(|s| !is_null_type(s)).collect();
        if let [only] = others.as_slice() {
            let mut inner = describe(openapi, only);
            inner.nullable |= nullable;
            return inner;
        }
        let types: Option<Vec<String>> = others
            .iter()
            .map(|s| s.get("type").and_then(Value::as_str).map(str::to_string))
            .collect();
        return Property {
            kind: types.map_or(Kind::Untyped, Kind::AnyOf),
            nullable,
            enumeration: None,
            items: None,
        };
    }
    let plain = |schema: &Value| {
        schema
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("string")
            .to_string()
    };
    if let Some(values) = property.get("enum").and_then(Value::as_array) {
        return Property {
            kind: Kind::Plain(plain(property)),
            nullable,
            enumeration: Some(values),
            items,
        };
    }
    if let Some(name) = reference(property) {
        let target = openapi.pointer(&format!("/components/schemas/{}", escape(name)));
        if let Some(values) = target.and_then(|t| t.get("enum")).and_then(Value::as_array) {
            return Property {
                kind: Kind::Plain(target.map(plain).unwrap_or_default()),
                nullable,
                enumeration: Some(values),
                items,
            };
        }
        return Property {
            kind: Kind::Ref(name.to_string()),
            nullable,
            enumeration: None,
            items,
        };
    }
    let kind = match property.get("type").and_then(Value::as_str) {
        Some(t) => Kind::Plain(t.to_string()),
        None => Kind::Untyped,
    };
    Property {
        kind,
        nullable,
        enumeration: None,
        items,
    }
}

fn kind_of(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "a list",
        Value::Object(_) => "an object",
    }
}

fn has_type(json_type: &str, value: &Value) -> bool {
    match json_type {
        "boolean" => value.is_boolean(),
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        "string" => value.is_string(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        _ => true,
    }
}

fn check_value(
    findings: &mut Vec<String>,
    label: &str,
    property: &Property,
    value: &Value,
    openapi: &Value,
) {
    if value.is_null() {
        if !property.nullable {
            findings.push(format!("{label}: is not nullable, the spec has null"));
        }
        return;
    }
    if let Some(values) = property.enumeration {
        if !values.contains(value) {
            let allowed: Vec<String> = values
                .iter()
                .map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_string))
                .collect();
            findings.push(format!(
                "{label}: {value} is not one of {}",
                allowed.join(", ")
            ));
        }
        return;
    }
    match &property.kind {
        Kind::Ref(component) => match value.as_object() {
            Some(object) => check_object(findings, label, component, object, openapi),
            None => findings.push(format!(
                "{label}: expects an object {component}, the spec has {}",
                kind_of(value)
            )),
        },
        Kind::Plain(json_type) => {
            if !has_type(json_type, value) {
                findings.push(format!(
                    "{label}: expects {json_type}, the spec has {}",
                    kind_of(value)
                ));
            } else if let (Some(items), Some(list)) = (property.items, value.as_array()) {
                let element = describe(openapi, items);
                for (i, item) in list.iter().enumerate() {
                    check_value(findings, &format!("{label}[{i}]"), &element, item, openapi);
                }
            }
        }
        Kind::AnyOf(json_types) => {
            if !json_types.iter().any(|t| has_type(t, value)) {
                findings.push(format!(
                    "{label}: expects one of {}, the spec has {}",
                    json_types.join(", "),
                    kind_of(value)
                ));
            }
        }
        Kind::Untyped => {}
    }
}

fn check_object(
    findings: &mut Vec<String>,
    label: &str,
    component: &str,
    object: &serde_json::Map<String, Value>,
    openapi: &Value,
) {
    let Some(properties) = properties_of(openapi, component) else {
        findings.push(format!("{label}: component {component} does not exist"));
        return;
    };
    for (key, value) in object {
        let at = format!("{label}.{key}");
        match properties.get(key) {
            Some(property) => {
                let property = describe(openapi, property);
                check_value(findings, &at, &property, value, openapi);
            }
            None => findings.push(format!("{at}: {component} has no property {key}")),
        }
    }
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
    let (component, found, kind, typed) = match shape {
        Shape::One(c) => (c, reference(schema), "one", true),
        Shape::Document(c) | Shape::Opaque(c) => (c, reference(schema), "one", false),
        Shape::List(c) | Shape::Documents(c) => {
            let is_array = schema.get("type").and_then(Value::as_str) == Some("array");
            let found = if is_array {
                schema.get("items").and_then(reference)
            } else {
                None
            };
            (c, found, "a list of", matches!(shape, Shape::List(_)))
        }
    };
    if found != Some(component) {
        findings.push(format!("{label} is not {kind} {component}"));
        return;
    }
    if matches!(shape, Shape::Opaque(_)) {
        if openapi
            .pointer(&format!("/components/schemas/{}", escape(component)))
            .is_none()
        {
            findings.push(format!("component {component} does not exist"));
        }
        return;
    }
    if !typed {
        // A document whose fields a spec names by path: the paths are checked
        // with `check_paths`; there is no wire type to compare.
        if properties_of(openapi, component).is_none() {
            findings.push(format!(
                "component {component} does not exist or has no properties"
            ));
        }
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
    let Some(theirs) = properties_of(openapi, component) else {
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
        let their = describe(openapi, their);
        if their.nullable && !our_nullable {
            findings.push(format!("{at}: nullable there, but not an Option here"));
        }
        match (our_type, their.kind) {
            (RustType::Ref(our_name), Kind::Ref(their_name)) => {
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
            (RustType::Plain(our_name), Kind::Plain(their_type)) => {
                if their_type != our_name {
                    findings.push(format!("{at}: {their_type} there, {our_name} here"));
                } else if our_name == "array" {
                    compare_list(findings, &at, our, their.items, defs, openapi);
                }
            }
            (RustType::AnyOf(mut our_types), Kind::AnyOf(mut their_types)) => {
                our_types.sort();
                their_types.sort();
                if our_types != their_types {
                    findings.push(format!(
                        "{at}: one of {} there, one of {} here",
                        their_types.join(", "),
                        our_types.join(", ")
                    ));
                }
            }
            (ours, theirs) => findings.push(format!(
                "{at}: {} here, {} there",
                ours.describe(),
                match theirs {
                    Kind::Ref(r) => format!("a reference to {r}"),
                    Kind::Plain(_) => "a plain type".to_string(),
                    Kind::AnyOf(types) => format!("one of {}", types.join(", ")),
                    Kind::Untyped => "an untyped schema".to_string(),
                }
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
    their_items: Option<&Value>,
    defs: Option<&Value>,
    openapi: &Value,
) {
    let our_element = ours
        .pointer("/items/$ref")
        .and_then(Value::as_str)
        .map(|r| r.trim_start_matches("#/$defs/"));
    let their_element = their_items.and_then(reference);
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
    /// An untagged enum of plain values.
    AnyOf(Vec<String>),
    Unknown,
}

impl RustType {
    fn describe(&self) -> String {
        match self {
            RustType::Plain(t) => format!("plain {t}"),
            RustType::Ref(r) => format!("a reference to {r}"),
            RustType::AnyOf(types) => format!("one of {}", types.join(", ")),
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
    // `Option<Struct>` is rendered as anyOf [ {$ref}, {type: null} ], an
    // untagged enum of plain values as anyOf [ {type}, {type}, … ].
    if let Some(any) = v.get("anyOf").and_then(Value::as_array) {
        let nullable = any.iter().any(is_null_type);
        let others: Vec<&Value> = any.iter().filter(|x| !is_null_type(x)).collect();
        if let [only] = others.as_slice() {
            return (rust_type(only).0, nullable);
        }
        let types: Option<Vec<String>> = others
            .iter()
            .map(|x| x.get("type").and_then(Value::as_str).map(str::to_string))
            .collect();
        return (types.map_or(RustType::Unknown, RustType::AnyOf), nullable);
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
