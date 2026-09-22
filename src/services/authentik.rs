//! Authentik: the tenant settings that no blueprint reaches (design §37).
//! `GET /api/v3/admin/settings/` answers the whole `Settings` document;
//! `PATCH` changes exactly the fields its body names, so a write carries
//! only what differs and every other setting stays as the tenant has it.
//!
//! The paths keep their trailing slash. Authentik (Django) answers a path
//! without one with a redirect, and converge does not carry the key through
//! a redirect (v0.30.0).

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::{
    client::{Reply, Transport},
    endpoint::{Endpoint, Shape},
    engine::{Change, Probe, Task},
    error::Error,
};

/// An administrator's endpoint, so the probe also proves what the token's
/// account may do: a 403 there instead of at the first write.
pub const VERSION: Endpoint = Endpoint {
    method: "GET",
    path: "/api/v3/admin/version/",
    request: None,
    response: Some(Shape::One("Version")),
};
pub const SETTINGS_READ: Endpoint = Endpoint {
    method: "GET",
    path: "/api/v3/admin/settings/",
    request: None,
    response: Some(Shape::Document("Settings")),
};
/// Partial: `PatchedSettingsRequest` has no required field, and authentik
/// leaves every setting the body does not name alone (measured on the host,
/// 2026-09-18: HTTP 200, the other fields unchanged).
pub const SETTINGS_WRITE: Endpoint = Endpoint {
    method: "PATCH",
    path: "/api/v3/admin/settings/",
    request: Some(Shape::Document("PatchedSettingsRequest")),
    response: Some(Shape::Document("Settings")),
};
pub const ENDPOINTS: [Endpoint; 3] = [VERSION, SETTINGS_READ, SETTINGS_WRITE];

/// The component a spec's fields are compared with (the answer), and the one
/// they are written with (the body of the `PATCH`). Both carry the same
/// properties in 2026.5.6; a field must be in each of them all the same.
pub const SETTINGS: &str = "Settings";
pub const SETTINGS_PATCH: &str = "PatchedSettingsRequest";

pub fn wire_types() -> Vec<schemars::Schema> {
    vec![schemars::schema_for!(Version)]
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct Version {
    #[serde(default)]
    pub version_current: Option<String>,
}

/// What an answer of authentik's may say in an error of this program. A
/// refused request answers `{"detail": …}`, a refused write DRF's validation
/// shape -- an object of field names to messages. The messages are
/// authentik's own and can quote the value they refused, so only the field
/// NAMES travel; the status says the rest.
fn refusal(reply: &Reply) -> Vec<String> {
    match reply.status {
        401 => return vec!["the token was refused".to_string()],
        403 => return vec!["the token's account is no administrator".to_string()],
        _ => {}
    }
    let Ok(Value::Object(body)) = serde_json::from_str::<Value>(&reply.body) else {
        return Vec::new();
    };
    let named: Vec<&str> = body
        .keys()
        .map(String::as_str)
        .filter(|key| *key != "detail" && *key != "code")
        .collect();
    if named.is_empty() {
        Vec::new()
    } else {
        vec![format!("these fields were refused: {}", named.join(", "))]
    }
}

/// Readiness: the version, from an endpoint only an administrator may ask.
/// A token authentik does not know, and one whose account is no
/// administrator, are both fatal -- waiting does not make either right.
pub fn probe(t: &dyn Transport) -> Result<String, Probe> {
    let reply = t
        .get(VERSION.path)
        .map_err(|e| Probe::NotYet(e.to_string()))?;
    match reply.status {
        200 => {}
        401 | 403 => {
            return Err(Probe::Fatal(Error::Status {
                method: VERSION.method,
                path: VERSION.path.to_string(),
                status: reply.status,
                validation: refusal(&reply),
            }))
        }
        other => return Err(Probe::NotYet(format!("HTTP {other}"))),
    }
    let version: Version = serde_json::from_str(&reply.body)
        .map_err(|e| Probe::NotYet(format!("unexpected answer: {}", crate::error::shape(&e))))?;
    version
        .version_current
        .filter(|v| !v.is_empty())
        .ok_or_else(|| Probe::NotYet("the answer has no version".to_string()))
}

/// Fields of the tenant settings by name. Each one is a top-level field of
/// `Settings`: `flags` and `footer_links` may be set whole, but no path
/// reaches into them -- authentik takes `flags` as one object, and a partial
/// one would drop what it leaves out.
pub struct Settings {
    pub set: BTreeMap<String, Value>,
}

impl Settings {
    fn missing(&self, current: &Map<String, Value>) -> Vec<String> {
        self.set
            .keys()
            .filter(|field| !current.contains_key(*field))
            .map(|field| format!("{SETTINGS}: {field}"))
            .collect()
    }

    /// The fields whose value differs, in the order of the spec.
    fn differing<'a>(&'a self, current: &Map<String, Value>) -> Vec<(&'a String, &'a Value)> {
        self.set
            .iter()
            .filter(|(field, desired)| current.get(*field) != Some(*desired))
            .collect()
    }
}

