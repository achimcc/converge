//! SuggestArr (2.14.0): its whole flat configuration document.
//!
//! SuggestArr publishes an OpenAPI description for its public `/api/v1`
//! surface only, and that surface carries jobs and webhooks — not the
//! configuration. Field names here therefore come from recorded answers
//! (`tests/fixtures/suggestarr-2.14.0`) and from the service's source, as
//! for ntfy, bindery, Seerr and Koel.
//!
//! Three things about this service decide the shape of this module:
//!
//! * **The database is authoritative, the file is not.** `config.yaml` is
//!   read at startup and copied into the `integrations` table when a row is
//!   missing or empty (`migrate_integrations_from_config`); after that the
//!   table wins on every read (`merge_db_integrations_into_flat`). Writing
//!   the file therefore sets a value once and never again — a rotated key
//!   would never arrive. `POST /api/config/save` writes both.
//!
//! * **A partial write is a loss.** `save_env_vars` builds the file from
//!   *every* known key, taking each one from the request body or else from
//!   its **default**. A body with one field would reset all the others. The
//!   document is therefore read, changed and sent back whole — the same
//!   shape as Jellyfin's named configurations.
//!
//! * **The admin endpoints answer with real keys.** `GET /api/config/fetch`
//!   returns secrets in plaintext (it is admin-only). converge can compare
//!   them without a `hand_over`, but no value may reach a change line or an
//!   error: every secret field is reported as `(hidden)`.
//!
//! Authentication is a JWT from `POST /api/auth/login` (see `login`). The
//! account is a named service account; its password lives in a systemd
//! credential.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::{
    client::{expect_status_at, Transport},
    endpoint::{Endpoint, Shape},
    engine::{Change, Probe, Task, HIDDEN},
    error::Error,
    secret::Secret,
};

pub const LOGIN: Endpoint = Endpoint {
    method: "POST",
    path: "/api/auth/login",
    request: None,
    response: None,
};

pub const FETCH: Endpoint = Endpoint {
    method: "GET",
    path: "/api/config/fetch",
    request: None,
    response: Some(Shape::Opaque("Configuration")),
};

pub const SAVE: Endpoint = Endpoint {
    method: "POST",
    path: "/api/config/save",
    request: Some(Shape::Opaque("Configuration")),
    response: None,
};

pub const LIBRARIES: Endpoint = Endpoint {
    method: "GET",
    path: "/api/jellyfin/libraries",
    request: None,
    response: None,
};

/// SuggestArr reports no version: `/api/v1/status` names the API version
/// (`v1`) and `/api/health` the state of its dependencies. As for Koel, the
/// readiness probe therefore returns this instead of a version.
pub const VERSION_NOT_REPORTED: &str = "(not reported)";

/// The key the fetch endpoint adds to the flat configuration: the
/// integrations table, as an object. It is not a configuration key — it is
/// the endpoint's convenience for the frontend — and it is never sent back.
const INTEGRATIONS: &str = "integrations";

/// The fields whose value is a secret. The names are SuggestArr's own
/// `_SECRET_KEYS` (`blueprints/config/routes.py`); a value of any of them is
/// reported as `(hidden)`, whether it comes from a credential or not.
const SECRET_FIELDS: &[&str] = &[
    "TMDB_API_KEY",
    "OMDB_API_KEY",
    "PLEX_TOKEN",
    "JELLYFIN_TOKEN",
    "SEER_TOKEN",
    "SEER_USER_PSW",
    "SEER_SESSION_TOKEN",
    "DB_PASSWORD",
    "OPENAI_API_KEY",
    "TRAKT_CLIENT_SECRET",
    "TRAKT_ACCESS_TOKEN",
    "TRAKT_REFRESH_TOKEN",
];

pub fn is_secret_field(name: &str) -> bool {
    SECRET_FIELDS.contains(&name)
}

/// The libraries SuggestArr reads a watch history from, as the desired state
/// describes them: everything Jellyfin has, minus the collection types named
/// here. An empty list in SuggestArr does **not** mean "none" — the client
/// then fetches all of them — so the home videos have to be named out.
#[derive(Debug, Clone, PartialEq)]
pub struct LibraryRule {
    pub exclude_collection_types: Vec<String>,
}

