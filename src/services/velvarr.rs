//! Velvarr (0.39.3): the integrations with Jellyfin and Whisparr, and the
//! accounts (design §58). Velvarr publishes no OpenAPI description; every
//! field name here is read off its source at the release commit, and the
//! fixtures under `tests/fixtures/velvarr-0.39.3` are built from that source
//! (`constructed-…`) until a running instance has been recorded.
//!
//! No API key: Velvarr's accounts ARE Jellyfin accounts, a sign-in is checked
//! with Jellyfin's `AuthenticateByName`, and the session is nothing but the
//! `HttpOnly` cookie `velvarr_session` that `POST /api/login` -- or, on the
//! very first run, `POST /api/setup` -- sets. Every request behind it sends
//! that cookie back as `Cookie: velvarr_session=…`; every write also carries
//! the `Origin` Velvarr was configured with (`VELVARR_ORIGIN`).

use std::{cell::RefCell, collections::BTreeMap};

use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::{
    client::{expect_status_at, Reply, Transport},
    endpoint::{is_path_segment, Endpoint},
    engine::{shortened, Change, Probe, Task},
    error::Error,
    secret::Secret,
};

pub const HEALTH: Endpoint = Endpoint {
    method: "GET",
    path: "/api/health",
    request: None,
    response: None,
};

pub const STATUS: Endpoint = Endpoint {
    method: "GET",
    path: "/api/status",
    request: None,
    response: None,
};

pub const SETUP_INSPECT: Endpoint = Endpoint {
    method: "POST",
    path: "/api/setup/inspect",
    request: None,
    response: None,
};

pub const SETUP: Endpoint = Endpoint {
    method: "POST",
    path: "/api/setup",
    request: None,
    response: None,
};

pub const LOGIN: Endpoint = Endpoint {
    method: "POST",
    path: "/api/login",
    request: None,
    response: None,
};

pub const INTEGRATIONS: Endpoint = Endpoint {
    method: "GET",
    path: "/api/admin/integrations",
    request: None,
    response: None,
};

pub const INTEGRATIONS_WRITE: Endpoint = Endpoint {
    method: "PATCH",
    path: "/api/admin/integrations",
    request: None,
    response: None,
};

/// Whisparr as Velvarr reaches it: its root folders and quality profiles.
pub const WHISPARR_STATUS: Endpoint = Endpoint {
    method: "GET",
    path: "/api/admin/whisparr",
    request: None,
    response: None,
};

pub const USERS: Endpoint = Endpoint {
    method: "GET",
    path: "/api/admin/users",
    request: None,
    response: None,
};

pub const USERS_IMPORT: Endpoint = Endpoint {
    method: "POST",
    path: "/api/admin/users/import",
    request: None,
    response: None,
};

pub const USER_UPDATE: Endpoint = Endpoint {
    method: "PATCH",
    path: "/api/admin/users/{id}",
    request: None,
    response: None,
};

/// The session cookie (`COOKIE_NAME`, `src/server/security.ts`).
pub const COOKIE: &str = "velvarr_session";

/// Velvarr's API names no version of its own.
pub const VERSION_NOT_REPORTED: &str = "(not reported)";

/// The roles an account may have (`fieldRole`, `parse.ts`).
pub const ROLES: [&str; 3] = ["admin", "moderator", "requester"];

// --- answers and refusals ---------------------------------------------------

fn decode<T: for<'de> Deserialize<'de>>(path: &str, body: &str) -> Result<T, Error> {
    serde_json::from_str(body).map_err(|e| Error::Decode {
        path: path.to_string(),
        reason: crate::error::shape(&e),
    })
}

/// A body that may hold a secret: a serializing error names the path only.
fn text_of(method: &'static str, path: &str, body: &Value) -> Result<String, Error> {
    serde_json::to_string(body).map_err(|_| Error::Request {
        method,
        path: path.to_string(),
        reason: "cannot serialize the body".to_string(),
    })
}

/// The error code of a refusal: `{"error":{"code":…,"message":…}}`
/// (`errorResponse`, `route.ts`). Only the code is shown -- a word from
/// Velvarr's own vocabulary, checked to be one; the message can carry an
/// upstream's text.
fn code_of(body: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Refusal {
        error: Inner,
    }
    #[derive(Deserialize)]
    struct Inner {
        code: String,
    }
    let refusal: Refusal = serde_json::from_str(body).ok()?;
    let code = refusal.error.code;
    (!code.is_empty()
        && code.len() <= 64
        && code.chars().all(|c| c.is_ascii_lowercase() || c == '_'))
    .then_some(code)
}

/// What a code means for the one who reads the journal.
fn hint(code: &str) -> Option<&'static str> {
    Some(match code {
        "origin_missing" | "origin_mismatch" => {
            "desired.origin must be exactly the VELVARR_ORIGIN Velvarr runs with"
        }
        "bad_content_type" | "cross_site_request" => {
            "Velvarr refused the write as a cross-site one"
        }
        "too_many_attempts" => {
            "Velvarr allows 5 failed sign-ins per account and 5 minutes; wait before the next run"
        }
        "upstream_auth" => "Jellyfin refused the account or its password",
        "not_admitted" => "the account is not admitted to Velvarr",
        "account_disabled" => "the account is disabled in Velvarr or in Jellyfin",
        "remote_denied" => "remote access is off for the account in Jellyfin",
        "forbidden" => "the account is not an administrator in Velvarr",
        "setup_secret_invalid" => "the setup secret is not VELVARR_SETUP_SECRET",
        "not_initialized" => "Velvarr is not set up",
        "already_initialized" => "Velvarr is already set up",
        "server_mismatch" => "the Jellyfin at that URL is another server than the one set up",
        "owner_protected" => "the owner account cannot be disabled or demoted",
        _ => return None,
    })
}

/// A refusal as an error: the status, Velvarr's code and what it means --
/// nothing else from the body.
fn refused(method: &'static str, path: &str, reply: &Reply) -> Error {
    let validation = match code_of(&reply.body) {
        Some(code) => match hint(&code) {
            Some(hint) => vec![format!("{code} ({hint})")],
            None => vec![code],
        },
        None => Vec::new(),
    };
    Error::Status {
        method,
        path: path.to_string(),
        status: reply.status,
        validation,
    }
}

fn expect(method: &'static str, path: &str, reply: &Reply, accepted: &[u16]) -> Result<(), Error> {
    if accepted.contains(&reply.status) {
        Ok(())
    } else {
        Err(refused(method, path, reply))
    }
}

/// Readiness: `GET /api/health` answers `{"ok":true}`, without a session.
pub fn probe(t: &dyn Transport) -> Result<String, Probe> {
    #[derive(Deserialize)]
    struct Health {
        ok: bool,
    }
    let reply = t
        .get(HEALTH.path)
        .map_err(|e| Probe::NotYet(e.to_string()))?;
    if reply.status != 200 {
        return Err(Probe::NotYet(format!("HTTP {}", reply.status)));
    }
    let health: Health = serde_json::from_str(&reply.body)
        .map_err(|e| Probe::NotYet(format!("unexpected answer: {}", crate::error::shape(&e))))?;
    if health.ok {
        Ok(VERSION_NOT_REPORTED.to_string())
    } else {
        Err(Probe::NotYet("health is not ok yet".to_string()))
    }
}

/// `GET /api/status`: whether Velvarr is set up, and whether it could be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct Status {
    pub initialized: bool,
    #[serde(rename = "setupReady")]
    pub setup_ready: bool,
}

/// Asked before anything else, without a session: a plan must not set
/// Velvarr up, and the setup is a write.
pub fn status(t: &dyn Transport) -> Result<Status, Error> {
    let reply = t.get(STATUS.path)?;
    expect_status_at(STATUS.method, STATUS.path, &reply, &[200])?;
    decode(STATUS.path, &reply.body)
}

// --- the session --------------------------------------------------------------

/// What the first run sends to set Velvarr up: the integration it starts
/// with and the setup secret. The account is the one converge signs in as,
/// so it becomes Velvarr's owner.
pub struct Setup {
    pub jellyfin_url: String,
    pub jellyfin_external_url: String,
    /// Library names; the setup takes ids, read from `/api/setup/inspect`.
    pub libraries: Vec<String>,
    pub whisparr_url: Option<String>,
    pub setup_secret: Secret,
    pub jellyfin_api_key: Secret,
    pub whisparr_api_key: Option<Secret>,
}