impl Task for Settings {
    type Current = Map<String, Value>;

    fn probe(&self, t: &dyn Transport) -> Result<String, Probe> {
        probe(t)
    }

    fn read(&self, t: &dyn Transport) -> Result<Self::Current, Error> {
        let reply = t.get(SETTINGS_READ.path)?;
        if reply.status != 200 {
            return Err(Error::Status {
                method: SETTINGS_READ.method,
                path: SETTINGS_READ.path.to_string(),
                status: reply.status,
                validation: refusal(&reply),
            });
        }
        // serde's message would quote what it stumbled over, and that is a
        // foreign answer. Say only where it failed.
        match serde_json::from_str::<Value>(&reply.body) {
            Ok(Value::Object(document)) => Ok(document),
            Ok(_) => Err(Error::Decode {
                path: SETTINGS_READ.path.to_string(),
                reason: "not an object".to_string(),
            }),
            Err(e) => Err(Error::Decode {
                path: SETTINGS_READ.path.to_string(),
                reason: format!(
                    "not JSON ({:?} error at line {} column {})",
                    e.classify(),
                    e.line(),
                    e.column()
                ),
            }),
        }
    }

    fn diff(&self, current: &Self::Current) -> Result<Vec<Change>, Error> {
        let missing = self.missing(current);
        if !missing.is_empty() {
            return Err(Error::MissingField(missing));
        }
        Ok(self
            .differing(current)
            .into_iter()
            .map(|(field, desired)| Change {
                subject: SETTINGS.to_string(),
                field: field.clone(),
                current: current.get(field).unwrap_or(&Value::Null).to_string(),
                desired: desired.to_string(),
            })
            .collect())
    }

    fn notes(&self, _current: &Self::Current) -> Vec<String> {
        Vec::new()
    }

