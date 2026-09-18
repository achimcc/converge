//! Audiobookshelf: the authentication settings (design §27). Read with
//! `GET /api/auth-settings`, written with `PATCH` -- a partial body, key by
//! key, which Audiobookshelf applies in the running process (it switches auth
//! strategies on and off right there, `MiscController.updateAuthSettings`).
//!
//! The answer carries the OIDC client secret in clear text. It comes from a
//! credential, is compared without ever being shown, and is written only when
//! it differs.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::{
    client::{Reply, Transport},
    endpoint::Endpoint,
    engine::{Change, Probe, Task, HIDDEN},
    error::Error,
    secret::Secret,
};

pub const STATUS: Endpoint = Endpoint {
    method: "GET",
    path: "/status",
    request: None,
    response: None,
};
pub const AUTH_SETTINGS: Endpoint = Endpoint {
    method: "GET",
    path: "/api/auth-settings",
    request: None,
    response: None,
};
pub const AUTH_SETTINGS_WRITE: Endpoint = Endpoint {
    method: "PATCH",
    path: "/api/auth-settings",
    request: None,
    response: None,
};
pub const USERS: Endpoint = Endpoint {
    method: "GET",
    path: "/api/users",
    request: None,
    response: None,
};
/// `/api/users/{id}`, with `{"permissions": {...}}`: Audiobookshelf merges
/// the named keys into the account's permissions and takes booleans only
/// (`UserController.update`).
pub const USER_UPDATE: Endpoint = Endpoint {
    method: "PATCH",
    path: "/api/users/{id}",
    request: None,
    response: None,
};
pub const ENDPOINTS: [Endpoint; 5] = [
    STATUS,
    AUTH_SETTINGS,
    AUTH_SETTINGS_WRITE,
    USERS,
    USER_UPDATE,
];

/// The one key Audiobookshelf keeps an empty string for. Every other key it
/// stores `""` as `null` on a write (`MiscController.updateAuthSettings`), and
/// a stored `""` it reads as `null` when comparing -- so the two are one value
/// here as well, or a spec saying `null` against an answer saying `""` would
/// differ forever while Audiobookshelf reports nothing to update.
const KEEPS_EMPTY: &str = "authOpenIDSubfolderForRedirectURLs";

/// Sorted by Audiobookshelf before it compares (`authActiveAuthMethods`).
const SORTED: &str = "authActiveAuthMethods";

#[derive(Deserialize)]
struct Status {
    #[serde(default, rename = "serverVersion")]
    server_version: Option<String>,
    #[serde(default, rename = "isInit")]
    is_init: Option<bool>,
}

/// Readiness: `/status` needs no token and answers with the version once a
/// root account exists. A refused token shows up at the first read.
pub fn probe(t: &dyn Transport) -> Result<String, Probe> {
    let reply = t
        .get(STATUS.path)
        .map_err(|e| Probe::NotYet(e.to_string()))?;
    if reply.status != 200 {
        return Err(Probe::NotYet(format!("HTTP {}", reply.status)));
    }
    let status: Status = serde_json::from_str(&reply.body)
        .map_err(|e| Probe::NotYet(format!("unexpected answer: {e}")))?;
    if status.is_init != Some(true) {
        return Err(Probe::NotYet("no root account yet".to_string()));
    }
    status
        .server_version
        .filter(|v| !v.is_empty())
        .ok_or_else(|| Probe::NotYet("the answer has no version".to_string()))
}

/// A status error that carries nothing from the body: the settings hold the
/// client secret.
fn refuse(ep: &Endpoint, reply: &Reply) -> Error {
    Error::Status {
        method: ep.method,
        path: ep.path.to_string(),
        status: reply.status,
        validation: match reply.status {
            401 => vec!["the token was refused".to_string()],
            403 => vec!["the token's account is no administrator".to_string()],
            _ => Vec::new(),
        },
    }
}

/// The value as Audiobookshelf compares it.
fn normalized(key: &str, value: &Value) -> Value {
    match value {
        Value::String(s) if s.is_empty() && key != KEEPS_EMPTY => Value::Null,
        Value::Array(items) if key == SORTED => {
            let mut items = items.clone();
            items.sort_by_key(|v| v.to_string());
            Value::Array(items)
        }
        other => other.clone(),
    }
}

