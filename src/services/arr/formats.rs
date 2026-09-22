//! `custom-formats`: Radarr's and Sonarr's own custom formats (design §41).
//!
//! A custom format is a name, a switch that puts it into the file name, and
//! a list of specifications -- each a small rule (`ReleaseTitleSpecification`
//! with a regular expression, `ResolutionSpecification` with a number) that
//! can be negated and required. Scoring them is `quality-profiles`' job
//! (`format_scores`); this task only says which formats exist and what they
//! match.
//!
//! **Formats the spec does not name are left alone**, and there is no
//! `exactly` here. On the host this was written for, Recyclarr writes some
//! seventy formats from TRaSH's templates into the same collection, and a
//! spec that named "the whole collection" would take every one of them away.
//! They are counted in a note rather than listed: seventy-eight lines saying
//! "left as it is" are noise, not information.
//!
//! The specification list of a format the spec *does* name is complete: a
//! specification the spec does not name is removed from that format. Within
//! one format there is no second writer.
//!
//! `fields` entries depend on the implementation and have no schema
//! (`Field.value` is untyped in the description, as for providers, §12), so
//! they are compared against the answer at runtime. Everything else a
//! specification carries -- `id`, `implementationName`, `infoLink`, the
//! select options of a field -- travels back untouched.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::{
    client::{expect_status, expect_status_at, Transport},
    endpoint::{Endpoint, Shape},
    engine::{shortened, Change, Probe, Task},
    error::Error,
};

use super::probe;

pub const CUSTOM_FORMAT_LIST: Endpoint = Endpoint {
    method: "GET",
    path: "/api/v3/customformat",
    request: None,
    response: Some(Shape::Documents("CustomFormatResource")),
};
pub const CUSTOM_FORMAT_CREATE: Endpoint = Endpoint {
    method: "POST",
    path: "/api/v3/customformat",
    request: Some(Shape::Document("CustomFormatResource")),
    response: None,
};
pub const CUSTOM_FORMAT_UPDATE: Endpoint = Endpoint {
    method: "PUT",
    path: "/api/v3/customformat/{id}",
    request: Some(Shape::Document("CustomFormatResource")),
    response: None,
};

/// The component a specification is, for `schema-check --spec`.
pub const SPECIFICATION_COMPONENT: &str = "CustomFormatSpecificationSchema";
/// The component the format itself is.
pub const FORMAT_COMPONENT: &str = "CustomFormatResource";

/// One specification of a format, as the spec declares it.
#[derive(Debug, Clone, PartialEq)]
pub struct SpecificationTarget {
    pub name: String,
    pub implementation: String,
    pub negate: bool,
    pub required: bool,
    /// `fields` entries by name: the value each must carry.
    pub fields: BTreeMap<String, Value>,
}

/// One custom format, as the spec declares it.
#[derive(Debug, Clone, PartialEq)]
pub struct FormatTarget {
    pub include_custom_format_when_renaming: bool,
    pub specifications: Vec<SpecificationTarget>,
}

pub struct CustomFormats {
    pub formats: BTreeMap<String, FormatTarget>,
}

fn name_of(object: &Map<String, Value>) -> Option<&str> {
    object.get("name").and_then(Value::as_str)
}

fn subject(name: &str) -> String {
    format!("custom format {name}")
}

fn in_format(format: &str, specification: &str) -> String {
    format!("custom format {format}: specification {specification}")
}

/// The specifications of a format as objects; a format without the key has
/// none (Servarr omits null values).
fn specifications(entry: &Map<String, Value>) -> Vec<&Map<String, Value>> {
    entry
        .get("specifications")
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(Value::as_object).collect())
        .unwrap_or_default()
}

/// A `fields` entry of a specification by name.
fn field<'a>(specification: &'a Map<String, Value>, name: &str) -> Option<&'a Map<String, Value>> {
    specification
        .get("fields")?
        .as_array()?
        .iter()
        .filter_map(Value::as_object)
        .find(|f| f.get("name").and_then(Value::as_str) == Some(name))
}

/// A field without a `value` key holds null: Servarr omits null values.
fn value_of(field: &Map<String, Value>) -> Value {
    field.get("value").cloned().unwrap_or(Value::Null)
}

/// The switch as the service answers it. It is nullable there, and an absent
/// one means the same as `false` -- the format is not put into a file name.
fn renaming(entry: &Map<String, Value>) -> Value {
    match entry.get("includeCustomFormatWhenRenaming") {
        Some(Value::Bool(b)) => Value::Bool(*b),
        _ => Value::Bool(false),
    }
}

