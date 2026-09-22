//! Prowlarr's own tasks. `app-profiles` (design §41): the profiles that say
//! how an indexer is searched -- RSS, automatic search, interactive search,
//! and the minimum number of seeders below which a torrent is not handed on.
//!
//! Every indexer carries an `appProfileId`, and a profile is the only place
//! where `minimumSeeders` can be set at all. Prowlarr ships exactly one
//! profile (`Standard`, id 1) and has no other way of writing one than its
//! API, so the host's second profile was a thing somebody had clicked
//! together -- which is what this task replaces.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{
    client::{expect_status, expect_status_at, Transport},
    endpoint::{Endpoint, Shape},
    engine::{shortened, Change, Probe, Task},
    error::Error,
    services::servarr,
};

pub const APP_PROFILE_LIST: Endpoint = Endpoint {
    method: "GET",
    path: "/api/v1/appprofile",
    request: None,
    response: Some(Shape::List("AppProfileResource")),
};
pub const APP_PROFILE_CREATE: Endpoint = Endpoint {
    method: "POST",
    path: "/api/v1/appprofile",
    request: Some(Shape::One("AppProfileResource")),
    response: None,
};
pub const APP_PROFILE_UPDATE: Endpoint = Endpoint {
    method: "PUT",
    path: "/api/v1/appprofile/{id}",
    request: Some(Shape::One("AppProfileResource")),
    response: None,
};

pub const ENDPOINTS: [Endpoint; 3] = [APP_PROFILE_LIST, APP_PROFILE_CREATE, APP_PROFILE_UPDATE];

/// The component the description names, field for field. None of them is
/// nullable except `name`, and Prowlarr answers all six -- a missing one is
/// a decode error naming it, not a silent default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AppProfileResource {
    pub id: i32,
    #[serde(default)]
    pub name: Option<String>,
    pub enable_rss: bool,
    pub enable_automatic_search: bool,
    pub enable_interactive_search: bool,
    pub minimum_seeders: i32,
    #[serde(flatten)]
    #[schemars(skip)]
    pub rest: Map<String, Value>,
}

/// The fields of an app profile a spec may set. `id` and `name` are not
/// among them: `name` is how a profile is found, `id` is the service's.
pub const SETTABLE: [&str; 4] = [
    "enableAutomaticSearch",
    "enableInteractiveSearch",
    "enableRss",
    "minimumSeeders",
];

pub fn wire_types() -> Vec<schemars::Schema> {
    vec![schemars::schema_for!(AppProfileResource)]
}

/// App profiles by name; a missing one is added, a differing one written
/// whole. Profiles the spec does not name are a note, as everywhere else.
pub struct AppProfiles {
    pub profiles: BTreeMap<String, BTreeMap<String, Value>>,
}

fn subject(name: &str) -> String {
    format!("app profile {name}")
}

/// The profile as one object again, so a field the spec names can be
/// compared and written by its wire name without a match arm per field.
fn document(profile: &AppProfileResource) -> Map<String, Value> {
    match serde_json::to_value(profile) {
        Ok(Value::Object(map)) => map,
        _ => Map::new(),
    }
}

impl AppProfiles {
    fn find<'a>(current: &'a [AppProfileResource], name: &str) -> Option<&'a AppProfileResource> {
        current.iter().find(|p| p.name.as_deref() == Some(name))
    }
}

/// The profiles by name, read once. Used by this task and, for the name an
/// indexer spec gives instead of an `appProfileId`, by `providers` (§41).
pub fn read_profiles(t: &dyn Transport) -> Result<Vec<AppProfileResource>, Error> {
    let path = APP_PROFILE_LIST.path.to_string();
    let reply = t.get(APP_PROFILE_LIST.path)?;
    expect_status(&APP_PROFILE_LIST, &reply, &[200])?;
    let list: Vec<AppProfileResource> =
        serde_json::from_str(&reply.body).map_err(|e| Error::Decode {
            path: path.clone(),
            reason: crate::error::shape(&e),
        })?;
    // Prowlarr always has `Standard`; an empty answer is a service that has
    // not finished starting, not a service without profiles.
    if list.is_empty() {
        return Err(Error::EmptyList { path });
    }
    if let Some(index) = list.iter().position(|p| p.name.is_none()) {
        return Err(Error::MissingName { path, index });
    }
    Ok(list)
}

impl Task for AppProfiles {
    type Current = Vec<AppProfileResource>;