pub struct AuthSettings {
    pub set: BTreeMap<String, Value>,
    pub secrets: BTreeMap<String, Secret>,
}

const SUBJECT: &str = "auth settings";

impl AuthSettings {
    fn missing(&self, current: &Map<String, Value>) -> Vec<String> {
        self.set
            .keys()
            .chain(self.secrets.keys())
            .filter(|k| !current.contains_key(*k))
            .map(|k| format!("{SUBJECT}: {k}"))
            .collect()
    }

    /// The keys to send, with the value to send.
    fn differing(&self, current: &Map<String, Value>) -> Vec<(String, Value, bool)> {
        let mut out = Vec::new();
        for (key, desired) in &self.set {
            let now = current.get(key).unwrap_or(&Value::Null);
            if normalized(key, now) != normalized(key, desired) {
                out.push((key.clone(), desired.clone(), false));
            }
        }
        for (key, secret) in &self.secrets {
            let now = current.get(key).and_then(Value::as_str).unwrap_or("");
            if now != secret.expose() {
                out.push((
                    key.clone(),
                    Value::String(secret.expose().to_string()),
                    true,
                ));
            }
        }
        out
    }
}

impl Task for AuthSettings {
    type Current = Map<String, Value>;

    fn probe(&self, t: &dyn Transport) -> Result<String, Probe> {
        probe(t)
    }

    fn read(&self, t: &dyn Transport) -> Result<Self::Current, Error> {
        let reply = t.get(AUTH_SETTINGS.path)?;
        if reply.status != 200 {
            return Err(refuse(&AUTH_SETTINGS, &reply));
        }
        // serde's message could quote a value -- the secret is in here.
        match serde_json::from_str::<Value>(&reply.body) {
            Ok(Value::Object(map)) => Ok(map),
            Ok(_) => Err(Error::Decode {
                path: AUTH_SETTINGS.path.to_string(),
                reason: "not an object".to_string(),
            }),
            Err(e) => Err(Error::Decode {
                path: AUTH_SETTINGS.path.to_string(),
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
            .map(|(key, desired, secret)| Change {
                subject: SUBJECT.to_string(),
                current: if secret {
                    HIDDEN.to_string()
                } else {
                    current.get(&key).unwrap_or(&Value::Null).to_string()
                },
                desired: if secret {
                    HIDDEN.to_string()
                } else {
                    desired.to_string()
                },
                field: key,
            })
            .collect())
    }

    fn notes(&self, _current: &Self::Current) -> Vec<String> {
        Vec::new()
    }

    /// Only what differs travels: Audiobookshelf applies the body key by key.
    fn write(&self, t: &dyn Transport, current: &Self::Current) -> Result<(), Error> {
        let body: Map<String, Value> = self
            .differing(current)
            .into_iter()
            .map(|(key, value, _)| (key, value))
            .collect();
        if body.is_empty() {
            return Ok(());
        }
        let reply = t.patch_json(AUTH_SETTINGS_WRITE.path, &Value::Object(body).to_string())?;
        if reply.status != 200 {
            return Err(refuse(&AUTH_SETTINGS_WRITE, &reply));
        }
        Ok(())
    }
}

// --- admin-permissions (design §28) ----------------------------------------

/// Permissions every account of the named types must hold. Audiobookshelf
/// gives an account type no permission of its own (`User.canDelete` reads
/// `permissions.delete` alone), and an account created by an OIDC login
/// starts with a reader's -- so an administrator could not delete anything
/// until someone set the fields. The advanced-permissions claim cannot do it:
/// Audiobookshelf skips admin and root accounts there
/// (`OidcAuthStrategy.js`, `if (user.type === 'admin' || ...) return`).
///
/// The answer of `GET /api/users` carries each account's `token`. Nothing but
/// the username, the type and the permission names and values named here is
/// ever shown, and no error carries anything from a body.
pub struct AdminPermissions {
    pub types: Vec<String>,
    pub permissions: BTreeMap<String, bool>,
}

/// A permission that differs: its name, the value now (if any), the value
/// wanted.
type Difference = (String, Option<bool>, bool);

#[derive(Deserialize)]
struct Users {
    users: Vec<UserEntry>,
}

/// Only these fields are kept; the token and everything else is dropped at
/// decoding.
#[derive(Deserialize, Clone)]
pub struct UserEntry {
    id: String,
    #[serde(default)]
    username: Option<String>,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    permissions: Map<String, Value>,
}

impl UserEntry {
    fn label(&self) -> String {
        format!("account {}", self.username.as_deref().unwrap_or(&self.id))
    }
}

impl AdminPermissions {
    /// Per account of a named type, the permissions that differ.
    fn differing<'a>(&self, users: &'a [UserEntry]) -> Vec<(&'a UserEntry, Vec<Difference>)> {
        users
            .iter()
            .filter(|u| self.types.contains(&u.kind))
            .filter_map(|u| {
                let diffs: Vec<Difference> = self
                    .permissions
                    .iter()
                    .filter_map(|(key, desired)| {
                        let now = u.permissions.get(key).and_then(Value::as_bool);
                        (now != Some(*desired)).then(|| (key.clone(), now, *desired))
                    })
                    .collect();
                (!diffs.is_empty()).then_some((u, diffs))
            })
            .collect()
    }
}

impl Task for AdminPermissions {
    type Current = Vec<UserEntry>;

