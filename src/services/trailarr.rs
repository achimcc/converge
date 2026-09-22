//! Trailarr: its links to Radarr and Sonarr, and the settings every trailer
//! profile shares (design §9). Trailarr is a FastAPI application; its
//! OpenAPI description is generated from the pydantic models, so a field the
//! models do not have is ignored on the way in without a word -- the host's
//! shell unit sent `monitor` for years. `schema-check` is what catches that.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{
    client::{expect_status, expect_status_at, Transport},
    endpoint::{Endpoint, Shape},
    engine::{shortened, Change, Probe, Task, HIDDEN},
    error::Error,
    secret::Secret,
};

pub const SETTINGS: Endpoint = Endpoint {
    method: "GET",
    path: "/api/v1/settings/",
    request: None,
    response: Some(Shape::One("Settings")),
};
pub const CONNECTIONS: Endpoint = Endpoint {
    method: "GET",
    path: "/api/v1/connections/",
    request: None,
    response: Some(Shape::List("ConnectionRead")),
};
/// Answers 201 with a string, which the description says and this program
/// does not read.
pub const CONNECTION_CREATE: Endpoint = Endpoint {
    method: "POST",
    path: "/api/v1/connections/",
    request: Some(Shape::One("ConnectionCreate")),
    response: None,
};
pub const CONNECTION_UPDATE: Endpoint = Endpoint {
    method: "PUT",
    path: "/api/v1/connections/{connection_id}",
    request: Some(Shape::One("ConnectionUpdate")),
    response: None,
};
/// Only with `"exactly": true` (design §39). Answers 200 with a string, as
/// the create does; nothing of the answer is read.
pub const CONNECTION_DELETE: Endpoint = Endpoint {
    method: "DELETE",
    path: "/api/v1/connections/{connection_id}",
    request: None,
    response: None,
};
pub const TRAILER_PROFILES: Endpoint = Endpoint {
    method: "GET",
    path: "/api/v1/trailerprofiles/",
    request: None,
    response: Some(Shape::List("TrailerProfileRead")),
};
pub const TRAILER_PROFILE_SETTING: Endpoint = Endpoint {
    method: "POST",
    path: "/api/v1/trailerprofiles/{trailerprofile_id}/setting",
    request: Some(Shape::One("UpdateSetting")),
    response: None,
};
pub const ENDPOINTS: [Endpoint; 7] = [
    SETTINGS,
    CONNECTIONS,
    CONNECTION_CREATE,
    CONNECTION_UPDATE,
    CONNECTION_DELETE,
    TRAILER_PROFILES,
    TRAILER_PROFILE_SETTING,
];

/// The components a spec's `set` fields are checked against: a connection's
/// fields must be writable both when it is added and when it is updated.
pub const CONNECTION_CREATE_COMPONENT: &str = "ConnectionCreate";
pub const CONNECTION_UPDATE_COMPONENT: &str = "ConnectionUpdate";
pub const TRAILER_PROFILE_READ: &str = "TrailerProfileRead";

pub fn wire_types() -> Vec<schemars::Schema> {
    vec![
        schemars::schema_for!(Settings),
        schemars::schema_for!(ConnectionRead),
        schemars::schema_for!(ConnectionCreate),
        schemars::schema_for!(ConnectionUpdate),
        schemars::schema_for!(TrailerProfileRead),
        schemars::schema_for!(UpdateSetting),
    ]
}

/// Only the version. The real answer carries some thirty-six other settings,
/// Trailarr's own API key and the web UI's password among them; they are
/// never decoded, so they cannot be kept or printed.
#[derive(Deserialize, JsonSchema)]
pub struct Settings {
    #[serde(default)]
    pub version: Option<String>,
}

/// The fields of a connection converge reads by name. Everything else --
/// `url`, `arr_type`, `path_mappings`, … -- stays in `rest`, where the spec's
/// `set` fields are looked up.
#[derive(Deserialize, JsonSchema)]
pub struct ConnectionRead {
    pub id: i64,
    pub name: String,
    /// Trailarr answers with the key of the Radarr or Sonarr it connects to,
    /// in the clear.
    #[schemars(with = "String")]
    pub api_key: Secret,
    #[serde(flatten)]
    #[schemars(skip)]
    pub rest: Map<String, Value>,
}

/// `path_mappings` is required when a connection is added and when it is
/// updated, so both bodies carry it by name; the spec's other fields travel
/// in `rest`.
#[derive(Serialize, JsonSchema)]
pub struct ConnectionCreate {
    pub name: String,
    pub api_key: String,
    pub path_mappings: Vec<PathMappingCRU>,
    #[serde(flatten)]
    #[schemars(skip)]
    pub rest: Map<String, Value>,
}

