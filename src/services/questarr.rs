//! Questarr (1.4.2): the account, download clients, the Prowlarr sync and the
//! import configuration (design §47). Questarr publishes no OpenAPI
//! description, so field names are checked against recorded answers
//! (`tests/fixtures/questarr-1.4.2`).
//!
//! No API key, no cookie: every route behind `/api` wants
//! `Authorization: Bearer <JWT>`, and the only way to a token is a sign-in
//! with the account's name and password. Questarr has exactly one account,
//! created by `POST /api/auth/setup` while there is none — so the first run
//! sets it up, and every later one signs in.

use std::{cell::RefCell, collections::BTreeMap};

use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::{
    client::{expect_status_at, Transport},
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

pub const AUTH_STATUS: Endpoint = Endpoint {
    method: "GET",
    path: "/api/auth/status",
    request: None,
    response: None,
};

pub const SETUP: Endpoint = Endpoint {
    method: "POST",
    path: "/api/auth/setup",
    request: None,
    response: None,
};

pub const LOGIN: Endpoint = Endpoint {
    method: "POST",
    path: "/api/auth/login",
    request: None,
    response: None,
};

/// Questarr's API names no version anywhere.
pub const VERSION_NOT_REPORTED: &str = "(not reported)";

#[derive(Deserialize)]
struct Health {
    status: Option<String>,
}

/// `GET /api/auth/status`. The field is required: an answer without it is
/// not Questarr's, and "no account" must never be guessed.
#[derive(Deserialize)]
struct AuthStatus {
    #[serde(rename = "hasUsers")]
    has_users: bool,
}

/// What a setup or a login answered. Only the token is read.
#[derive(Deserialize)]
struct TokenAnswer {
    token: Secret,
}

/// Readiness: `GET /api/health` answers `status: ok`, without a token.
pub fn probe(t: &dyn Transport) -> Result<String, Probe> {
    let reply = t
        .get(HEALTH.path)
        .map_err(|e| Probe::NotYet(e.to_string()))?;
    if reply.status != 200 {
        return Err(Probe::NotYet(format!("HTTP {}", reply.status)));
    }
    let health: Health = serde_json::from_str(&reply.body)
        .map_err(|e| Probe::NotYet(format!("unexpected answer: {}", crate::error::shape(&e))))?;
    match health.status.as_deref() {
        Some("ok") => Ok(VERSION_NOT_REPORTED.to_string()),
        _ => Err(Probe::NotYet("health is not ok yet".to_string())),
    }
}

/// The body of both the setup and the login. Built here, so the password
/// never travels in `argv`; a serializing failure names the path only.
fn account_body(endpoint: &Endpoint, username: &str, password: &Secret) -> Result<String, Error> {
    serde_json::to_string(&json!({
        "username": username,
        "password": password.expose(),
    }))
    .map_err(|_| Error::Request {
        method: endpoint.method,
        path: endpoint.path.to_string(),
        reason: "cannot serialize the body".to_string(),
    })
}

fn token_of(endpoint: &Endpoint, body: &str) -> Result<Secret, Error> {
    let answer: TokenAnswer = serde_json::from_str(body).map_err(|e| Error::Decode {
        path: endpoint.path.to_string(),
        reason: crate::error::shape(&e),
    })?;
    Ok(answer.token)
}

/// Exchanges the account's name and password for a JWT — and sets the
/// account up first when Questarr has none yet.
///
/// The setup sends the account alone. Questarr would also take IGDB
/// credentials there, but what it stores that way shadows the ones from its
/// environment for good.
///
/// A setup answered with 403 lost a race against another run (the account
/// check comes before everything else there), so the sign-in follows. A
/// refused sign-in is final: Questarr allows 20 of them per 15 minutes and
/// address, successful ones included, and a wrong password stays wrong.
pub fn sign_in(t: &dyn Transport, username: &str, password: &Secret) -> Result<Secret, Error> {
    let reply = t.get(AUTH_STATUS.path)?;
    expect_status_at(AUTH_STATUS.method, AUTH_STATUS.path, &reply, &[200])?;
    let status: AuthStatus = serde_json::from_str(&reply.body).map_err(|e| Error::Decode {
        path: AUTH_STATUS.path.to_string(),
        reason: crate::error::shape(&e),
    })?;

    if !status.has_users {
        let reply = t.post_json(SETUP.path, &account_body(&SETUP, username, password)?)?;
        if reply.status != 403 {
            expect_status_at(SETUP.method, SETUP.path, &reply, &[200])?;
            return token_of(&SETUP, &reply.body);
        }
    }

    let reply = t.post_json(LOGIN.path, &account_body(&LOGIN, username, password)?)?;
    let refused = |reason: String| Error::Status {
        method: LOGIN.method,
        path: LOGIN.path.to_string(),
        status: reply.status,
        validation: vec![reason],
    };
    match reply.status {
        401 | 403 => Err(refused(format!(
            "the account {username} or its password was refused"
        ))),
        429 => Err(refused(
            "Questarr allows 20 sign-ins per 15 minutes and address; this one was over the limit"
                .to_string(),
        )),
        _ => {
            expect_status_at(LOGIN.method, LOGIN.path, &reply, &[200])?;
            token_of(&LOGIN, &reply.body)
        }
    }
}

// --- download clients -------------------------------------------------------

pub const DOWNLOADERS: Endpoint = Endpoint {
    method: "GET",
    path: "/api/downloaders",
    request: None,
    response: None,
};

/// How a stored download-client password is answered: never the value.
const MASKED: &str = "********";

/// One download client the spec declares, its secrets read from credentials.
///
/// `secrets` may hold `username` and `password`. SABnzbd's API key lives in
/// `username` (Questarr has no key column) and is answered in clear, so it is
/// compared, unseen. A `password` is answered as `********` and handed over on
/// every apply.
pub struct ClientTarget {
    pub name: String,
    pub set: BTreeMap<String, Value>,
    pub secrets: BTreeMap<String, Secret>,
}

pub struct DownloadClients {
    pub clients: Vec<ClientTarget>,
}

fn decode(path: &str, body: &str) -> Result<Value, Error> {
    serde_json::from_str(body).map_err(|e| Error::Decode {
        path: path.to_string(),
        reason: crate::error::shape(&e),
    })
}

/// A body that may hold a secret: a serializing error names the path only.
fn text_of(method: &'static str, path: &str, body: &Map<String, Value>) -> Result<String, Error> {
    serde_json::to_string(body).map_err(|_| Error::Request {
        method,
        path: path.to_string(),
        reason: "cannot serialize the body".to_string(),
    })
}

impl DownloadClients {
    fn subject(name: &str) -> String {
        format!("download client {name}")
    }

    /// The one entry of that name. Questarr has no unique index on `name`, so
    /// two of them are refused rather than one of them picked.
    fn find<'a>(
        current: &'a [Map<String, Value>],
        name: &str,
    ) -> Result<Option<&'a Map<String, Value>>, Error> {
        let mut found = current
            .iter()
            .filter(|e| e.get("name").and_then(Value::as_str) == Some(name));
        let first = found.next();
        let others = found.count();
        if others > 0 {
            return Err(Error::Mismatch(vec![format!(
                "{} exists {} times in Questarr; the spec cannot say which one it means",
                Self::subject(name),
                others + 1
            )]));
        }
        Ok(first)
    }

    /// What an update has to send for an existing entry, and the change lines
    /// that say so. A secret never appears in a line.
    fn changes_of(
        target: &ClientTarget,
        entry: &Map<String, Value>,
        missing: &mut Vec<String>,
        mismatch: &mut Vec<String>,
    ) -> (Vec<Change>, Map<String, Value>) {
        let subject = Self::subject(&target.name);
        let (mut changes, mut body) = (Vec::new(), Map::new());
        for (field, desired) in &target.set {
            match entry.get(field) {
                None => missing.push(format!("{subject}: {field}")),
                Some(current) if current != desired => {
                    changes.push(Change {
                        subject: subject.clone(),
                        field: field.clone(),
                        current: shortened(current),
                        desired: shortened(desired),
                    });
                    body.insert(field.clone(), desired.clone());
                }
                Some(_) => {}
            }
        }
        for (field, secret) in &target.secrets {
            let Some(current) = entry.get(field) else {
                missing.push(format!("{subject}: {field}"));
                continue;
            };
            let change = if field == "password" {
                // Answered as `********` when one is stored, `null` or empty
                // when none is. Whether a stored one is the credential's
                // cannot be read; `hand_over` writes it on every apply.
                match current.as_str() {
                    Some(MASKED) => None,
                    Some(shown) if !shown.is_empty() => {
                        mismatch.push(format!(
                            "{subject}: Questarr answers password with a value, so it is not write-only here"
                        ));
                        None
                    }
                    _ => Some("(not stored)"),
                }
            } else if current.as_str() == Some(secret.expose()) {
                None
            } else {
                Some("(another value, not shown)")
            };
            if let Some(shown) = change {
                changes.push(Change {
                    subject: subject.clone(),
                    field: field.clone(),
                    current: shown.to_string(),
                    desired: "(the credential's, not shown)".to_string(),
                });
                body.insert(field.clone(), Value::from(secret.expose()));
            }
        }
        (changes, body)
    }

    fn update_path(entry: &Map<String, Value>) -> Result<String, Error> {
        // `read` has checked every id.
        let id = entry.get("id").and_then(Value::as_str).unwrap_or_default();
        Ok(format!("{}/{id}", DOWNLOADERS.path))
    }
}