    fn probe(&self, t: &dyn Transport) -> Result<String, Probe> {
        probe(t)
    }

    fn read(&self, t: &dyn Transport) -> Result<Self::Current, Error> {
        let reply = t.get(USERS.path)?;
        if reply.status != 200 {
            return Err(refuse(&USERS, &reply));
        }
        let users: Users = serde_json::from_str(&reply.body).map_err(|e| Error::Decode {
            path: USERS.path.to_string(),
            reason: format!(
                "not a list of users ({:?} error at line {} column {})",
                e.classify(),
                e.line(),
                e.column()
            ),
        })?;
        Ok(users.users)
    }

    fn diff(&self, current: &Self::Current) -> Result<Vec<Change>, Error> {
        Ok(self
            .differing(current)
            .into_iter()
            .flat_map(|(user, diffs)| {
                diffs.into_iter().map(move |(key, now, desired)| Change {
                    subject: user.label(),
                    field: format!("permissions.{key}"),
                    current: now.map_or("(missing)".to_string(), |b| b.to_string()),
                    desired: desired.to_string(),
                })
            })
            .collect())
    }

    fn notes(&self, current: &Self::Current) -> Vec<String> {
        if current.iter().any(|u| self.types.contains(&u.kind)) {
            Vec::new()
        } else {
            vec![format!(
                "no account of type {} yet -- nothing to set",
                self.types.join("/")
            )]
        }
    }