fn session_of(endpoint: &Endpoint, reply: &Reply) -> Result<Secret, Error> {
    reply.cookie(COOKIE).ok_or_else(|| Error::Decode {
        path: endpoint.path.to_string(),
        reason: format!("the answer sets no {COOKIE} cookie"),
    })
}

/// Signs in with the account's name and password; the answer's cookie is the
/// session. One sign-in per run: Velvarr counts failed ones per account
/// (five in five minutes), and each one opens a Jellyfin session.
pub fn login(t: &dyn Transport, username: &str, password: &Secret) -> Result<Secret, Error> {
    let body = json!({"username": username, "password": password.expose()});
    let reply = t.post_json(LOGIN.path, &text_of(LOGIN.method, LOGIN.path, &body)?)?;
    expect(LOGIN.method, LOGIN.path, &reply, &[200])?;
    session_of(&LOGIN, &reply)
}

/// Sets Velvarr up: `POST /api/setup/inspect` names the libraries the
/// account sees, by id, and `POST /api/setup` stores the integration and
/// makes the account the owner. A setup answered `already_initialized` lost
/// a race against another run, and the sign-in follows.
pub fn set_up(
    t: &dyn Transport,
    username: &str,
    password: &Secret,
    setup: &Setup,
) -> Result<Secret, Error> {
    #[derive(Deserialize)]
    struct Inspected {
        libraries: Vec<Library>,
    }
    let mut body = json!({
        "setupSecret": setup.setup_secret.expose(),
        "username": username,
        "password": password.expose(),
        "jellyfinUrl": setup.jellyfin_url,
        "jellyfinExternalUrl": setup.jellyfin_external_url,
        "jellyfinApiKey": setup.jellyfin_api_key.expose(),
    });
    let path = SETUP_INSPECT.path;
    let reply = t.post_json(path, &text_of(SETUP_INSPECT.method, path, &body)?)?;
    expect(SETUP_INSPECT.method, path, &reply, &[200])?;
    let inspected: Inspected = decode(path, &reply.body)?;
    let ids = library_ids(&inspected.libraries, &setup.libraries, username)?;
    body["libraryIds"] = json!(ids);
    if let (Some(url), Some(key)) = (&setup.whisparr_url, &setup.whisparr_api_key) {
        body["whisparrUrl"] = json!(url);
        body["whisparrApiKey"] = json!(key.expose());
    }
    let path = SETUP.path;
    let reply = t.post_json(path, &text_of(SETUP.method, path, &body)?)?;
    if reply.status == 409 && code_of(&reply.body).as_deref() == Some("already_initialized") {
        return login(t, username, password);
    }
    expect(SETUP.method, path, &reply, &[200])?;
    session_of(&SETUP, &reply)
}

/// One library a Jellyfin account sees.
#[derive(Debug, Clone, Deserialize)]
pub struct Library {
    pub id: String,
    pub name: String,
}

/// The ids of the named libraries, in the spec's order. A name the account
/// does not see, or one two libraries share, is an error.
fn library_ids(seen: &[Library], names: &[String], username: &str) -> Result<Vec<String>, Error> {
    let (mut ids, mut unknown, mut twice) = (Vec::new(), Vec::new(), Vec::new());
    for name in names {
        let matching: Vec<&Library> = seen.iter().filter(|l| l.name == *name).collect();
        match matching.as_slice() {
            [one] => ids.push(one.id.clone()),
            [] => unknown.push(format!("library {name} (as {username} sees them)")),
            _ => twice.push(format!(
                "{} Jellyfin libraries are named {name}; the spec cannot say which one it means",
                matching.len()
            )),
        }
    }
    if !twice.is_empty() {
        return Err(Error::Mismatch(twice));
    }
    if !unknown.is_empty() {
        return Err(Error::NotFound(unknown));
    }
    Ok(ids)
}

// --- integrations -------------------------------------------------------------

pub struct JellyfinTarget {
    pub url: String,
    pub external_url: String,
    /// Library names. Velvarr chooses its libraries at setup and no `PATCH`
    /// changes them, so they are checked, never written.
    pub libraries: Option<Vec<String>>,
    pub api_key: Secret,
}

/// What Velvarr hands to Whisparr when a request is approved. The quality
/// profile is named; Velvarr stores its id.
#[derive(Debug, Clone, PartialEq)]
pub struct Delivery {
    pub enabled: bool,
    pub root_folder_path: String,
    pub quality_profile: String,
    pub search_on_add: bool,
}

pub struct WhisparrTarget {
    pub url: String,
    pub api_key: Secret,
    pub delivery: Option<Delivery>,
    /// `{whisparrPrefix, jellyfinPrefix}` objects, in order.
    pub path_mappings: Option<Vec<Value>>,
}

pub struct Integrations {
    pub jellyfin: JellyfinTarget,
    /// `None` leaves Velvarr's Whisparr connection as it is.
    pub whisparr: Option<WhisparrTarget>,
    /// Whether this run's write already sent the keys, so an apply sends
    /// them once, not twice.
    pub sent: RefCell<bool>,
}

/// What `read` found: the answer of `GET /api/admin/integrations`, the
/// libraries the signed-in account sees among the configured ones (only when
/// the spec names libraries), and Whisparr's quality profiles (only when the
/// spec names a delivery and Velvarr already reaches that Whisparr).
pub struct IntegrationsNow {
    pub shape: Map<String, Value>,
    pub libraries: Option<Vec<Library>>,
    pub profiles: Option<Vec<(i64, String)>>,
}

fn at<'a>(map: &'a Map<String, Value>, section: &str, field: &str) -> Option<&'a Value> {
    map.get(section)?.as_object()?.get(field)
}

/// The quality profiles of the Whisparr Velvarr reaches now, by id and name.
fn whisparr_profiles(t: &dyn Transport) -> Result<Vec<(i64, String)>, Error> {
    #[derive(Deserialize)]
    struct Profile {
        id: i64,
        name: String,
    }
    #[derive(Deserialize)]
    struct Answer {
        configured: bool,
        #[serde(default)]
        error: Option<Value>,
        #[serde(default)]
        profiles: Option<Vec<Profile>>,
    }
    let path = WHISPARR_STATUS.path;
    let reply = t.get(path)?;
    expect(WHISPARR_STATUS.method, path, &reply, &[200])?;
    let answer: Answer = decode(path, &reply.body)?;
    if !answer.configured {
        return Err(Error::Refused(
            "Velvarr has no Whisparr connection to read quality profiles from".to_string(),
        ));
    }
    if let Some(error) = answer.error {
        // Velvarr reports an upstream refusal here instead of failing the
        // request; its code says which, its message stays unread.
        let code = code_of(&json!({ "error": error }).to_string())
            .unwrap_or_else(|| "unknown".to_string());
        return Err(Error::Refused(format!(
            "Velvarr cannot read Whisparr's quality profiles: {code}"
        )));
    }
    let profiles = answer.profiles.ok_or_else(|| Error::Decode {
        path: path.to_string(),
        reason: "missing field `profiles`".to_string(),
    })?;
    if profiles.is_empty() {
        return Err(Error::EmptyList {
            path: path.to_string(),
        });
    }
    Ok(profiles.into_iter().map(|p| (p.id, p.name)).collect())
}

impl Integrations {
    /// Whether Velvarr already reaches the Whisparr the spec names: then its
    /// profiles can be read through Velvarr before anything is written.
    fn whisparr_is_current(&self, shape: &Map<String, Value>) -> bool {
        let Some(target) = &self.whisparr else {
            return false;
        };
        at(shape, "whisparr", "url").and_then(Value::as_str) == Some(target.url.as_str())
            && at(shape, "whisparr", "apiKeyConfigured") == Some(&Value::Bool(true))
    }