/// Every field but `path_mappings` is optional in the description (`null`
/// allowed). converge always sends name and key.
#[derive(Serialize, JsonSchema)]
pub struct ConnectionUpdate {
    pub name: Option<String>,
    pub api_key: Option<String>,
    pub path_mappings: Vec<PathMappingCRU>,
    #[serde(flatten)]
    #[schemars(skip)]
    pub rest: Map<String, Value>,
}

/// Named exactly like the component (`PathMappingCRU`: create, read,
/// update). Ids and the Plex section key travel in `rest`, untouched.
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct PathMappingCRU {
    pub path_from: String,
    pub path_to: String,
    #[serde(flatten)]
    #[schemars(skip)]
    pub rest: Map<String, Value>,
}

/// A profile's fields stay in `rest`: the spec names them, and
/// `schema-check` checks those names against the component by path.
#[derive(Deserialize, JsonSchema)]
pub struct TrailerProfileRead {
    pub id: i64,
    #[serde(flatten)]
    #[schemars(skip)]
    pub rest: Map<String, Value>,
}

#[derive(Serialize, JsonSchema)]
pub struct UpdateSetting {
    pub key: String,
    pub value: SettingValue,
}

/// `UpdateSetting.value` is `anyOf: [integer, string, boolean]`. Inlined, so
/// the derived schema is that `anyOf` too and not a reference to a component
/// the description does not have.
#[derive(Serialize, JsonSchema)]
#[serde(untagged)]
#[schemars(inline)]
pub enum SettingValue {
    Integer(i64),
    String(String),
    Boolean(bool),
}

impl SettingValue {
    fn from_json(value: &Value) -> Option<SettingValue> {
        match value {
            Value::Bool(b) => Some(SettingValue::Boolean(*b)),
            Value::String(s) => Some(SettingValue::String(s.clone())),
            Value::Number(n) => n.as_i64().map(SettingValue::Integer),
            _ => None,
        }
    }
}

/// Readiness: the settings answer with a version. A refused key is fatal.
pub fn probe(t: &dyn Transport) -> Result<String, Probe> {
    let reply = t
        .get(SETTINGS.path)
        .map_err(|e| Probe::NotYet(e.to_string()))?;
    match reply.status {
        200 => {}
        401 | 403 => {
            return Err(Probe::Fatal(Error::Status {
                method: SETTINGS.method,
                path: SETTINGS.path.to_string(),
                status: reply.status,
                validation: vec!["the API key was refused".to_string()],
            }))
        }
        other => return Err(Probe::NotYet(format!("HTTP {other}"))),
    }
    let settings: Settings = serde_json::from_str(&reply.body)
        .map_err(|e| Probe::NotYet(format!("unexpected answer: {}", crate::error::shape(&e))))?;
    settings
        .version
        .filter(|v| !v.is_empty())
        .ok_or_else(|| Probe::NotYet("the answer has no version".to_string()))
}

fn decode<T: for<'de> Deserialize<'de>>(path: &str, body: &str) -> Result<T, Error> {
    serde_json::from_str(body).map_err(|e| Error::Decode {
        path: path.to_string(),
        reason: crate::error::shape(&e),
    })
}

fn serialize(method: &'static str, path: &str, value: &impl Serialize) -> Result<String, Error> {
    serde_json::to_string(value).map_err(|e| Error::Request {
        method,
        path: path.to_string(),
        reason: format!("cannot serialize: {e}"),
    })
}

// --- connections -----------------------------------------------------------

/// One connection the spec declares, its key read from a credential.
pub struct ConnectionTarget {
    pub name: String,
    pub set: BTreeMap<String, Value>,
    pub api_key: Secret,
}

pub struct Connections {
    pub connections: Vec<ConnectionTarget>,
    /// The spec names every connection there should be: the others are
    /// removed (design §39). False unless the spec says `"exactly": true`.
    pub exactly: bool,
}

/// The fields Trailarr needs to add a connection and has no default for
/// (`ConnectionCreate.required`, besides name, key and path mappings).
const REQUIRED_TO_ADD: [&str; 2] = ["arr_type", "url"];

fn subject(name: &str) -> String {
    format!("connection {name}")
}

fn find<'a>(current: &'a [ConnectionRead], name: &str) -> Option<&'a ConnectionRead> {
    current.iter().find(|c| c.name == name)
}

/// The spec's fields without `path_mappings`, which both bodies carry by name.
fn other_fields(set: &BTreeMap<String, Value>) -> Map<String, Value> {
    set.iter()
        .filter(|(field, _)| field.as_str() != "path_mappings")
        .map(|(field, value)| (field.clone(), value.clone()))
        .collect()
}

fn mappings(path: &str, value: &Value) -> Result<Vec<PathMappingCRU>, Error> {
    serde_json::from_value(value.clone()).map_err(|e| Error::Decode {
        path: path.to_string(),
        reason: format!("path_mappings: {}", crate::error::shape(&e)),
    })
}