impl CustomFormats {
    fn find<'a>(current: &'a [Map<String, Value>], name: &str) -> Option<&'a Map<String, Value>> {
        current.iter().find(|e| name_of(e) == Some(name))
    }

    /// Differences of a format the service already has, and the field names
    /// the answer does not carry.
    fn changes_of(
        name: &str,
        target: &FormatTarget,
        entry: &Map<String, Value>,
        missing: &mut Vec<String>,
    ) -> Vec<Change> {
        let mut changes = Vec::new();
        let wanted = Value::Bool(target.include_custom_format_when_renaming);
        let have = renaming(entry);
        if have != wanted {
            changes.push(Change {
                subject: subject(name),
                field: "includeCustomFormatWhenRenaming".to_string(),
                current: shortened(&have),
                desired: shortened(&wanted),
            });
        }
        let present = specifications(entry);
        for want in &target.specifications {
            let at = in_format(name, &want.name);
            let Some(had) = present
                .iter()
                .find(|s| name_of(s) == Some(want.name.as_str()))
            else {
                changes.push(Change {
                    subject: at,
                    field: String::new(),
                    current: "(missing)".to_string(),
                    desired: "(added)".to_string(),
                });
                continue;
            };
            for (field_name, desired) in [
                ("implementation", Value::from(want.implementation.clone())),
                ("negate", Value::from(want.negate)),
                ("required", Value::from(want.required)),
            ] {
                match had.get(field_name) {
                    None => missing.push(format!("{at}: {field_name}")),
                    Some(have) if *have != desired => changes.push(Change {
                        subject: at.clone(),
                        field: field_name.to_string(),
                        current: shortened(have),
                        desired: shortened(&desired),
                    }),
                    Some(_) => {}
                }
            }
            for (field_name, desired) in &want.fields {
                match field(had, field_name) {
                    None => missing.push(format!("{at}: fields.{field_name}")),
                    Some(f) => {
                        let have = value_of(f);
                        if have != *desired {
                            changes.push(Change {
                                subject: at.clone(),
                                field: format!("fields.{field_name}"),
                                current: shortened(&have),
                                desired: shortened(desired),
                            });
                        }
                    }
                }
            }
        }
        // The specification list of a named format is complete: one the spec
        // does not name goes. Within a format there is no second writer.
        for had in &present {
            let Some(had_name) = name_of(had) else {
                continue;
            };
            if !target.specifications.iter().any(|s| s.name == had_name) {
                changes.push(Change {
                    subject: in_format(name, had_name),
                    field: String::new(),
                    current: "(present)".to_string(),
                    desired: "(removed)".to_string(),
                });
            }
        }
        changes
    }

    /// The body of a write: the format as it was read, with the named fields
    /// changed and the specification list as the spec declares it.
    fn body(
        name: &str,
        target: &FormatTarget,
        entry: Option<&Map<String, Value>>,
        missing: &mut Vec<String>,
    ) -> Map<String, Value> {
        let mut body = entry.cloned().unwrap_or_default();
        body.insert("name".to_string(), Value::from(name.to_string()));
        body.insert(
            "includeCustomFormatWhenRenaming".to_string(),
            Value::Bool(target.include_custom_format_when_renaming),
        );
        let present: Vec<Map<String, Value>> = entry
            .map(|e| specifications(e).into_iter().cloned().collect())
            .unwrap_or_default();
        let mut list = Vec::new();
        for want in &target.specifications {
            let at = in_format(name, &want.name);
            let existing = present
                .iter()
                .find(|s| name_of(s) == Some(want.name.as_str()));
            let mut specification = existing.cloned().unwrap_or_default();
            specification.insert("name".to_string(), Value::from(want.name.clone()));
            specification.insert(
                "implementation".to_string(),
                Value::from(want.implementation.clone()),
            );
            specification.insert("negate".to_string(), Value::Bool(want.negate));
            specification.insert("required".to_string(), Value::Bool(want.required));
            match existing {
                // The entries the service answered, with the named values
                // changed: what the spec does not name stays.
                Some(_) => {
                    for (field_name, value) in &want.fields {
                        if !set_field(&mut specification, field_name, value.clone()) {
                            missing.push(format!("{at}: fields.{field_name}"));
                        }
                    }
                }
                // A new specification: the service fills the rest in from
                // the implementation's schema.
                None => {
                    let fields: Vec<Value> = want
                        .fields
                        .iter()
                        .map(|(field_name, value)| {
                            serde_json::json!({ "name": field_name, "value": value })
                        })
                        .collect();
                    specification.insert("fields".to_string(), Value::Array(fields));
                    specification.remove("id");
                }
            }
            list.push(Value::Object(specification));
        }
        body.insert("specifications".to_string(), Value::Array(list));
        body
    }
}