/// One library as `GET /api/jellyfin/libraries` reports it (Jellyfin's
/// `VirtualFolders` shape, which SuggestArr passes through). Only the three
/// fields read are declared.
#[derive(Debug, Clone, Deserialize)]
pub struct Library {
    #[serde(rename = "ItemId")]
    pub item_id: Option<String>,
    #[serde(rename = "Name")]
    pub name: Option<String>,
    #[serde(rename = "CollectionType")]
    #[serde(default)]
    pub collection_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LibraryAnswer {
    items: Vec<Library>,
}

/// What a login answered. Only the token is read.
#[derive(Deserialize)]
struct LoginAnswer {
    access_token: Secret,
}

/// Exchanges username and password for a JWT. The password never travels in
/// `argv` and neither value reaches a log line: the body is built here, and
/// a failure names the status, never the credentials.
///
/// `/api/auth/login` is one of the few routes SuggestArr lets through
/// unauthenticated, and the middleware accepts the resulting bearer token
/// before it ever looks at `AUTH_MODE` — so this works in `trusted_header`
/// mode without widening the header's trusted peers.
pub fn login(t: &dyn Transport, username: &str, password: &Secret) -> Result<Secret, Error> {
    let body = serde_json::to_string(&json!({
        "username": username,
        "password": password.expose(),
    }))
    .map_err(|e| Error::Request {
        method: LOGIN.method,
        path: LOGIN.path.to_string(),
        reason: format!("cannot serialize: {e}"),
    })?;
    let reply = t.post_json(LOGIN.path, &body)?;
    if reply.status == 401 || reply.status == 403 {
        return Err(Error::Status {
            method: LOGIN.method,
            path: LOGIN.path.to_string(),
            status: reply.status,
            validation: vec![format!(
                "the account {username} or its password was refused"
            )],
        });
    }
    expect_status_at(LOGIN.method, LOGIN.path, &reply, &[200])?;
    let answer: LoginAnswer = serde_json::from_str(&reply.body).map_err(|e| Error::Decode {
        path: LOGIN.path.to_string(),
        reason: e.to_string(),
    })?;
    Ok(answer.access_token)
}

/// The whole configuration: plain fields, fields whose value comes from a
/// systemd credential, and — optionally — the Jellyfin libraries, derived
/// from what the service itself reports rather than written down.
pub struct Configuration {
    pub set: BTreeMap<String, Value>,
    pub secrets: BTreeMap<String, Secret>,
    pub libraries: Option<LibraryRule>,
}

/// What a run reads: the configuration document, and the libraries when the
/// desired state derives them.
pub struct Snapshot {
    pub config: Value,
    pub libraries: Vec<Library>,
}

impl Configuration {
    /// The desired `JELLYFIN_LIBRARIES` value, in the shape SuggestArr
    /// stores and its client reads (`{id, name}`, `jellyfin_client.py`).
    ///
    /// An empty result is an error: without a library there is no watch
    /// history, the automation would quietly suggest nothing, and nothing
    /// would turn red.
    fn desired_libraries(&self, rule: &LibraryRule, current: &[Library]) -> Result<Value, Error> {
        let excluded: BTreeSet<&str> = rule
            .exclude_collection_types
            .iter()
            .map(String::as_str)
            .collect();
        let kept: Vec<Value> = current
            .iter()
            .filter(|l| !excluded.contains(l.collection_type.as_deref().unwrap_or_default()))
            .filter_map(|l| match (&l.item_id, &l.name) {
                (Some(id), Some(name)) if !id.is_empty() && !name.is_empty() => {
                    Some(json!({ "id": id, "name": name }))
                }
                _ => None,
            })
            .collect();
        if kept.is_empty() {
            return Err(Error::EmptyList {
                path: format!("{}: every library is excluded or unnamed", LIBRARIES.path),
            });
        }
        Ok(Value::Array(kept))
    }