impl Connections {
    fn changes_of(
        &self,
        target: &ConnectionTarget,
        connection: &ConnectionRead,
        missing: &mut Vec<String>,
    ) -> Vec<Change> {
        let subject = subject(&target.name);
        let mut changes = Vec::new();
        for (field, desired) in &target.set {
            match connection.rest.get(field) {
                None => missing.push(format!("{subject}: {field}")),
                Some(current) if current != desired => changes.push(Change {
                    subject: subject.clone(),
                    field: field.clone(),
                    current: shortened(current),
                    desired: shortened(desired),
                }),
                Some(_) => {}
            }
        }
        if connection.api_key.expose() != target.api_key.expose() {
            changes.push(Change {
                subject,
                field: "api_key".to_string(),
                current: if connection.api_key.expose().is_empty() {
                    "(empty)".to_string()
                } else {
                    HIDDEN.to_string()
                },
                desired: format!("{HIDDEN} from its credential"),
            });
        }
        changes
    }

    /// The connections the spec does not name. Without `exactly` this is
    /// what `notes` reports and nothing else happens to them.
    fn not_in_the_spec<'a>(&self, current: &'a [ConnectionRead]) -> Vec<&'a ConnectionRead> {
        current
            .iter()
            .filter(|c| !self.connections.iter().any(|t| t.name == c.name))
            .collect()
    }
}

impl Task for Connections {
    type Current = Vec<ConnectionRead>;

    fn probe(&self, t: &dyn Transport) -> Result<String, Probe> {
        probe(t)
    }

    /// An empty list is a valid answer here: a fresh Trailarr has no
    /// connection, and every connection the spec names then shows up as a
    /// change -- the check is never over an empty set.
    fn read(&self, t: &dyn Transport) -> Result<Self::Current, Error> {
        let reply = t.get(CONNECTIONS.path)?;
        expect_status(&CONNECTIONS, &reply, &[200])?;
        decode(CONNECTIONS.path, &reply.body)
    }

    fn diff(&self, current: &Self::Current) -> Result<Vec<Change>, Error> {
        let mut missing = Vec::new();
        let mut cannot_add = Vec::new();
        let mut changes = Vec::new();
        for target in &self.connections {
            match find(current, &target.name) {
                Some(connection) => {
                    changes.extend(self.changes_of(target, connection, &mut missing))
                }
                None => {
                    let lacking: Vec<&str> = REQUIRED_TO_ADD
                        .into_iter()
                        .filter(|field| !target.set.contains_key(*field))
                        .collect();
                    if !lacking.is_empty() {
                        cannot_add.push(format!(
                            "{} (to add it, set needs {})",
                            subject(&target.name),
                            lacking.join(" and ")
                        ));
                    }
                    changes.push(Change {
                        subject: subject(&target.name),
                        field: String::new(),
                        current: "(missing)".to_string(),
                        desired: "(added)".to_string(),
                    });
                }
            }
        }
        if !cannot_add.is_empty() {
            return Err(Error::NotFound(cannot_add));
        }
        if missing.is_empty() {
            Ok(changes)
        } else {
            Err(Error::MissingField(missing))
        }
    }

    /// With `exactly` a connection outside the spec is a removal, and a
    /// removal is a change -- not a note that says it was left alone.
    fn notes(&self, current: &Self::Current) -> Vec<String> {
        if self.exactly {
            return Vec::new();
        }
        self.not_in_the_spec(current)
            .into_iter()
            .map(|c| format!("not in the spec: {}", subject(&c.name)))
            .collect()
    }

    fn surplus(&self, current: &Self::Current) -> Result<Vec<String>, Error> {
        if !self.exactly {
            return Ok(Vec::new());
        }
        Ok(self
            .not_in_the_spec(current)
            .into_iter()
            .map(|c| subject(&c.name))
            .collect())
    }

    /// One `DELETE` per surplus connection. Trailarr answers with the API
    /// keys of the Radarr and Sonarr it connects to, so a refusal names the
    /// connection and carries nothing of the body.
    fn remove(&self, t: &dyn Transport, current: &Self::Current) -> Result<(), Error> {
        if !self.exactly {
            return Ok(());
        }
        for connection in self.not_in_the_spec(current) {
            let path = CONNECTION_DELETE
                .path
                .replace("{connection_id}", &connection.id.to_string());
            let reply = t.delete(&path)?;
            if !(200..300).contains(&reply.status) {
                return Err(Error::Status {
                    method: CONNECTION_DELETE.method,
                    path,
                    status: reply.status,
                    validation: vec![format!("{} was not removed", subject(&connection.name))],
                });
            }
        }
        Ok(())
    }

