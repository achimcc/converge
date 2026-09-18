//! Kavita: its server settings, a document whose fields a spec names by path
//! (design §25). `POST /api/Settings` replaces the whole document, so every
//! write sends the document as it was read, with only the named fields
//! changed -- the same shape as Jellyfin's server configuration (§6).
//!
//! Two values in the answer need care. `oidcConfig.secret` comes back as
//! asterisks of its length, and Kavita puts the stored secret back in when it
//! receives exactly those asterisks (`SettingsService.UpdateOidcSettings`); a
//! round trip therefore keeps it. `smtpConfig.password` comes back in clear
//! text. Neither may be named by a spec (see `spec.rs`), so neither can appear
//! in a change, and no error here carries anything from a settings body.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use crate::{
    client::{expect_status, Reply, Transport},
    endpoint::{Endpoint, Shape},
    engine::{Change, Probe, Task},
    error::Error,
    paths,
};

/// Answers only an administrator, so the probe also proves that the key
/// belongs to one.
pub const SERVER_INFO: Endpoint = Endpoint {
    method: "GET",
    path: "/api/Server/server-info-slim",
    request: None,
    response: Some(Shape::One("ServerInfoSlimDto")),
};
pub const SETTINGS_READ: Endpoint = Endpoint {
    method: "GET",
    path: "/api/Settings",
    request: None,
    response: Some(Shape::Document("ServerSettingDto")),
};
pub const SETTINGS_WRITE: Endpoint = Endpoint {
    method: "POST",
    path: "/api/Settings",
    request: Some(Shape::Document("ServerSettingDto")),
    response: None,
};
pub const ENDPOINTS: [Endpoint; 3] = [SERVER_INFO, SETTINGS_READ, SETTINGS_WRITE];

/// The component a spec's paths are checked against.
pub const SERVER_SETTINGS: &str = "ServerSettingDto";