    /// Every field this task writes, with its desired value.
    fn wanted(&self, current: &Snapshot) -> Result<BTreeMap<String, Value>, Error> {
        let mut wanted = self.set.clone();
        for (field, secret) in &self.secrets {
            wanted.insert(field.clone(), Value::String(secret.expose().to_string()));
        }
        if let Some(rule) = &self.libraries {
            wanted.insert(
                "JELLYFIN_LIBRARIES".to_string(),
                self.desired_libraries(rule, &current.libraries)?,
            );
        }
        Ok(wanted)
    }
}

fn decode_config(body: &str) -> Result<Value, Error> {
    let document: Value = serde_json::from_str(body).map_err(|e| Error::Decode {
        path: FETCH.path.to_string(),
        reason: e.to_string(),
    })?;
    if !document.is_object() {
        return Err(Error::Decode {
            path: FETCH.path.to_string(),
            reason: "not an object".to_string(),
        });
    }
    Ok(document)
}

/// A value for a change line: hidden for a secret field, shortened
/// otherwise. A library list is long, so it is named by its entries.
fn shown(field: &str, value: Option<&Value>) -> String {
    if is_secret_field(field) {
        return HIDDEN.to_string();
    }
    match value {
        None => "(absent)".to_string(),
        Some(Value::Array(entries)) if field == "JELLYFIN_LIBRARIES" => {
            let names: Vec<&str> = entries
                .iter()
                .filter_map(|e| e.get("name").and_then(Value::as_str))
                .collect();
            format!("[{}]", names.join(", "))
        }
        Some(value) => crate::engine::shortened(value),
    }
}

impl Task for Configuration {
    type Current = Snapshot;

    /// Readiness and the token in one request, as for Koel: the
    /// configuration itself. A refused token stays refused however long one
    /// waits; a service still starting answers 5xx or refuses the connection.
    ///
    /// With a library rule, readiness also means the libraries answer: they
    /// come from Jellyfin *through* SuggestArr, and when both restart
    /// together SuggestArr is up well before Jellyfin — it then answers 404
    /// ("No library found") or 5xx. Waiting helps there, so any answer but
    /// 200 is `NotYet`; an empty list in a 200 is still an error in `read`.
    fn probe(&self, t: &dyn Transport) -> Result<String, Probe> {
        let reply = t
            .get(FETCH.path)
            .map_err(|e| Probe::NotYet(e.to_string()))?;
        match reply.status {
            200 => {
                decode_config(&reply.body).map_err(Probe::Fatal)?;
                if self.libraries.is_some() {
                    let libraries = t
                        .get(LIBRARIES.path)
                        .map_err(|e| Probe::NotYet(e.to_string()))?;
                    if libraries.status != 200 {
                        return Err(Probe::NotYet(format!(
                            "{} answered HTTP {} -- Jellyfin behind SuggestArr is not ready",
                            LIBRARIES.path, libraries.status
                        )));
                    }
                }
                Ok(VERSION_NOT_REPORTED.to_string())
            }
            401 | 403 => Err(Probe::Fatal(Error::Status {
                method: FETCH.method,
                path: FETCH.path.to_string(),
                status: reply.status,
                validation: vec!["the token was refused".to_string()],
            })),
            other => Err(Probe::NotYet(format!("HTTP {other}"))),
        }
    }

    fn read(&self, t: &dyn Transport) -> Result<Snapshot, Error> {
        let reply = t.get(FETCH.path)?;
        expect_status_at(FETCH.method, FETCH.path, &reply, &[200])?;
        let config = decode_config(&reply.body)?;

        let libraries = match self.libraries {
            None => Vec::new(),
            Some(_) => {
                let reply = t.get(LIBRARIES.path)?;
                expect_status_at(LIBRARIES.method, LIBRARIES.path, &reply, &[200])?;
                let answer: LibraryAnswer =
                    serde_json::from_str(&reply.body).map_err(|e| Error::Decode {
                        path: LIBRARIES.path.to_string(),
                        reason: e.to_string(),
                    })?;
                if answer.items.is_empty() {
                    return Err(Error::EmptyList {
                        path: LIBRARIES.path.to_string(),
                    });
                }
                answer.items
            }
        };
        Ok(Snapshot { config, libraries })
    }