    fn write(&self, t: &dyn Transport, current: &Self::Current) -> Result<(), Error> {
        for target in &self.connections {
            let Some(connection) = find(current, &target.name) else {
                let body = ConnectionCreate {
                    name: target.name.clone(),
                    api_key: target.api_key.expose().to_string(),
                    path_mappings: match target.set.get("path_mappings") {
                        Some(value) => mappings(CONNECTION_CREATE.path, value)?,
                        None => Vec::new(),
                    },
                    rest: other_fields(&target.set),
                };
                let body = serialize(CONNECTION_CREATE.method, CONNECTION_CREATE.path, &body)?;
                let reply = t.post_json(CONNECTION_CREATE.path, &body)?;
                // 201 is what the description lists; 200 is accepted as well.
                expect_status(&CONNECTION_CREATE, &reply, &[200, 201])?;
                continue;
            };
            let mut missing = Vec::new();
            if self.changes_of(target, connection, &mut missing).is_empty() {
                continue;
            }
            let path = CONNECTION_UPDATE
                .path
                .replace("{connection_id}", &connection.id.to_string());
            // Required in the update: the spec's list if it names one, else
            // the one the connection has, so it is never emptied by accident.
            let path_mappings = match target.set.get("path_mappings") {
                Some(value) => mappings(&path, value)?,
                None => match connection.rest.get("path_mappings") {
                    Some(value) => mappings(&path, value)?,
                    None => {
                        return Err(Error::MissingField(vec![format!(
                            "{}: path_mappings",
                            subject(&target.name)
                        )]))
                    }
                },
            };
            let body = ConnectionUpdate {
                name: Some(target.name.clone()),
                api_key: Some(target.api_key.expose().to_string()),
                path_mappings,
                rest: other_fields(&target.set),
            };
            let body = serialize(CONNECTION_UPDATE.method, &path, &body)?;
            let reply = t.put_json(&path, &body)?;
            expect_status_at(CONNECTION_UPDATE.method, &path, &reply, &[200, 201])?;
        }
        Ok(())
    }
}

// --- trailer-profiles ------------------------------------------------------

/// The same fields in every trailer profile.
pub struct TrailerProfiles {
    pub set: BTreeMap<String, Value>,
}

impl TrailerProfiles {
    /// The fields of `profile` that differ, in the spec's order.
    fn differing<'a>(
        &'a self,
        profile: &TrailerProfileRead,
        missing: &mut Vec<String>,
    ) -> Vec<(&'a String, Change)> {
        let subject = format!("trailer profile {}", profile.id);
        let mut differing = Vec::new();
        for (field, desired) in &self.set {
            match profile.rest.get(field) {
                None => missing.push(format!("{subject}: {field}")),
                Some(current) if current != desired => differing.push((
                    field,
                    Change {
                        subject: subject.clone(),
                        field: field.clone(),
                        current: shortened(current),
                        desired: shortened(desired),
                    },
                )),
                Some(_) => {}
            }
        }
        differing
    }
}

impl Task for TrailerProfiles {
    type Current = Vec<TrailerProfileRead>;

    fn probe(&self, t: &dyn Transport) -> Result<String, Probe> {
        probe(t)
    }

    fn read(&self, t: &dyn Transport) -> Result<Self::Current, Error> {
        let reply = t.get(TRAILER_PROFILES.path)?;
        expect_status(&TRAILER_PROFILES, &reply, &[200])?;
        let profiles: Vec<TrailerProfileRead> = decode(TRAILER_PROFILES.path, &reply.body)?;
        if profiles.is_empty() {
            return Err(Error::EmptyList {
                path: TRAILER_PROFILES.path.to_string(),
            });
        }
        Ok(profiles)
    }

    fn diff(&self, current: &Self::Current) -> Result<Vec<Change>, Error> {
        let mut missing = Vec::new();
        let changes: Vec<Change> = current
            .iter()
            .flat_map(|profile| self.differing(profile, &mut missing))
            .map(|(_, change)| change)
            .collect();
        if missing.is_empty() {
            Ok(changes)
        } else {
            Err(Error::MissingField(missing))
        }
    }

    fn notes(&self, _current: &Self::Current) -> Vec<String> {
        Vec::new()
    }