    /// Only the differing fields travel: `PATCH` changes what its body names
    /// and leaves every other setting alone.
    fn write(&self, t: &dyn Transport, current: &Self::Current) -> Result<(), Error> {
        let missing = self.missing(current);
        if !missing.is_empty() {
            return Err(Error::MissingField(missing));
        }
        let body: Map<String, Value> = self
            .differing(current)
            .into_iter()
            .map(|(field, desired)| (field.clone(), desired.clone()))
            .collect();
        if body.is_empty() {
            return Ok(());
        }
        let reply = t.patch_json(SETTINGS_WRITE.path, &Value::Object(body).to_string())?;
        if reply.status == 200 {
            return Ok(());
        }
        Err(Error::Status {
            method: SETTINGS_WRITE.method,
            path: SETTINGS_WRITE.path.to_string(),
            status: reply.status,
            validation: refusal(&reply),
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::testing::{ok, FakeTransport, Step};

    const SETTINGS_JSON: &str =
        include_str!("../../tests/fixtures/authentik-2026.5.6/settings.json");
    const VERSION_JSON: &str = include_str!("../../tests/fixtures/authentik-2026.5.6/version.json");

    /// What the host's hourly shell unit set before converge did (design §37).
    fn declared() -> Settings {
        let set = [
            ("reputation_lower_limit", json!(-10)),
            ("impersonation", json!(false)),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        Settings { set }
    }

    fn settings(body: &str) -> FakeTransport {
        FakeTransport::default()
            .on_get(SETTINGS_READ.path, vec![ok(body)])
            .on_put(vec![ok(body)])
    }

    #[test]
    fn the_probe_reads_the_version_and_a_refused_token_is_fatal() {
        let t = FakeTransport::default().on_get(VERSION.path, vec![ok(VERSION_JSON)]);
        assert_eq!(probe(&t).ok().unwrap(), "2026.5.6");
        for (status, said) in [(401, "the token was refused"), (403, "no administrator")] {
            let t = FakeTransport::default().on_get(
                VERSION.path,
                vec![Step::Answer(
                    status,
                    r#"{"detail":"Authentication credentials were not provided."}"#.to_string(),
                )],
            );
            match probe(&t) {
                Err(Probe::Fatal(e)) => {
                    let e = e.to_string();
                    assert!(e.contains(said), "{status}: {e}");
                    // Authentik's own sentence stays in authentik.
                    assert!(!e.contains("credentials"), "{status}: {e}");
                }
                _ => panic!("{status} is not fatal"),
            }
        }
        let t =
            FakeTransport::default().on_get(VERSION.path, vec![Step::Answer(502, String::new())]);
        assert!(matches!(probe(&t), Err(Probe::NotYet(_))));
    }

    #[test]
    fn the_recorded_settings_already_match() {
        let task = declared();
        let current = task.read(&settings(SETTINGS_JSON)).unwrap();
        assert_eq!(task.diff(&current).unwrap(), vec![]);
        assert_eq!(task.notes(&current), Vec::<String>::new());
    }

    #[test]
    fn one_differing_field_travels_alone_in_the_patch() {
        let mut recorded: Value = serde_json::from_str(SETTINGS_JSON).unwrap();
        recorded["impersonation"] = json!(true);
        let task = declared();
        let t = settings(&recorded.to_string());
        let current = task.read(&t).unwrap();
        let changes = task.diff(&current).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(
            changes[0].to_string(),
            "Settings: impersonation true -> false"
        );
        task.write(&t, &current).unwrap();
        let written = t.written.borrow();
        assert_eq!(written.len(), 1);
        assert_eq!(written[0].0, "/api/v3/admin/settings/");
        // Only what differs: everything else is authentik's to keep.
        let sent: Value = serde_json::from_str(&written[0].1).unwrap();
        assert_eq!(sent, json!({"impersonation": false}));
    }

    #[test]
    fn a_field_the_answer_does_not_carry_is_an_error_before_any_write() {
        let mut task = declared();
        task.set.insert("impersonaton".to_string(), json!(false));
        let t = settings(SETTINGS_JSON);
        let current = task.read(&t).unwrap();
        let err = task.diff(&current).err().unwrap().to_string();
        assert!(err.contains("impersonaton"), "{err}");
        assert!(task.write(&t, &current).is_err(), "it wrote all the same");
        assert!(t.written.borrow().is_empty());
    }

    #[test]
    fn a_refused_write_names_the_fields_and_repeats_no_sentence() {
        let task = declared();
        let t = FakeTransport::default()
            .on_get(SETTINGS_READ.path, vec![ok(SETTINGS_JSON)])
            .on_put(vec![Step::Answer(
                400,
                r#"{"reputation_lower_limit":["Ensure this value is less than or equal to 0."]}"#
                    .to_string(),
            )]);
        let mut current = task.read(&t).unwrap();
        current.insert("impersonation".to_string(), json!(true));
        let err = task.write(&t, &current).err().unwrap().to_string();
        assert!(err.contains("HTTP 400"), "{err}");
        assert!(err.contains("reputation_lower_limit"), "{err}");
        // The message is authentik's, and it may quote what it refused.
        assert!(!err.contains("Ensure this value"), "{err}");
    }

    #[test]
    fn an_answer_that_is_no_json_or_no_object_quotes_nothing() {
        let task = declared();
        let err = task
            .read(&settings("token=hunter2"))
            .err()
            .unwrap()
            .to_string();
        assert!(!err.contains("hunter2"), "{err}");
        let err = task.read(&settings("[]")).err().unwrap().to_string();
        assert!(err.contains("not an object"), "{err}");
    }
}