impl Task for DownloadClients {
    type Current = Vec<Map<String, Value>>;

    fn probe(&self, t: &dyn Transport) -> Result<String, Probe> {
        probe(t)
    }

    /// An empty list is a valid answer: a fresh Questarr has no client, and
    /// every client the spec names then shows up as a change.
    fn read(&self, t: &dyn Transport) -> Result<Self::Current, Error> {
        let path = DOWNLOADERS.path;
        let reply = t.get(path)?;
        expect_status_at("GET", path, &reply, &[200])?;
        let entries: Vec<Map<String, Value>> = serde_json::from_value(decode(path, &reply.body)?)
            .map_err(|e| Error::Decode {
            path: path.to_string(),
            reason: crate::error::shape(&e),
        })?;
        for (index, entry) in entries.iter().enumerate() {
            let Some(name) = entry.get("name").and_then(Value::as_str) else {
                return Err(Error::MissingName {
                    path: path.to_string(),
                    index,
                });
            };
            // The id goes into a path; an answer must not aim a write elsewhere.
            if !entry
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(is_path_segment)
            {
                return Err(Error::Decode {
                    path: path.to_string(),
                    reason: format!("{} has no id that is a path segment", Self::subject(name)),
                });
            }
        }
        Ok(entries)
    }

    fn diff(&self, current: &Self::Current) -> Result<Vec<Change>, Error> {
        let (mut missing, mut mismatch, mut changes) = (Vec::new(), Vec::new(), Vec::new());
        for target in &self.clients {
            match Self::find(current, &target.name)? {
                Some(entry) => {
                    changes.extend(Self::changes_of(target, entry, &mut missing, &mut mismatch).0)
                }
                None => changes.push(Change {
                    subject: Self::subject(&target.name),
                    field: String::new(),
                    current: "(missing)".to_string(),
                    desired: "(added)".to_string(),
                }),
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

    fn notes(&self, current: &Self::Current) -> Vec<String> {
        current
            .iter()
            .filter_map(|e| e.get("name").and_then(Value::as_str))
            .filter(|name| !self.clients.iter().any(|t| t.name == *name))
            .map(|name| format!("not in the spec: {}", Self::subject(name)))
            .collect()
    }

    fn write(&self, t: &dyn Transport, current: &Self::Current) -> Result<(), Error> {
        for target in &self.clients {
            match Self::find(current, &target.name)? {
                None => {
                    let mut body: Map<String, Value> = target
                        .set
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    for (field, secret) in &target.secrets {
                        body.insert(field.clone(), Value::from(secret.expose()));
                    }
                    body.insert("name".to_string(), Value::from(target.name.clone()));
                    let path = DOWNLOADERS.path;
                    let reply = t.post_json(path, &text_of("POST", path, &body)?)?;
                    expect_status_at("POST", path, &reply, &[200, 201])?;
                }
                Some(entry) => {
                    let (mut missing, mut mismatch) = (Vec::new(), Vec::new());
                    let (_, body) = Self::changes_of(target, entry, &mut missing, &mut mismatch);
                    if body.is_empty() {
                        continue;
                    }
                    let path = Self::update_path(entry)?;
                    let reply = t.patch_json(&path, &text_of("PATCH", &path, &body)?)?;
                    expect_status_at("PATCH", &path, &reply, &[200])?;
                }
            }
        }
        Ok(())
    }

    /// A stored password is answered masked, so a stale one cannot be seen by
    /// reading. It is handed over on every apply, with a `PATCH` that carries
    /// only the password: Questarr leaves every field a `PATCH` omits.
    fn hand_over(&self, t: &dyn Transport, current: &Self::Current) -> Result<Vec<String>, Error> {
        let mut lines = Vec::new();
        for target in &self.clients {
            let Some(password) = target.secrets.get("password") else {
                continue;
            };
            let Some(entry) = Self::find(current, &target.name)? else {
                return Err(Error::NotFound(vec![Self::subject(&target.name)]));
            };
            let path = Self::update_path(entry)?;
            let mut body = Map::new();
            body.insert("password".to_string(), Value::from(password.expose()));
            let reply = t.patch_json(&path, &text_of("PATCH", &path, &body)?)?;
            expect_status_at("PATCH", &path, &reply, &[200])?;
            lines.push(format!(
                "{}: password handed over from credentials (write-only; Questarr keeps the rest of the row)",
                Self::subject(&target.name)
            ));
        }
        Ok(lines)
    }
}

// --- import configuration ---------------------------------------------------

pub const IMPORT_CONFIG: Endpoint = Endpoint {
    method: "GET",
    path: "/api/imports/config",
    request: None,
    response: None,
};

/// The fields of the import configuration (`importConfigPatchSchema`,
/// `server/routes/import.ts`). Questarr refuses any other key.
pub const IMPORT_CONFIG_FIELDS: [&str; 10] = [
    "enablePostProcessing",
    "autoUnpack",
    "renamePattern",
    "overwriteExisting",
    "transferMode",
    "importPlatformIds",
    "ignoredExtensions",
    "minFileSize",
    "libraryRoot",
    "autoDeleteAfterImport",
];

pub const TRANSFER_MODES: [&str; 4] = ["move", "copy", "hardlink", "symlink"];

/// The import configuration of the account that signed in: where finished
/// downloads go and how. It is kept per user, so it is the signed-in
/// account's that is read and written.
pub struct ImportConfig {
    pub config: BTreeMap<String, Value>,
}

impl ImportConfig {
    const SUBJECT: &'static str = "import configuration";

    /// The change lines and the body a `PATCH` has to carry: what differs,
    /// and nothing else.
    fn changes_of(
        &self,
        current: &Map<String, Value>,
        missing: &mut Vec<String>,
    ) -> (Vec<Change>, Map<String, Value>) {
        let (mut changes, mut body) = (Vec::new(), Map::new());
        for (field, desired) in &self.config {
            match current.get(field) {
                None => missing.push(format!("{}: {field}", Self::SUBJECT)),
                Some(now) if now != desired => {
                    changes.push(Change {
                        subject: Self::SUBJECT.to_string(),
                        field: field.clone(),
                        current: shortened(now),
                        desired: shortened(desired),
                    });
                    body.insert(field.clone(), desired.clone());
                }
                Some(_) => {}
            }
        }
        (changes, body)
    }
}

impl Task for ImportConfig {
    type Current = Map<String, Value>;

    fn probe(&self, t: &dyn Transport) -> Result<String, Probe> {
        probe(t)
    }

    fn read(&self, t: &dyn Transport) -> Result<Self::Current, Error> {
        let path = IMPORT_CONFIG.path;
        let reply = t.get(path)?;
        expect_status_at("GET", path, &reply, &[200])?;
        serde_json::from_value(decode(path, &reply.body)?).map_err(|e| Error::Decode {
            path: path.to_string(),
            reason: crate::error::shape(&e),
        })
    }

    fn diff(&self, current: &Self::Current) -> Result<Vec<Change>, Error> {
        let mut missing = Vec::new();
        let (changes, _) = self.changes_of(current, &mut missing);
        if !missing.is_empty() {
            return Err(Error::MissingField(missing));
        }
        Ok(changes)
    }

    fn notes(&self, _current: &Self::Current) -> Vec<String> {
        Vec::new()
    }

    /// One `PATCH` with the differing fields. Questarr validates it strictly
    /// (an unknown key or a transfer mode it does not know is a 400) and
    /// leaves every field the body omits.
    fn write(&self, t: &dyn Transport, current: &Self::Current) -> Result<(), Error> {
        let mut missing = Vec::new();
        let (_, body) = self.changes_of(current, &mut missing);
        if body.is_empty() {
            return Ok(());
        }
        let path = IMPORT_CONFIG.path;
        let reply = t.patch_json(path, &text_of("PATCH", path, &body)?)?;
        expect_status_at("PATCH", path, &reply, &[200])
    }
}

// --- Prowlarr sync ----------------------------------------------------------

pub const INDEXERS: Endpoint = Endpoint {
    method: "GET",
    path: "/api/indexers",
    request: None,
    response: None,
};

pub const PROWLARR_SYNC: Endpoint = Endpoint {
    method: "POST",
    path: "/api/indexers/prowlarr/sync",
    request: None,
    response: None,
};

/// The indexers of one Prowlarr, copied into Questarr.
///
/// Questarr does not keep the connection: a sync is one request that reads
/// Prowlarr's indexers and stores each as an indexer of its own, with the
/// URL `<Prowlarr>/<id>/api` and Prowlarr's key. So the state can only be
/// read off the indexers -- "none of them comes from this Prowlarr" is the
/// one difference a plan can see. The key is answered masked; every apply
/// syncs once, which hands the key over again and picks up indexers Prowlarr
/// has gained since.
pub struct ProwlarrSync {
    /// Prowlarr's base URL as Questarr reaches it, without a trailing slash.
    pub url: String,
    pub api_key: Secret,
    /// What this run's sync answered, so an apply syncs once, not twice.
    pub synced: RefCell<Option<String>>,
}

#[derive(Deserialize)]
struct SyncAnswer {
    success: bool,
    results: SyncResults,
}

/// `errors` is not read: it is Questarr's own text about a request to
/// Prowlarr, and such a request carries the key in its URL.
#[derive(Deserialize)]
struct SyncResults {
    added: u64,
    updated: u64,
    failed: u64,
}

impl ProwlarrSync {
    const SUBJECT: &'static str = "Prowlarr sync";

    /// Whether an indexer was copied from this Prowlarr: its URL is
    /// `<Prowlarr>/<id>/api`. The slash keeps `…:599` from matching `…:5998`.
    fn came_from_here(&self, entry: &Map<String, Value>) -> bool {
        entry
            .get("url")
            .and_then(Value::as_str)
            .is_some_and(|url| url.starts_with(&format!("{}/", self.url)))
    }

    /// One sync. The line it returns is what the run reports as handed over.
    fn sync(&self, t: &dyn Transport) -> Result<String, Error> {
        let path = PROWLARR_SYNC.path;
        let mut body = Map::new();
        body.insert("url".to_string(), Value::from(self.url.clone()));
        body.insert("apiKey".to_string(), Value::from(self.api_key.expose()));
        let reply = t.post_json(path, &text_of("POST", path, &body)?)?;
        expect_status_at("POST", path, &reply, &[200])?;
        let answer: SyncAnswer = serde_json::from_str(&reply.body).map_err(|e| Error::Decode {
            path: path.to_string(),
            reason: crate::error::shape(&e),
        })?;
        if !answer.success || answer.results.failed > 0 {
            return Err(Error::Refused(format!(
                "{}: {} indexer(s) could not be synced ({} added, {} updated); Questarr's log says why",
                Self::SUBJECT,
                answer.results.failed,
                answer.results.added,
                answer.results.updated
            )));
        }
        let line = format!(
            "{}: {} added, {} updated; apiKey handed over from credentials (write-only)",
            Self::SUBJECT,
            answer.results.added,
            answer.results.updated
        );
        *self.synced.borrow_mut() = Some(line.clone());
        Ok(line)
    }
}

impl Task for ProwlarrSync {
    type Current = Vec<Map<String, Value>>;

    fn probe(&self, t: &dyn Transport) -> Result<String, Probe> {
        probe(t)
    }

    /// An empty list is a valid answer: nothing was synced yet.
    fn read(&self, t: &dyn Transport) -> Result<Self::Current, Error> {
        let path = INDEXERS.path;
        let reply = t.get(path)?;
        expect_status_at("GET", path, &reply, &[200])?;
        let entries: Vec<Map<String, Value>> = serde_json::from_value(decode(path, &reply.body)?)
            .map_err(|e| Error::Decode {
            path: path.to_string(),
            reason: crate::error::shape(&e),
        })?;
        if let Some(index) = entries
            .iter()
            .position(|e| e.get("name").and_then(Value::as_str).is_none())
        {
            return Err(Error::MissingName {
                path: path.to_string(),
                index,
            });
        }
        Ok(entries)
    }

    fn diff(&self, current: &Self::Current) -> Result<Vec<Change>, Error> {
        if current.iter().any(|e| self.came_from_here(e)) {
            return Ok(Vec::new());
        }
        Ok(vec![Change {
            subject: Self::SUBJECT.to_string(),
            field: String::new(),
            current: "(none synced)".to_string(),
            desired: "(synced from Prowlarr)".to_string(),
        }])
    }

    fn notes(&self, current: &Self::Current) -> Vec<String> {
        current
            .iter()
            .filter(|e| !self.came_from_here(e))
            .filter_map(|e| e.get("name").and_then(Value::as_str))
            .map(|name| format!("not from this Prowlarr: indexer {name}"))
            .collect()
    }

    fn write(&self, t: &dyn Transport, _current: &Self::Current) -> Result<(), Error> {
        self.sync(t).map(|_| ())
    }

    /// Every apply syncs once: the key is answered masked, so a rotated one
    /// can only arrive by being sent again -- and Prowlarr may have gained
    /// indexers. A run whose write has just synced does not sync twice.
    fn hand_over(&self, t: &dyn Transport, _current: &Self::Current) -> Result<Vec<String>, Error> {
        let already = self.synced.borrow().clone();
        match already {
            Some(line) => Ok(vec![line]),
            None => Ok(vec![self.sync(t)?]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{ok, FakeTransport, Step};
    use serde_json::Value;

    const HEALTH_OK: &str = include_str!("../../tests/fixtures/questarr-1.4.2/health.json");
    const FRESH: &str = include_str!("../../tests/fixtures/questarr-1.4.2/auth-status-fresh.json");
    const SET_UP: &str =
        include_str!("../../tests/fixtures/questarr-1.4.2/auth-status-set-up.json");
    const SETUP_ANSWER: &str =
        include_str!("../../tests/fixtures/questarr-1.4.2/setup-answer.json");
    const SETUP_REFUSED: &str =
        include_str!("../../tests/fixtures/questarr-1.4.2/setup-refused.json");
    const LOGIN_ANSWER: &str =
        include_str!("../../tests/fixtures/questarr-1.4.2/login-answer.json");
    const LOGIN_REFUSED: &str =
        include_str!("../../tests/fixtures/questarr-1.4.2/login-refused.json");

    /// A value nobody would type, so a leak is easy to search for.
    const PASSWORD: &str = "account-pw-4c1e7a-never-print-me";

    fn password() -> Secret {
        Secret::new(PASSWORD.to_string())
    }

    fn questarr(status: &str, writes: Vec<Step>) -> FakeTransport {
        FakeTransport::default()
            .on_get("/api/auth/status", vec![ok(status)])
            .on_put(writes)
    }

    fn sent(t: &FakeTransport, index: usize) -> (String, Value) {
        let (path, body) = t.written.borrow()[index].clone();
        (path, serde_json::from_str(&body).unwrap())
    }

    #[test]
    fn the_probe_wants_status_ok_and_names_no_version() {
        let t = FakeTransport::default().on_get("/api/health", vec![ok(HEALTH_OK)]);
        assert_eq!(probe(&t).ok().unwrap(), VERSION_NOT_REPORTED);

        let t =
            FakeTransport::default().on_get("/api/health", vec![Step::Answer(503, String::new())]);
        assert!(matches!(probe(&t), Err(Probe::NotYet(_))));

        let t =
            FakeTransport::default().on_get("/api/health", vec![ok(r#"{"status":"starting"}"#)]);
        assert!(matches!(probe(&t), Err(Probe::NotYet(_))));
    }

    #[test]
    fn a_fresh_questarr_gets_its_account_and_the_setup_answers_the_token() {
        let t = questarr(FRESH, vec![ok(SETUP_ANSWER)]);
        let token = sign_in(&t, "achim", &password()).unwrap();
        assert_eq!(token.expose(), "<masked>");
        let (path, body) = sent(&t, 0);
        assert_eq!(path, "/api/auth/setup");
        // Only the account: IGDB sent here would shadow the environment's
        // credentials in Questarr's database for good.
        assert_eq!(body, json!({"username": "achim", "password": PASSWORD}));
        assert_eq!(t.written.borrow().len(), 1);
    }

    #[test]
    fn a_questarr_with_an_account_is_signed_in_to() {
        let t = questarr(SET_UP, vec![ok(LOGIN_ANSWER)]);
        let token = sign_in(&t, "achim", &password()).unwrap();
        assert_eq!(token.expose(), "<masked>");
        let (path, body) = sent(&t, 0);
        assert_eq!(path, "/api/auth/login");
        assert_eq!(body, json!({"username": "achim", "password": PASSWORD}));
    }

    #[test]
    fn a_setup_that_lost_the_race_signs_in_instead() {
        // Two runs start together: both read "no account", one sets it up,
        // the other gets 403 "Setup already completed".
        let t = questarr(
            FRESH,
            vec![Step::Answer(403, SETUP_REFUSED.into()), ok(LOGIN_ANSWER)],
        );
        let token = sign_in(&t, "achim", &password()).unwrap();
        assert_eq!(token.expose(), "<masked>");
        assert_eq!(
            *t.calls.borrow(),
            [
                ("POST", "/api/auth/setup".to_string()),
                ("POST", "/api/auth/login".to_string())
            ]
        );
    }

    #[test]
    fn a_refused_password_names_the_account_and_never_the_password() {
        let t = questarr(SET_UP, vec![Step::Answer(401, LOGIN_REFUSED.into())]);
        let error = sign_in(&t, "achim", &password()).err().unwrap();
        assert!(matches!(error, Error::Status { status: 401, .. }));
        let text = error.to_string();
        assert!(
            text.contains("the account achim or its password was refused"),
            "{text}"
        );
        assert!(!text.contains(PASSWORD), "{text}");
    }

    #[test]
    fn the_sign_in_limit_is_named() {
        // 20 sign-ins per 15 minutes and address, successful ones included
        // (`authRateLimiter`); waiting a poll interval will not help.
        let t = questarr(SET_UP, vec![Step::Answer(429, "Too many requests".into())]);
        let error = sign_in(&t, "achim", &password()).err().unwrap();
        assert!(matches!(error, Error::Status { status: 429, .. }));
        let text = error.to_string();
        assert!(text.contains("20 sign-ins per 15 minutes"), "{text}");
    }

    #[test]
    fn a_status_answer_without_has_users_is_not_guessed_at() {
        let t = questarr(r#"{"status":"ok"}"#, vec![ok(LOGIN_ANSWER)]);
        let error = sign_in(&t, "achim", &password()).err().unwrap();
        assert!(matches!(error, Error::Decode { .. }), "{error}");
        assert!(t.written.borrow().is_empty());
    }

    #[test]
    fn a_setup_refused_for_its_values_is_an_error_not_a_login() {
        // 400: the username or password breaks Questarr's rules (3..50
        // characters, password at least 6). Signing in cannot fix that.
        let t = questarr(
            FRESH,
            vec![Step::Answer(
                400,
                r#"{"error":"Password must be at least 6 characters"}"#.into(),
            )],
        );
        let error = sign_in(&t, "achim", &password()).err().unwrap();
        assert!(matches!(error, Error::Status { status: 400, .. }));
        assert_eq!(t.written.borrow().len(), 1);
        assert!(!error.to_string().contains(PASSWORD));
    }

    // --- download clients ---------------------------------------------------

    use crate::{
        engine::{run, Mode, Outcome, Timing},
        testing::FakeClock,
    };

    const CLIENTS: &str = include_str!("../../tests/fixtures/questarr-1.4.2/downloaders.json");
    const NO_CLIENTS: &str =
        include_str!("../../tests/fixtures/questarr-1.4.2/downloaders-empty.json");
    const CREATED: &str =
        include_str!("../../tests/fixtures/questarr-1.4.2/downloader-created.json");

    /// The made-up key the fixture was recorded with: SABnzbd's API key, which
    /// Questarr keeps in `username` and answers in clear.
    const SAB_KEY: &str = "fixture-sab-key-never-real";
    const QB_PASSWORD: &str = "qb-pw-9d2b6f-never-print-me";

    fn map(value: Value) -> BTreeMap<String, Value> {
        serde_json::from_value(value).unwrap()
    }

    fn secrets(pairs: &[(&str, &str)]) -> BTreeMap<String, Secret> {
        pairs
            .iter()
            .map(|(field, value)| (field.to_string(), Secret::new(value.to_string())))
            .collect()
    }

    fn hosts_clients(sab_key: &str, category: &str) -> DownloadClients {
        DownloadClients {
            clients: vec![
                ClientTarget {
                    name: "sabnzbd".to_string(),
                    set: map(json!({"type": "sabnzbd", "url": "10.0.10.10", "port": 8080,
                                    "useSsl": false, "category": category, "enabled": true})),
                    secrets: secrets(&[("username", sab_key)]),
                },
                ClientTarget {
                    name: "qbittorrent".to_string(),
                    set: map(
                        json!({"type": "qbittorrent", "url": "10.0.10.11", "port": 8080,
                                    "useSsl": false, "username": "admin",
                                    "category": "questarr", "enabled": true}),
                    ),
                    secrets: secrets(&[("password", QB_PASSWORD)]),
                },
            ],
        }
    }

    fn clients(lists: Vec<Step>) -> FakeTransport {
        FakeTransport::default()
            .on_get("/api/health", vec![ok(HEALTH_OK)])
            .on_get("/api/downloaders", lists)
    }

    fn id_of(name: &str) -> String {
        let all: Vec<Map<String, Value>> = serde_json::from_str(CLIENTS).unwrap();
        let entry = all.iter().find(|e| e["name"] == name).unwrap();
        entry["id"].as_str().unwrap().to_string()
    }

    fn apply(task: &DownloadClients, t: &FakeTransport) -> Result<crate::engine::Report, Error> {
        run(Mode::Apply, task, t, &FakeClock::new(), Timing::default())
    }

    #[test]
    fn the_hosts_clients_match_and_every_apply_hands_only_the_password_over() {
        let task = hosts_clients(SAB_KEY, "questarr");
        let t = clients(vec![ok(CLIENTS)]).on_put(vec![ok("{}")]);
        let report = apply(&task, &t).unwrap();
        assert_eq!(report.outcome, Outcome::Unchanged);
        assert_eq!(
            report.handed_over,
            ["download client qbittorrent: password handed over from credentials (write-only; Questarr keeps the rest of the row)"]
        );
        // One PATCH, with ONLY the password. SABnzbd's key is not written: it
        // is answered in clear and already the credential's.
        assert_eq!(t.written.borrow().len(), 1);
        let (path, body) = sent(&t, 0);
        assert_eq!(path, format!("/api/downloaders/{}", id_of("qbittorrent")));
        assert_eq!(body, json!({"password": QB_PASSWORD}));
        assert_eq!(*t.calls.borrow(), [("PATCH", path)]);
        let printed = format!("{:?} {:?}", report.handed_over, report.notes);
        assert!(!printed.contains(SAB_KEY) && !printed.contains(QB_PASSWORD));
    }

    #[test]
    fn a_plan_writes_nothing() {
        let task = hosts_clients(SAB_KEY, "questarr");
        let t = clients(vec![ok(CLIENTS)]);
        let report = run(Mode::Plan, &task, &t, &FakeClock::new(), Timing::default()).unwrap();
        assert_eq!(report.outcome, Outcome::Unchanged);
        assert!(t.written.borrow().is_empty());
    }

    #[test]
    fn missing_clients_are_created_with_every_field_and_their_secrets() {
        let task = hosts_clients(SAB_KEY, "questarr");
        // Empty at first; after the two POSTs the list is the recorded one.
        // Two creations (201), then the password's hand-over (a PATCH, 200).
        let t = clients(vec![ok(NO_CLIENTS), ok(CLIENTS)]).on_put(vec![
            Step::Answer(201, CREATED.into()),
            Step::Answer(201, CREATED.into()),
            ok("{}"),
        ]);
        let report = apply(&task, &t).unwrap();
        let Outcome::Changed(changes) = &report.outcome else {
            panic!("{:?}", report.outcome)
        };
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].subject, "download client sabnzbd");
        assert_eq!(
            (changes[0].current.as_str(), changes[0].desired.as_str()),
            ("(missing)", "(added)")
        );

        let (path, body) = sent(&t, 0);
        assert_eq!(path, "/api/downloaders");
        assert_eq!(
            body,
            json!({"name": "sabnzbd", "type": "sabnzbd", "url": "10.0.10.10", "port": 8080,
                   "useSsl": false, "category": "questarr", "enabled": true, "username": SAB_KEY})
        );
        let (path, body) = sent(&t, 1);
        assert_eq!(path, "/api/downloaders");
        assert_eq!(body["password"], QB_PASSWORD);
        assert_eq!(body["username"], "admin");
        assert_eq!(
            t.calls.borrow()[..2],
            [("POST", path.clone()), ("POST", path)]
        );
        // The new qBittorrent row gets its password handed over like any other.
        let (path, body) = sent(&t, 2);
        assert_eq!(path, format!("/api/downloaders/{}", id_of("qbittorrent")));
        assert_eq!(body, json!({"password": QB_PASSWORD}));
        assert_eq!(t.written.borrow().len(), 3);
        let printed = format!("{changes:?} {:?}", report.notes);
        assert!(
            !printed.contains(SAB_KEY) && !printed.contains(QB_PASSWORD),
            "{printed}"
        );
    }

    #[test]
    fn a_differing_field_is_patched_alone() {
        // The recorded SABnzbd has category `questarr`; the spec wants another.
        let task = hosts_clients(SAB_KEY, "spiele");
        let after = CLIENTS.replacen(r#""category": "questarr""#, r#""category": "spiele""#, 1);
        let t = clients(vec![ok(CLIENTS), ok(&after)]).on_put(vec![ok("{}")]);
        let report = apply(&task, &t).unwrap();
        let Outcome::Changed(changes) = &report.outcome else {
            panic!("{:?}", report.outcome)
        };
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].field, "category");
        assert_eq!(
            (changes[0].current.as_str(), changes[0].desired.as_str()),
            ("\"questarr\"", "\"spiele\"")
        );
        let (path, body) = sent(&t, 0);
        assert_eq!(path, format!("/api/downloaders/{}", id_of("sabnzbd")));
        assert_eq!(body, json!({"category": "spiele"}));
    }

    #[test]
    fn another_key_in_username_is_a_change_that_shows_no_value() {
        const NEW_KEY: &str = "sab-key-3e8f1c-never-print-me";
        let task = hosts_clients(NEW_KEY, "questarr");
        let after = CLIENTS.replacen(SAB_KEY, NEW_KEY, 1);
        let t = clients(vec![ok(CLIENTS), ok(&after)]).on_put(vec![ok("{}")]);
        let report = apply(&task, &t).unwrap();
        let Outcome::Changed(changes) = &report.outcome else {
            panic!("{:?}", report.outcome)
        };
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].field, "username");
        assert_eq!(changes[0].current, "(another value, not shown)");
        assert_eq!(changes[0].desired, "(the credential's, not shown)");
        let (_, body) = sent(&t, 0);
        assert_eq!(body, json!({"username": NEW_KEY}));
        let printed = format!("{changes:?} {:?} {:?}", report.notes, report.handed_over);
        assert!(
            !printed.contains(NEW_KEY) && !printed.contains(SAB_KEY),
            "{printed}"
        );
    }

    #[test]
    fn a_password_questarr_does_not_hold_is_a_change() {
        // The recorded SABnzbd has `password: null`. A spec that declares one
        // must see that as "not stored", not as fine.
        let mut task = hosts_clients(SAB_KEY, "questarr");
        task.clients[0].secrets.insert(
            "password".to_string(),
            Secret::new("sab-pw-never-print-me".to_string()),
        );
        let t = clients(vec![ok(CLIENTS)]);
        let report = run(Mode::Plan, &task, &t, &FakeClock::new(), Timing::default()).unwrap();
        let Outcome::Differs(changes) = &report.outcome else {
            panic!("{:?}", report.outcome)
        };
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].field, "password");
        assert_eq!(changes[0].current, "(not stored)");
    }

    #[test]
    fn two_clients_of_one_name_are_refused_not_guessed_at() {
        // Questarr has no unique index on `name`.
        let twice = CLIENTS.replacen(r#""name": "qbittorrent""#, r#""name": "sabnzbd""#, 1);
        let task = hosts_clients(SAB_KEY, "questarr");
        let t = clients(vec![ok(&twice)]);
        let error = apply(&task, &t).err().unwrap();
        assert!(matches!(error, Error::Mismatch(_)), "{error}");
        assert!(
            error
                .to_string()
                .contains("download client sabnzbd exists 2 times"),
            "{error}"
        );
        assert!(t.written.borrow().is_empty());
    }

    #[test]
    fn a_field_questarr_does_not_answer_is_a_typo_not_a_change() {
        let mut task = hosts_clients(SAB_KEY, "questarr");
        task.clients[0]
            .set
            .insert("catagory".to_string(), json!("questarr"));
        let t = clients(vec![ok(CLIENTS)]);
        let error = apply(&task, &t).err().unwrap();
        assert!(matches!(error, Error::MissingField(_)), "{error}");
        assert!(
            error
                .to_string()
                .contains("download client sabnzbd: catagory"),
            "{error}"
        );
        assert!(t.written.borrow().is_empty());
    }

    #[test]
    fn a_client_outside_the_spec_is_noted_and_left_alone() {
        let mut task = hosts_clients(SAB_KEY, "questarr");
        task.clients.pop();
        let t = clients(vec![ok(CLIENTS)]);
        let report = apply(&task, &t).unwrap();
        assert_eq!(
            report.notes,
            ["not in the spec: download client qbittorrent"]
        );
        assert!(t.written.borrow().is_empty());
        assert!(t.deleted.borrow().is_empty());
    }

    #[test]
    fn an_id_that_is_no_path_segment_is_not_written_to() {
        let odd = CLIENTS.replacen(&id_of("qbittorrent"), "../auth/setup", 1);
        let task = hosts_clients(SAB_KEY, "questarr");
        let t = clients(vec![ok(&odd)]);
        let error = apply(&task, &t).err().unwrap();
        assert!(matches!(error, Error::Decode { .. }), "{error}");
        assert!(t.written.borrow().is_empty());
    }

    // --- import configuration -----------------------------------------------

    const IMPORT_DEFAULT: &str =
        include_str!("../../tests/fixtures/questarr-1.4.2/imports-config-default.json");
    const IMPORT_SET: &str =
        include_str!("../../tests/fixtures/questarr-1.4.2/imports-config-set.json");
    const IMPORT_REFUSED: &str =
        include_str!("../../tests/fixtures/questarr-1.4.2/imports-config-refused.json");

    fn hosts_import() -> ImportConfig {
        ImportConfig {
            config: map(json!({
                "enablePostProcessing": true,
                "autoUnpack": true,
                "transferMode": "copy",
                "libraryRoot": "/tank/spiele/roms",
                "autoDeleteAfterImport": false,
                "overwriteExisting": false
            })),
        }
    }

    fn import(configs: Vec<Step>) -> FakeTransport {
        FakeTransport::default()
            .on_get("/api/health", vec![ok(HEALTH_OK)])
            .on_get("/api/imports/config", configs)
    }

    #[test]
    fn a_fresh_import_configuration_gets_only_what_differs() {
        // Questarr's defaults: post-processing off, hardlink, /data.
        let task = hosts_import();
        let t = import(vec![ok(IMPORT_DEFAULT), ok(IMPORT_SET)]).on_put(vec![ok(IMPORT_SET)]);
        let report = run(Mode::Apply, &task, &t, &FakeClock::new(), Timing::default()).unwrap();
        let Outcome::Changed(changes) = &report.outcome else {
            panic!("{:?}", report.outcome)
        };
        let fields: Vec<&str> = changes.iter().map(|c| c.field.as_str()).collect();
        assert_eq!(
            fields,
            [
                "autoUnpack",
                "enablePostProcessing",
                "libraryRoot",
                "transferMode"
            ]
        );
        assert!(changes.iter().all(|c| c.subject == "import configuration"));
        let root = changes.iter().find(|c| c.field == "libraryRoot").unwrap();
        assert_eq!(
            (root.current.as_str(), root.desired.as_str()),
            ("\"/data\"", "\"/tank/spiele/roms\"")
        );

        assert_eq!(t.written.borrow().len(), 1);
        let (path, body) = sent(&t, 0);
        assert_eq!(path, "/api/imports/config");
        assert_eq!(
            body,
            json!({"enablePostProcessing": true, "autoUnpack": true,
                   "transferMode": "copy", "libraryRoot": "/tank/spiele/roms"})
        );
        assert_eq!(*t.calls.borrow(), [("PATCH", path)]);
    }

    #[test]
    fn a_matching_import_configuration_is_left_alone() {
        let task = hosts_import();
        let t = import(vec![ok(IMPORT_SET)]);
        let report = run(Mode::Apply, &task, &t, &FakeClock::new(), Timing::default()).unwrap();
        assert_eq!(report.outcome, Outcome::Unchanged);
        assert!(report.handed_over.is_empty());
        assert!(t.written.borrow().is_empty());
    }

    #[test]
    fn an_import_field_questarr_does_not_answer_is_not_a_change() {
        let mut task = hosts_import();
        task.config.insert("libraryroot".to_string(), json!("/x"));
        let t = import(vec![ok(IMPORT_DEFAULT)]);
        let error = run(Mode::Apply, &task, &t, &FakeClock::new(), Timing::default())
            .err()
            .unwrap();
        assert!(matches!(error, Error::MissingField(_)), "{error}");
        assert!(
            error
                .to_string()
                .contains("import configuration: libraryroot"),
            "{error}"
        );
        assert!(t.written.borrow().is_empty());
    }

    #[test]
    fn a_refused_import_configuration_is_an_error_with_its_status() {
        let task = hosts_import();
        let t =
            import(vec![ok(IMPORT_DEFAULT)]).on_put(vec![Step::Answer(400, IMPORT_REFUSED.into())]);
        let error = run(Mode::Apply, &task, &t, &FakeClock::new(), Timing::default())
            .err()
            .unwrap();
        assert!(
            matches!(error, Error::Status { status: 400, .. }),
            "{error}"
        );
    }

    // --- Prowlarr sync ------------------------------------------------------

    const NO_INDEXERS: &str =
        include_str!("../../tests/fixtures/questarr-1.4.2/indexers-empty.json");
    const INDEXERS_SYNCED: &str = include_str!("../../tests/fixtures/questarr-1.4.2/indexers.json");
    const SYNC_ANSWER: &str =
        include_str!("../../tests/fixtures/questarr-1.4.2/prowlarr-sync-answer.json");
    const SYNC_AGAIN: &str =
        include_str!("../../tests/fixtures/questarr-1.4.2/prowlarr-sync-answer-again.json");
    const SYNC_UNREACHABLE: &str =
        include_str!("../../tests/fixtures/questarr-1.4.2/prowlarr-sync-unreachable.json");

    /// The stub the fixtures were recorded against.
    const PROWLARR: &str = "http://127.0.0.1:5998";
    const PROWLARR_KEY: &str = "prowlarr-key-5a7d2e-never-print-me";

    fn sync_of(url: &str) -> ProwlarrSync {
        ProwlarrSync {
            url: url.to_string(),
            api_key: Secret::new(PROWLARR_KEY.to_string()),
            synced: RefCell::new(None),
        }
    }

    fn indexers(lists: Vec<Step>) -> FakeTransport {
        FakeTransport::default()
            .on_get("/api/health", vec![ok(HEALTH_OK)])
            .on_get("/api/indexers", lists)
    }

    #[test]
    fn a_questarr_without_indexers_is_synced_once_and_read_back() {
        let task = sync_of(PROWLARR);
        let t = indexers(vec![ok(NO_INDEXERS), ok(INDEXERS_SYNCED)]).on_put(vec![ok(SYNC_ANSWER)]);
        let report = run(Mode::Apply, &task, &t, &FakeClock::new(), Timing::default()).unwrap();
        let Outcome::Changed(changes) = &report.outcome else {
            panic!("{:?}", report.outcome)
        };
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].subject, "Prowlarr sync");
        assert_eq!(
            (changes[0].current.as_str(), changes[0].desired.as_str()),
            ("(none synced)", "(synced from Prowlarr)")
        );
        // ONE request: the write's sync is also this run's hand-over.
        assert_eq!(t.written.borrow().len(), 1);
        let (path, body) = sent(&t, 0);
        assert_eq!(path, "/api/indexers/prowlarr/sync");
        assert_eq!(body, json!({"url": PROWLARR, "apiKey": PROWLARR_KEY}));
        assert_eq!(
            report.handed_over,
            ["Prowlarr sync: 2 added, 0 updated; apiKey handed over from credentials (write-only)"]
        );
        let printed = format!("{changes:?} {:?} {:?}", report.notes, report.handed_over);
        assert!(!printed.contains(PROWLARR_KEY), "{printed}");
    }

    #[test]
    fn a_synced_questarr_is_unchanged_and_every_apply_syncs_once_more() {
        let task = sync_of(PROWLARR);
        let t = indexers(vec![ok(INDEXERS_SYNCED)]).on_put(vec![ok(SYNC_AGAIN)]);
        let report = run(Mode::Apply, &task, &t, &FakeClock::new(), Timing::default()).unwrap();
        assert_eq!(report.outcome, Outcome::Unchanged);
        assert_eq!(t.written.borrow().len(), 1);
        assert_eq!(
            report.handed_over,
            ["Prowlarr sync: 0 added, 2 updated; apiKey handed over from credentials (write-only)"]
        );
    }

    #[test]
    fn a_plan_sees_only_whether_anything_was_synced_and_writes_nothing() {
        let t = indexers(vec![ok(INDEXERS_SYNCED)]);
        let report = run(
            Mode::Plan,
            &sync_of(PROWLARR),
            &t,
            &FakeClock::new(),
            Timing::default(),
        )
        .unwrap();
        assert_eq!(report.outcome, Outcome::Unchanged);
        assert!(t.written.borrow().is_empty());

        let t = indexers(vec![ok(NO_INDEXERS)]);
        let report = run(
            Mode::Plan,
            &sync_of(PROWLARR),
            &t,
            &FakeClock::new(),
            Timing::default(),
        )
        .unwrap();
        assert!(matches!(report.outcome, Outcome::Differs(ref c) if c.len() == 1));
        assert!(t.written.borrow().is_empty());
    }

    #[test]
    fn indexers_of_another_prowlarr_do_not_count_as_synced() {
        // The same host, another port: a prefix without the slash would match.
        let t = indexers(vec![ok(INDEXERS_SYNCED)]);
        let report = run(
            Mode::Plan,
            &sync_of("http://127.0.0.1:599"),
            &t,
            &FakeClock::new(),
            Timing::default(),
        )
        .unwrap();
        assert!(matches!(report.outcome, Outcome::Differs(_)));
        assert_eq!(
            report.notes,
            [
                "not from this Prowlarr: indexer Fixture Usenet",
                "not from this Prowlarr: indexer Fixture Torrent"
            ]
        );
    }

    #[test]
    fn a_sync_that_lost_indexers_is_an_error_that_shows_only_their_number() {
        // What Questarr puts into `errors` is its own text about a request to
        // Prowlarr; it is counted, never printed.
        let answer = json!({"success": true, "message": "x",
            "results": {"added": 1, "updated": 0, "failed": 1,
                        "errors": [format!("request to {PROWLARR}/2/api?apikey={PROWLARR_KEY} failed")]}})
        .to_string();
        let t = indexers(vec![ok(NO_INDEXERS)]).on_put(vec![ok(&answer)]);
        let error = run(
            Mode::Apply,
            &sync_of(PROWLARR),
            &t,
            &FakeClock::new(),
            Timing::default(),
        )
        .err()
        .unwrap();
        let text = error.to_string();
        assert!(text.contains("1 indexer(s) could not be synced"), "{text}");
        assert!(!text.contains(PROWLARR_KEY), "{text}");
    }

    #[test]
    fn an_unreachable_prowlarr_is_questarrs_500() {
        let t = indexers(vec![ok(NO_INDEXERS)])
            .on_put(vec![Step::Answer(500, SYNC_UNREACHABLE.into())]);
        let error = run(
            Mode::Apply,
            &sync_of(PROWLARR),
            &t,
            &FakeClock::new(),
            Timing::default(),
        )
        .err()
        .unwrap();
        assert!(
            matches!(error, Error::Status { status: 500, .. }),
            "{error}"
        );
    }
}