    fn diff(&self, current: &Snapshot) -> Result<Vec<Change>, Error> {
        let wanted = self.wanted(current)?;
        let document = current.config.as_object().expect("read checked the shape");

        // A field the document does not name is a typo in the spec, not
        // something to create: `fetch` answers with every key the service
        // knows, including the ones that are unset.
        let missing: Vec<String> = wanted
            .keys()
            .filter(|field| !document.contains_key(*field))
            .map(|field| format!("Configuration: {field}"))
            .collect();
        if !missing.is_empty() {
            return Err(Error::MissingField(missing));
        }

        Ok(wanted
            .iter()
            .filter(|(field, desired)| document.get(*field) != Some(*desired))
            .map(|(field, desired)| Change {
                subject: "Configuration".to_string(),
                field: field.clone(),
                current: shown(field, document.get(field)),
                desired: shown(field, Some(desired)),
            })
            .collect())
    }

    fn notes(&self, current: &Snapshot) -> Vec<String> {
        let Some(rule) = &self.libraries else {
            return Vec::new();
        };
        let excluded: Vec<String> = current
            .libraries
            .iter()
            .filter(|l| {
                rule.exclude_collection_types
                    .iter()
                    .any(|t| Some(t.as_str()) == l.collection_type.as_deref())
            })
            .map(|l| {
                format!(
                    "{} ({})",
                    l.name.as_deref().unwrap_or("(unnamed)"),
                    l.collection_type.as_deref().unwrap_or("(no type)")
                )
            })
            .collect();
        if excluded.is_empty() {
            return vec![format!(
                "no library of Jellyfin's {} is excluded",
                rule.exclude_collection_types.join(", ")
            )];
        }
        vec![format!("libraries left out: {}", excluded.join(", "))]
    }

    fn write(&self, t: &dyn Transport, current: &Snapshot) -> Result<(), Error> {
        let wanted = self.wanted(current)?;
        let mut document: Map<String, Value> = current
            .config
            .as_object()
            .expect("read checked the shape")
            .clone();
        // Never send back what the endpoint only added for the frontend.
        document.remove(INTEGRATIONS);
        for (field, value) in wanted {
            document.insert(field, value);
        }
        let body = serde_json::to_string(&Value::Object(document)).map_err(|e| Error::Request {
            method: SAVE.method,
            path: SAVE.path.to_string(),
            reason: format!("cannot serialize: {e}"),
        })?;
        let reply = t.post_json(SAVE.path, &body)?;
        expect_status_at(SAVE.method, SAVE.path, &reply, &[200])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{ok, FakeTransport, Step};

    const FETCHED: &str = include_str!("../../tests/fixtures/suggestarr-2.14.0/config-fetch.json");
    const LIBRARY_ANSWER: &str =
        include_str!("../../tests/fixtures/suggestarr-2.14.0/jellyfin-libraries.json");

    fn fields(pairs: &[(&str, Value)]) -> BTreeMap<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect()
    }

    fn task(set: &[(&str, Value)], secrets: &[(&str, &str)]) -> Configuration {
        Configuration {
            set: fields(set),
            secrets: secrets
                .iter()
                .map(|(f, v)| ((*f).to_string(), Secret::new((*v).to_string())))
                .collect(),
            libraries: None,
        }
    }

    fn transport() -> FakeTransport {
        FakeTransport::default()
            .on_get(FETCH.path, vec![ok(FETCHED)])
            .on_get(LIBRARIES.path, vec![ok(LIBRARY_ANSWER)])
            .on_put(vec![Step::Answer(
                200,
                r#"{"status":"success"}"#.to_string(),
            )])
    }

    #[test]
    fn a_field_that_already_holds_the_wanted_value_is_no_change() {
        let task = task(&[("CRON_TIMES", json!("0 4 * * *"))], &[]);
        let t = transport();
        assert_eq!(task.diff(&task.read(&t).unwrap()).unwrap(), vec![]);
    }