    /// The delivery as Velvarr stores it, its profile resolved -- or `None`
    /// while the profiles are not known.
    fn resolved_delivery(
        delivery: &Delivery,
        profiles: Option<&[(i64, String)]>,
    ) -> Result<Option<Value>, Error> {
        let Some(profiles) = profiles else {
            return Ok(None);
        };
        let matching: Vec<i64> = profiles
            .iter()
            .filter(|(_, name)| *name == delivery.quality_profile)
            .map(|(id, _)| *id)
            .collect();
        let id = match matching.as_slice() {
            [id] => *id,
            [] => {
                return Err(Error::NotFound(vec![format!(
                    "Whisparr quality profile {}",
                    delivery.quality_profile
                )]))
            }
            _ => {
                return Err(Error::Mismatch(vec![format!(
                    "{} Whisparr quality profiles are named {}",
                    matching.len(),
                    delivery.quality_profile
                )]))
            }
        };
        Ok(Some(json!({
            "enabled": delivery.enabled,
            "rootFolderPath": delivery.root_folder_path,
            "qualityProfileId": id,
            "searchOnAdd": delivery.search_on_add,
        })))
    }

    fn delivery_shown(delivery: &Delivery) -> Value {
        json!({
            "enabled": delivery.enabled,
            "rootFolderPath": delivery.root_folder_path,
            "qualityProfile": delivery.quality_profile,
            "searchOnAdd": delivery.search_on_add,
        })
    }

    /// The four fields of a stored delivery, or `Null` for none.
    fn delivery_now(shape: &Map<String, Value>) -> Value {
        match at(shape, "whisparr", "delivery").and_then(Value::as_object) {
            Some(d) => {
                let pick = |k: &str| d.get(k).cloned().unwrap_or(Value::Null);
                json!({
                    "enabled": pick("enabled"),
                    "rootFolderPath": pick("rootFolderPath"),
                    "qualityProfileId": pick("qualityProfileId"),
                    "searchOnAdd": pick("searchOnAdd"),
                })
            }
            None => Value::Null,
        }
    }

    /// The body of a `PATCH` that carries the whole connection. Velvarr's
    /// `nextWhisparr` builds a new connection from `whisparrUrl` and takes
    /// `delivery` and `pathMappings` from that new one, not the stored one --
    /// so a body with a URL and without them would drop both. Whatever the
    /// spec leaves to Velvarr is sent back as it is stored.
    fn body(&self, shape: &Map<String, Value>, delivery: Option<&Value>) -> Value {
        let mut body = json!({
            "jellyfinUrl": self.jellyfin.url,
            "jellyfinExternalUrl": self.jellyfin.external_url,
            "jellyfinApiKey": self.jellyfin.api_key.expose(),
        });
        if let Some(target) = &self.whisparr {
            body["whisparrUrl"] = json!(target.url);
            body["whisparrApiKey"] = json!(target.api_key.expose());
            let stored = at(shape, "whisparr", "delivery").filter(|d| d.is_object());
            if let Some(delivery) = delivery.or(stored) {
                body["delivery"] = delivery.clone();
            }
            let mappings = match &target.path_mappings {
                Some(list) => Some(Value::Array(list.clone())),
                None => at(shape, "whisparr", "pathMappings")
                    .filter(|m| m.is_array())
                    .cloned(),
            };
            if let Some(mappings) = mappings {
                body["pathMappings"] = mappings;
            }
        }
        body
    }

    fn patch(&self, t: &dyn Transport, body: &Value) -> Result<(), Error> {
        let path = INTEGRATIONS_WRITE.path;
        let reply = t.patch_json(path, &text_of(INTEGRATIONS_WRITE.method, path, body)?)?;
        expect(INTEGRATIONS_WRITE.method, path, &reply, &[200])
    }

    /// Writes the whole connection. A delivery whose profile cannot be named
    /// yet -- Velvarr does not reach that Whisparr before this write -- takes
    /// two: the connection first, then the delivery with the profile read
    /// through it.
    fn send(&self, t: &dyn Transport, now: &IntegrationsNow) -> Result<(), Error> {
        let delivery = match self.whisparr.as_ref().and_then(|w| w.delivery.as_ref()) {
            None => None,
            Some(delivery) => match Self::resolved_delivery(delivery, now.profiles.as_deref())? {
                Some(resolved) => Some(resolved),
                None => {
                    self.patch(t, &self.body(&now.shape, None))?;
                    let profiles = whisparr_profiles(t)?;
                    Self::resolved_delivery(delivery, Some(&profiles))?
                }
            },
        };
        self.patch(t, &self.body(&now.shape, delivery.as_ref()))?;
        *self.sent.borrow_mut() = true;
        Ok(())
    }
}

fn change(subject: &str, field: &str, current: String, desired: String) -> Change {
    Change {
        subject: subject.to_string(),
        field: field.to_string(),
        current,
        desired,
    }
}

impl Task for Integrations {
    type Current = IntegrationsNow;

    fn probe(&self, t: &dyn Transport) -> Result<String, Probe> {
        probe(t)
    }

    fn read(&self, t: &dyn Transport) -> Result<Self::Current, Error> {
        let path = INTEGRATIONS.path;
        let reply = t.get(path)?;
        expect(INTEGRATIONS.method, path, &reply, &[200])?;
        let shape: Map<String, Value> = decode(path, &reply.body)?;
        if !shape.get("jellyfin").is_some_and(Value::is_object) {
            return Err(Error::MissingField(vec!["jellyfin".to_string()]));
        }
        let libraries = match self.jellyfin.libraries {
            Some(_) => Some(Users::read_from(t)?.libraries),
            None => None,
        };
        let wants_profiles = self.whisparr.as_ref().is_some_and(|w| w.delivery.is_some());
        let profiles = if wants_profiles && self.whisparr_is_current(&shape) {
            Some(whisparr_profiles(t)?)
        } else {
            None
        };
        Ok(IntegrationsNow {
            shape,
            libraries,
            profiles,
        })
    }

    fn diff(&self, now: &Self::Current) -> Result<Vec<Change>, Error> {
        let (mut changes, mut missing, mut mismatch) = (Vec::new(), Vec::new(), Vec::new());
        let shape = &now.shape;
        let text = |section: &str, field: &str| at(shape, section, field).cloned();

        for (field, desired) in [
            ("url", &self.jellyfin.url),
            ("externalUrl", &self.jellyfin.external_url),
        ] {
            match text("jellyfin", field) {
                None => missing.push(format!("jellyfin.{field}")),
                Some(current) if current.as_str() != Some(desired.as_str()) => {
                    changes.push(change(
                        "jellyfin",
                        field,
                        shortened(&current),
                        shortened(&json!(desired)),
                    ))
                }
                Some(_) => {}
            }
        }
        match text("jellyfin", "apiKeyConfigured") {
            None => missing.push("jellyfin.apiKeyConfigured".to_string()),
            Some(Value::Bool(true)) => {}
            Some(_) => changes.push(change(
                "jellyfin",
                "apiKey",
                "(not stored)".to_string(),
                "(the credential's, not shown)".to_string(),
            )),
        }
        if let (Some(names), Some(seen)) = (&self.jellyfin.libraries, &now.libraries) {
            let configured: Vec<String> = match text("jellyfin", "libraryIds") {
                Some(Value::Array(ids)) => ids
                    .iter()
                    .filter_map(|id| id.as_str().map(str::to_string))
                    .collect(),
                _ => {
                    missing.push("jellyfin.libraryIds".to_string());
                    Vec::new()
                }
            };
            let mut have = Vec::new();
            for id in &configured {
                match seen.iter().find(|l| l.id == *id) {
                    Some(library) => have.push(library.name.clone()),
                    None => mismatch.push(format!(
                        "Velvarr's library {id} is not one the signed-in account sees in Jellyfin"
                    )),
                }
            }
            let mut want = names.clone();
            have.sort();
            want.sort();
            if mismatch.is_empty() && have != want {
                mismatch.push(format!(
                    "Velvarr chooses its libraries at setup only and no write changes them: \
                     it has {have:?}, the spec names {want:?}"
                ));
            }
        }

        if let Some(target) = &self.whisparr {
            if !shape.get("whisparr").is_some_and(Value::is_object) {
                changes.push(change(
                    "whisparr",
                    "",
                    "(missing)".to_string(),
                    "(added)".to_string(),
                ));
            } else {
                match text("whisparr", "url") {
                    None => missing.push("whisparr.url".to_string()),
                    Some(current) if current.as_str() != Some(target.url.as_str()) => {
                        changes.push(change(
                            "whisparr",
                            "url",
                            shortened(&current),
                            shortened(&json!(target.url)),
                        ))
                    }
                    Some(_) => {}
                }
                match text("whisparr", "apiKeyConfigured") {
                    None => missing.push("whisparr.apiKeyConfigured".to_string()),
                    Some(Value::Bool(true)) => {}
                    Some(_) => changes.push(change(
                        "whisparr",
                        "apiKey",
                        "(not stored)".to_string(),
                        "(the credential's, not shown)".to_string(),
                    )),
                }
                if let Some(delivery) = &target.delivery {
                    let current = Self::delivery_now(shape);
                    match Self::resolved_delivery(delivery, now.profiles.as_deref())? {
                        Some(resolved) if resolved == current => {}
                        Some(resolved) => changes.push(change(
                            "whisparr",
                            "delivery",
                            shortened(&current),
                            shortened(&resolved),
                        )),
                        // Not readable before the write: Velvarr reaches
                        // another Whisparr, or none, so far.
                        None => changes.push(change(
                            "whisparr",
                            "delivery",
                            shortened(&current),
                            shortened(&Self::delivery_shown(delivery)),
                        )),
                    }
                }
                if let Some(mappings) = &target.path_mappings {
                    let desired = Value::Array(mappings.clone());
                    match text("whisparr", "pathMappings") {
                        None => missing.push("whisparr.pathMappings".to_string()),
                        Some(current) if current != desired => changes.push(change(
                            "whisparr",
                            "pathMappings",
                            shortened(&current),
                            shortened(&desired),
                        )),
                        Some(_) => {}
                    }
                }
            }
        }
        if !mismatch.is_empty() {
            return Err(Error::Mismatch(mismatch));
        }
        if !missing.is_empty() {
            return Err(Error::MissingField(missing));
        }
        Ok(changes)
    }