    fn probe(&self, t: &dyn Transport) -> Result<String, Probe> {
        servarr::probe_status(t, servarr::V1.status)
    }

    fn read(&self, t: &dyn Transport) -> Result<Self::Current, Error> {
        read_profiles(t)
    }

    fn diff(&self, current: &Self::Current) -> Result<Vec<Change>, Error> {
        let (mut missing, mut changes) = (Vec::new(), Vec::new());
        for (name, set) in &self.profiles {
            let Some(profile) = Self::find(current, name) else {
                changes.push(Change {
                    subject: subject(name),
                    field: String::new(),
                    current: "(missing)".to_string(),
                    desired: "(added)".to_string(),
                });
                continue;
            };
            let object = document(profile);
            for (field, desired) in set {
                match object.get(field) {
                    None => missing.push(format!("{}: {field}", subject(name))),
                    Some(have) if have != desired => changes.push(Change {
                        subject: subject(name),
                        field: field.clone(),
                        current: shortened(have),
                        desired: shortened(desired),
                    }),
                    Some(_) => {}
                }
            }
        }
        if !missing.is_empty() {
            return Err(Error::MissingField(missing));
        }
        Ok(changes)
    }

    fn notes(&self, current: &Self::Current) -> Vec<String> {
        let untouched: Vec<&str> = current
            .iter()
            .filter_map(|p| p.name.as_deref())
            .filter(|name| !self.profiles.contains_key(*name))
            .collect();
        if untouched.is_empty() {
            Vec::new()
        } else {
            vec![format!(
                "not in the spec, left as they are: {}",
                untouched.join(", ")
            )]
        }
    }

    fn write(&self, t: &dyn Transport, current: &Self::Current) -> Result<(), Error> {
        for (name, set) in &self.profiles {
            match Self::find(current, name) {
                None => {
                    let mut body = Map::new();
                    body.insert("name".to_string(), Value::from(name.clone()));
                    for (field, value) in set {
                        body.insert(field.clone(), value.clone());
                    }
                    let ep = APP_PROFILE_CREATE;
                    let text = serialize(ep.method, ep.path, &Value::Object(body))?;
                    let reply = t.post_json(ep.path, &text)?;
                    expect_status(&ep, &reply, &[200, 201])?;
                }
                Some(profile) => {
                    // The whole object as it was read, with the named fields
                    // changed: what the spec does not name travels back.
                    let mut body = document(profile);
                    let mut differs = false;
                    for (field, value) in set {
                        if body.get(field) != Some(value) {
                            differs = true;
                        }
                        body.insert(field.clone(), value.clone());
                    }
                    if !differs {
                        continue;
                    }
                    let ep = APP_PROFILE_UPDATE;
                    let path = ep.path.replace("{id}", &profile.id.to_string());
                    let text = serialize(ep.method, &path, &Value::Object(body))?;
                    let reply = t.put_json(&path, &text)?;
                    // 202 as everywhere in Servarr: accepted, not yet saved.
                    expect_status_at(ep.method, &path, &reply, &[200, 202])?;
                }
            }
        }
        Ok(())
    }
}