pub fn wire_types() -> Vec<schemars::Schema> {
    vec![schemars::schema_for!(ServerInfoSlimDto)]
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfoSlimDto {
    #[serde(default)]
    pub kavita_version: Option<String>,
}

/// Readiness: the slim server info answers with a version. It is an
/// administrator's endpoint, so a refused key -- or the key of an account
/// that is no administrator -- is fatal. (Kavita 0.9.1.4 reports itself as
/// 0.9.1.3; the version here is what Kavita says, not what is installed.)
pub fn probe(t: &dyn Transport) -> Result<String, Probe> {
    let reply = t
        .get(SERVER_INFO.path)
        .map_err(|e| Probe::NotYet(e.to_string()))?;
    match reply.status {
        200 => {}
        401 | 403 => {
            return Err(Probe::Fatal(Error::Status {
                method: SERVER_INFO.method,
                path: SERVER_INFO.path.to_string(),
                status: reply.status,
                validation: vec![
                    "the key was refused, or its account is no administrator".to_string()
                ],
            }))
        }
        other => return Err(Probe::NotYet(format!("HTTP {other}"))),
    }
    let info: ServerInfoSlimDto = serde_json::from_str(&reply.body)
        .map_err(|e| Probe::NotYet(format!("unexpected answer: {e}")))?;
    info.kavita_version
        .filter(|v| !v.is_empty())
        .ok_or_else(|| Probe::NotYet("the answer has no version".to_string()))
}

/// Kavita refuses a settings write with a plain, translated sentence
/// (`BadRequest(await localizationService.TranslateAsync(...))`). It names a
/// rule, never a value, so a short single line is shown; anything else is
/// left out, as a body that might quote the request would be.
fn refusal(reply: &Reply) -> Vec<String> {
    let text = match serde_json::from_str::<Value>(&reply.body) {
        Ok(Value::String(s)) => s,
        Ok(_) => return Vec::new(),
        Err(_) => reply.body.trim().to_string(),
    };
    if text.is_empty() || text.len() > 200 || text.contains('\n') {
        Vec::new()
    } else {
        vec![text]
    }
}

pub struct ServerSettings {
    pub set: BTreeMap<String, Value>,
}

impl ServerSettings {
    fn missing(&self, document: &Value) -> Vec<String> {
        self.set
            .keys()
            .filter(|path| paths::get(document, path).is_none())
            .map(|path| format!("{SERVER_SETTINGS}: {path}"))
            .collect()
    }
}

impl Task for ServerSettings {
    type Current = Value;

    fn probe(&self, t: &dyn Transport) -> Result<String, Probe> {
        probe(t)
    }

    fn read(&self, t: &dyn Transport) -> Result<Value, Error> {
        let reply = t.get(SETTINGS_READ.path)?;
        expect_status(&SETTINGS_READ, &reply, &[200])?;
        // serde's message could quote a value -- the SMTP password is in
        // this answer in clear text. Say only where it failed.
        let document: Value = serde_json::from_str(&reply.body).map_err(|e| Error::Decode {
            path: SETTINGS_READ.path.to_string(),
            reason: format!(
                "not JSON ({:?} error at line {} column {})",
                e.classify(),
                e.line(),
                e.column()
            ),
        })?;
        if !document.is_object() {
            return Err(Error::Decode {
                path: SETTINGS_READ.path.to_string(),
                reason: "not an object".to_string(),
            });
        }
        Ok(document)
    }

    fn diff(&self, current: &Value) -> Result<Vec<Change>, Error> {
        let missing = self.missing(current);
        if !missing.is_empty() {
            return Err(Error::MissingField(missing));
        }
        Ok(self
            .set
            .iter()
            .filter_map(|(path, desired)| {
                let now = paths::get(current, path)?;
                (now != desired).then(|| Change {
                    subject: SERVER_SETTINGS.to_string(),
                    field: path.clone(),
                    current: now.to_string(),
                    desired: desired.to_string(),
                })
            })
            .collect())
    }

    fn notes(&self, _current: &Value) -> Vec<String> {
        Vec::new()
    }

    fn write(&self, t: &dyn Transport, current: &Value) -> Result<(), Error> {
        let mut updated = current.clone();
        let missing: Vec<String> = self
            .set
            .iter()
            .filter(|(path, value)| !paths::set(&mut updated, path, (*value).clone()))
            .map(|(path, _)| format!("{SERVER_SETTINGS}: {path}"))
            .collect();
        if !missing.is_empty() {
            return Err(Error::MissingField(missing));
        }
        let body = updated.to_string();
        let reply = t.post_json(SETTINGS_WRITE.path, &body)?;
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

    const SETTINGS_JSON: &str = include_str!("../../tests/fixtures/kavita-0.9.1.4/settings.json");
    const INFO_JSON: &str =
        include_str!("../../tests/fixtures/kavita-0.9.1.4/server-info-slim.json");

    /// What the host declares (design §25).
    fn declared() -> ServerSettings {
        let set = [
            ("oidcConfig.provisionAccounts", json!(true)),
            ("oidcConfig.syncUserSettings", json!(true)),
            ("oidcConfig.requireVerifiedEmail", json!(false)),
            ("oidcConfig.disablePasswordAuthentication", json!(true)),
            ("oidcConfig.autoLogin", json!(true)),
            ("oidcConfig.rolesClaim", json!("kavita_roles")),
            ("oidcConfig.rolesPrefix", json!("")),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        ServerSettings { set }
    }

    fn settings(body: &str) -> FakeTransport {
        FakeTransport::default()
            .on_get(SETTINGS_READ.path, vec![ok(body)])
            .on_put(vec![ok(body)])
    }

    #[test]
    fn probe_reads_the_version_and_a_refused_key_is_fatal() {
        let t = FakeTransport::default().on_get(SERVER_INFO.path, vec![ok(INFO_JSON)]);
        assert_eq!(probe(&t).ok().unwrap(), "0.9.1.3");
        for status in [401, 403] {
            let t = FakeTransport::default()
                .on_get(SERVER_INFO.path, vec![Step::Answer(status, String::new())]);
            match probe(&t) {
                Err(Probe::Fatal(e)) => {
                    assert!(e.to_string().contains("no administrator"), "{status}: {e}")
                }
                _ => panic!("{status} is not fatal"),
            }
        }
        let t = FakeTransport::default()
            .on_get(SERVER_INFO.path, vec![Step::Answer(502, String::new())]);
        assert!(matches!(probe(&t), Err(Probe::NotYet(_))));
    }

    #[test]
    fn the_recorded_settings_already_match() {
        let task = declared();
        let current = task.read(&settings(SETTINGS_JSON)).unwrap();
        assert_eq!(task.diff(&current).unwrap(), vec![]);
    }

    #[test]
    fn a_switch_off_is_one_change_and_the_write_keeps_everything_else() {
        let mut recorded: Value = serde_json::from_str(SETTINGS_JSON).unwrap();
        recorded["oidcConfig"]["autoLogin"] = json!(false);
        let body = recorded.to_string();
        let task = declared();
        let t = settings(&body);
        let current = task.read(&t).unwrap();
        let changes = task.diff(&current).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(
            changes[0].to_string(),
            "ServerSettingDto: oidcConfig.autoLogin false -> true"
        );
        task.write(&t, &current).unwrap();
        let written = t.written.borrow();
        assert_eq!(written.len(), 1);
        assert_eq!(written[0].0, "/api/Settings");
        let sent: Value = serde_json::from_str(&written[0].1).unwrap();
        let mut expected = recorded.clone();
        expected["oidcConfig"]["autoLogin"] = json!(true);
        // Everything else travels back as it came -- the masked secret too,
        // which is how Kavita knows to keep the stored one.
        assert_eq!(sent, expected);
        assert_eq!(sent["oidcConfig"]["secret"], json!("********"));
    }

    #[test]
    fn a_path_kavita_does_not_answer_is_an_error_before_any_write() {
        let mut task = declared();
        task.set
            .insert("oidcConfig.autoLogn".to_string(), json!(true));
        let current = task.read(&settings(SETTINGS_JSON)).unwrap();
        let err = task.diff(&current).err().unwrap().to_string();
        assert!(err.contains("oidcConfig.autoLogn"), "{err}");
    }

    #[test]
    fn a_refused_write_shows_kavitas_sentence_but_never_a_long_body() {
        let task = declared();
        let t = FakeTransport::default()
            .on_get(SETTINGS_READ.path, vec![ok(SETTINGS_JSON)])
            .on_put(vec![Step::Answer(
                400,
                "\"The Authority is not valid\"".to_string(),
            )]);
        let current = task.read(&t).unwrap();
        let err = task.write(&t, &current).err().unwrap().to_string();
        assert!(err.contains("HTTP 400"), "{err}");
        assert!(err.contains("The Authority is not valid"), "{err}");

        let long = format!("\"{}\"", "x".repeat(300));
        let t = FakeTransport::default()
            .on_get(SETTINGS_READ.path, vec![ok(SETTINGS_JSON)])
            .on_put(vec![Step::Answer(400, long)]);
        let err = task.write(&t, &current).err().unwrap().to_string();
        assert!(!err.contains("xxx"), "{err}");
    }

    #[test]
    fn an_answer_that_is_no_json_quotes_nothing() {
        let task = declared();
        let err = task
            .read(&settings("password=hunter2"))
            .err()
            .unwrap()
            .to_string();
        assert!(!err.contains("hunter2"), "{err}");
        let err = task.read(&settings("[]")).err().unwrap().to_string();
        assert!(err.contains("not an object"), "{err}");
    }
}