    /// One PATCH per account, with the differing keys only.
    fn write(&self, t: &dyn Transport, current: &Self::Current) -> Result<(), Error> {
        for (user, diffs) in self.differing(current) {
            let permissions: Map<String, Value> = diffs
                .into_iter()
                .map(|(key, _, desired)| (key, Value::Bool(desired)))
                .collect();
            let body = serde_json::json!({ "permissions": permissions }).to_string();
            let path = USER_UPDATE.path.replace("{id}", &user.id);
            let reply = t.patch_json(&path, &body)?;
            if reply.status != 200 {
                return Err(Error::Status {
                    method: USER_UPDATE.method,
                    path,
                    status: reply.status,
                    validation: match reply.status {
                        403 => vec!["the token's account may not change this account".to_string()],
                        _ => Vec::new(),
                    },
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::testing::{ok, FakeTransport, Step};

    const STATUS_JSON: &str =
        include_str!("../../tests/fixtures/audiobookshelf-2.36.0/status.json");
    const SETTINGS_JSON: &str =
        include_str!("../../tests/fixtures/audiobookshelf-2.36.0/auth-settings.json");
    const SECRET: &str = "<masked>";

    /// What the host declares, with the masked client id and secret of the
    /// fixture standing in for the real ones.
    fn declared(secret: &str) -> AuthSettings {
        let set = json!({
            "authOpenIDIssuerURL": "https://auth.rusty-vault.de/application/o/audiobookshelf/",
            "authOpenIDClientID": "<masked>",
            "authOpenIDGroupClaim": "abs_roles",
            "authOpenIDSubfolderForRedirectURLs": "",
            "authOpenIDMobileRedirectURIs": ["audiobookshelf://oauth", "shelfplayer://callback", "lissen://oauth", "storii://oauth"],
            "authOpenIDAutoRegister": true,
            "authOpenIDAutoLaunch": true,
            "authOpenIDMatchExistingBy": null,
            "authOpenIDAdvancedPermsClaim": null,
            "authActiveAuthMethods": ["openid"]
        });
        AuthSettings {
            set: serde_json::from_value(set).unwrap(),
            secrets: [(
                "authOpenIDClientSecret".to_string(),
                Secret::new(secret.to_string()),
            )]
            .into_iter()
            .collect(),
        }
    }

    fn settings(body: &str) -> FakeTransport {
        FakeTransport::default()
            .on_get(AUTH_SETTINGS.path, vec![ok(body)])
            .on_put(vec![ok(r#"{"updated":true}"#)])
    }

    #[test]
    fn probe_reads_the_version_once_a_root_account_exists() {
        let t = FakeTransport::default().on_get(STATUS.path, vec![ok(STATUS_JSON)]);
        assert_eq!(probe(&t).ok().unwrap(), "2.36.0");
        let t = FakeTransport::default().on_get(
            STATUS.path,
            vec![ok(r#"{"isInit":false,"serverVersion":"2.36.0"}"#)],
        );
        assert!(matches!(probe(&t), Err(Probe::NotYet(_))));
    }

    #[test]
    fn the_recorded_settings_match_and_an_empty_string_is_null() {
        // The fixture holds `authOpenIDAdvancedPermsClaim: ""`; the spec says
        // null. Audiobookshelf treats both as one.
        let task = declared(SECRET);
        let current = task.read(&settings(SETTINGS_JSON)).unwrap();
        assert_eq!(current["authOpenIDAdvancedPermsClaim"], json!(""));
        assert_eq!(task.diff(&current).unwrap(), vec![]);
    }

    #[test]
    fn the_subfolder_keeps_its_empty_string() {
        let mut task = declared(SECRET);
        task.set.insert(KEEPS_EMPTY.to_string(), Value::Null);
        let current = task.read(&settings(SETTINGS_JSON)).unwrap();
        assert_eq!(task.diff(&current).unwrap().len(), 1);
    }

    #[test]
    fn a_rotated_secret_is_a_hidden_change_and_only_what_differs_is_sent() {
        let rotated = "a-brand-new-client-secret-4711";
        let mut recorded: Value = serde_json::from_str(SETTINGS_JSON).unwrap();
        recorded["authOpenIDAutoLaunch"] = json!(false);
        let task = declared(rotated);
        let t = settings(&recorded.to_string());
        let current = task.read(&t).unwrap();
        let changes: Vec<String> = task
            .diff(&current)
            .unwrap()
            .iter()
            .map(|c| c.to_string())
            .collect();
        assert_eq!(
            changes,
            [
                "auth settings: authOpenIDAutoLaunch false -> true",
                "auth settings: authOpenIDClientSecret (hidden) -> (hidden)"
            ]
        );
        assert!(!changes.concat().contains(rotated));
        task.write(&t, &current).unwrap();
        let written = t.written.borrow();
        assert_eq!(written.len(), 1);
        assert_eq!(written[0].0, "/api/auth-settings");
        let sent: Value = serde_json::from_str(&written[0].1).unwrap();
        assert_eq!(
            sent,
            json!({"authOpenIDAutoLaunch": true, "authOpenIDClientSecret": rotated})
        );
    }

    const USERS_JSON: &str = include_str!("../../tests/fixtures/audiobookshelf-2.36.0/users.json");

    fn admin_rights() -> AdminPermissions {
        AdminPermissions {
            types: vec!["admin".into(), "root".into()],
            permissions: [
                "download",
                "update",
                "delete",
                "upload",
                "createEreader",
                "accessAllLibraries",
                "accessAllTags",
            ]
            .into_iter()
            .map(|k| (k.to_string(), true))
            .collect(),
        }
    }

    fn users(body: &str) -> FakeTransport {
        FakeTransport::default()
            .on_get(USERS.path, vec![ok(body)])
            .on_put(vec![ok("{}")])
    }

    #[test]
    fn the_recorded_admins_already_hold_their_rights_and_users_are_untouched() {
        let task = admin_rights();
        let current = task.read(&users(USERS_JSON)).unwrap();
        assert_eq!(current.len(), 5);
        assert_eq!(task.diff(&current).unwrap(), vec![]);
        assert_eq!(task.notes(&current), Vec::<String>::new());
    }

    #[test]
    fn a_new_admin_with_a_readers_rights_gets_only_the_missing_keys() {
        // The third account (a reader) becomes an admin by its group claim.
        let mut recorded: Value = serde_json::from_str(USERS_JSON).unwrap();
        recorded["users"][2]["type"] = json!("admin");
        recorded["users"][2]["token"] = json!("leaky-token-4711");
        recorded["users"][2]["permissions"]
            .as_object_mut()
            .unwrap()
            .remove("createEreader");
        let task = admin_rights();
        let t = users(&recorded.to_string());
        let current = task.read(&t).unwrap();
        let changes: Vec<String> = task
            .diff(&current)
            .unwrap()
            .iter()
            .map(|c| c.to_string())
            .collect();
        assert_eq!(
            changes,
            [
                "account konto3: permissions.createEreader (missing) -> true",
                "account konto3: permissions.delete false -> true",
                "account konto3: permissions.update false -> true",
                "account konto3: permissions.upload false -> true",
            ]
        );
        assert!(!changes.concat().contains("leaky"));
        task.write(&t, &current).unwrap();
        let written = t.written.borrow();
        assert_eq!(written.len(), 1);
        assert_eq!(written[0].0, "/api/users/user-id-3");
        let sent: Value = serde_json::from_str(&written[0].1).unwrap();
        assert_eq!(
            sent,
            json!({"permissions": {"createEreader": true, "delete": true, "update": true, "upload": true}})
        );
    }

    #[test]
    fn no_account_of_the_types_is_a_note_and_a_refusal_names_nothing() {
        let task = admin_rights();
        let only_readers = r#"{"users":[{"id":"u1","username":"a","type":"user","permissions":{},"token":"leaky"}]}"#;
        let current = task.read(&users(only_readers)).unwrap();
        assert_eq!(task.diff(&current).unwrap(), vec![]);
        assert_eq!(
            task.notes(&current),
            ["no account of type admin/root yet -- nothing to set"]
        );
        let t = FakeTransport::default()
            .on_get(USERS.path, vec![Step::Answer(403, "leaky-token".into())]);
        let err = task.read(&t).err().unwrap().to_string();
        assert!(
            err.contains("no administrator") && !err.contains("leaky"),
            "{err}"
        );
        let err = task
            .read(&users("{\"users\": \"leaky-token\"}"))
            .err()
            .unwrap()
            .to_string();
        assert!(!err.contains("leaky"), "{err}");
    }

    #[test]
    fn a_refused_token_and_a_missing_key_name_nothing_from_the_body() {
        let task = declared(SECRET);
        for (status, words) in [(401, "refused"), (403, "no administrator")] {
            let t = FakeTransport::default().on_get(
                AUTH_SETTINGS.path,
                vec![Step::Answer(status, "secret-in-body".into())],
            );
            let err = task.read(&t).err().unwrap().to_string();
            assert!(err.contains(words), "{err}");
            assert!(!err.contains("secret-in-body"), "{err}");
        }
        let mut t2 = declared(SECRET);
        t2.set.insert("authOpenIDNoSuchKey".into(), json!(1));
        let current = t2.read(&settings(SETTINGS_JSON)).unwrap();
        let err = t2.diff(&current).err().unwrap().to_string();
        assert!(err.contains("authOpenIDNoSuchKey"), "{err}");
    }
}