    #[test]
    fn a_plain_field_is_compared_by_value() {
        let task = task(&[("FILTER_RATING_SOURCE", json!("both"))], &[]);
        let t = transport();
        let changes = task.diff(&task.read(&t).unwrap()).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(
            changes[0].to_string(),
            r#"Configuration: FILTER_RATING_SOURCE "tmdb" -> "both""#
        );
    }

    #[test]
    fn a_secret_never_reaches_the_change_line() {
        let task = task(&[], &[("OMDB_API_KEY", "the-real-omdb-key")]);
        let t = transport();
        let changes = task.diff(&task.read(&t).unwrap()).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(
            changes[0].to_string(),
            "Configuration: OMDB_API_KEY (hidden) -> (hidden)"
        );
        assert!(!changes[0].to_string().contains("the-real-omdb-key"));
    }

    #[test]
    fn a_field_the_service_does_not_know_is_missing_not_added() {
        let task = task(&[("OMDB_API_KEZ", json!("x"))], &[]);
        let t = transport();
        let current = task.read(&t).unwrap();
        assert!(matches!(task.diff(&current), Err(Error::MissingField(_))));
    }

    #[test]
    fn the_write_carries_the_whole_document_without_integrations() {
        let task = task(&[("FILTER_IMDB_THRESHOLD", json!(6.0))], &[]);
        let t = transport();
        let current = task.read(&t).unwrap();
        task.write(&t, &current).unwrap();
        let written = t.written.borrow();
        assert_eq!(written[0].0, SAVE.path);
        let sent: Value = serde_json::from_str(&written[0].1).unwrap();
        assert!(sent.get(INTEGRATIONS).is_none(), "integrations is not sent");
        assert_eq!(sent["FILTER_IMDB_THRESHOLD"], json!(6.0));
        // Everything else travels unchanged, or the service would reset it
        // to its default.
        let fetched: Value = serde_json::from_str(FETCHED).unwrap();
        assert_eq!(sent["CRON_TIMES"], fetched["CRON_TIMES"]);
        assert_eq!(sent["SELECTED_SERVICE"], fetched["SELECTED_SERVICE"]);
        assert_eq!(
            sent.as_object().unwrap().len(),
            fetched.as_object().unwrap().len() - 1,
            "every key but integrations"
        );
    }

    #[test]
    fn libraries_come_from_the_service_and_leave_the_home_videos_out() {
        let task = Configuration {
            set: BTreeMap::new(),
            secrets: BTreeMap::new(),
            libraries: Some(LibraryRule {
                exclude_collection_types: vec!["homevideos".to_string()],
            }),
        };
        let t = transport();
        let current = task.read(&t).unwrap();
        // The fixture was recorded with exactly this desired state in place.
        assert_eq!(task.diff(&current).unwrap(), vec![]);
        assert_eq!(
            task.notes(&current),
            vec!["libraries left out: Privat (homevideos)".to_string()]
        );
    }

    #[test]
    fn a_library_list_that_would_be_empty_is_an_error() {
        let task = Configuration {
            set: BTreeMap::new(),
            secrets: BTreeMap::new(),
            libraries: Some(LibraryRule {
                exclude_collection_types: vec![
                    "boxsets".to_string(),
                    "movies".to_string(),
                    "homevideos".to_string(),
                    "music".to_string(),
                    "tvshows".to_string(),
                ],
            }),
        };
        let t = transport();
        let current = task.read(&t).unwrap();
        assert!(matches!(task.diff(&current), Err(Error::EmptyList { .. })));
    }

    #[test]
    fn a_changed_library_set_names_the_libraries_not_their_ids() {
        let task = Configuration {
            set: BTreeMap::new(),
            secrets: BTreeMap::new(),
            libraries: Some(LibraryRule {
                exclude_collection_types: vec!["homevideos".to_string(), "music".to_string()],
            }),
        };
        let t = transport();
        let changes = task.diff(&task.read(&t).unwrap()).unwrap();
        assert_eq!(changes.len(), 1);
        let line = changes[0].to_string();
        assert!(line.contains("Musik"), "{line}");
        assert!(!line.contains("id"), "{line}");
    }