    fn notes(&self, now: &Self::Current) -> Vec<String> {
        if self.whisparr.is_none() && now.shape.get("whisparr").is_some_and(Value::is_object) {
            vec!["not in the spec: the Whisparr connection, left as it is".to_string()]
        } else {
            Vec::new()
        }
    }

    fn write(&self, t: &dyn Transport, now: &Self::Current) -> Result<(), Error> {
        self.send(t, now)
    }

    /// Velvarr answers a key as `apiKeyConfigured` and nothing else, so a
    /// rotated one cannot be seen by reading. Every apply sends both keys
    /// with the whole connection -- once: a run whose write has just sent
    /// them does not send them again (design §58).
    fn hand_over(&self, t: &dyn Transport, now: &Self::Current) -> Result<Vec<String>, Error> {
        if !*self.sent.borrow() {
            self.send(t, now)?;
        }
        let mut lines = vec![
            "jellyfin: apiKey handed over from credentials (write-only; Velvarr checks it against Jellyfin)"
                .to_string(),
        ];
        if self.whisparr.is_some() {
            lines.push(
                "whisparr: apiKey handed over from credentials (write-only; Velvarr shows only that one is stored)"
                    .to_string(),
            );
        }
        Ok(lines)
    }
}

// --- accounts -----------------------------------------------------------------

/// What a spec wants of one account. Fields it leaves out stay as they are.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AccountTarget {
    pub role: Option<String>,
    pub enabled: Option<bool>,
    pub auto_approve: Option<bool>,
    /// Library names among the ones Velvarr is set up with.
    pub libraries: Option<Vec<String>>,
}

pub struct Accounts {
    /// Jellyfin account name → what it should be in Velvarr.
    pub accounts: BTreeMap<String, AccountTarget>,
    /// The account converge signs in as: it must stay an enabled admin.
    pub signed_in_as: String,
}

/// `GET /api/admin/users`: the admitted accounts, and the configured
/// libraries the signed-in account sees.
pub struct Users {
    pub accounts: Vec<Map<String, Value>>,
    pub libraries: Vec<Library>,
}

impl Users {
    fn read_from(t: &dyn Transport) -> Result<Users, Error> {
        #[derive(Deserialize)]
        struct Answer {
            accounts: Vec<Map<String, Value>>,
            libraries: Vec<Library>,
        }
        let path = USERS.path;
        let reply = t.get(path)?;
        expect(USERS.method, path, &reply, &[200])?;
        let answer: Answer = decode(path, &reply.body)?;
        for (index, account) in answer.accounts.iter().enumerate() {
            if account.get("name").and_then(Value::as_str).is_none() {
                return Err(Error::MissingName {
                    path: path.to_string(),
                    index,
                });
            }
            // The id goes into a path; an answer must not aim a write elsewhere.
            if !account
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(is_path_segment)
            {
                return Err(Error::Decode {
                    path: path.to_string(),
                    reason: format!("account {index} has no id that is a path segment"),
                });
            }
        }
        Ok(Users {
            accounts: answer.accounts,
            libraries: answer.libraries,
        })
    }
}

impl Accounts {
    fn subject(name: &str) -> String {
        format!("account {name}")
    }