/// Sets `value` on the `fields` entry called `name`; false if there is none.
fn set_field(specification: &mut Map<String, Value>, name: &str, value: Value) -> bool {
    let Some(list) = specification
        .get_mut("fields")
        .and_then(Value::as_array_mut)
    else {
        return false;
    };
    match list
        .iter_mut()
        .filter_map(Value::as_object_mut)
        .find(|f| f.get("name").and_then(Value::as_str) == Some(name))
    {
        Some(f) => {
            f.insert("value".to_string(), value);
            true
        }
        None => false,
    }
}

impl Task for CustomFormats {
    type Current = Vec<Map<String, Value>>;

    fn probe(&self, t: &dyn Transport) -> Result<String, Probe> {
        probe(t)
    }

    /// An empty list is a valid answer: a fresh service has no custom format,
    /// and every format the spec names then shows up as a change.
    fn read(&self, t: &dyn Transport) -> Result<Self::Current, Error> {
        let path = CUSTOM_FORMAT_LIST.path.to_string();
        let reply = t.get(CUSTOM_FORMAT_LIST.path)?;
        expect_status(&CUSTOM_FORMAT_LIST, &reply, &[200])?;
        let list: Vec<Map<String, Value>> =
            serde_json::from_str(&reply.body).map_err(|e| Error::Decode {
                path: path.clone(),
                reason: crate::error::shape(&e),
            })?;
        if let Some(index) = list.iter().position(|e| name_of(e).is_none()) {
            return Err(Error::MissingName { path, index });
        }
        Ok(list)
    }

    fn diff(&self, current: &Self::Current) -> Result<Vec<Change>, Error> {
        let (mut missing, mut changes) = (Vec::new(), Vec::new());
        for (name, target) in &self.formats {
            match Self::find(current, name) {
                Some(entry) => changes.extend(Self::changes_of(name, target, entry, &mut missing)),
                None => changes.push(Change {
                    subject: subject(name),
                    field: String::new(),
                    current: "(missing)".to_string(),
                    desired: "(added)".to_string(),
                }),
            }
        }
        if !missing.is_empty() {
            return Err(Error::MissingField(missing));
        }
        Ok(changes)
    }

    /// Counted, not listed. On the host Recyclarr writes some seventy
    /// formats into this collection; naming each of them would bury the one
    /// line that says something.
    fn notes(&self, current: &Self::Current) -> Vec<String> {
        let others = current
            .iter()
            .filter_map(name_of)
            .filter(|name| !self.formats.contains_key(*name))
            .count();
        if others == 0 {
            Vec::new()
        } else {
            vec![format!(
                "not in the spec, left as they are: {others} other formats"
            )]
        }
    }