    #[test]
    fn an_empty_library_answer_is_an_error_not_an_empty_list() {
        let task = Configuration {
            set: BTreeMap::new(),
            secrets: BTreeMap::new(),
            libraries: Some(LibraryRule {
                exclude_collection_types: vec!["homevideos".to_string()],
            }),
        };
        let t = FakeTransport::default()
            .on_get(FETCH.path, vec![ok(FETCHED)])
            .on_get(LIBRARIES.path, vec![ok(r#"{"items":[],"message":"none"}"#)]);
        assert!(matches!(task.read(&t), Err(Error::EmptyList { .. })));
    }

    #[test]
    fn login_returns_the_token_and_a_refusal_names_no_password() {
        let t = FakeTransport::default().on_put(vec![
            Step::Answer(
                200,
                r#"{"access_token":"jwt-value","role":"admin","username":"converge"}"#.to_string(),
            ),
            Step::Answer(401, r#"{"error":"Invalid credentials"}"#.to_string()),
        ]);
        let token = match login(&t, "converge", &Secret::new("pw".to_string())) {
            Ok(token) => token,
            Err(e) => panic!("the login failed: {e}"),
        };
        assert_eq!(token.expose(), "jwt-value");
        let Err(err) = login(&t, "converge", &Secret::new("pw".to_string())) else {
            panic!("a refused password is an error");
        };
        let text = err.to_string();
        assert!(text.contains("converge"), "{text}");
        assert!(!text.contains("pw"), "{text}");
    }

    #[test]
    fn probe_waits_while_jellyfin_behind_suggestarr_has_no_libraries_yet() {
        // Recorded 2026-09-17 on the host: req-01 and jelly-01 restarted in
        // the same deploy, SuggestArr answered its config at once, but
        // `/api/jellyfin/libraries` said 404 ("No library found") for the 40
        // seconds Jellyfin needed. That is a service still starting, not a
        // configuration error.
        let task = Configuration {
            set: BTreeMap::new(),
            secrets: BTreeMap::new(),
            libraries: Some(LibraryRule {
                exclude_collection_types: vec!["homevideos".to_string()],
            }),
        };
        let starting = FakeTransport::default()
            .on_get(FETCH.path, vec![ok(FETCHED)])
            .on_get(
                LIBRARIES.path,
                vec![Step::Answer(
                    404,
                    r#"{"message":"No library found","type":"error"}"#.to_string(),
                )],
            );
        assert!(matches!(task.probe(&starting), Err(Probe::NotYet(_))));

        let ready = FakeTransport::default()
            .on_get(FETCH.path, vec![ok(FETCHED)])
            .on_get(LIBRARIES.path, vec![ok(LIBRARY_ANSWER)]);
        assert_eq!(task.probe(&ready).ok().unwrap(), VERSION_NOT_REPORTED);

        // Without a library rule the libraries are not asked at all.
        let plain = FakeTransport::default().on_get(FETCH.path, vec![ok(FETCHED)]);
        assert_eq!(
            Configuration {
                set: BTreeMap::new(),
                secrets: BTreeMap::new(),
                libraries: None
            }
            .probe(&plain)
            .ok()
            .unwrap(),
            VERSION_NOT_REPORTED
        );
    }

    #[test]
    fn probe_distinguishes_a_refused_token_from_a_service_still_starting() {
        let refused = FakeTransport::default().on_get(
            FETCH.path,
            vec![Step::Answer(401, r#"{"error":"x"}"#.to_string())],
        );
        let task = task(&[], &[]);
        assert!(matches!(task.probe(&refused), Err(Probe::Fatal(_))));

        let starting =
            FakeTransport::default().on_get(FETCH.path, vec![Step::Answer(502, String::new())]);
        assert!(matches!(task.probe(&starting), Err(Probe::NotYet(_))));

        let ready = FakeTransport::default().on_get(FETCH.path, vec![ok(FETCHED)]);
        assert_eq!(task.probe(&ready).ok().unwrap(), VERSION_NOT_REPORTED);
    }
}