fn serialize(method: &'static str, path: &str, value: &Value) -> Result<String, Error> {
    serde_json::to_string(value).map_err(|e| Error::Request {
        method,
        path: path.to_string(),
        reason: format!("cannot serialize: {e}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{ok, FakeTransport, Step};

    const PROFILES: &str = include_str!("../../tests/fixtures/prowlarr-2.5.2.5491/appprofile.json");

    fn listing(body: &str) -> FakeTransport {
        FakeTransport::default().on_get(APP_PROFILE_LIST.path, vec![ok(body)])
    }

    fn task(name: &str, set: Value) -> AppProfiles {
        let set: BTreeMap<String, Value> = serde_json::from_value(set).unwrap();
        AppProfiles {
            profiles: [(name.to_string(), set)].into_iter().collect(),
        }
    }

    fn standard() -> AppProfiles {
        task(
            "Standard",
            serde_json::json!({
                "enableRss": true,
                "enableAutomaticSearch": true,
                "enableInteractiveSearch": true,
                "minimumSeeders": 1
            }),
        )
    }

    #[test]
    fn the_recorded_profile_already_matches() {
        let task = standard();
        let current = task.read(&listing(PROFILES)).unwrap();
        assert_eq!(current.len(), 1);
        assert_eq!(current[0].name.as_deref(), Some("Standard"));
        assert_eq!(task.diff(&current).unwrap(), vec![]);
        assert_eq!(task.notes(&current), Vec::<String>::new());
    }

    #[test]
    fn one_differing_field_is_one_change_and_one_put_with_the_whole_object() {
        let task = task("Standard", serde_json::json!({ "minimumSeeders": 5 }));
        let t = listing(PROFILES).on_put(vec![Step::Answer(202, String::new())]);
        let current = task.read(&t).unwrap();
        let changes = task.diff(&current).unwrap();
        assert_eq!(changes.len(), 1, "{changes:?}");
        assert_eq!(
            changes[0].to_string(),
            "app profile Standard: minimumSeeders 1 -> 5"
        );

        task.write(&t, &current).unwrap();
        let written = t.written.borrow();
        assert_eq!(written.len(), 1);
        assert_eq!(written[0].0, "/api/v1/appprofile/1");
        let sent: Value = serde_json::from_str(&written[0].1).unwrap();
        let mut expected: Value = serde_json::from_str(PROFILES).unwrap();
        let expected = expected.as_array_mut().unwrap()[0].clone();
        // The whole object, only `minimumSeeders` changed.
        assert_eq!(sent["id"], expected["id"]);
        assert_eq!(sent["name"], expected["name"]);
        assert_eq!(sent["enableRss"], expected["enableRss"]);
        assert_eq!(
            sent["enableAutomaticSearch"],
            expected["enableAutomaticSearch"]
        );
        assert_eq!(
            sent["enableInteractiveSearch"],
            expected["enableInteractiveSearch"]
        );
        assert_eq!(sent["minimumSeeders"], 5);
    }

    #[test]
    fn an_unchanged_profile_is_not_written() {
        let t = listing(PROFILES).on_put(vec![Step::Answer(202, String::new())]);
        let task = standard();
        let current = task.read(&t).unwrap();
        task.write(&t, &current).unwrap();
        assert!(t.written.borrow().is_empty());
    }

    #[test]
    fn a_missing_profile_is_added_with_name_and_fields() {
        let task = task(
            "Wenig Seeder",
            serde_json::json!({ "minimumSeeders": 0, "enableRss": true }),
        );
        let t = listing(PROFILES).on_put(vec![Step::Answer(201, String::new())]);
        let current = task.read(&t).unwrap();
        let changes = task.diff(&current).unwrap();
        assert_eq!(
            changes[0].to_string(),
            "app profile Wenig Seeder: (missing) -> (added)"
        );
        task.write(&t, &current).unwrap();
        let written = t.written.borrow();
        assert_eq!(written.len(), 1);
        assert_eq!(written[0].0, "/api/v1/appprofile");
        let sent: Value = serde_json::from_str(&written[0].1).unwrap();
        assert_eq!(sent["name"], "Wenig Seeder");
        assert_eq!(sent["minimumSeeders"], 0);
        assert_eq!(sent["enableRss"], true);
        assert!(sent.get("id").is_none(), "{sent}");
    }

    #[test]
    fn a_profile_outside_the_spec_is_a_note_and_nothing_else() {
        let task = task("Wenig Seeder", serde_json::json!({ "minimumSeeders": 0 }));
        let current = task.read(&listing(PROFILES)).unwrap();
        assert_eq!(
            task.notes(&current),
            ["not in the spec, left as they are: Standard"]
        );
    }

    #[test]
    fn empty_list_and_nameless_profile_are_errors() {
        let task = standard();
        let err = task.read(&listing("[]")).err().unwrap().to_string();
        assert!(err.contains("empty list"), "{err}");
        let nameless = r#"[{"id":1,"enableRss":true,"enableAutomaticSearch":true,
                            "enableInteractiveSearch":true,"minimumSeeders":1}]"#;
        let err = task.read(&listing(nameless)).err().unwrap().to_string();
        assert!(err.contains("has no name"), "{err}");
    }

    #[test]
    fn write_names_the_profile_path_on_failure() {
        let task = task("Standard", serde_json::json!({ "minimumSeeders": 5 }));
        let t = listing(PROFILES).on_put(vec![Step::Answer(500, "boom".into())]);
        let current = task.read(&t).unwrap();
        assert_eq!(
            task.write(&t, &current).err().unwrap().to_string(),
            "PUT /api/v1/appprofile/1 answered HTTP 500"
        );
    }

    #[test]
    fn the_wire_type_is_named_after_its_component() {
        let titles: Vec<String> = wire_types()
            .iter()
            .map(|s| s.as_value()["title"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(titles, ["AppProfileResource"]);
    }
}