    /// One request per field: the endpoint sets a single setting. It confirms
    /// before the value is visible (observed by the host's shell unit), which
    /// the engine's reading back covers.
    fn write(&self, t: &dyn Transport, current: &Self::Current) -> Result<(), Error> {
        for profile in current {
            let path = TRAILER_PROFILE_SETTING
                .path
                .replace("{trailerprofile_id}", &profile.id.to_string());
            let mut missing = Vec::new();
            for (field, _) in self.differing(profile, &mut missing) {
                let value =
                    SettingValue::from_json(&self.set[field]).ok_or_else(|| Error::Request {
                        method: TRAILER_PROFILE_SETTING.method,
                        path: path.clone(),
                        reason: format!("{field}: not a string, a boolean or an integer"),
                    })?;
                let body = UpdateSetting {
                    key: field.clone(),
                    value,
                };
                let body = serialize(TRAILER_PROFILE_SETTING.method, &path, &body)?;
                let reply = t.post_json(&path, &body)?;
                expect_status_at(TRAILER_PROFILE_SETTING.method, &path, &reply, &[200])?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::{
        engine::{run, Mode},
        testing::{ok, FakeTransport, Step},
    };

    const SETTINGS_JSON: &str =
        include_str!("../../tests/fixtures/trailarr-0.11.5/constructed-settings-version-only.json");
    const CONNECTIONS_JSON: &str =
        include_str!("../../tests/fixtures/trailarr-0.11.5/connections.json");
    const PROFILES_JSON: &str =
        include_str!("../../tests/fixtures/trailarr-0.11.5/trailerprofiles.json");

    fn fields(value: Value) -> BTreeMap<String, Value> {
        value.as_object().unwrap().clone().into_iter().collect()
    }

    fn with(body: &str, edit: impl FnOnce(&mut Value)) -> String {
        let mut v: Value = serde_json::from_str(body).unwrap();
        edit(&mut v);
        v.to_string()
    }

    /// The host's two connections. The fixtures carry `<masked>` where the
    /// keys were; a credential with exactly that value is "already set".
    fn host_connections(key: &str) -> Connections {
        Connections {
            connections: vec![
                ConnectionTarget {
                    name: "Radarr".to_string(),
                    set: fields(json!({
                        "arr_type": "radarr", "url": "http://127.0.0.1:7878",
                        "monitor_new_media": true, "external_url": "", "path_mappings": []
                    })),
                    api_key: Secret::new(key.to_string()),
                },
                ConnectionTarget {
                    name: "Sonarr".to_string(),
                    set: fields(json!({
                        "arr_type": "sonarr", "url": "http://127.0.0.1:8989",
                        "monitor_new_media": true, "external_url": "", "path_mappings": []
                    })),
                    api_key: Secret::new(key.to_string()),
                },
            ],
            exactly: false,
        }
    }

    fn connections_transport(body: &str) -> FakeTransport {
        FakeTransport::default()
            .on_get(CONNECTIONS.path, vec![ok(body)])
            .on_put(vec![Step::Answer(
                201,
                r#""Connection Updated Successfully!""#.into(),
            )])
    }

    #[test]
    fn probe_reads_the_version_and_a_refused_key_is_fatal() {
        let t = FakeTransport::default().on_get(SETTINGS.path, vec![ok(SETTINGS_JSON)]);
        assert_eq!(probe(&t).ok().unwrap(), "v0.11.5");
        for (status, fatal) in [(401, true), (403, true), (503, false)] {
            let t = FakeTransport::default()
                .on_get(SETTINGS.path, vec![Step::Answer(status, String::new())]);
            assert_eq!(matches!(probe(&t), Err(Probe::Fatal(_))), fatal, "{status}");
        }
        let t = FakeTransport::default().on_get(SETTINGS.path, vec![ok(r#"{"api_key":"x"}"#)]);
        assert!(matches!(probe(&t), Err(Probe::NotYet(_))));
    }

    #[test]
    fn connections_recorded_state_is_already_desired() {
        let task = host_connections("<masked>");
        let t = connections_transport(CONNECTIONS_JSON);
        let current = task.read(&t).unwrap();
        assert_eq!(task.diff(&current).unwrap(), vec![]);
        assert_eq!(task.notes(&current), Vec::<String>::new());
    }

    #[test]
    fn a_changed_url_is_one_change_and_one_put_with_name_key_and_mappings() {
        let mut task = host_connections("<masked>");
        task.connections[0]
            .set
            .insert("url".to_string(), json!("http://10.0.20.11:7878"));
        task.connections[0].set.remove("path_mappings");
        let recorded_mappings = json!([{"path_from": "/media", "path_to": "/tank/media", "id": 3, "connection_id": 1, "plex_section_key": null}]);
        let body = with(CONNECTIONS_JSON, |v| {
            v[0]["path_mappings"] = recorded_mappings.clone();
        });
        let t = connections_transport(&body);
        let current = task.read(&t).unwrap();
        let changes = task.diff(&current).unwrap();
        assert_eq!(changes.len(), 1, "{changes:?}");
        assert_eq!(
            changes[0].to_string(),
            r#"connection Radarr: url "http://127.0.0.1:7878" -> "http://10.0.20.11:7878""#
        );
        task.write(&t, &current).unwrap();
        let written = t.written.borrow();
        assert_eq!(written.len(), 1, "Sonarr already matched");
        assert_eq!(written[0].0, "/api/v1/connections/1");
        let sent: Value = serde_json::from_str(&written[0].1).unwrap();
        assert_eq!(
            sent,
            json!({
                "name": "Radarr", "api_key": "<masked>",
                "arr_type": "radarr", "url": "http://10.0.20.11:7878",
                "monitor_new_media": true, "external_url": "",
                // Not in the spec: the connection's own list, untouched.
                "path_mappings": recorded_mappings
            })
        );
    }

    #[test]
    fn a_missing_connection_is_added_with_a_connection_create_body() {
        let task = host_connections("<masked>");
        let only_sonarr = with(CONNECTIONS_JSON, |v| {
            v.as_array_mut().unwrap().remove(0);
        });
        let t = connections_transport(&only_sonarr);
        let current = task.read(&t).unwrap();
        let changes = task.diff(&current).unwrap();
        assert_eq!(changes.len(), 1, "{changes:?}");
        assert_eq!(
            changes[0].to_string(),
            "connection Radarr: (missing) -> (added)"
        );
        task.write(&t, &current).unwrap();
        let written = t.written.borrow();
        assert_eq!(written.len(), 1);
        assert_eq!(written[0].0, "/api/v1/connections/");
        let sent: Value = serde_json::from_str(&written[0].1).unwrap();
        assert_eq!(
            sent,
            json!({
                "name": "Radarr", "api_key": "<masked>",
                "arr_type": "radarr", "url": "http://127.0.0.1:7878",
                "monitor_new_media": true, "external_url": "", "path_mappings": []
            })
        );
    }

    #[test]
    fn a_fresh_trailarr_without_connections_gets_both() {
        let task = host_connections("k");
        let t = connections_transport("[]");
        let changes = task.diff(&task.read(&t).unwrap()).unwrap();
        assert_eq!(changes.len(), 2);
    }

    #[test]
    fn a_missing_connection_without_url_cannot_be_added() {
        let mut task = host_connections("<masked>");
        task.connections[0].set = fields(json!({"monitor_new_media": true}));
        let t = connections_transport("[]");
        let err = task
            .diff(&task.read(&t).unwrap())
            .err()
            .unwrap()
            .to_string();
        assert!(
            err.starts_with(
                "not found on the service: connection Radarr (to add it, set needs arr_type and url)"
            ),
            "{err}"
        );
    }

    #[test]
    fn a_differing_key_is_a_hidden_change_and_travels_only_in_the_body() {
        let key = "s3cr3t-radarr-key";
        let mut task = host_connections("<masked>");
        task.connections[0].api_key = Secret::new(key.to_string());
        let t = connections_transport(CONNECTIONS_JSON);
        let current = task.read(&t).unwrap();
        let changes = task.diff(&current).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(
            changes[0].to_string(),
            "connection Radarr: api_key (hidden) -> (hidden) from its credential"
        );
        let shown = format!("{changes:?}");
        assert!(
            !shown.contains(key) && !shown.contains("<masked>"),
            "{shown}"
        );
        task.write(&t, &current).unwrap();
        let sent: Value = serde_json::from_str(&t.written.borrow()[0].1).unwrap();
        assert_eq!(sent["api_key"], key);

        let empty = with(CONNECTIONS_JSON, |v| v[0]["api_key"] = json!(""));
        let changes = task.diff(&task.read(&connections_transport(&empty)).unwrap());
        assert_eq!(
            changes.unwrap()[0].to_string(),
            "connection Radarr: api_key (empty) -> (hidden) from its credential"
        );
    }

    #[test]
    fn a_connection_outside_the_spec_is_a_note_and_left_alone() {
        let mut task = host_connections("<masked>");
        task.connections.remove(1);
        let t = connections_transport(CONNECTIONS_JSON);
        let current = task.read(&t).unwrap();
        assert_eq!(task.diff(&current).unwrap(), vec![]);
        assert_eq!(task.notes(&current), ["not in the spec: connection Sonarr"]);
    }

    // --- exactly (design §39) ---------------------------------------------

    /// The same spec with `exactly` on, and Sonarr dropped from it, so the
    /// recorded Sonarr connection is the surplus.
    fn only_radarr(exactly: bool) -> Connections {
        let mut task = host_connections("<masked>");
        task.connections.remove(1);
        task.exactly = exactly;
        task
    }

    #[test]
    fn without_exactly_a_surplus_is_named_nowhere_and_removed_nowhere() {
        let task = only_radarr(false);
        let t = connections_transport(CONNECTIONS_JSON).on_delete(vec![Step::Answer(
            200,
            r#""Connection Deleted Successfully!""#.into(),
        )]);
        let current = task.read(&t).unwrap();
        assert_eq!(task.surplus(&current).unwrap(), Vec::<String>::new());
        task.remove(&t, &current).unwrap();
        assert!(t.deleted.borrow().is_empty());
        // It stays the note it has always been.
        assert_eq!(task.notes(&current), ["not in the spec: connection Sonarr"]);
    }

    #[test]
    fn with_exactly_a_surplus_connection_is_a_removal_and_no_longer_a_note() {
        let task = only_radarr(true);
        let t = connections_transport(CONNECTIONS_JSON);
        let current = task.read(&t).unwrap();
        assert_eq!(task.surplus(&current).unwrap(), ["connection Sonarr"]);
        assert_eq!(task.notes(&current), Vec::<String>::new());
    }

    #[test]
    fn plan_says_present_removed_and_apply_deletes_before_it_writes() {
        let task = only_radarr(true);
        // Radarr's URL differs too, so there is a write to come after.
        let mut task = task;
        task.connections[0]
            .set
            .insert("url".to_string(), json!("http://10.0.20.11:7878"));
        let after: Value = {
            let mut v: Value = serde_json::from_str(CONNECTIONS_JSON).unwrap();
            let list = v.as_array_mut().unwrap();
            list.retain(|c| c["name"] != "Sonarr");
            list[0]["url"] = json!("http://10.0.20.11:7878");
            v
        };
        let t = FakeTransport::default()
            .on_get(SETTINGS.path, vec![ok(SETTINGS_JSON)])
            .on_get(
                CONNECTIONS.path,
                vec![ok(CONNECTIONS_JSON), ok(&after.to_string())],
            )
            .on_put(vec![Step::Answer(
                200,
                r#""Connection Updated Successfully!""#.into(),
            )])
            .on_delete(vec![Step::Answer(
                200,
                r#""Connection Deleted Successfully!""#.into(),
            )]);

        let plan = run(
            Mode::Plan,
            &task,
            &t,
            &crate::testing::FakeClock::new(),
            crate::engine::Timing::default(),
        )
        .unwrap();
        match plan.outcome {
            crate::engine::Outcome::Differs(changes) => assert_eq!(
                changes[0].to_string(),
                "connection Sonarr: (present) -> (removed)"
            ),
            other => panic!("expected Differs, got {other:?}"),
        }
        assert!(t.deleted.borrow().is_empty(), "plan removes nothing");

        let t = FakeTransport::default()
            .on_get(SETTINGS.path, vec![ok(SETTINGS_JSON)])
            .on_get(
                CONNECTIONS.path,
                vec![ok(CONNECTIONS_JSON), ok(&after.to_string())],
            )
            .on_put(vec![Step::Answer(
                200,
                r#""Connection Updated Successfully!""#.into(),
            )])
            .on_delete(vec![Step::Answer(
                200,
                r#""Connection Deleted Successfully!""#.into(),
            )]);
        run(
            Mode::Apply,
            &task,
            &t,
            &crate::testing::FakeClock::new(),
            crate::engine::Timing::default(),
        )
        .unwrap();
        // Sonarr has id 2 in the recording.
        assert_eq!(t.deleted.borrow().as_slice(), ["/api/v1/connections/2"]);
        let calls = t.calls.borrow();
        assert_eq!(calls[0], ("DELETE", "/api/v1/connections/2".to_string()));
        assert_eq!(calls[1].0, "PUT", "{calls:?}");
    }

    /// Trailarr answers with the keys of the services it connects to, so no
    /// part of a body may reach an error -- only the connection's name.
    #[test]
    fn a_refused_removal_names_the_connection_and_nothing_from_the_body() {
        let task = only_radarr(true);
        let t = connections_transport(CONNECTIONS_JSON).on_delete(vec![Step::Answer(
            409,
            r#"{"detail":"ERFUNDENER-SCHLUESSEL-XYZ"}"#.into(),
        )]);
        let current = task.read(&t).unwrap();
        let err = task.remove(&t, &current).err().unwrap().to_string();
        assert_eq!(
            err,
            "DELETE /api/v1/connections/2 answered HTTP 409: connection Sonarr was not removed"
        );
        assert!(!err.contains("ERFUNDENER"), "{err}");
    }

    #[test]
    fn a_set_field_the_answer_lacks_is_missing() {
        // `monitor` is what the host's shell sent; Trailarr has no such field.
        let mut task = host_connections("<masked>");
        task.connections[0]
            .set
            .insert("monitor".to_string(), json!(true));
        let t = connections_transport(CONNECTIONS_JSON);
        let err = task
            .diff(&task.read(&t).unwrap())
            .err()
            .unwrap()
            .to_string();
        assert_eq!(
            err,
            "the answer has no such field: connection Radarr: monitor"
        );
    }

    #[test]
    fn a_refused_update_names_the_connection_path() {
        let mut task = host_connections("<masked>");
        task.connections[1]
            .set
            .insert("monitor_new_media".to_string(), json!(false));
        let t = FakeTransport::default()
            .on_get(CONNECTIONS.path, vec![ok(CONNECTIONS_JSON)])
            .on_put(vec![Step::Answer(422, r#"{"detail":[]}"#.into())]);
        let current = task.read(&t).unwrap();
        assert_eq!(
            task.write(&t, &current).err().unwrap().to_string(),
            "PUT /api/v1/connections/2 answered HTTP 422"
        );
    }

    fn host_profiles() -> TrailerProfiles {
        TrailerProfiles {
            set: fields(json!({
                "search_query": "{title} {year} deutscher trailer",
                "always_search": true,
                "exclude_words": "reaction,review,recap,explained,breakdown,honest trailer,parodie,parody,fan edit,fan trailer,making of,behind the scenes,interview,soundtrack",
                "file_format": "mp4",
                "video_format": "h264",
                "audio_format": "aac"
            })),
        }
    }

    fn profiles_transport(body: &str) -> FakeTransport {
        FakeTransport::default()
            .on_get(TRAILER_PROFILES.path, vec![ok(body)])
            .on_put(vec![ok("{}")])
    }

    #[test]
    fn trailer_profiles_recorded_state_is_already_desired() {
        let task = host_profiles();
        let t = profiles_transport(PROFILES_JSON);
        assert_eq!(task.diff(&task.read(&t).unwrap()).unwrap(), vec![]);
    }

    #[test]
    fn one_differing_field_is_one_post_per_profile() {
        let mut task = host_profiles();
        task.set.insert("retry_count".to_string(), json!(3));
        let t = profiles_transport(PROFILES_JSON);
        let current = task.read(&t).unwrap();
        let changes = task.diff(&current).unwrap();
        let shown: Vec<String> = changes.iter().map(ToString::to_string).collect();
        assert_eq!(
            shown,
            [
                "trailer profile 1: retry_count 2 -> 3",
                "trailer profile 2: retry_count 2 -> 3"
            ]
        );
        task.write(&t, &current).unwrap();
        let written = t.written.borrow();
        let sent: Vec<(String, Value)> = written
            .iter()
            .map(|(path, body)| (path.clone(), serde_json::from_str(body).unwrap()))
            .collect();
        assert_eq!(
            sent,
            [
                (
                    "/api/v1/trailerprofiles/1/setting".to_string(),
                    json!({"key": "retry_count", "value": 3})
                ),
                (
                    "/api/v1/trailerprofiles/2/setting".to_string(),
                    json!({"key": "retry_count", "value": 3})
                ),
            ]
        );
    }

    #[test]
    fn only_the_profile_that_differs_is_written_and_long_values_are_shortened() {
        let task = host_profiles();
        let body = with(PROFILES_JSON, |v| {
            v[1]["exclude_words"] = json!("reaction");
            v[1]["always_search"] = json!(false);
        });
        let t = profiles_transport(&body);
        let current = task.read(&t).unwrap();
        let changes = task.diff(&current).unwrap();
        assert_eq!(changes.len(), 2, "{changes:?}");
        assert_eq!(
            changes[0].to_string(),
            "trailer profile 2: always_search false -> true"
        );
        assert!(changes[1].to_string().contains("…"), "{}", changes[1]);
        task.write(&t, &current).unwrap();
        let written = t.written.borrow();
        assert_eq!(written.len(), 2);
        assert!(written
            .iter()
            .all(|(path, _)| path == "/api/v1/trailerprofiles/2/setting"));
        let sent: Value = serde_json::from_str(&written[0].1).unwrap();
        assert_eq!(sent, json!({"key": "always_search", "value": true}));
    }

    #[test]
    fn no_profiles_is_an_error_and_so_is_an_unknown_field() {
        let task = host_profiles();
        let err = task
            .read(&profiles_transport("[]"))
            .err()
            .unwrap()
            .to_string();
        assert_eq!(err, "/api/v1/trailerprofiles/ returned an empty list");

        let mut task = host_profiles();
        task.set.insert("search".to_string(), json!("x"));
        let t = profiles_transport(PROFILES_JSON);
        let err = task
            .diff(&task.read(&t).unwrap())
            .err()
            .unwrap()
            .to_string();
        assert_eq!(
            err,
            "the answer has no such field: trailer profile 1: search; trailer profile 2: search"
        );
    }

    #[test]
    fn wire_types_are_named_after_their_components() {
        let titles: Vec<String> = wire_types()
            .iter()
            .map(|s| s.as_value()["title"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            titles,
            [
                "Settings",
                "ConnectionRead",
                "ConnectionCreate",
                "ConnectionUpdate",
                "TrailerProfileRead",
                "UpdateSetting"
            ]
        );
    }
}
