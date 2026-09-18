//! bindery (1.33): download clients, Prowlarr instances, root folders and
//! settings (design §14). bindery publishes no OpenAPI description, so field
//! names are checked against the answer at runtime and against recorded
//! answers, as for ntfy (§10).
//!
//! Write-only values. bindery answers every `apiKey` and `password` as an
//! empty string, next to an `apiKeyConfigured` / `passwordConfigured` flag. An
//! update decodes over the stored row, and an empty or omitted secret keeps
//! the stored one (`applyDownloadClientCredentials`,
//! `resolveWriteOnlyAPIKey`). A stale secret cannot be seen by reading, so a
//! secret is handed over on every `apply` with a `PUT` that carries only the
//! secret fields -- the rest of the row stays as it is. A Prowlarr instance
//! whose key changes passes the new key on to every indexer synced from it.
//!
//! Nothing is deleted; a secret is never cleared (`clearApiKey` is never
//! sent).

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::{
    client::{expect_status_at, Transport},
    endpoint::Endpoint,
    engine::{shortened, Change, Probe, Task},
    error::Error,
    secret::Secret,
};

pub const HEALTH: Endpoint = Endpoint {
    method: "GET",
    path: "/api/v1/health",
    request: None,
    response: None,
};

/// One kind of named entry and where bindery keeps it.
pub struct ResourceApi {
    /// How an entry is named in output: `download client`.
    pub subject: &'static str,
    /// The field that names an entry: `name`, or `path` for root folders.
    pub key: &'static str,
    pub list: &'static str,
    pub create: &'static str,
    /// `None` where bindery has no update (root folders).
    pub update: Option<&'static str>,
}

pub static DOWNLOAD_CLIENTS: ResourceApi = ResourceApi {
    subject: "download client",
    key: "name",
    list: "/api/v1/downloadclient",
    create: "/api/v1/downloadclient",
    update: Some("/api/v1/downloadclient/{id}"),
};

pub static PROWLARR_INSTANCES: ResourceApi = ResourceApi {
    subject: "Prowlarr instance",
    key: "name",
    list: "/api/v1/prowlarr",
    create: "/api/v1/prowlarr",
    update: Some("/api/v1/prowlarr/{id}"),
};

pub static ROOT_FOLDERS: ResourceApi = ResourceApi {
    subject: "root folder",
    key: "path",
    list: "/api/v1/rootfolder",
    create: "/api/v1/rootfolder",
    update: None,
};

/// The secret fields bindery answers as an empty string, each with the flag
/// that says whether one is stored.
const WRITE_ONLY: [(&str, &str); 2] = [
    ("apiKey", "apiKeyConfigured"),
    ("password", "passwordConfigured"),
];

/// Readiness: `GET /api/v1/health` answers `status: ok` and a version.
pub fn probe(t: &dyn Transport) -> Result<String, Probe> {
    let reply = t
        .get(HEALTH.path)
        .map_err(|e| Probe::NotYet(e.to_string()))?;
    match reply.status {
        200 => {}
        401 | 403 => {
            return Err(Probe::Fatal(Error::Status {
                method: HEALTH.method,
                path: HEALTH.path.to_string(),
                status: reply.status,
                validation: vec!["the API key was refused".to_string()],
            }))
        }
        other => return Err(Probe::NotYet(format!("HTTP {other}"))),
    }
    let body: Value = serde_json::from_str(&reply.body)
        .map_err(|e| Probe::NotYet(format!("unexpected answer: {e}")))?;
    if body.get("status").and_then(Value::as_str) != Some("ok") {
        return Err(Probe::NotYet("health is not ok yet".to_string()));
    }
    body.get("version")
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
        .ok_or_else(|| Probe::NotYet("the answer has no version".to_string()))
}