    fn write(&self, t: &dyn Transport, current: &Self::Current) -> Result<(), Error> {
        let mut missing = Vec::new();
        let mut requests = Vec::new();
        for (name, target) in &self.formats {
            let entry = Self::find(current, name);
            if let Some(entry) = entry {
                // The same comparison `diff` makes, and the same complaint
                // about a field name the answer does not carry: a write with
                // one of those in it would drop the value silently.
                let changes = Self::changes_of(name, target, entry, &mut missing);
                if changes.is_empty() && missing.is_empty() {
                    continue;
                }
            }
            let mut body = Self::body(name, target, entry, &mut missing);
            match entry {
                Some(entry) => {
                    let id =
                        entry
                            .get("id")
                            .and_then(Value::as_i64)
                            .ok_or_else(|| Error::Decode {
                                path: CUSTOM_FORMAT_LIST.path.to_string(),
                                reason: format!("{} has no integer id", subject(name)),
                            })?;
                    let path = CUSTOM_FORMAT_UPDATE.path.replace("{id}", &id.to_string());
                    requests.push((CUSTOM_FORMAT_UPDATE, path, body));
                }
                None => {
                    body.remove("id");
                    requests.push((
                        CUSTOM_FORMAT_CREATE,
                        CUSTOM_FORMAT_CREATE.path.to_string(),
                        body,
                    ));
                }
            }
        }
        // Nothing is written while a field name does not fit the answer.
        if !missing.is_empty() {
            return Err(Error::MissingField(missing));
        }
        for (ep, path, body) in requests {
            let text = serde_json::to_string(&body).map_err(|e| Error::Request {
                method: ep.method,
                path: path.clone(),
                reason: format!("cannot serialize: {e}"),
            })?;
            let reply = if ep.method == "POST" {
                t.post_json(&path, &text)?
            } else {
                t.put_json(&path, &text)?
            };
            // 201 Created for a new format, 202 Accepted for an update.
            expect_status_at(ep.method, &path, &reply, &[200, 201, 202])?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{ok, FakeTransport, Step};

    const RADARR: &str =
        include_str!("../../../tests/fixtures/radarr-6.3.0.10514/customformat.json");
    const SONARR: &str =
        include_str!("../../../tests/fixtures/sonarr-4.0.19.2979/customformat.json");

    fn listing(body: &str) -> FakeTransport {
        FakeTransport::default().on_get(CUSTOM_FORMAT_LIST.path, vec![ok(body)])
    }

    /// `AV1` as the recording has it: one `ReleaseTitleSpecification`.
    fn av1(regex: &str) -> CustomFormats {
        CustomFormats {
            formats: [(
                "AV1".to_string(),
                FormatTarget {
                    include_custom_format_when_renaming: false,
                    specifications: vec![SpecificationTarget {
                        name: "AV1".to_string(),
                        implementation: "ReleaseTitleSpecification".to_string(),
                        negate: false,
                        required: true,
                        fields: [("value".to_string(), Value::from(regex))]
                            .into_iter()
                            .collect(),
                    }],
                },
            )]
            .into_iter()
            .collect(),
        }
    }

    /// The host's own format: a 3D rule nothing else writes.
    fn three_d() -> CustomFormats {
        CustomFormats {
            formats: [(
                "3D".to_string(),
                FormatTarget {
                    include_custom_format_when_renaming: true,
                    specifications: vec![SpecificationTarget {
                        name: "3D".to_string(),
                        implementation: "ReleaseTitleSpecification".to_string(),
                        negate: false,
                        required: true,
                        fields: [(
                            "value".to_string(),
                            Value::from(
                                r"(?i)\b(3d|hsbs|h-sbs|half-sbs|sbs|hou|h-ou|half-ou|ou)\b",
                            ),
                        )]
                        .into_iter()
                        .collect(),
                    }],
                },
            )]
            .into_iter()
            .collect(),
        }
    }

    #[test]
    fn the_recorded_format_already_matches_in_both_services() {
        for body in [RADARR, SONARR] {
            let task = av1(r"\bAV1\b");
            let current = task.read(&listing(body)).unwrap();
            assert_eq!(current.len(), 5);
            assert_eq!(task.diff(&current).unwrap(), vec![]);
            // The other four are counted, not listed.
            assert_eq!(
                task.notes(&current),
                ["not in the spec, left as they are: 4 other formats"]
            );
        }
    }

    #[test]
    fn a_differing_regular_expression_is_one_change_and_one_put_with_the_whole_object() {
        let task = av1(r"\bAV1|AOMedia\b");
        let t = listing(RADARR).on_put(vec![Step::Answer(202, String::new())]);
        let current = task.read(&t).unwrap();
        let changes = task.diff(&current).unwrap();
        assert_eq!(changes.len(), 1, "{changes:?}");
        assert_eq!(
            changes[0].to_string(),
            r#"custom format AV1: specification AV1: fields.value "\\bAV1\\b" -> "\\bAV1|AOMedia\\b""#
        );

        task.write(&t, &current).unwrap();
        let written = t.written.borrow();
        assert_eq!(written.len(), 1, "only AV1 differs");
        assert_eq!(written[0].0, "/api/v3/customformat/44");
        let sent: Value = serde_json::from_str(&written[0].1).unwrap();
        let recorded: Value = serde_json::from_str(RADARR).unwrap();
        let mut expected = recorded
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == "AV1")
            .unwrap()
            .clone();
        expected["specifications"][0]["fields"][0]["value"] = Value::from(r"\bAV1|AOMedia\b");
        // Byte for byte the recorded format, one value changed: the help
        // text, `implementationName`, `infoLink` and the id all travel back.
        assert_eq!(sent, expected);
    }

    #[test]
    fn the_renaming_switch_is_compared_and_written() {
        let task = three_d();
        let t = listing(RADARR);
        let current = task.read(&t).unwrap();
        let changes = task.diff(&current).unwrap();
        assert_eq!(changes.len(), 1, "{changes:?}");
        assert_eq!(
            changes[0].to_string(),
            "custom format 3D: (missing) -> (added)"
        );
    }

    #[test]
    fn a_missing_format_is_posted_with_its_specification_and_no_id() {
        let task = three_d();
        let t = listing(RADARR).on_put(vec![Step::Answer(201, String::new())]);
        let current = task.read(&t).unwrap();
        task.write(&t, &current).unwrap();
        let written = t.written.borrow();
        assert_eq!(written.len(), 1);
        assert_eq!(written[0].0, "/api/v3/customformat");
        let sent: Value = serde_json::from_str(&written[0].1).unwrap();
        assert_eq!(sent["name"], "3D");
        assert_eq!(sent["includeCustomFormatWhenRenaming"], true);
        assert!(sent.get("id").is_none(), "{sent}");
        let specification = &sent["specifications"][0];
        assert_eq!(specification["name"], "3D");
        assert_eq!(specification["implementation"], "ReleaseTitleSpecification");
        assert_eq!(specification["negate"], false);
        assert_eq!(specification["required"], true);
        assert_eq!(specification["fields"][0]["name"], "value");
        assert_eq!(
            specification["fields"][0]["value"],
            r"(?i)\b(3d|hsbs|h-sbs|half-sbs|sbs|hou|h-ou|half-ou|ou)\b"
        );
        assert!(specification.get("id").is_none(), "{specification}");
    }

    #[test]
    fn a_field_name_the_specification_does_not_have_is_an_error_before_any_write() {
        let mut task = av1(r"\bAV1\b");
        let target = task.formats.get_mut("AV1").unwrap();
        target.specifications[0]
            .fields
            .insert("regex".to_string(), Value::from("x"));
        let t = listing(RADARR).on_put(vec![Step::Answer(202, String::new())]);
        let current = task.read(&t).unwrap();
        let err = task.diff(&current).err().unwrap().to_string();
        assert_eq!(
            err,
            "the answer has no such field: custom format AV1: specification AV1: fields.regex"
        );
        assert!(task.write(&t, &current).is_err());
        assert!(t.written.borrow().is_empty(), "nothing was written");
    }

    /// The specification list of a named format is complete: `x265 (HD)` has
    /// two, and a spec naming one takes the other away.
    #[test]
    fn a_specification_the_spec_does_not_name_is_a_removal_within_the_format() {
        let task = CustomFormats {
            formats: [(
                "x265 (HD)".to_string(),
                FormatTarget {
                    include_custom_format_when_renaming: false,
                    specifications: vec![SpecificationTarget {
                        name: "x265/HEVC".to_string(),
                        implementation: "ReleaseTitleSpecification".to_string(),
                        negate: false,
                        required: true,
                        fields: BTreeMap::new(),
                    }],
                },
            )]
            .into_iter()
            .collect(),
        };
        let t = listing(RADARR).on_put(vec![Step::Answer(202, String::new())]);
        let current = task.read(&t).unwrap();
        let changes = task.diff(&current).unwrap();
        assert_eq!(changes.len(), 1, "{changes:?}");
        assert_eq!(
            changes[0].to_string(),
            "custom format x265 (HD): specification Not 2160p: (present) -> (removed)"
        );
        task.write(&t, &current).unwrap();
        let sent: Value = serde_json::from_str(&t.written.borrow()[0].1).unwrap();
        assert_eq!(sent["specifications"].as_array().unwrap().len(), 1);
        assert_eq!(sent["specifications"][0]["name"], "x265/HEVC");
    }

    #[test]
    fn a_negate_that_differs_is_a_change() {
        let mut task = av1(r"\bAV1\b");
        task.formats.get_mut("AV1").unwrap().specifications[0].negate = true;
        let current = task.read(&listing(SONARR)).unwrap();
        let changes = task.diff(&current).unwrap();
        assert_eq!(changes.len(), 1, "{changes:?}");
        assert_eq!(
            changes[0].to_string(),
            "custom format AV1: specification AV1: negate false -> true"
        );
    }

    #[test]
    fn a_nameless_format_is_an_error_and_an_empty_list_is_not() {
        let task = av1(r"\bAV1\b");
        assert!(task.read(&listing("[]")).unwrap().is_empty());
        let err = task
            .read(&listing(r#"[{"id":1,"specifications":[]}]"#))
            .err()
            .unwrap()
            .to_string();
        assert!(err.contains("has no name"), "{err}");
    }

    #[test]
    fn write_names_the_format_path_on_failure() {
        let task = av1(r"\bAV1|AOMedia\b");
        let t = listing(SONARR).on_put(vec![Step::Answer(500, "boom".into())]);
        let current = task.read(&t).unwrap();
        assert_eq!(
            task.write(&t, &current).err().unwrap().to_string(),
            "PUT /api/v3/customformat/43 answered HTTP 500"
        );
    }
}