    fn find<'a>(users: &'a Users, name: &str) -> Result<Option<&'a Map<String, Value>>, Error> {
        let mut found = users
            .accounts
            .iter()
            .filter(|a| a.get("name").and_then(Value::as_str) == Some(name));
        let first = found.next();
        if found.next().is_some() {
            return Err(Error::Mismatch(vec![format!(
                "{} exists more than once in Velvarr; the spec cannot say which one it means",
                Self::subject(name)
            )]));
        }
        Ok(first)
    }

    /// The body Velvarr's `PATCH` wants -- `enabled`, `role` and `libraryIds`
    /// always, `autoApprove` when the spec names it -- and the change lines.
    fn update_of(
        name: &str,
        target: &AccountTarget,
        account: &Map<String, Value>,
        libraries: &[Library],
        missing: &mut Vec<String>,
    ) -> Result<(Vec<Change>, Value), Error> {
        let subject = Self::subject(name);
        let mut changes = Vec::new();
        let mut current = |field: &str| match account.get(field) {
            Some(value) => value.clone(),
            None => {
                missing.push(format!("{subject}: {field}"));
                Value::Null
            }
        };
        let role_now = current("role");
        let enabled_now = current("enabled");
        let ids_now = current("libraryIds");
        let auto_now = current("autoApprove");

        let mut body = json!({
            "enabled": enabled_now,
            "role": role_now,
            "libraryIds": ids_now,
        });
        if let Some(role) = &target.role {
            if role_now.as_str() != Some(role.as_str()) {
                changes.push(change(
                    &subject,
                    "role",
                    shortened(&role_now),
                    shortened(&json!(role)),
                ));
            }
            body["role"] = json!(role);
        }
        if let Some(enabled) = target.enabled {
            if enabled_now != Value::Bool(enabled) {
                changes.push(change(
                    &subject,
                    "enabled",
                    shortened(&enabled_now),
                    enabled.to_string(),
                ));
            }
            body["enabled"] = json!(enabled);
        }
        if let Some(auto) = target.auto_approve {
            if auto_now != Value::Bool(auto) {
                changes.push(change(
                    &subject,
                    "autoApprove",
                    shortened(&auto_now),
                    auto.to_string(),
                ));
            }
            body["autoApprove"] = json!(auto);
        }
        if let Some(names) = &target.libraries {
            let ids = library_ids(libraries, names, "Velvarr is set up with")?;
            let mut want = ids.clone();
            let mut have: Vec<String> = ids_now
                .as_array()
                .map(|all| {
                    all.iter()
                        .filter_map(|id| id.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            want.sort();
            have.sort();
            if want != have {
                let shown: Vec<String> = have
                    .iter()
                    .map(|id| {
                        libraries
                            .iter()
                            .find(|l| l.id == *id)
                            .map_or_else(|| id.clone(), |l| l.name.clone())
                    })
                    .collect();
                changes.push(change(
                    &subject,
                    "libraries",
                    shortened(&json!(shown)),
                    shortened(&json!(names)),
                ));
            }
            body["libraryIds"] = json!(ids);
        }
        Ok((changes, body))
    }

    /// The owner and the account converge signs in as stay enabled admins:
    /// Velvarr refuses the first, and the second would lock converge out.
    fn guard(
        &self,
        name: &str,
        target: &AccountTarget,
        account: &Map<String, Value>,
    ) -> Option<String> {
        let demoted = target.role.as_deref().is_some_and(|r| r != "admin");
        let disabled = target.enabled == Some(false);
        if !(demoted || disabled) {
            return None;
        }
        if account.get("isOwner") == Some(&Value::Bool(true)) {
            return Some(format!(
                "{} is Velvarr's owner, which cannot be disabled or demoted",
                Self::subject(name)
            ));
        }
        (name == self.signed_in_as).then(|| {
            format!(
                "{} is the account converge signs in as; disabled or demoted it locks converge out",
                Self::subject(name)
            )
        })
    }
}

impl Task for Accounts {
    type Current = Users;

    fn probe(&self, t: &dyn Transport) -> Result<String, Probe> {
        probe(t)
    }

    /// Never empty: the account converge signed in as is one of them.
    fn read(&self, t: &dyn Transport) -> Result<Self::Current, Error> {
        let users = Users::read_from(t)?;
        if users.accounts.is_empty() {
            return Err(Error::EmptyList {
                path: USERS.path.to_string(),
            });
        }
        Ok(users)
    }

    fn diff(&self, users: &Self::Current) -> Result<Vec<Change>, Error> {
        let (mut changes, mut missing, mut mismatch) = (Vec::new(), Vec::new(), Vec::new());
        for (name, target) in &self.accounts {
            match Self::find(users, name)? {
                None => changes.push(change(
                    &Self::subject(name),
                    "",
                    "(missing)".to_string(),
                    "(imported from Jellyfin)".to_string(),
                )),
                Some(account) => {
                    if let Some(reason) = self.guard(name, target, account) {
                        mismatch.push(reason);
                        continue;
                    }
                    let (found, _) =
                        Self::update_of(name, target, account, &users.libraries, &mut missing)?;
                    changes.extend(found);
                }
            }
        }
        if !mismatch.is_empty() {
            return Err(Error::Mismatch(mismatch));
        }
        if !missing.is_empty() {
            return Err(Error::MissingField(missing));
        }
        Ok(changes)
    }

    fn notes(&self, users: &Self::Current) -> Vec<String> {
        users
            .accounts
            .iter()
            .filter_map(|a| a.get("name").and_then(Value::as_str))
            .filter(|name| !self.accounts.contains_key(*name))
            .map(|name| format!("not in the spec: {}", Self::subject(name)))
            .collect()
    }

    /// An account Velvarr does not have yet is imported first -- and the
    /// import takes every Jellyfin account at once, as disabled requesters;
    /// Velvarr has no import by name. Then each differing account gets one
    /// `PATCH`.
    fn write(&self, t: &dyn Transport, users: &Self::Current) -> Result<(), Error> {
        let mut imported = None;
        if self
            .accounts
            .keys()
            .any(|name| Self::find(users, name).ok().flatten().is_none())
        {
            let path = USERS_IMPORT.path;
            let reply = t.post_json(path, "{}")?;
            expect(USERS_IMPORT.method, path, &reply, &[200])?;
            imported = Some(Users::read_from(t)?);
        }
        let users = imported.as_ref().unwrap_or(users);
        let mut absent = Vec::new();
        for (name, target) in &self.accounts {
            let Some(account) = Self::find(users, name)? else {
                absent.push(format!(
                    "Jellyfin account {name} (the import did not bring it)"
                ));
                continue;
            };
            if let Some(reason) = self.guard(name, target, account) {
                return Err(Error::Mismatch(vec![reason]));
            }
            let mut missing = Vec::new();
            let (changes, body) =
                Self::update_of(name, target, account, &users.libraries, &mut missing)?;
            if !missing.is_empty() {
                return Err(Error::MissingField(missing));
            }
            if changes.is_empty() {
                continue;
            }
            // `read_from` has checked every id.
            let id = account
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let path = USER_UPDATE.path.replace("{id}", id);
            let reply = t.patch_json(&path, &text_of(USER_UPDATE.method, &path, &body)?)?;
            expect(USER_UPDATE.method, &path, &reply, &[200])?;
        }
        if !absent.is_empty() {
            return Err(Error::NotFound(absent));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        engine::{run, Mode, Outcome, Timing},
        testing::{ok, FakeClock, FakeTransport, Step},
    };

    const HEALTH_OK: &str =
        include_str!("../../tests/fixtures/velvarr-0.39.3/constructed-health.json");
    const FRESH: &str =
        include_str!("../../tests/fixtures/velvarr-0.39.3/constructed-status-fresh.json");
    const SET_UP: &str =
        include_str!("../../tests/fixtures/velvarr-0.39.3/constructed-status-set-up.json");
    const INSPECT: &str =
        include_str!("../../tests/fixtures/velvarr-0.39.3/constructed-setup-inspect.json");
    const SETUP_ANSWER: &str =
        include_str!("../../tests/fixtures/velvarr-0.39.3/constructed-setup.json");
    const LOGIN_ANSWER: &str =
        include_str!("../../tests/fixtures/velvarr-0.39.3/constructed-login.json");
    const UPSTREAM_AUTH: &str =
        include_str!("../../tests/fixtures/velvarr-0.39.3/constructed-error-upstream-auth.json");
    const ORIGIN_MISMATCH: &str =
        include_str!("../../tests/fixtures/velvarr-0.39.3/constructed-error-origin-mismatch.json");
    const TOO_MANY: &str = include_str!(
        "../../tests/fixtures/velvarr-0.39.3/constructed-error-too-many-attempts.json"
    );
    const ALREADY: &str = include_str!(
        "../../tests/fixtures/velvarr-0.39.3/constructed-error-already-initialized.json"
    );
    const INTEGRATIONS_NOW: &str =
        include_str!("../../tests/fixtures/velvarr-0.39.3/constructed-integrations.json");
    const INTEGRATIONS_DELIVERY: &str =
        include_str!("../../tests/fixtures/velvarr-0.39.3/constructed-integrations-delivery.json");
    const INTEGRATIONS_NO_WHISPARR: &str = include_str!(
        "../../tests/fixtures/velvarr-0.39.3/constructed-integrations-no-whisparr.json"
    );
    const WHISPARR_NOW: &str =
        include_str!("../../tests/fixtures/velvarr-0.39.3/constructed-whisparr-status.json");
    const WHISPARR_ERROR: &str =
        include_str!("../../tests/fixtures/velvarr-0.39.3/constructed-whisparr-status-error.json");
    const USERS_NOW: &str =
        include_str!("../../tests/fixtures/velvarr-0.39.3/constructed-users.json");
    const USERS_BEFORE: &str =
        include_str!("../../tests/fixtures/velvarr-0.39.3/constructed-users-before-import.json");
    const IMPORTED: &str =
        include_str!("../../tests/fixtures/velvarr-0.39.3/constructed-users-import.json");
    const UPDATED: &str =
        include_str!("../../tests/fixtures/velvarr-0.39.3/constructed-user-updated.json");

    /// Values nobody would type, so a leak is easy to search for.
    const PASSWORD: &str = "velvarr-pw-5d1c-never-print-me";
    const SETUP_SECRET: &str = "setup-secret-0a9f-never-print-me-0123456789";
    const JELLYFIN_KEY: &str = "jellyfin-key-7e2b-never-print-me";
    const WHISPARR_KEY: &str = "whisparr-key-91c4-never-print-me";
    const SESSION: &str = "sess-b81d-never-print-me";

    fn cookie_line() -> String {
        format!("{COOKIE}={SESSION}; Path=/; HttpOnly; SameSite=Strict; Max-Age=604800")
    }

    fn password() -> Secret {
        Secret::new(PASSWORD.to_string())
    }

    fn sent(t: &FakeTransport, index: usize) -> (String, Value) {
        let (path, body) = t.written.borrow()[index].clone();
        (path, serde_json::from_str(&body).unwrap())
    }

    fn setup(whisparr: bool) -> Setup {
        Setup {
            jellyfin_url: "http://10.0.10.5:8096".into(),
            jellyfin_external_url: "https://jellyfin.example".into(),
            libraries: vec!["XXX".into()],
            whisparr_url: whisparr.then(|| "http://10.0.10.12:6969".into()),
            setup_secret: Secret::new(SETUP_SECRET.into()),
            jellyfin_api_key: Secret::new(JELLYFIN_KEY.into()),
            whisparr_api_key: whisparr.then(|| Secret::new(WHISPARR_KEY.into())),
        }
    }

    #[test]
    fn the_probe_wants_ok_and_names_no_version() {
        let t = FakeTransport::default().on_get("/api/health", vec![ok(HEALTH_OK)]);
        assert_eq!(probe(&t).ok().unwrap(), VERSION_NOT_REPORTED);
        let t =
            FakeTransport::default().on_get("/api/health", vec![Step::Answer(502, String::new())]);
        assert!(matches!(probe(&t), Err(Probe::NotYet(_))));
    }

    #[test]
    fn the_status_says_whether_velvarr_is_set_up_without_writing() {
        let t = FakeTransport::default().on_get("/api/status", vec![ok(FRESH)]);
        assert_eq!(
            status(&t).unwrap(),
            Status {
                initialized: false,
                setup_ready: true
            }
        );
        let t = FakeTransport::default().on_get("/api/status", vec![ok(SET_UP)]);
        assert!(status(&t).unwrap().initialized);
        assert!(t.written.borrow().is_empty());
        // An answer without the field is not Velvarr's: never guessed at.
        let t = FakeTransport::default().on_get("/api/status", vec![ok(r#"{"ok":true}"#)]);
        assert!(matches!(status(&t), Err(Error::Decode { .. })));
    }

    #[test]
    fn a_login_answers_the_session_from_its_cookie() {
        let t = FakeTransport::default().on_put(vec![Step::WithCookie(
            200,
            LOGIN_ANSWER.into(),
            cookie_line(),
        )]);
        let session = login(&t, "converge", &password()).unwrap();
        assert_eq!(session.expose(), SESSION);
        let (path, body) = sent(&t, 0);
        assert_eq!(path, "/api/login");
        assert_eq!(body, json!({"username": "converge", "password": PASSWORD}));
    }

    #[test]
    fn a_login_without_the_cookie_is_an_error() {
        let t = FakeTransport::default().on_put(vec![ok(LOGIN_ANSWER)]);
        let error = login(&t, "converge", &password()).err().unwrap();
        assert!(
            error.to_string().contains("sets no velvarr_session cookie"),
            "{error}"
        );
    }

    #[test]
    fn a_refused_login_names_the_code_and_never_the_password() {
        // Velvarr answers Jellyfin's refusal as 502, not 401: a 401 would
        // sign every browser out (`errorResponse`, route.ts).
        let t = FakeTransport::default().on_put(vec![Step::Answer(502, UPSTREAM_AUTH.into())]);
        let error = login(&t, "converge", &password()).err().unwrap();
        let text = error.to_string();
        assert!(matches!(error, Error::Status { status: 502, .. }));
        assert!(
            text.contains("upstream_auth (Jellyfin refused the account or its password)"),
            "{text}"
        );
        assert!(!text.contains(PASSWORD), "{text}");

        let t = FakeTransport::default().on_put(vec![Step::Answer(429, TOO_MANY.into())]);
        let text = login(&t, "converge", &password())
            .err()
            .unwrap()
            .to_string();
        assert!(
            text.contains("5 failed sign-ins per account and 5 minutes"),
            "{text}"
        );
    }

    #[test]
    fn an_origin_refusal_points_at_the_spec_and_shows_no_message() {
        let t = FakeTransport::default().on_put(vec![Step::Answer(403, ORIGIN_MISMATCH.into())]);
        let text = login(&t, "converge", &password())
            .err()
            .unwrap()
            .to_string();
        assert!(
            text.contains("origin_mismatch (desired.origin must be exactly"),
            "{text}"
        );
        // The message names both origins; only the code is shown.
        assert!(!text.contains("elsewhere.example"), "{text}");
    }

    #[test]
    fn a_fresh_velvarr_is_set_up_with_the_libraries_by_id() {
        let t = FakeTransport::default().on_put(vec![
            ok(INSPECT),
            Step::WithCookie(200, SETUP_ANSWER.into(), cookie_line()),
        ]);
        let session = set_up(&t, "converge", &password(), &setup(true)).unwrap();
        assert_eq!(session.expose(), SESSION);
        let (path, inspect) = sent(&t, 0);
        assert_eq!(path, "/api/setup/inspect");
        assert_eq!(
            inspect,
            json!({
                "setupSecret": SETUP_SECRET,
                "username": "converge",
                "password": PASSWORD,
                "jellyfinUrl": "http://10.0.10.5:8096",
                "jellyfinExternalUrl": "https://jellyfin.example",
                "jellyfinApiKey": JELLYFIN_KEY,
            })
        );
        let (path, commit) = sent(&t, 1);
        assert_eq!(path, "/api/setup");
        assert_eq!(
            commit["libraryIds"],
            json!(["098873858ff54b6c9a23aefae609dd20"])
        );
        assert_eq!(commit["whisparrUrl"], json!("http://10.0.10.12:6969"));
        assert_eq!(commit["whisparrApiKey"], json!(WHISPARR_KEY));
        assert_eq!(commit["setupSecret"], json!(SETUP_SECRET));
    }

    #[test]
    fn a_setup_without_whisparr_sends_no_whisparr_keys() {
        let t = FakeTransport::default().on_put(vec![
            ok(INSPECT),
            Step::WithCookie(200, SETUP_ANSWER.into(), cookie_line()),
        ]);
        set_up(&t, "converge", &password(), &setup(false)).unwrap();
        let (_, commit) = sent(&t, 1);
        assert!(commit.get("whisparrUrl").is_none());
        assert!(commit.get("whisparrApiKey").is_none());
    }

    #[test]
    fn a_setup_names_a_library_it_cannot_see_before_anything_is_stored() {
        let mut wrong = setup(false);
        wrong.libraries = vec!["Pornos".into()];
        let t = FakeTransport::default().on_put(vec![ok(INSPECT)]);
        let error = set_up(&t, "converge", &password(), &wrong).err().unwrap();
        assert!(matches!(error, Error::NotFound(_)), "{error}");
        assert!(error.to_string().contains("library Pornos"), "{error}");
        // Only the inspection went out; it stores nothing.
        assert_eq!(t.written.borrow().len(), 1);
    }

    #[test]
    fn a_setup_that_lost_the_race_signs_in_instead() {
        let t = FakeTransport::default().on_put(vec![
            ok(INSPECT),
            Step::Answer(409, ALREADY.into()),
            Step::WithCookie(200, LOGIN_ANSWER.into(), cookie_line()),
        ]);
        let session = set_up(&t, "converge", &password(), &setup(false)).unwrap();
        assert_eq!(session.expose(), SESSION);
        assert_eq!(
            *t.calls.borrow(),
            [
                ("POST", "/api/setup/inspect".to_string()),
                ("POST", "/api/setup".to_string()),
                ("POST", "/api/login".to_string()),
            ]
        );
    }

    // --- integrations -------------------------------------------------------

    fn integrations(whisparr: Option<WhisparrTarget>) -> Integrations {
        Integrations {
            jellyfin: JellyfinTarget {
                url: "http://10.0.10.5:8096".into(),
                external_url: "https://jellyfin.example".into(),
                libraries: None,
                api_key: Secret::new(JELLYFIN_KEY.into()),
            },
            whisparr,
            sent: RefCell::new(false),
        }
    }

    fn whisparr_target(delivery: Option<Delivery>, mappings: Option<Vec<Value>>) -> WhisparrTarget {
        WhisparrTarget {
            url: "http://10.0.10.12:6969".into(),
            api_key: Secret::new(WHISPARR_KEY.into()),
            delivery,
            path_mappings: mappings,
        }
    }

    fn delivery(profile: &str) -> Delivery {
        Delivery {
            enabled: true,
            root_folder_path: "/tank/data/media/xxx".into(),
            quality_profile: profile.into(),
            search_on_add: true,
        }
    }

    fn mappings() -> Vec<Value> {
        vec![json!({"whisparrPrefix": "/tank/data/media/xxx", "jellyfinPrefix": "/media/xxx"})]
    }

    fn velvarr() -> FakeTransport {
        FakeTransport::default().on_get("/api/health", vec![ok(HEALTH_OK)])
    }

    #[test]
    fn integrations_as_stored_are_unchanged_and_every_apply_hands_both_keys_over() {
        let task = integrations(Some(whisparr_target(
            Some(delivery("HD-1080p")),
            Some(mappings()),
        )));
        let t = velvarr()
            .on_get("/api/admin/integrations", vec![ok(INTEGRATIONS_DELIVERY)])
            .on_get("/api/admin/whisparr", vec![ok(WHISPARR_NOW)])
            .on_put(vec![ok(INTEGRATIONS_DELIVERY)]);
        let report = run(Mode::Apply, &task, &t, &FakeClock::new(), Timing::default()).unwrap();
        assert_eq!(report.outcome, Outcome::Unchanged);
        assert_eq!(report.handed_over.len(), 2);
        assert_eq!(t.written.borrow().len(), 1);
        let (path, body) = sent(&t, 0);
        assert_eq!(path, "/api/admin/integrations");
        assert_eq!(
            body,
            json!({
                "jellyfinUrl": "http://10.0.10.5:8096",
                "jellyfinExternalUrl": "https://jellyfin.example",
                "jellyfinApiKey": JELLYFIN_KEY,
                "whisparrUrl": "http://10.0.10.12:6969",
                "whisparrApiKey": WHISPARR_KEY,
                "delivery": {"enabled": true, "rootFolderPath": "/tank/data/media/xxx",
                             "qualityProfileId": 4, "searchOnAdd": true},
                "pathMappings": mappings(),
            })
        );
        for line in &report.handed_over {
            assert!(
                !line.contains(JELLYFIN_KEY) && !line.contains(WHISPARR_KEY),
                "{line}"
            );
        }
        // A plan reads and sends nothing.
        let t = velvarr()
            .on_get("/api/admin/integrations", vec![ok(INTEGRATIONS_DELIVERY)])
            .on_get("/api/admin/whisparr", vec![ok(WHISPARR_NOW)]);
        let report = run(Mode::Plan, &task, &t, &FakeClock::new(), Timing::default()).unwrap();
        assert_eq!(report.outcome, Outcome::Unchanged);
        assert!(t.written.borrow().is_empty());
    }

    #[test]
    fn a_body_with_a_whisparr_url_carries_the_stored_delivery_and_mappings() {
        // Velvarr's `nextWhisparr` takes delivery and path mappings from the
        // connection it builds out of `whisparrUrl`, not from the stored one:
        // a body without them would drop both. A spec that leaves them to
        // Velvarr gets them sent back as stored.
        let task = integrations(Some(whisparr_target(None, None)));
        let t = velvarr()
            .on_get("/api/admin/integrations", vec![ok(INTEGRATIONS_DELIVERY)])
            .on_put(vec![ok(INTEGRATIONS_DELIVERY)]);
        run(Mode::Apply, &task, &t, &FakeClock::new(), Timing::default()).unwrap();
        let (_, body) = sent(&t, 0);
        assert_eq!(body["delivery"]["qualityProfileId"], json!(4));
        assert_eq!(body["pathMappings"], json!(mappings()));
    }

    #[test]
    fn a_spec_without_whisparr_leaves_the_connection_alone() {
        let task = integrations(None);
        let t = velvarr()
            .on_get("/api/admin/integrations", vec![ok(INTEGRATIONS_NOW)])
            .on_put(vec![ok(INTEGRATIONS_NOW)]);
        let report = run(Mode::Apply, &task, &t, &FakeClock::new(), Timing::default()).unwrap();
        assert_eq!(report.outcome, Outcome::Unchanged);
        assert!(report.notes[0].contains("Whisparr connection, left as it is"));
        let (_, body) = sent(&t, 0);
        for key in ["whisparrUrl", "whisparrApiKey", "delivery", "pathMappings"] {
            assert!(body.get(key).is_none(), "{key}");
        }
    }

    #[test]
    fn a_new_whisparr_and_its_delivery_take_two_writes_and_are_read_back() {
        // Velvarr reaches no Whisparr yet, so the profile can only be named
        // through it after the connection is stored.
        let task = integrations(Some(whisparr_target(Some(delivery("Any")), None)));
        let t = velvarr()
            .on_get(
                "/api/admin/integrations",
                vec![ok(INTEGRATIONS_NO_WHISPARR), ok(&with_delivery(1))],
            )
            .on_get("/api/admin/whisparr", vec![ok(WHISPARR_NOW)])
            .on_put(vec![ok("{}")]);
        let report = run(Mode::Apply, &task, &t, &FakeClock::new(), Timing::default()).unwrap();
        let Outcome::Changed(changes) = report.outcome else {
            panic!("{:?}", report.outcome);
        };
        assert_eq!(changes[0].to_string(), "whisparr: (missing) -> (added)");
        assert_eq!(t.written.borrow().len(), 2);
        let (_, first) = sent(&t, 0);
        assert!(first.get("delivery").is_none(), "{first}");
        assert_eq!(first["whisparrApiKey"], json!(WHISPARR_KEY));
        let (_, second) = sent(&t, 1);
        assert_eq!(second["delivery"]["qualityProfileId"], json!(1));
    }

    fn with_delivery(profile_id: i64) -> String {
        let mut now: Value = serde_json::from_str(INTEGRATIONS_DELIVERY).unwrap();
        now["whisparr"]["delivery"]["qualityProfileId"] = json!(profile_id);
        now["whisparr"]["pathMappings"] = json!([]);
        now.to_string()
    }

    #[test]
    fn a_delivery_names_its_profile_and_an_unknown_one_is_refused() {
        let task = integrations(Some(whisparr_target(Some(delivery("Any")), None)));
        let t = velvarr()
            .on_get("/api/admin/integrations", vec![ok(INTEGRATIONS_DELIVERY)])
            .on_get("/api/admin/whisparr", vec![ok(WHISPARR_NOW)]);
        let report = run(Mode::Plan, &task, &t, &FakeClock::new(), Timing::default()).unwrap();
        let Outcome::Differs(changes) = report.outcome else {
            panic!("{:?}", report.outcome);
        };
        assert!(
            changes[0].to_string().starts_with("whisparr: delivery {"),
            "{}",
            changes[0]
        );
        assert!(changes[0].to_string().contains(r#""qualityProfileId":1"#));

        let task = integrations(Some(whisparr_target(Some(delivery("4K")), None)));
        let t = velvarr()
            .on_get("/api/admin/integrations", vec![ok(INTEGRATIONS_DELIVERY)])
            .on_get("/api/admin/whisparr", vec![ok(WHISPARR_NOW)]);
        let error = run(Mode::Plan, &task, &t, &FakeClock::new(), Timing::default())
            .err()
            .unwrap();
        assert!(
            error.to_string().contains("Whisparr quality profile 4K"),
            "{error}"
        );
    }

    #[test]
    fn a_whisparr_velvarr_cannot_reach_is_named_by_code() {
        let task = integrations(Some(whisparr_target(Some(delivery("Any")), None)));
        let t = velvarr()
            .on_get("/api/admin/integrations", vec![ok(INTEGRATIONS_DELIVERY)])
            .on_get("/api/admin/whisparr", vec![ok(WHISPARR_ERROR)]);
        let error = run(Mode::Plan, &task, &t, &FakeClock::new(), Timing::default())
            .err()
            .unwrap();
        let text = error.to_string();
        assert!(
            text.contains("cannot read Whisparr's quality profiles: upstream_auth"),
            "{text}"
        );
        assert!(!text.contains("rejected"), "{text}");
    }

    #[test]
    fn a_changed_url_is_a_change_and_the_libraries_are_only_checked() {
        let mut task = integrations(None);
        task.jellyfin.url = "http://10.0.10.6:8096".into();
        task.jellyfin.libraries = Some(vec!["XXX".into()]);
        let t = velvarr()
            .on_get("/api/admin/integrations", vec![ok(INTEGRATIONS_NOW)])
            .on_get("/api/admin/users", vec![ok(USERS_NOW)]);
        let report = run(Mode::Plan, &task, &t, &FakeClock::new(), Timing::default()).unwrap();
        let Outcome::Differs(changes) = report.outcome else {
            panic!("{:?}", report.outcome);
        };
        assert_eq!(
            changes[0].to_string(),
            r#"jellyfin: url "http://10.0.10.5:8096" -> "http://10.0.10.6:8096""#
        );

        // Another library than the one set up: no write can change it.
        task.jellyfin.libraries = Some(vec!["Filme".into()]);
        let t = velvarr()
            .on_get("/api/admin/integrations", vec![ok(INTEGRATIONS_NOW)])
            .on_get("/api/admin/users", vec![ok(USERS_NOW)]);
        let error = run(Mode::Plan, &task, &t, &FakeClock::new(), Timing::default())
            .err()
            .unwrap();
        assert!(matches!(error, Error::Mismatch(_)), "{error}");
        assert!(error.to_string().contains("at setup only"), "{error}");
    }

    #[test]
    fn a_key_velvarr_does_not_hold_is_a_change_that_shows_no_value() {
        let mut now: Value = serde_json::from_str(INTEGRATIONS_NOW).unwrap();
        now["whisparr"]["apiKeyConfigured"] = json!(false);
        let task = integrations(Some(whisparr_target(None, None)));
        let t = velvarr().on_get("/api/admin/integrations", vec![ok(&now.to_string())]);
        let report = run(Mode::Plan, &task, &t, &FakeClock::new(), Timing::default()).unwrap();
        let Outcome::Differs(changes) = report.outcome else {
            panic!("{:?}", report.outcome);
        };
        assert_eq!(
            changes[0].to_string(),
            "whisparr: apiKey (not stored) -> (the credential's, not shown)"
        );
    }

    // --- accounts -----------------------------------------------------------

    fn accounts(pairs: Vec<(&str, AccountTarget)>) -> Accounts {
        Accounts {
            accounts: pairs.into_iter().map(|(n, a)| (n.to_string(), a)).collect(),
            signed_in_as: "converge".into(),
        }
    }

    fn anna() -> AccountTarget {
        AccountTarget {
            role: Some("requester".into()),
            enabled: Some(true),
            auto_approve: Some(false),
            libraries: Some(vec!["XXX".into()]),
        }
    }

    #[test]
    fn a_missing_account_is_imported_and_then_set() {
        let task = accounts(vec![("anna", anna())]);
        let after: Value = {
            let mut users: Value = serde_json::from_str(USERS_NOW).unwrap();
            users["accounts"][1]["enabled"] = json!(true);
            users["accounts"][1]["libraryIds"] = json!(["098873858ff54b6c9a23aefae609dd20"]);
            users
        };
        let t = velvarr()
            .on_get(
                "/api/admin/users",
                vec![ok(USERS_BEFORE), ok(USERS_NOW), ok(&after.to_string())],
            )
            .on_put(vec![ok(IMPORTED), ok(UPDATED)]);
        let report = run(Mode::Apply, &task, &t, &FakeClock::new(), Timing::default()).unwrap();
        let Outcome::Changed(changes) = report.outcome else {
            panic!("{:?}", report.outcome);
        };
        assert_eq!(
            changes[0].to_string(),
            "account anna: (missing) -> (imported from Jellyfin)"
        );
        assert_eq!(
            *t.calls.borrow(),
            [
                ("POST", "/api/admin/users/import".to_string()),
                (
                    "PATCH",
                    "/api/admin/users/7d1e2f3a4b5c4d6e8f90a1b2c3d4e5f6".to_string()
                ),
            ]
        );
        let (_, body) = sent(&t, 1);
        assert_eq!(
            body,
            json!({"enabled": true, "role": "requester", "autoApprove": false,
                   "libraryIds": ["098873858ff54b6c9a23aefae609dd20"]})
        );
    }

    #[test]
    fn an_account_as_stored_is_unchanged_and_nothing_is_written() {
        let task = accounts(vec![(
            "anna",
            AccountTarget {
                role: Some("requester".into()),
                enabled: Some(false),
                ..AccountTarget::default()
            },
        )]);
        let t = velvarr().on_get("/api/admin/users", vec![ok(USERS_NOW)]);
        let report = run(Mode::Apply, &task, &t, &FakeClock::new(), Timing::default()).unwrap();
        assert_eq!(report.outcome, Outcome::Unchanged);
        assert_eq!(report.notes, ["not in the spec: account converge"]);
        assert!(t.written.borrow().is_empty());
    }

    #[test]
    fn a_field_the_spec_leaves_out_is_sent_as_stored() {
        // Velvarr's PATCH wants enabled, role and libraryIds every time.
        let task = accounts(vec![(
            "anna",
            AccountTarget {
                enabled: Some(true),
                ..AccountTarget::default()
            },
        )]);
        let t = velvarr()
            .on_get(
                "/api/admin/users",
                vec![
                    ok(USERS_NOW),
                    ok(&USERS_NOW.replace(
                        r#""name":"anna","role":"requester","enabled":false"#,
                        r#""name":"anna","role":"requester","enabled":true"#,
                    )),
                ],
            )
            .on_put(vec![ok(UPDATED)]);
        run(Mode::Apply, &task, &t, &FakeClock::new(), Timing::default()).unwrap();
        let (_, body) = sent(&t, 0);
        assert_eq!(
            body,
            json!({"enabled": true, "role": "requester", "libraryIds": []})
        );
    }

    #[test]
    fn the_owner_and_the_signed_in_account_stay_enabled_admins() {
        let task = accounts(vec![(
            "converge",
            AccountTarget {
                role: Some("moderator".into()),
                ..AccountTarget::default()
            },
        )]);
        let t = velvarr().on_get("/api/admin/users", vec![ok(USERS_NOW)]);
        let error = run(Mode::Plan, &task, &t, &FakeClock::new(), Timing::default())
            .err()
            .unwrap();
        assert!(error.to_string().contains("Velvarr's owner"), "{error}");

        let mut task = accounts(vec![(
            "anna",
            AccountTarget {
                enabled: Some(false),
                ..AccountTarget::default()
            },
        )]);
        task.signed_in_as = "anna".into();
        let t = velvarr().on_get("/api/admin/users", vec![ok(USERS_NOW)]);
        let error = run(Mode::Plan, &task, &t, &FakeClock::new(), Timing::default())
            .err()
            .unwrap();
        assert!(error.to_string().contains("locks converge out"), "{error}");
    }

    #[test]
    fn an_account_jellyfin_does_not_have_is_named_after_the_import() {
        let task = accounts(vec![("bob", anna())]);
        let t = velvarr()
            .on_get("/api/admin/users", vec![ok(USERS_NOW)])
            .on_put(vec![ok(IMPORTED)]);
        let error = run(Mode::Apply, &task, &t, &FakeClock::new(), Timing::default())
            .err()
            .unwrap();
        assert!(matches!(error, Error::NotFound(_)), "{error}");
        assert!(
            error.to_string().contains("Jellyfin account bob"),
            "{error}"
        );
    }

    #[test]
    fn a_library_velvarr_is_not_set_up_with_is_refused_before_a_write() {
        let mut target = anna();
        target.libraries = Some(vec!["Filme".into()]);
        let task = accounts(vec![("anna", target)]);
        let t = velvarr().on_get("/api/admin/users", vec![ok(USERS_NOW)]);
        let error = run(Mode::Apply, &task, &t, &FakeClock::new(), Timing::default())
            .err()
            .unwrap();
        assert!(error.to_string().contains("library Filme"), "{error}");
        assert!(t.written.borrow().is_empty());
    }

    #[test]
    fn an_account_id_that_moves_the_path_is_refused() {
        let users = USERS_NOW.replace("7d1e2f3a4b5c4d6e8f90a1b2c3d4e5f6", "../setup");
        let task = accounts(vec![("anna", anna())]);
        let t = velvarr().on_get("/api/admin/users", vec![ok(&users)]);
        let error = run(Mode::Plan, &task, &t, &FakeClock::new(), Timing::default())
            .err()
            .unwrap();
        assert!(error.to_string().contains("path segment"), "{error}");
    }
}