fn decode(path: &str, body: &str) -> Result<Value, Error> {
    serde_json::from_str(body).map_err(|e| Error::Decode {
        path: path.to_string(),
        reason: e.to_string(),
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

// --- named entries ---------------------------------------------------------------

/// One entry the spec declares, its secrets read from credentials.
pub struct ResourceTarget {
    pub key: String,
    pub set: BTreeMap<String, Value>,
    pub secret_fields: BTreeMap<String, Secret>,
}

pub struct Resources {
    pub api: &'static ResourceApi,
    pub entries: Vec<ResourceTarget>,
}

impl Resources {
    fn subject(&self, key: &str) -> String {
        format!("{} {key}", self.api.subject)
    }

    fn key_of<'a>(&self, entry: &'a Map<String, Value>) -> Option<&'a str> {
        entry.get(self.api.key).and_then(Value::as_str)
    }

    fn find<'a>(
        &self,
        current: &'a [Map<String, Value>],
        key: &str,
    ) -> Option<&'a Map<String, Value>> {
        current.iter().find(|e| self.key_of(e) == Some(key))
    }

    fn flag_of(field: &str) -> Option<&'static str> {
        WRITE_ONLY
            .iter()
            .find(|(name, _)| *name == field)
            .map(|(_, flag)| *flag)
    }

    /// Differences of an existing entry. A secret is never compared: bindery
    /// answers it empty. Only a missing one (its flag false) is a change.
    fn changes_of(
        &self,
        target: &ResourceTarget,
        entry: &Map<String, Value>,
        missing: &mut Vec<String>,
        mismatch: &mut Vec<String>,
    ) -> Vec<Change> {
        let subject = self.subject(&target.key);
        let mut changes = Vec::new();
        for (field, desired) in &target.set {
            match entry.get(field) {
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
        for field in target.secret_fields.keys() {
            let Some(flag) = Self::flag_of(field) else {
                mismatch.push(format!(
                    "{subject}: {field} is not one of bindery's write-only fields ({})",
                    WRITE_ONLY.map(|(name, _)| name).join(", ")
                ));
                continue;
            };
            match (entry.get(field), entry.get(flag)) {
                (None, _) | (_, None) => missing.push(format!("{subject}: {field}")),
                // A value bindery shows would be compared -- but bindery shows
                // none, and a spec must not carry one it would print.
                (Some(v), _) if v.as_str().is_some_and(|s| !s.is_empty()) => {
                    mismatch.push(format!(
                    "{subject}: bindery answers {field} with a value, so it is not write-only here"
                ))
                }
                (Some(_), Some(configured)) if configured != &Value::Bool(true) => {
                    changes.push(Change {
                        subject: subject.clone(),
                        field: field.clone(),
                        current: "(not stored)".to_string(),
                        desired: "(the credential's, not shown)".to_string(),
                    })
                }
                _ => {}
            }
        }
        changes
    }

    /// The spec's fields and secrets, for a create or an update.
    fn body(&self, target: &ResourceTarget, with_key: bool) -> Map<String, Value> {
        let mut body: Map<String, Value> = target
            .set
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for (field, secret) in &target.secret_fields {
            body.insert(field.clone(), Value::from(secret.expose()));
        }
        if with_key {
            body.insert(self.api.key.to_string(), Value::from(target.key.clone()));
        }
        body
    }

    fn update_path(
        &self,
        target: &ResourceTarget,
        entry: &Map<String, Value>,
    ) -> Result<String, Error> {
        let Some(update) = self.api.update else {
            return Err(Error::Mismatch(vec![format!(
                "{}: bindery cannot update a {}",
                self.subject(&target.key),
                self.api.subject
            )]));
        };
        let id = entry
            .get("id")
            .and_then(Value::as_i64)
            .ok_or_else(|| Error::Decode {
                path: self.api.list.to_string(),
                reason: format!("{} has no integer id", self.subject(&target.key)),
            })?;
        Ok(update.replace("{id}", &id.to_string()))
    }
}

impl Task for Resources {
    type Current = Vec<Map<String, Value>>;

    fn probe(&self, t: &dyn Transport) -> Result<String, Probe> {
        probe(t)
    }

    /// An empty list is a valid answer: a fresh bindery has no entry, and
    /// every entry the spec names then shows up as a change.
    fn read(&self, t: &dyn Transport) -> Result<Self::Current, Error> {
        let reply = t.get(self.api.list)?;
        expect_status_at("GET", self.api.list, &reply, &[200])?;
        let entries: Vec<Map<String, Value>> =
            serde_json::from_value(decode(self.api.list, &reply.body)?).map_err(|e| {
                Error::Decode {
                    path: self.api.list.to_string(),
                    reason: e.to_string(),
                }
            })?;
        if let Some(index) = entries.iter().position(|e| self.key_of(e).is_none()) {
            return Err(Error::MissingName {
                path: self.api.list.to_string(),
                index,
            });
        }
        Ok(entries)
    }

    fn diff(&self, current: &Self::Current) -> Result<Vec<Change>, Error> {
        let (mut missing, mut mismatch, mut changes) = (Vec::new(), Vec::new(), Vec::new());
        for target in &self.entries {
            match self.find(current, &target.key) {
                Some(entry) => {
                    changes.extend(self.changes_of(target, entry, &mut missing, &mut mismatch))
                }
                None => changes.push(Change {
                    subject: self.subject(&target.key),
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
            .filter_map(|e| self.key_of(e))
            .filter(|key| !self.entries.iter().any(|t| t.key == *key))
            .map(|key| format!("not in the spec: {}", self.subject(key)))
            .collect()
    }

    fn write(&self, t: &dyn Transport, current: &Self::Current) -> Result<(), Error> {
        for target in &self.entries {
            match self.find(current, &target.key) {
                None => {
                    let path = self.api.create;
                    let reply =
                        t.post_json(path, &text_of("POST", path, &self.body(target, true))?)?;
                    expect_status_at("POST", path, &reply, &[200, 201])?;
                }
                Some(entry) => {
                    let (mut missing, mut mismatch) = (Vec::new(), Vec::new());
                    if self
                        .changes_of(target, entry, &mut missing, &mut mismatch)
                        .is_empty()
                    {
                        continue;
                    }
                    let path = self.update_path(target, entry)?;
                    let reply =
                        t.put_json(&path, &text_of("PUT", &path, &self.body(target, false))?)?;
                    expect_status_at("PUT", &path, &reply, &[200])?;
                }
            }
        }
        Ok(())
    }

    /// Every secret, on every `apply`: a `PUT` with only the secret fields,
    /// over the stored row.
    fn hand_over(&self, t: &dyn Transport, current: &Self::Current) -> Result<Vec<String>, Error> {
        let mut lines = Vec::new();
        for target in self.entries.iter().filter(|e| !e.secret_fields.is_empty()) {
            let Some(entry) = self.find(current, &target.key) else {
                return Err(Error::NotFound(vec![self.subject(&target.key)]));
            };
            let path = self.update_path(target, entry)?;
            let body: Map<String, Value> = target
                .secret_fields
                .iter()
                .map(|(field, secret)| (field.clone(), Value::from(secret.expose())))
                .collect();
            let reply = t.put_json(&path, &text_of("PUT", &path, &body)?)?;
            expect_status_at("PUT", &path, &reply, &[200])?;
            let names: Vec<&str> = target.secret_fields.keys().map(String::as_str).collect();
            lines.push(format!(
                "{}: {} handed over from credentials (write-only; bindery keeps the rest of the row)",
                self.subject(&target.key),
                names.join(", ")
            ));
        }
        Ok(lines)
    }
}

// --- OIDC providers --------------------------------------------------------------

/// The provider list of bindery's own login. `GET` answers the public view of
/// every provider -- `client_secret` is write-only and never in it
/// (`ProviderPublicConfig`) -- and `PUT` **replaces the whole list**. So an
/// entry the spec does not name goes back untouched, and nothing is deleted.
///
/// The secret travels like bindery's other write-only fields (§14): on every
/// `apply`, because a stale one cannot be seen by reading. bindery needs it
/// when a provider is **added** ("client_secret required for new provider")
/// and keeps the stored one when it is left out for an existing entry.
pub const OIDC_LIST: Endpoint = Endpoint {
    method: "GET",
    path: "/api/v1/auth/oidc/providers",
    request: None,
    response: None,
};

pub struct OidcTarget {
    pub id: String,
    pub set: BTreeMap<String, Value>,
    pub secret_fields: BTreeMap<String, Secret>,
}

pub struct OidcProviders {
    pub entries: Vec<OidcTarget>,
}

impl OidcProviders {
    fn subject(id: &str) -> String {
        format!("OIDC provider {id}")
    }

    fn find<'a>(current: &'a [Map<String, Value>], id: &str) -> Option<&'a Map<String, Value>> {
        current
            .iter()
            .find(|e| e.get("id").and_then(Value::as_str) == Some(id))
    }

    /// The declared fields that differ from the answer. A field the answer
    /// does not carry is an error, never an addition (§3).
    fn changes_of(
        target: &OidcTarget,
        entry: &Map<String, Value>,
        missing: &mut Vec<String>,
    ) -> Vec<Change> {
        let mut changes = Vec::new();
        for (field, desired) in &target.set {
            match entry.get(field) {
                None => missing.push(format!("{}: {field}", Self::subject(&target.id))),
                Some(value) if value != desired => changes.push(Change {
                    subject: Self::subject(&target.id),
                    field: field.clone(),
                    current: shortened(value),
                    desired: shortened(desired),
                }),
                Some(_) => {}
            }
        }
        changes
    }

    /// The list as it goes back: every entry bindery holds, with the declared
    /// fields set, plus the declared entries it does not hold yet. `status` is
    /// bindery's own and is not sent back.
    fn list_to_send(&self, current: &[Map<String, Value>], with_secrets: bool) -> Vec<Value> {
        let mut out: Vec<Map<String, Value>> = current
            .iter()
            .map(|entry| {
                let mut row = entry.clone();
                row.remove("status");
                if let Some(id) = entry.get("id").and_then(Value::as_str) {
                    if let Some(target) = self.entries.iter().find(|t| t.id == id) {
                        for (field, value) in &target.set {
                            row.insert(field.clone(), value.clone());
                        }
                        if with_secrets {
                            for (field, secret) in &target.secret_fields {
                                row.insert(field.clone(), Value::from(secret.expose()));
                            }
                        }
                    }
                }
                row
            })
            .collect();
        for target in &self.entries {
            if Self::find(current, &target.id).is_some() {
                continue;
            }
            let mut row = Map::new();
            row.insert("id".to_string(), Value::from(target.id.clone()));
            for (field, value) in &target.set {
                row.insert(field.clone(), value.clone());
            }
            // A new provider is refused without its secret, so it travels here
            // even when nothing else changed.
            for (field, secret) in &target.secret_fields {
                row.insert(field.clone(), Value::from(secret.expose()));
            }
            out.push(row);
        }
        out.into_iter().map(Value::Object).collect()
    }

    fn put_list(&self, t: &dyn Transport, list: Vec<Value>) -> Result<(), Error> {
        let path = OIDC_LIST.path;
        let body = Value::Array(list);
        let text = serde_json::to_string(&body).map_err(|_| Error::Request {
            method: "PUT",
            path: path.to_string(),
            reason: "cannot serialize the body".to_string(),
        })?;
        let reply = t.put_json(path, &text)?;
        expect_status_at("PUT", path, &reply, &[200, 201, 204])?;
        Ok(())
    }
}

impl Task for OidcProviders {
    type Current = Vec<Map<String, Value>>;

    fn probe(&self, t: &dyn Transport) -> Result<String, Probe> {
        probe(t)
    }

    fn read(&self, t: &dyn Transport) -> Result<Self::Current, Error> {
        let path = OIDC_LIST.path;
        let reply = t.get(path)?;
        expect_status_at("GET", path, &reply, &[200])?;
        let entries: Vec<Map<String, Value>> = serde_json::from_value(decode(path, &reply.body)?)
            .map_err(|e| Error::Decode {
            path: path.to_string(),
            reason: e.to_string(),
        })?;
        if let Some(index) = entries
            .iter()
            .position(|e| e.get("id").and_then(Value::as_str).is_none())
        {
            return Err(Error::MissingName {
                path: path.to_string(),
                index,
            });
        }
        Ok(entries)
    }

    fn diff(&self, current: &Self::Current) -> Result<Vec<Change>, Error> {
        let (mut missing, mut changes) = (Vec::new(), Vec::new());
        for target in &self.entries {
            match Self::find(current, &target.id) {
                Some(entry) => changes.extend(Self::changes_of(target, entry, &mut missing)),
                None => changes.push(Change {
                    subject: Self::subject(&target.id),
                    field: String::new(),
                    current: "(missing)".to_string(),
                    desired: "(added)".to_string(),
                }),
            }
        }
        if missing.is_empty() {
            Ok(changes)
        } else {
            Err(Error::MissingField(missing))
        }
    }

    fn notes(&self, current: &Self::Current) -> Vec<String> {
        current
            .iter()
            .filter_map(|e| e.get("id").and_then(Value::as_str))
            .filter(|id| !self.entries.iter().any(|t| t.id == *id))
            .map(|id| format!("not in the spec: {}", Self::subject(id)))
            .collect()
    }

    fn write(&self, t: &dyn Transport, current: &Self::Current) -> Result<(), Error> {
        self.put_list(t, self.list_to_send(current, false))
    }

    /// The secret on every `apply`, with the whole list around it.
    fn hand_over(&self, t: &dyn Transport, current: &Self::Current) -> Result<Vec<String>, Error> {
        let with_secrets: Vec<&OidcTarget> = self
            .entries
            .iter()
            .filter(|t| !t.secret_fields.is_empty())
            .collect();
        if with_secrets.is_empty() {
            return Ok(Vec::new());
        }
        self.put_list(t, self.list_to_send(current, true))?;
        Ok(with_secrets
            .iter()
            .map(|target| {
                let names: Vec<&str> = target.secret_fields.keys().map(String::as_str).collect();
                format!(
                    "{}: {} handed over from credentials (write-only; the list travels whole)",
                    Self::subject(&target.id),
                    names.join(", ")
                )
            })
            .collect())
    }
}

// --- indexers --------------------------------------------------------------------

/// The indexers Prowlarr syncs into bindery. Reading is the whole list; an
/// update is `PUT /api/v1/indexer/{id}` and decodes **over the stored row**
/// (`IndexerHandler.Update`), so a body carrying `enabled` alone leaves every
/// other field as it is.
pub const INDEXER_LIST: Endpoint = Endpoint {
    method: "GET",
    path: "/api/v1/indexer",
    request: None,
    response: None,
};

/// The kind of indexer this task speaks about. bindery's rows have no
/// `implementation` field -- the kind is `type` (measured 2026-09-18 on the
/// running service; the host's shell unit read `.implementation // .type` and
/// its first branch never matched).
const TORZNAB: &str = "torznab";

/// The only task in this program that writes to an entry the spec does **not**
/// name, and it is deliberately narrow rather than a general rule (design
/// §23): the named indexers are on, every **other** `torznab` one is off, and
/// anything else bindery holds -- a `newznab` row, say -- is left alone.
///
/// Saying "on" explicitly matters: before a name was added to the list, this
/// very task had switched that indexer off, and a spec that only switched
/// things off would leave it that way and report success.
///
/// A name bindery does not hold is an error. Without that, a typo would
/// silently switch off every torznab source -- the failure this task exists to
/// prevent, in the shape that looks like success.
pub struct Indexers {
    pub enabled: Vec<String>,
}

impl Indexers {
    fn subject(name: &str) -> String {
        format!("indexer {name}")
    }

    fn name_of(entry: &Map<String, Value>) -> Option<&str> {
        entry.get("name").and_then(Value::as_str)
    }

    fn is_torznab(entry: &Map<String, Value>) -> bool {
        entry.get("type").and_then(Value::as_str) == Some(TORZNAB)
    }

    /// Whether the spec names this row.
    fn named(&self, entry: &Map<String, Value>) -> bool {
        Self::name_of(entry).is_some_and(|n| self.enabled.iter().any(|d| d == n))
    }

    /// What `enabled` must become for this row, or `None` where the task says
    /// nothing about it.
    fn wanted(&self, entry: &Map<String, Value>) -> Option<bool> {
        if self.named(entry) {
            Some(true)
        } else if Self::is_torznab(entry) {
            Some(false)
        } else {
            None
        }
    }
}

impl Task for Indexers {
    type Current = Vec<Map<String, Value>>;

    fn probe(&self, t: &dyn Transport) -> Result<String, Probe> {
        probe(t)
    }

    fn read(&self, t: &dyn Transport) -> Result<Self::Current, Error> {
        let path = INDEXER_LIST.path;
        let reply = t.get(path)?;
        expect_status_at("GET", path, &reply, &[200])?;
        let entries: Vec<Map<String, Value>> = serde_json::from_value(decode(path, &reply.body)?)
            .map_err(|e| Error::Decode {
            path: path.to_string(),
            reason: e.to_string(),
        })?;
        if let Some(index) = entries.iter().position(|e| Self::name_of(e).is_none()) {
            return Err(Error::MissingName {
                path: path.to_string(),
                index,
            });
        }
        Ok(entries)
    }

    fn diff(&self, current: &Self::Current) -> Result<Vec<Change>, Error> {
        if current.is_empty() {
            return Err(Error::EmptyList {
                path: INDEXER_LIST.path.to_string(),
            });
        }
        let unknown: Vec<String> = self
            .enabled
            .iter()
            .filter(|name| {
                !current
                    .iter()
                    .any(|e| Self::name_of(e) == Some(name.as_str()))
            })
            .map(|name| Self::subject(name))
            .collect();
        if !unknown.is_empty() {
            return Err(Error::NotFound(unknown));
        }
        let mut changes = Vec::new();
        for entry in current {
            let (Some(name), Some(wanted)) = (Self::name_of(entry), self.wanted(entry)) else {
                continue;
            };
            let is = entry.get("enabled").and_then(Value::as_bool);
            if is != Some(wanted) {
                changes.push(Change {
                    subject: Self::subject(name),
                    field: "enabled".to_string(),
                    current: is.map_or_else(|| "(missing)".to_string(), |b| b.to_string()),
                    desired: wanted.to_string(),
                });
            }
        }
        Ok(changes)
    }

    /// The rows this task says nothing about, so that "left alone" is a line
    /// in the run and not an omission someone has to infer.
    fn notes(&self, current: &Self::Current) -> Vec<String> {
        current
            .iter()
            .filter(|e| self.wanted(e).is_none())
            .filter_map(Self::name_of)
            .map(|name| format!("left alone, not a torznab indexer: {}", Self::subject(name)))
            .collect()
    }

    /// Only the rows that differ, and each with `enabled` alone in the body.
    ///
    /// **Every `PUT` here has a price beyond the field it sets:** bindery's
    /// update puts `seedRatioSource` on `user`, and its Prowlarr syncer then
    /// never touches that row's seed-ratio override again
    /// (`applyProwlarrSeedRatio`, bindery #1065). Writing only on a difference
    /// is therefore not a nicety. The host's shell unit wrote every row on
    /// every guest start; on 2026-09-18 five of its six indexers stood on
    /// `user` because of it.
    fn write(&self, t: &dyn Transport, current: &Self::Current) -> Result<(), Error> {
        for entry in current {
            let (Some(id), Some(wanted)) =
                (entry.get("id").and_then(Value::as_i64), self.wanted(entry))
            else {
                continue;
            };
            if entry.get("enabled").and_then(Value::as_bool) == Some(wanted) {
                continue;
            }
            let path = format!("{}/{id}", INDEXER_LIST.path);
            let mut body = Map::new();
            body.insert("enabled".to_string(), Value::Bool(wanted));
            let text = text_of("PUT", &path, &body)?;
            let reply = t.put_json(&path, &text)?;
            expect_status_at("PUT", &path, &reply, &[200])?;
        }
        Ok(())
    }
}

// --- settings --------------------------------------------------------------------

/// Settings by key: `GET /api/v1/setting/{key}` answers `{key, value}`, and
/// `PUT` takes `{value}`.
pub struct Settings {
    pub set: BTreeMap<String, Value>,
}

fn setting_path(key: &str) -> String {
    format!("/api/v1/setting/{key}")
}

impl Task for Settings {
    type Current = BTreeMap<String, Value>;

    fn probe(&self, t: &dyn Transport) -> Result<String, Probe> {
        probe(t)
    }

    fn notes(&self, _current: &Self::Current) -> Vec<String> {
        Vec::new()
    }

    fn read(&self, t: &dyn Transport) -> Result<Self::Current, Error> {
        let mut values = BTreeMap::new();
        let mut unknown = Vec::new();
        for key in self.set.keys() {
            let path = setting_path(key);
            let reply = t.get(&path)?;
            if reply.status == 404 {
                unknown.push(format!("setting {key}"));
                continue;
            }
            expect_status_at("GET", &path, &reply, &[200])?;
            let body = decode(&path, &reply.body)?;
            let value = body.get("value").cloned().ok_or_else(|| Error::Decode {
                path: path.clone(),
                reason: "the answer has no value".to_string(),
            })?;
            values.insert(key.clone(), value);
        }
        if unknown.is_empty() {
            Ok(values)
        } else {
            Err(Error::NotFound(unknown))
        }
    }

    fn diff(&self, current: &Self::Current) -> Result<Vec<Change>, Error> {
        Ok(self
            .set
            .iter()
            .filter(|(key, desired)| current.get(*key) != Some(*desired))
            .map(|(key, desired)| Change {
                subject: format!("setting {key}"),
                field: String::new(),
                current: current.get(key).map(shortened).unwrap_or_default(),
                desired: shortened(desired),
            })
            .collect())
    }

    fn write(&self, t: &dyn Transport, current: &Self::Current) -> Result<(), Error> {
        for (key, desired) in &self.set {
            if current.get(key) == Some(desired) {
                continue;
            }
            let path = setting_path(key);
            let mut body = Map::new();
            body.insert("value".to_string(), desired.clone());
            let reply = t.put_json(&path, &text_of("PUT", &path, &body)?)?;
            expect_status_at("PUT", &path, &reply, &[200])?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::{
        engine::{run, Mode, Outcome, Timing},
        testing::{ok, FakeClock, FakeTransport, Step},
    };

    const HEALTH_OK: &str = include_str!("../../tests/fixtures/bindery-1.33.2/health.json");
    const CLIENTS: &str = include_str!("../../tests/fixtures/bindery-1.33.2/downloadclient.json");
    const PROWLARR: &str = include_str!("../../tests/fixtures/bindery-1.33.2/prowlarr.json");
    const OIDC: &str = include_str!("../../tests/fixtures/bindery-1.33.2/auth-oidc-providers.json");
    const INDEXERS: &str = include_str!("../../tests/fixtures/bindery-1.33.2/indexers.json");
    const FOLDERS: &str = include_str!("../../tests/fixtures/bindery-1.33.2/rootfolder.json");
    const IMPORT_MODE: &str =
        include_str!("../../tests/fixtures/bindery-1.33.2/setting-import.mode.json");

    /// Values nobody would type, so a leak is easy to search for.
    const SAB_KEY: &str = "sab-key-7f3a9c-never-print-me";
    const QB_PASSWORD: &str = "qb-pw-7f3a9c-never-print-me";

    fn map(value: Value) -> BTreeMap<String, Value> {
        value.as_object().unwrap().clone().into_iter().collect()
    }

    fn secrets(pairs: &[(&str, &str)]) -> BTreeMap<String, Secret> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), Secret::new(v.to_string())))
            .collect()
    }

    /// The host's two clients, as the recording holds them (the user name
    /// masked on the host).
    fn hosts_clients() -> Resources {
        Resources {
            api: &DOWNLOAD_CLIENTS,
            entries: vec![
                ResourceTarget {
                    key: "sabnzbd".to_string(),
                    set: map(
                        json!({"type": "sabnzbd", "host": "10.0.10.10", "port": 8080,
                                    "useSsl": false, "category": "books", "enabled": true}),
                    ),
                    secret_fields: secrets(&[("apiKey", SAB_KEY)]),
                },
                ResourceTarget {
                    key: "qbittorrent".to_string(),
                    set: map(
                        json!({"type": "qbittorrent", "host": "10.0.10.11", "port": 8080,
                                    "useSsl": false, "username": "<masked>",
                                    "category": "bindery", "enabled": true}),
                    ),
                    secret_fields: secrets(&[("password", QB_PASSWORD)]),
                },
            ],
        }
    }

    fn bindery(path: &str, lists: Vec<Step>) -> FakeTransport {
        FakeTransport::default()
            .on_get("/api/v1/health", vec![ok(HEALTH_OK)])
            .on_get(path, lists)
    }

    fn sent(t: &FakeTransport, index: usize) -> (String, Value) {
        let written = t.written.borrow();
        (
            written[index].0.clone(),
            serde_json::from_str(&written[index].1).unwrap(),
        )
    }

    #[test]
    fn probe_needs_status_ok_and_a_version_and_a_refused_key_is_fatal() {
        let t = FakeTransport::default().on_get("/api/v1/health", vec![ok(HEALTH_OK)]);
        assert_eq!(probe(&t).ok().unwrap(), "1.33.2");
        let t = FakeTransport::default().on_get(
            "/api/v1/health",
            vec![ok(r#"{"status":"starting","version":"1.33.2"}"#)],
        );
        assert!(matches!(probe(&t), Err(Probe::NotYet(_))));
        let t = FakeTransport::default()
            .on_get("/api/v1/health", vec![Step::Answer(401, String::new())]);
        assert!(matches!(probe(&t), Err(Probe::Fatal(_))));
    }

    #[test]
    fn the_hosts_clients_match_and_every_run_hands_only_the_secret_over() {
        let task = hosts_clients();
        let t = bindery("/api/v1/downloadclient", vec![ok(CLIENTS)])
            .on_put(vec![Step::Answer(200, "{}".into())]);
        let report = run(Mode::Apply, &task, &t, &FakeClock::new(), Timing::default()).unwrap();
        assert_eq!(report.outcome, Outcome::Unchanged);
        assert_eq!(
            report.handed_over,
            [
                "download client sabnzbd: apiKey handed over from credentials (write-only; bindery keeps the rest of the row)",
                "download client qbittorrent: password handed over from credentials (write-only; bindery keeps the rest of the row)"
            ]
        );
        // A PUT with ONLY the secret: bindery decodes it over the stored row.
        let (path, body) = sent(&t, 0);
        assert_eq!(path, "/api/v1/downloadclient/1");
        assert_eq!(body, json!({"apiKey": SAB_KEY}));
        let (path, body) = sent(&t, 1);
        assert_eq!(path, "/api/v1/downloadclient/2");
        assert_eq!(body, json!({"password": QB_PASSWORD}));
        let printed = format!("{:?} {:?}", report.handed_over, report.notes);
        assert!(!printed.contains(SAB_KEY) && !printed.contains(QB_PASSWORD));
    }

    #[test]
    fn plan_neither_writes_nor_hands_over() {
        let task = hosts_clients();
        let t = bindery("/api/v1/downloadclient", vec![ok(CLIENTS)]);
        let report = run(Mode::Plan, &task, &t, &FakeClock::new(), Timing::default()).unwrap();
        assert_eq!(report.outcome, Outcome::Unchanged);
        assert!(report.handed_over.is_empty());
        assert!(t.written.borrow().is_empty());
    }

    #[test]
    fn a_visible_difference_is_a_change_and_the_update_carries_the_spec_and_the_secret() {
        let mut task = hosts_clients();
        task.entries[0]
            .set
            .insert("category".to_string(), json!("audiobooks"));
        let changed = CLIENTS.replacen(r#""category": "books""#, r#""category": "audiobooks""#, 1);
        assert_ne!(
            changed, CLIENTS,
            "the recording spells the field differently"
        );
        let t = bindery("/api/v1/downloadclient", vec![ok(CLIENTS), ok(&changed)])
            .on_put(vec![Step::Answer(200, "{}".into())]);
        let report = run(Mode::Apply, &task, &t, &FakeClock::new(), Timing::default()).unwrap();
        match &report.outcome {
            Outcome::Changed(changes) => assert_eq!(
                changes.iter().map(|c| c.to_string()).collect::<Vec<_>>(),
                [r#"download client sabnzbd: category "books" -> "audiobooks""#]
            ),
            other => panic!("expected Changed, got {other:?}"),
        }
        let (path, body) = sent(&t, 0);
        assert_eq!(path, "/api/v1/downloadclient/1");
        assert_eq!(body["category"], "audiobooks");
        assert_eq!(body["apiKey"], SAB_KEY);
        assert!(body.get("name").is_none() && body.get("id").is_none());
    }

    #[test]
    fn a_missing_entry_is_added_with_its_key_and_a_secret_not_stored_is_a_change() {
        let task = hosts_clients();
        let only_qb = {
            let v: Value = serde_json::from_str(CLIENTS).unwrap();
            Value::Array(
                v.as_array()
                    .unwrap()
                    .iter()
                    .filter(|e| e["name"] != "sabnzbd")
                    .cloned()
                    .collect(),
            )
            .to_string()
        };
        let t = bindery("/api/v1/downloadclient", vec![ok(&only_qb)]);
        let changes = task.diff(&task.read(&t).unwrap()).unwrap();
        assert_eq!(
            changes[0].to_string(),
            "download client sabnzbd: (missing) -> (added)"
        );
        let t = t.on_put(vec![Step::Answer(201, "{}".into())]);
        task.write(&t, &task.read(&t).unwrap()).unwrap();
        let (path, body) = sent(&t, 0);
        assert_eq!(path, "/api/v1/downloadclient");
        assert_eq!(body["name"], "sabnzbd");
        assert_eq!(body["apiKey"], SAB_KEY);

        let not_stored = CLIENTS.replacen(
            r#""apiKeyConfigured": true"#,
            r#""apiKeyConfigured": false"#,
            1,
        );
        assert_ne!(not_stored, CLIENTS);
        let t = bindery("/api/v1/downloadclient", vec![ok(&not_stored)]);
        let changes = task.diff(&task.read(&t).unwrap()).unwrap();
        assert_eq!(
            changes.iter().map(|c| c.to_string()).collect::<Vec<_>>(),
            ["download client sabnzbd: apiKey (not stored) -> (the credential's, not shown)"]
        );
    }

    #[test]
    fn a_field_the_answer_lacks_or_a_shown_secret_is_an_error() {
        let mut task = hosts_clients();
        task.entries[0].set.insert("priorty".to_string(), json!(1));
        let t = bindery("/api/v1/downloadclient", vec![ok(CLIENTS)]);
        let err = task
            .diff(&task.read(&t).unwrap())
            .err()
            .unwrap()
            .to_string();
        assert!(err.contains("download client sabnzbd: priorty"), "{err}");

        let task = hosts_clients();
        let shown = CLIENTS.replacen(r#""apiKey": """#, r#""apiKey": "visible""#, 1);
        assert_ne!(shown, CLIENTS);
        let t = bindery("/api/v1/downloadclient", vec![ok(&shown)]);
        let err = task
            .diff(&task.read(&t).unwrap())
            .err()
            .unwrap()
            .to_string();
        assert!(err.contains("answers apiKey with a value"), "{err}");
    }

    #[test]
    fn the_prowlarr_instance_and_the_root_folder_match_the_host() {
        let task = Resources {
            api: &PROWLARR_INSTANCES,
            entries: vec![ResourceTarget {
                key: "media-01".to_string(),
                set: map(
                    json!({"url": "http://10.0.10.10:9696", "syncOnStartup": true, "enabled": true}),
                ),
                secret_fields: secrets(&[("apiKey", "prowlarr-key-never-print")]),
            }],
        };
        let t = bindery("/api/v1/prowlarr", vec![ok(PROWLARR)])
            .on_put(vec![Step::Answer(200, "{}".into())]);
        let current = task.read(&t).unwrap();
        assert_eq!(task.diff(&current).unwrap(), vec![]);
        assert_eq!(
            task.hand_over(&t, &current).unwrap(),
            ["Prowlarr instance media-01: apiKey handed over from credentials (write-only; bindery keeps the rest of the row)"]
        );
        assert_eq!(
            sent(&t, 0),
            (
                "/api/v1/prowlarr/1".to_string(),
                json!({"apiKey": "prowlarr-key-never-print"})
            )
        );

        let folders = Resources {
            api: &ROOT_FOLDERS,
            entries: vec![ResourceTarget {
                key: "/tank/data/media/books".to_string(),
                set: BTreeMap::new(),
                secret_fields: BTreeMap::new(),
            }],
        };
        let t = bindery("/api/v1/rootfolder", vec![ok(FOLDERS)]);
        let current = folders.read(&t).unwrap();
        assert_eq!(folders.diff(&current).unwrap(), vec![]);
        assert!(folders.hand_over(&t, &current).unwrap().is_empty());
        let t = bindery("/api/v1/rootfolder", vec![ok("[]")])
            .on_put(vec![Step::Answer(201, "{}".into())]);
        folders.write(&t, &folders.read(&t).unwrap()).unwrap();
        assert_eq!(
            sent(&t, 0),
            (
                "/api/v1/rootfolder".to_string(),
                json!({"path": "/tank/data/media/books"})
            )
        );
    }

    #[test]
    fn a_setting_is_compared_by_value_written_as_value_and_an_unknown_key_is_not_found() {
        let task = Settings {
            set: map(json!({"import.mode": "copy"})),
        };
        let t = bindery("/api/v1/setting/import.mode", vec![ok(IMPORT_MODE)]);
        assert_eq!(task.diff(&task.read(&t).unwrap()).unwrap(), vec![]);

        let task = Settings {
            set: map(json!({"import.mode": "move"})),
        };
        let t = bindery("/api/v1/setting/import.mode", vec![ok(IMPORT_MODE)])
            .on_put(vec![Step::Answer(200, "{}".into())]);
        let current = task.read(&t).unwrap();
        assert_eq!(
            task.diff(&current).unwrap()[0].to_string(),
            r#"setting import.mode: "copy" -> "move""#
        );
        task.write(&t, &current).unwrap();
        assert_eq!(
            sent(&t, 0),
            (
                "/api/v1/setting/import.mode".to_string(),
                json!({"value": "move"})
            )
        );

        let task = Settings {
            set: map(json!({"nope": "x"})),
        };
        let t = bindery("/api/v1/setting/nope", vec![Step::Answer(404, "{}".into())]);
        assert!(task
            .read(&t)
            .err()
            .unwrap()
            .to_string()
            .contains("setting nope"));
    }

    fn oidc(set: serde_json::Value, mit_geheimnis: bool) -> OidcProviders {
        OidcProviders {
            entries: vec![OidcTarget {
                id: "authentik".to_string(),
                set: set.as_object().unwrap().clone().into_iter().collect(),
                secret_fields: if mit_geheimnis {
                    BTreeMap::from([(
                        "client_secret".to_string(),
                        Secret::new("s3cret".to_string()),
                    )])
                } else {
                    BTreeMap::new()
                },
            }],
        }
    }

    /// The recorded answer matches what the host declares, so a spec over it
    /// is `unchanged` -- and the secret still travels.
    #[test]
    fn the_oidc_provider_of_the_host_is_unchanged_and_the_secret_is_handed_over() {
        let task = oidc(
            json!({"client_id": "bindery", "scopes": ["openid", "profile", "email"]}),
            true,
        );
        let t = FakeTransport::default()
            .on_get(HEALTH.path, vec![ok(HEALTH_OK)])
            .on_get(OIDC_LIST.path, vec![ok(OIDC), ok(OIDC)])
            .on_put(vec![Step::Answer(200, String::new())]);
        let report = run(Mode::Apply, &task, &t, &FakeClock::new(), Timing::default()).unwrap();
        assert_eq!(report.outcome, Outcome::Unchanged);
        assert_eq!(report.handed_over.len(), 1);
        assert!(report.handed_over[0].contains("client_secret handed over"));
        // The list goes back whole, with the secret and without `status`.
        let written = t.written.borrow();
        assert_eq!(written.len(), 1);
        let sent: Value = serde_json::from_str(&written[0].1).unwrap();
        assert_eq!(sent[0]["client_secret"], "s3cret");
        assert_eq!(
            sent[0]["issuer"],
            "https://auth.example.org/application/o/bindery/"
        );
        assert!(sent[0].get("status").is_none(), "status is bindery's own");
    }

    /// A foreign provider is left alone, and it travels back untouched --
    /// `PUT` replaces the whole list, so dropping it would delete it.
    #[test]
    fn a_provider_the_spec_does_not_name_travels_back_unchanged() {
        let mut list: Value = serde_json::from_str(OIDC).unwrap();
        list.as_array_mut().unwrap().push(json!({
            "id": "fremd", "name": "Fremd", "issuer": "https://example.com/",
            "client_id": "x", "scopes": ["openid"]
        }));
        let body = list.to_string();
        let task = oidc(json!({"client_id": "bindery"}), false);
        let t = FakeTransport::default()
            .on_get(HEALTH.path, vec![ok(HEALTH_OK)])
            .on_get(OIDC_LIST.path, vec![ok(&body)]);
        let current = task.read(&t).unwrap();
        assert_eq!(task.diff(&current).unwrap(), vec![]);
        assert_eq!(
            task.notes(&current),
            vec!["not in the spec: OIDC provider fremd".to_string()]
        );
        let sent = task.list_to_send(&current, false);
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[1]["id"], "fremd");
        assert_eq!(sent[1]["client_id"], "x");
    }

    /// A changed field is one change, and the whole list carries it back.
    #[test]
    fn a_changed_issuer_is_written_with_the_whole_list() {
        let task = oidc(
            json!({"issuer": "https://auth.example.org/application/o/bindery2/"}),
            false,
        );
        let changed = {
            let mut v: Value = serde_json::from_str(OIDC).unwrap();
            v[0]["issuer"] = "https://auth.example.org/application/o/bindery2/".into();
            v.to_string()
        };
        let t = FakeTransport::default()
            .on_get(HEALTH.path, vec![ok(HEALTH_OK)])
            .on_get(OIDC_LIST.path, vec![ok(OIDC), ok(&changed)])
            .on_put(vec![Step::Answer(200, String::new())]);
        let report = run(Mode::Apply, &task, &t, &FakeClock::new(), Timing::default()).unwrap();
        match report.outcome {
            Outcome::Changed(changes) => assert_eq!(
                changes[0].to_string(),
                "OIDC provider authentik: issuer \"https://auth.example.org/application/o/bindery/\" -> \"https://auth.example.org/application/o/bindery2/\""
            ),
            other => panic!("expected Changed, got {other:?}"),
        }
    }

    /// A missing provider is added -- with its secret, because bindery
    /// refuses a new one without it.
    #[test]
    fn a_missing_provider_is_added_with_its_secret() {
        let task = oidc(json!({"client_id": "bindery"}), true);
        let t = FakeTransport::default()
            .on_get(HEALTH.path, vec![ok(HEALTH_OK)])
            .on_get(OIDC_LIST.path, vec![ok("[]"), ok(OIDC), ok(OIDC)])
            .on_put(vec![
                Step::Answer(200, String::new()),
                Step::Answer(200, String::new()),
            ]);
        let current = task.read(&t).unwrap();
        assert_eq!(
            task.diff(&current).unwrap()[0].to_string(),
            "OIDC provider authentik: (missing) -> (added)"
        );
        task.write(&t, &current).unwrap();
        let written = t.written.borrow();
        let sent: Value = serde_json::from_str(&written[0].1).unwrap();
        assert_eq!(sent[0]["client_secret"], "s3cret");
    }

    /// A field the answer does not carry is an error, never an addition.
    #[test]
    fn an_unknown_field_is_an_error() {
        let task = oidc(json!({"gibtsnicht": true}), false);
        let t = FakeTransport::default().on_get(OIDC_LIST.path, vec![ok(OIDC)]);
        let current = task.read(&t).unwrap();
        let reason = task.diff(&current).unwrap_err().to_string();
        assert!(reason.contains("gibtsnicht"), "{reason}");
    }

    fn indexers(namen: &[&str]) -> Indexers {
        Indexers {
            enabled: namen.iter().map(|n| (*n).to_string()).collect(),
        }
    }

    /// The recorded answer is already what the host declares: the two named
    /// indexers are on, the other torznab rows are off. So: `unchanged`, and
    /// nothing is written -- which matters here, because every `PUT` takes the
    /// seed-ratio override away from the Prowlarr syncer.
    #[test]
    fn the_indexers_of_the_host_are_unchanged_and_nothing_is_written() {
        let task = indexers(&["MyAnonamouse", "AudioBookBay"]);
        let t = FakeTransport::default()
            .on_get(HEALTH.path, vec![ok(HEALTH_OK)])
            .on_get(INDEXER_LIST.path, vec![ok(INDEXERS), ok(INDEXERS)]);
        let report = run(Mode::Apply, &task, &t, &FakeClock::new(), Timing::default()).unwrap();
        assert_eq!(report.outcome, Outcome::Unchanged);
        assert!(t.written.borrow().is_empty(), "nothing to write");
    }

    /// A newznab indexer the spec does not name stays as it is: the task says
    /// something about torznab, not about every indexer bindery holds.
    #[test]
    fn a_newznab_indexer_is_not_switched_off() {
        let task = indexers(&["MyAnonamouse", "AudioBookBay"]);
        let t = FakeTransport::default().on_get(INDEXER_LIST.path, vec![ok(INDEXERS)]);
        let current = task.read(&t).unwrap();
        assert_eq!(task.diff(&current).unwrap(), vec![]);
        let notes = task.notes(&current);
        assert!(
            notes.iter().any(|n| n.contains("Treasure Maps")),
            "the newznab row is named as left alone: {notes:?}"
        );
    }

    /// An unnamed torznab indexer is switched off, and only that row is
    /// written -- with a body that carries `enabled` and nothing else.
    #[test]
    fn an_unnamed_torznab_indexer_is_switched_off() {
        let on = {
            let mut v: Value = serde_json::from_str(INDEXERS).unwrap();
            v[0]["enabled"] = true.into();
            v.to_string()
        };
        let task = indexers(&["MyAnonamouse", "AudioBookBay"]);
        let t = FakeTransport::default()
            .on_get(HEALTH.path, vec![ok(HEALTH_OK)])
            .on_get(INDEXER_LIST.path, vec![ok(&on), ok(INDEXERS)])
            .on_put(vec![Step::Answer(200, String::new())]);
        let report = run(Mode::Apply, &task, &t, &FakeClock::new(), Timing::default()).unwrap();
        match report.outcome {
            Outcome::Changed(changes) => assert_eq!(
                changes[0].to_string(),
                "indexer Karagarga: enabled true -> false"
            ),
            other => panic!("expected Changed, got {other:?}"),
        }
        let written = t.written.borrow();
        assert_eq!(written.len(), 1, "only the row that differs");
        assert_eq!(written[0].0, "/api/v1/indexer/1");
        let sent: Value = serde_json::from_str(&written[0].1).unwrap();
        assert_eq!(sent, json!({"enabled": false}));
    }

    /// A named indexer that is off is switched on. Saying it explicitly is the
    /// point: this very task switched it off before it was named.
    #[test]
    fn a_named_indexer_that_is_off_is_switched_on() {
        let task = indexers(&["Karagarga"]);
        let t = FakeTransport::default().on_get(INDEXER_LIST.path, vec![ok(INDEXERS)]);
        let current = task.read(&t).unwrap();
        let changes = task.diff(&current).unwrap();
        assert!(
            changes
                .iter()
                .any(|c| c.to_string() == "indexer Karagarga: enabled false -> true"),
            "{changes:?}"
        );
    }

    /// A name bindery does not hold is an error, not an empty selection: a
    /// typo would otherwise switch off every torznab source and report
    /// success.
    #[test]
    fn a_name_bindery_does_not_hold_is_an_error() {
        let task = indexers(&["MyAnonamouse", "Tippfehler"]);
        let t = FakeTransport::default().on_get(INDEXER_LIST.path, vec![ok(INDEXERS)]);
        let current = task.read(&t).unwrap();
        let reason = task.diff(&current).unwrap_err().to_string();
        assert!(reason.contains("Tippfehler"), "{reason}");
    }

    /// An empty answer is an error, not "nothing to do" (CLAUDE.md).
    #[test]
    fn an_empty_indexer_list_is_an_error() {
        let task = indexers(&["MyAnonamouse"]);
        let t = FakeTransport::default().on_get(INDEXER_LIST.path, vec![ok("[]")]);
        let current = task.read(&t).unwrap();
        assert!(task.diff(&current).is_err());
    }
}
