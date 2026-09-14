//! Seerr (3.2): the settings its API keeps in `settings.json` (design §17).
//! Seerr ships an OpenAPI description, but it misnames fields the running
//! service uses (`JellyfinSettings.hostname` where the answer says `ip`,
//! `MainSettings` without `locale`), so field names are checked against the
//! answer at runtime and against recorded answers, as for bindery (§14).
//!
//! The API key acts as user 1 -- the administrator the first sign-in creates.
//! Until then every settings route answers 403, and waiting does not help:
//! the probe is fatal then. Secrets Seerr answers in the clear (the keys of
//! Radarr, Sonarr and Jellyfin, the webhook's header) are compared and never
//! printed (§13).

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Map, Value};

use crate::{
    client::{expect_status_at, Transport},
    endpoint::Endpoint,
    engine::{shortened, Change, Probe, Task, HIDDEN},
    error::Error,
    paths,
    secret::Secret,
    spec::SeerrKind,
};

pub const STATUS: Endpoint = Endpoint {
    method: "GET",
    path: "/api/v1/status",
    request: None,
    response: None,
};
pub const MAIN: &str = "/api/v1/settings/main";
pub const JELLYFIN: &str = "/api/v1/settings/jellyfin";
pub const LIBRARIES: &str = "/api/v1/settings/jellyfin/library";
pub const WEBHOOK: &str = "/api/v1/settings/notifications/webhook";

/// Readiness: `GET /api/v1/status` answers a version without a key; the
/// key itself is checked at `GET /settings/main`, which only user 1 may read.
pub fn probe(t: &dyn Transport) -> Result<String, Probe> {
    let reply = t
        .get(STATUS.path)
        .map_err(|e| Probe::NotYet(e.to_string()))?;
    if reply.status != 200 {
        return Err(Probe::NotYet(format!("HTTP {}", reply.status)));
    }
    let body: Value = serde_json::from_str(&reply.body)
        .map_err(|e| Probe::NotYet(format!("unexpected answer: {e}")))?;
    let version = body
        .get("version")
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
        .ok_or_else(|| Probe::NotYet("the answer has no version".to_string()))?;
    let reply = t.get(MAIN).map_err(|e| Probe::NotYet(e.to_string()))?;
    let refused = |reason: &str| {
        Probe::Fatal(Error::Status {
            method: "GET",
            path: MAIN.to_string(),
            status: reply.status,
            validation: vec![reason.to_string()],
        })
    };
    match reply.status {
        200 => Ok(version),
        401 => Err(refused("the API key was refused")),
        403 => Err(refused(
            "the API key acts as user 1, who exists only after a Jellyfin administrator has signed in once",
        )),
        other => Err(Probe::NotYet(format!("HTTP {other}"))),
    }
}

fn decode_object(path: &str, body: &str) -> Result<Value, Error> {
    let value: Value = serde_json::from_str(body).map_err(|e| Error::Decode {
        path: path.to_string(),
        reason: e.to_string(),
    })?;
    if value.is_object() {
        Ok(value)
    } else {
        Err(Error::Decode {
            path: path.to_string(),
            reason: "not an object".to_string(),
        })
    }
}

fn decode_list(path: &str, body: &str) -> Result<Vec<Map<String, Value>>, Error> {
    let value: Value = serde_json::from_str(body).map_err(|e| Error::Decode {
        path: path.to_string(),
        reason: e.to_string(),
    })?;
    serde_json::from_value(value).map_err(|_| Error::Decode {
        path: path.to_string(),
        reason: "not a list of objects".to_string(),
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

/// The differences between `document` and the fields named by path. A path
/// the document does not carry goes to `missing`, never into a change.
fn compare_fields(
    subject: &str,
    document: &Value,
    set: &BTreeMap<String, Value>,
    missing: &mut Vec<String>,
) -> Vec<Change> {
    let mut changes = Vec::new();
    for (path, desired) in set {
        match paths::get(document, path) {
            None => missing.push(format!("{subject}: {path}")),
            Some(current) if current != desired => changes.push(Change {
                subject: subject.to_string(),
                field: path.clone(),
                current: shortened(current),
                desired: shortened(desired),
            }),
            Some(_) => {}
        }
    }
    changes
}

/// A secret the service shows: compared, and a difference names neither
/// value.
fn compare_secret(
    subject: &str,
    document: &Value,
    path: &str,
    secret: &Secret,
    missing: &mut Vec<String>,
) -> Option<Change> {
    match paths::get(document, path) {
        None => {
            missing.push(format!("{subject}: {path}"));
            None
        }
        Some(Value::String(current)) if current == secret.expose() => None,
        Some(current) => Some(Change {
            subject: subject.to_string(),
            field: path.to_string(),
            current: if current == &Value::String(String::new()) {
                "(empty)".to_string()
            } else {
                HIDDEN.to_string()
            },
            desired: format!("{HIDDEN} from its credential"),
        }),
    }
}

// --- main ------------------------------------------------------------------

/// Top-level fields of `settings.main`. `POST /settings/main` merges the
/// body into the stored object, so only the spec's fields travel.
pub struct Main {
    pub set: BTreeMap<String, Value>,
}

impl Task for Main {
    type Current = Value;

    fn probe(&self, t: &dyn Transport) -> Result<String, Probe> {
        probe(t)
    }

    fn read(&self, t: &dyn Transport) -> Result<Self::Current, Error> {
        let reply = t.get(MAIN)?;
        expect_status_at("GET", MAIN, &reply, &[200])?;
        decode_object(MAIN, &reply.body)
    }

    fn diff(&self, current: &Self::Current) -> Result<Vec<Change>, Error> {
        let mut missing = Vec::new();
        let changes = compare_fields("main", current, &self.set, &mut missing);
        if missing.is_empty() {
            Ok(changes)
        } else {
            Err(Error::MissingField(missing))
        }
    }

    fn notes(&self, _current: &Self::Current) -> Vec<String> {
        Vec::new()
    }

    fn write(&self, t: &dyn Transport, _current: &Self::Current) -> Result<(), Error> {
        let body: Map<String, Value> = self
            .set
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let reply = t.post_json(MAIN, &text_of("POST", MAIN, &Value::Object(body))?)?;
        expect_status_at("POST", MAIN, &reply, &[200])
    }
}

// --- jellyfin --------------------------------------------------------------

/// The link to Jellyfin and the libraries Seerr scans: exactly the named
/// ones are enabled. `POST /settings/jellyfin` tests the connection and fills
/// `serverId` and `name` itself; the libraries are set through
/// `GET /settings/jellyfin/library?enable=<ids>`, and `?sync=true` makes
/// Seerr fetch the list from Jellyfin first.
pub struct Jellyfin {
    pub set: BTreeMap<String, Value>,
    pub api_key: Secret,
    pub libraries: Vec<String>,
}

struct Library {
    id: String,
    name: String,
    enabled: bool,
}

fn libraries_of(path: &str, value: &Value) -> Result<Vec<Library>, Error> {
    let Some(list) = value.as_array() else {
        return Err(Error::Decode {
            path: path.to_string(),
            reason: "libraries is not a list".to_string(),
        });
    };
    list.iter()
        .enumerate()
        .map(|(index, entry)| {
            let text = |key: &str| entry.get(key).and_then(Value::as_str).map(str::to_string);
            match (
                text("id"),
                text("name"),
                entry.get("enabled").and_then(Value::as_bool),
            ) {
                (Some(id), Some(name), Some(enabled)) => Ok(Library { id, name, enabled }),
                _ => Err(Error::Decode {
                    path: path.to_string(),
                    reason: format!("library {index} has no id, name and enabled"),
                }),
            }
        })
        .collect()
}

/// The query that sets the enabled libraries. **It always carries
/// `enable=`**: the same route without it disables every library.
pub fn library_query(sync: bool, ids: &[String]) -> String {
    let enable = format!("enable={}", ids.join(","));
    if sync {
        format!("{LIBRARIES}?sync=true&{enable}")
    } else {
        format!("{LIBRARIES}?{enable}")
    }
}

impl Jellyfin {
    fn library_changes(&self, libraries: &[Library]) -> Vec<Change> {
        let mut changes = Vec::new();
        for name in &self.libraries {
            let subject = format!("library {name}");
            match libraries.iter().find(|l| &l.name == name) {
                Some(l) if l.enabled => {}
                Some(_) => changes.push(Change {
                    subject,
                    field: "enabled".to_string(),
                    current: "false".to_string(),
                    desired: "true".to_string(),
                }),
                None => changes.push(Change {
                    subject,
                    field: String::new(),
                    current: "(unknown to Seerr)".to_string(),
                    desired: "(synced from Jellyfin, enabled)".to_string(),
                }),
            }
        }
        for l in libraries.iter().filter(|l| l.enabled) {
            if !self.libraries.contains(&l.name) {
                changes.push(Change {
                    subject: format!("library {}", l.name),
                    field: "enabled".to_string(),
                    current: "true".to_string(),
                    desired: "false".to_string(),
                });
            }
        }
        changes
    }

    fn connection_changes(&self, current: &Value, missing: &mut Vec<String>) -> Vec<Change> {
        let mut changes = compare_fields("jellyfin", current, &self.set, missing);
        changes.extend(compare_secret(
            "jellyfin",
            current,
            "apiKey",
            &self.api_key,
            missing,
        ));
        changes
    }

    /// The ids of the named libraries; the names Seerr does not know.
    fn ids_of(&self, libraries: &[Library]) -> (Vec<String>, Vec<String>) {
        let mut ids = Vec::new();
        let mut unknown = Vec::new();
        for name in &self.libraries {
            match libraries.iter().find(|l| &l.name == name) {
                Some(l) => ids.push(l.id.clone()),
                None => unknown.push(name.clone()),
            }
        }
        (ids, unknown)
    }
}

impl Task for Jellyfin {
    type Current = Value;

    fn probe(&self, t: &dyn Transport) -> Result<String, Probe> {
        probe(t)
    }

    fn read(&self, t: &dyn Transport) -> Result<Self::Current, Error> {
        let reply = t.get(JELLYFIN)?;
        expect_status_at("GET", JELLYFIN, &reply, &[200])?;
        let current = decode_object(JELLYFIN, &reply.body)?;
        match current.get("libraries") {
            Some(list) => libraries_of(JELLYFIN, list)?,
            None => {
                return Err(Error::MissingField(vec!["jellyfin: libraries".to_string()]));
            }
        };
        Ok(current)
    }

    fn diff(&self, current: &Self::Current) -> Result<Vec<Change>, Error> {
        let mut missing = Vec::new();
        let mut changes = self.connection_changes(current, &mut missing);
        if !missing.is_empty() {
            return Err(Error::MissingField(missing));
        }
        let libraries = libraries_of(JELLYFIN, &current["libraries"])?;
        changes.extend(self.library_changes(&libraries));
        Ok(changes)
    }

    fn notes(&self, _current: &Self::Current) -> Vec<String> {
        Vec::new()
    }

    fn write(&self, t: &dyn Transport, current: &Self::Current) -> Result<(), Error> {
        let mut missing = Vec::new();
        if !self.connection_changes(current, &mut missing).is_empty() {
            let mut body: Map<String, Value> = self
                .set
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            body.insert("apiKey".to_string(), Value::from(self.api_key.expose()));
            let reply = t.post_json(JELLYFIN, &text_of("POST", JELLYFIN, &Value::Object(body))?)?;
            expect_status_at("POST", JELLYFIN, &reply, &[200])?;
        }
        let mut libraries = libraries_of(JELLYFIN, &current["libraries"])?;
        if self.library_changes(&libraries).is_empty() {
            return Ok(());
        }
        let (mut ids, unknown) = self.ids_of(&libraries);
        if !unknown.is_empty() {
            // Seerr fetches the list from Jellyfin, keeping only the ids
            // named here enabled; the second call below enables the rest.
            let path = library_query(true, &ids);
            let reply = t.get(&path)?;
            expect_status_at("GET", &path, &reply, &[200])?;
            libraries = libraries_of(
                &path,
                &serde_json::from_str(&reply.body).map_err(|e| Error::Decode {
                    path: path.clone(),
                    reason: e.to_string(),
                })?,
            )?;
            let (known, still_unknown) = self.ids_of(&libraries);
            if !still_unknown.is_empty() {
                return Err(Error::NotFound(
                    still_unknown
                        .iter()
                        .map(|name| format!("library {name:?} (Jellyfin has no such library, or Seerr filters its type)"))
                        .collect(),
                ));
            }
            ids = known;
        }
        let enabled: BTreeSet<&str> = libraries
            .iter()
            .filter(|l| l.enabled)
            .map(|l| l.id.as_str())
            .collect();
        if enabled != ids.iter().map(String::as_str).collect::<BTreeSet<&str>>() {
            let path = library_query(false, &ids);
            let reply = t.get(&path)?;
            expect_status_at("GET", &path, &reply, &[200])?;
        }
        Ok(())
    }
}

// --- radarr-servers, sonarr-servers ------------------------------------------

/// One Radarr or Sonarr entry of Seerr's, by name. The profile and the
/// root folder are named; their id and path are looked up through Seerr's
/// own `POST /settings/{kind}/test`, which asks the service and stores
/// nothing.
pub struct ServerTarget {
    pub name: String,
    pub set: BTreeMap<String, Value>,
    pub api_key: Secret,
    pub profile: String,
    pub root_folder: String,
}

pub struct Servers {
    pub kind: SeerrKind,
    pub servers: Vec<ServerTarget>,
}

/// The entries as read, and what the test answered for each target.
pub struct LoadedServers {
    pub entries: Vec<Map<String, Value>>,
    pub profile_ids: BTreeMap<String, i64>,
}

impl Servers {
    fn list_path(&self) -> String {
        format!("/api/v1/settings/{}", self.kind.path())
    }

    fn subject(&self, name: &str) -> String {
        format!("{} server {name}", self.kind.label())
    }

    fn find<'a>(
        &self,
        entries: &'a [Map<String, Value>],
        name: &str,
    ) -> Option<&'a Map<String, Value>> {
        entries
            .iter()
            .find(|e| e.get("name").and_then(Value::as_str) == Some(name))
    }

    /// What the test needs: the connection fields of the spec and the key.
    fn test_body(&self, target: &ServerTarget) -> Value {
        let field = |name: &str, default: Value| target.set.get(name).cloned().unwrap_or(default);
        json!({
            "hostname": field("hostname", Value::Null),
            "port": field("port", Value::Null),
            "useSsl": field("useSsl", json!(false)),
            "baseUrl": field("baseUrl", json!("")),
            "apiKey": target.api_key.expose(),
        })
    }

    /// The profile's id and the root folder, both confirmed by the service.
    fn resolve(&self, t: &dyn Transport, target: &ServerTarget) -> Result<i64, Error> {
        let path = format!("{}/test", self.list_path());
        let reply = t.post_json(&path, &text_of("POST", &path, &self.test_body(target))?)?;
        expect_status_at("POST", &path, &reply, &[200])?;
        let answer = decode_object(&path, &reply.body)?;
        let names = |key: &str, field: &str| -> Vec<String> {
            answer[key]
                .as_array()
                .map(|list| {
                    list.iter()
                        .filter_map(|e| e.get(field).and_then(Value::as_str))
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default()
        };
        let subject = self.subject(&target.name);
        let profile = answer["profiles"]
            .as_array()
            .and_then(|list| {
                list.iter().find(|p| {
                    p.get("name").and_then(Value::as_str) == Some(target.profile.as_str())
                })
            })
            .and_then(|p| p.get("id").and_then(Value::as_i64));
        let Some(profile_id) = profile else {
            return Err(Error::NotFound(vec![format!(
                "profile {:?} of {subject} (it has: {})",
                target.profile,
                names("profiles", "name").join(", ")
            )]));
        };
        let folders = names("rootFolders", "path");
        if !folders.contains(&target.root_folder) {
            return Err(Error::NotFound(vec![format!(
                "root folder {:?} of {subject} (it has: {})",
                target.root_folder,
                folders.join(", ")
            )]));
        }
        Ok(profile_id)
    }

    /// The fields an entry must carry: the spec's, and the three resolved ones.
    fn desired_fields(&self, target: &ServerTarget, profile_id: i64) -> Map<String, Value> {
        let mut fields: Map<String, Value> = target
            .set
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        fields.insert("activeProfileId".to_string(), json!(profile_id));
        fields.insert("activeProfileName".to_string(), json!(target.profile));
        fields.insert("activeDirectory".to_string(), json!(target.root_folder));
        fields
    }

    fn changes_of(
        &self,
        target: &ServerTarget,
        entry: &Map<String, Value>,
        profile_id: i64,
        missing: &mut Vec<String>,
    ) -> Vec<Change> {
        let subject = self.subject(&target.name);
        let mut changes = Vec::new();
        for (field, desired) in &self.desired_fields(target, profile_id) {
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
        changes.extend(compare_secret(
            &subject,
            &Value::Object(entry.clone()),
            "apiKey",
            &target.api_key,
            missing,
        ));
        changes
    }
}

impl Task for Servers {
    type Current = LoadedServers;

    fn probe(&self, t: &dyn Transport) -> Result<String, Probe> {
        probe(t)
    }

    /// An empty list is valid: a fresh Seerr has no server, and every one the
    /// spec names then shows up as a change. The test runs for every target
    /// on every read, so a profile renamed in Radarr is seen at once.
    fn read(&self, t: &dyn Transport) -> Result<Self::Current, Error> {
        let path = self.list_path();
        let reply = t.get(&path)?;
        expect_status_at("GET", &path, &reply, &[200])?;
        let entries = decode_list(&path, &reply.body)?;
        if let Some(index) = entries
            .iter()
            .position(|e| e.get("name").and_then(Value::as_str).is_none())
        {
            return Err(Error::MissingName { path, index });
        }
        let mut profile_ids = BTreeMap::new();
        for target in &self.servers {
            profile_ids.insert(target.name.clone(), self.resolve(t, target)?);
        }
        Ok(LoadedServers {
            entries,
            profile_ids,
        })
    }

    fn diff(&self, current: &Self::Current) -> Result<Vec<Change>, Error> {
        let mut missing = Vec::new();
        let mut changes = Vec::new();
        for target in &self.servers {
            let profile_id = current.profile_ids[&target.name];
            match self.find(&current.entries, &target.name) {
                Some(entry) => {
                    changes.extend(self.changes_of(target, entry, profile_id, &mut missing))
                }
                None => changes.push(Change {
                    subject: self.subject(&target.name),
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
            .entries
            .iter()
            .filter_map(|e| e.get("name").and_then(Value::as_str))
            .filter(|name| !self.servers.iter().any(|t| t.name == *name))
            .map(|name| format!("not in the spec: {}", self.subject(name)))
            .collect()
    }

    /// `PUT` replaces the whole entry, so the entry as read goes back with
    /// only the named fields changed.
    fn write(&self, t: &dyn Transport, current: &Self::Current) -> Result<(), Error> {
        for target in &self.servers {
            let profile_id = current.profile_ids[&target.name];
            let mut fields = self.desired_fields(target, profile_id);
            fields.insert("apiKey".to_string(), Value::from(target.api_key.expose()));
            match self.find(&current.entries, &target.name) {
                None => {
                    fields.insert("name".to_string(), json!(target.name));
                    let path = self.list_path();
                    let reply =
                        t.post_json(&path, &text_of("POST", &path, &Value::Object(fields))?)?;
                    expect_status_at("POST", &path, &reply, &[200, 201])?;
                }
                Some(entry) => {
                    let mut missing = Vec::new();
                    if self
                        .changes_of(target, entry, profile_id, &mut missing)
                        .is_empty()
                    {
                        continue;
                    }
                    let id =
                        entry
                            .get("id")
                            .and_then(Value::as_i64)
                            .ok_or_else(|| Error::Decode {
                                path: self.list_path(),
                                reason: format!("{} has no integer id", self.subject(&target.name)),
                            })?;
                    let mut body = entry.clone();
                    body.extend(fields);
                    let path = format!("{}/{id}", self.list_path());
                    let reply = t.put_json(&path, &text_of("PUT", &path, &Value::Object(body))?)?;
                    expect_status_at("PUT", &path, &reply, &[200])?;
                }
            }
        }
        Ok(())
    }
}

// --- webhook ---------------------------------------------------------------

/// The webhook agent. Its payload template is stored base64-encoded, and
/// the agent parses it **twice** (`JSON.parse(JSON.parse(text))`): the
/// stored text must be a JSON string literal holding the template's JSON.
/// `POST` stores `base64(jsonPayload)` of what it gets, so the body carries
/// the template's JSON as a string literal -- what Seerr's own web UI does
/// not, which is why a template saved there fails in the agent. `GET`
/// answers `JSON.parse(text)`: the template's JSON as a string when it was
/// stored this way, or the object itself when the UI stored it.
pub struct Webhook {
    pub set: BTreeMap<String, Value>,
    pub payload: Map<String, Value>,
    pub headers: BTreeMap<String, Secret>,
}

enum Stored {
    /// A string the agent can parse twice.
    Double(Value),
    /// An object: parsed once, the agent fails on the second parse.
    Single,
}

impl Webhook {
    fn stored_payload(current: &Value, missing: &mut Vec<String>) -> Option<Stored> {
        match paths::get(current, "options.jsonPayload") {
            None => {
                missing.push("webhook: options.jsonPayload".to_string());
                None
            }
            Some(Value::String(text)) => Some(Stored::Double(
                serde_json::from_str::<Value>(text).unwrap_or(Value::Null),
            )),
            Some(_) => Some(Stored::Single),
        }
    }

    /// `jsonPayload` as the request carries it: the template's JSON, encoded
    /// once more as a JSON string.
    fn encoded_payload(&self) -> Result<String, Error> {
        let inner = serde_json::to_string(&Value::Object(self.payload.clone()));
        inner
            .and_then(|text| serde_json::to_string(&text))
            .map_err(|_| Error::Request {
                method: "POST",
                path: WEBHOOK.to_string(),
                reason: "cannot serialize the payload".to_string(),
            })
    }

    fn header_entries<'a>(current: &'a Value, missing: &mut Vec<String>) -> Option<&'a Vec<Value>> {
        match paths::get(current, "options.customHeaders") {
            Some(Value::Array(list)) => Some(list),
            _ => {
                missing.push("webhook: options.customHeaders (a list)".to_string());
                None
            }
        }
    }

    fn changes_of(&self, current: &Value, missing: &mut Vec<String>) -> Vec<Change> {
        let mut changes = compare_fields("webhook", current, &self.set, missing);
        let desired = Value::Object(self.payload.clone());
        match Self::stored_payload(current, missing) {
            None => {}
            Some(Stored::Double(stored)) if stored == desired => {}
            Some(Stored::Double(stored)) => changes.push(Change {
                subject: "webhook".to_string(),
                field: "options.jsonPayload".to_string(),
                current: shortened(&stored),
                desired: shortened(&desired),
            }),
            Some(Stored::Single) => changes.push(Change {
                subject: "webhook".to_string(),
                field: "options.jsonPayload".to_string(),
                current: "(stored once-encoded; the agent fails on it)".to_string(),
                desired: "(the template, encoded for the agent)".to_string(),
            }),
        }
        if let Some(entries) = Self::header_entries(current, missing) {
            for (key, secret) in &self.headers {
                let subject = format!("webhook: customHeaders[key={key}]");
                let found = entries
                    .iter()
                    .find(|e| e.get("key").and_then(Value::as_str) == Some(key.as_str()));
                match found {
                    None => changes.push(Change {
                        subject,
                        field: String::new(),
                        current: "(missing)".to_string(),
                        desired: "(added)".to_string(),
                    }),
                    Some(entry) => {
                        changes.extend(compare_secret(&subject, entry, "value", secret, missing))
                    }
                }
            }
        }
        changes
    }
}

impl Task for Webhook {
    type Current = Value;

    fn probe(&self, t: &dyn Transport) -> Result<String, Probe> {
        probe(t)
    }

    fn read(&self, t: &dyn Transport) -> Result<Self::Current, Error> {
        let reply = t.get(WEBHOOK)?;
        expect_status_at("GET", WEBHOOK, &reply, &[200])?;
        decode_object(WEBHOOK, &reply.body)
    }

    fn diff(&self, current: &Self::Current) -> Result<Vec<Change>, Error> {
        let mut missing = Vec::new();
        let changes = self.changes_of(current, &mut missing);
        if missing.is_empty() {
            Ok(changes)
        } else {
            Err(Error::MissingField(missing))
        }
    }

    fn notes(&self, _current: &Self::Current) -> Vec<String> {
        Vec::new()
    }

    /// The whole object goes back -- `POST` replaces it -- with the named
    /// fields changed, the payload encoded for the agent, and the spec's
    /// headers set among whatever other headers there are.
    fn write(&self, t: &dyn Transport, current: &Self::Current) -> Result<(), Error> {
        let mut body = current.clone();
        for (path, value) in &self.set {
            if !paths::set(&mut body, path, value.clone()) {
                return Err(Error::MissingField(vec![format!("webhook: {path}")]));
            }
        }
        let encoded = self.encoded_payload()?;
        if !paths::set(&mut body, "options.jsonPayload", Value::String(encoded)) {
            return Err(Error::MissingField(vec![
                "webhook: options.jsonPayload".to_string()
            ]));
        }
        let mut headers: Vec<Value> = match paths::get(&body, "options.customHeaders") {
            Some(Value::Array(list)) => list.clone(),
            _ => Vec::new(),
        };
        for (key, secret) in &self.headers {
            let value = json!({"key": key, "value": secret.expose()});
            match headers
                .iter_mut()
                .find(|e| e.get("key").and_then(Value::as_str) == Some(key.as_str()))
            {
                Some(entry) => *entry = value,
                None => headers.push(value),
            }
        }
        if !paths::set(&mut body, "options.customHeaders", Value::Array(headers)) {
            return Err(Error::MissingField(vec![
                "webhook: options.customHeaders".to_string()
            ]));
        }
        let reply = t.post_json(WEBHOOK, &text_of("POST", WEBHOOK, &body)?)?;
        expect_status_at("POST", WEBHOOK, &reply, &[200])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{ok, FakeTransport, Step};

    const STATUS_JSON: &str = r#"{"version":"3.2.0","commitTag":"local","updateAvailable":false,"commitsBehind":0,"restartRequired":false}"#;
    const MAIN_JSON: &str = include_str!("../../tests/fixtures/seerr-3.2.0/main.json");
    const JELLYFIN_JSON: &str = include_str!("../../tests/fixtures/seerr-3.2.0/jellyfin.json");
    const RADARR_JSON: &str = include_str!("../../tests/fixtures/seerr-3.2.0/radarr.json");
    const SONARR_JSON: &str = include_str!("../../tests/fixtures/seerr-3.2.0/sonarr.json");
    const RADARR_TEST: &str = include_str!("../../tests/fixtures/seerr-3.2.0/radarr-test.json");
    const SONARR_TEST: &str = include_str!("../../tests/fixtures/seerr-3.2.0/sonarr-test.json");
    const WEBHOOK_JSON: &str = include_str!("../../tests/fixtures/seerr-3.2.0/webhook.json");

    /// A value nobody would type, so a leak is easy to search for.
    const KEY: &str = "arr-key-7f3a9c-never-print-me";
    const MARK: &str = "webhook-mark-7f3a9c-never-print-me";

    fn map(value: Value) -> BTreeMap<String, Value> {
        value.as_object().unwrap().clone().into_iter().collect()
    }

    fn seerr() -> FakeTransport {
        FakeTransport::default()
            .on_get(STATUS.path, vec![ok(STATUS_JSON)])
            .on_get(MAIN, vec![ok(MAIN_JSON)])
    }

    fn sent(t: &FakeTransport, index: usize) -> (String, Value) {
        let written = t.written.borrow();
        (
            written[index].0.clone(),
            serde_json::from_str(&written[index].1).unwrap(),
        )
    }

    #[test]
    fn probe_needs_a_version_and_a_key_that_acts_as_user_1() {
        assert_eq!(probe(&seerr()).ok().unwrap(), "3.2.0");
        let t = FakeTransport::default()
            .on_get(STATUS.path, vec![ok(STATUS_JSON)])
            .on_get(MAIN, vec![Step::Answer(403, "{}".into())]);
        match probe(&t) {
            Err(Probe::Fatal(e)) => assert!(e.to_string().contains("user 1"), "{e}"),
            _ => panic!("403 is fatal"),
        }
        let t =
            FakeTransport::default().on_get(STATUS.path, vec![Step::Answer(503, String::new())]);
        assert!(matches!(probe(&t), Err(Probe::NotYet(_))));
    }

    // --- main

    fn hosts_main() -> Main {
        Main {
            set: map(json!({"applicationTitle": "Wunschliste", "locale": "de",
                "discoverRegion": "DE", "streamingRegion": "DE", "mediaServerType": 2,
                "localLogin": false, "mediaServerLogin": true, "newPlexLogin": true,
                "defaultPermissions": 8992, "cacheImages": false,
                "partialRequestsEnabled": true, "enableSpecialEpisodes": false})),
        }
    }

    #[test]
    fn main_recorded_state_is_already_desired_and_only_the_spec_travels() {
        let task = hosts_main();
        let t = seerr();
        assert_eq!(task.diff(&task.read(&t).unwrap()).unwrap(), vec![]);

        let mut task = hosts_main();
        task.set.insert("cacheImages".to_string(), json!(true));
        let t = seerr().on_put(vec![Step::Answer(200, "{}".into())]);
        let current = task.read(&t).unwrap();
        assert_eq!(
            task.diff(&current).unwrap()[0].to_string(),
            "main: cacheImages false -> true"
        );
        task.write(&t, &current).unwrap();
        let (path, body) = sent(&t, 0);
        assert_eq!(path, MAIN);
        assert_eq!(body["cacheImages"], json!(true));
        assert!(
            body.get("apiKey").is_none(),
            "the merge never carries the key"
        );
    }

    #[test]
    fn a_main_field_the_answer_lacks_is_an_error() {
        let task = Main {
            set: map(json!({"locale": "de", "lokale": "de"})),
        };
        let err = task.diff(&task.read(&seerr()).unwrap()).err().unwrap();
        assert_eq!(
            err.to_string(),
            "the answer has no such field: main: lokale"
        );
    }

    // --- jellyfin

    const FILME: &str = "7a2175bccb1f1a94152cbd2b2bae8f6d";
    const MEDIATHEK_FILME: &str = "7ee571d485deaa484dcede65597cd699";
    const MEDIATHEK_SERIEN: &str = "c70af6a113b51f9aab8594b81d74f1b7";
    const SERIEN: &str = "43cfe12fe7d9d8d21251e0964e0232e2";

    fn hosts_jellyfin(libraries: &[&str]) -> Jellyfin {
        Jellyfin {
            set: map(
                json!({"ip": "<masked>", "port": 8096, "useSsl": false, "urlBase": "",
                "externalHostname": "<masked>"}),
            ),
            api_key: Secret::new("<masked>".to_string()),
            libraries: libraries.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn with_jellyfin(t: FakeTransport) -> FakeTransport {
        t.on_get(JELLYFIN, vec![ok(JELLYFIN_JSON)])
    }

    #[test]
    fn jellyfin_recorded_state_is_already_desired() {
        let task = hosts_jellyfin(&["Filme", "Serien", "Mediathek Filme", "Mediathek Serien"]);
        let t = with_jellyfin(seerr());
        assert_eq!(task.diff(&task.read(&t).unwrap()).unwrap(), vec![]);
    }

    #[test]
    fn a_library_not_named_is_disabled_through_the_enable_query() {
        let task = hosts_jellyfin(&["Filme", "Serien", "Mediathek Filme"]);
        let enable = library_query(
            false,
            &[FILME.into(), SERIEN.into(), MEDIATHEK_FILME.into()],
        );
        let t = with_jellyfin(seerr()).on_get(&enable, vec![ok("[]")]);
        let current = task.read(&t).unwrap();
        assert_eq!(
            task.diff(&current).unwrap()[0].to_string(),
            "library Mediathek Serien: enabled true -> false"
        );
        // Only the enable call: the connection matched, so no POST.
        task.write(&t, &current).unwrap();
        assert!(t.written.borrow().is_empty());
        assert!(
            enable.contains(&format!("?enable={FILME},{SERIEN},{MEDIATHEK_FILME}")),
            "{enable}"
        );
        assert!(
            !enable.contains(MEDIATHEK_SERIEN),
            "not named, so not enabled"
        );
    }

    #[test]
    fn an_unknown_library_makes_seerr_sync_first_then_enables_all_named() {
        let task = hosts_jellyfin(&["Filme", "Serien", "Musik"]);
        let known: Vec<String> = vec![FILME.into(), SERIEN.into()];
        let synced = format!(
            r#"[{{"id":"{FILME}","name":"Filme","type":"movie","enabled":true}},
                {{"id":"{SERIEN}","name":"Serien","type":"show","enabled":true}},
                {{"id":"8a05b0252259a1dbd62df97522638439","name":"Musik","type":"music","enabled":false}}]"#
        );
        let all: Vec<String> = vec![
            FILME.into(),
            SERIEN.into(),
            "8a05b0252259a1dbd62df97522638439".into(),
        ];
        let t = with_jellyfin(seerr())
            .on_get(&library_query(true, &known), vec![ok(&synced)])
            .on_get(&library_query(false, &all), vec![ok("[]")]);
        let current = task.read(&t).unwrap();
        let shown: Vec<String> = task
            .diff(&current)
            .unwrap()
            .iter()
            .map(|c| c.to_string())
            .collect();
        assert!(
            shown.contains(
                &"library Musik: (unknown to Seerr) -> (synced from Jellyfin, enabled)".to_string()
            ),
            "{shown:?}"
        );
        task.write(&t, &current).unwrap();

        // Still unknown after the sync: an error that names the library.
        let without_musik = r#"[{"id":"x","name":"Filme","enabled":true},{"id":"y","name":"Serien","enabled":true}]"#;
        let t =
            with_jellyfin(seerr()).on_get(&library_query(true, &known), vec![ok(without_musik)]);
        let err = task.write(&t, &current).err().unwrap().to_string();
        assert!(err.contains(r#"library "Musik""#), "{err}");
    }

    #[test]
    fn the_library_query_always_carries_enable() {
        // Without `enable=` Seerr disables every library.
        assert_eq!(library_query(false, &[]), format!("{LIBRARIES}?enable="));
        assert_eq!(
            library_query(true, &["a".into(), "b".into()]),
            format!("{LIBRARIES}?sync=true&enable=a,b")
        );
    }

    #[test]
    fn a_changed_jellyfin_key_is_posted_with_the_connection_and_never_shown() {
        let mut task = hosts_jellyfin(&["Filme", "Serien", "Mediathek Filme", "Mediathek Serien"]);
        task.api_key = Secret::new(KEY.to_string());
        let t = with_jellyfin(seerr()).on_put(vec![Step::Answer(200, "{}".into())]);
        let current = task.read(&t).unwrap();
        let changes = task.diff(&current).unwrap();
        assert_eq!(
            changes[0].to_string(),
            "jellyfin: apiKey (hidden) -> (hidden) from its credential"
        );
        assert!(!format!("{changes:?}").contains(KEY));
        task.write(&t, &current).unwrap();
        let (path, body) = sent(&t, 0);
        assert_eq!(path, JELLYFIN);
        assert_eq!(body["apiKey"], KEY);
        assert_eq!(body["port"], 8096);
        assert!(body.get("libraries").is_none() && body.get("serverId").is_none());
    }

    // --- servers

    fn hosts_servers(kind: SeerrKind) -> Servers {
        let (name, port, folder) = match kind {
            SeerrKind::Radarr => ("Radarr", 7878, "/tank/data/media/movies"),
            SeerrKind::Sonarr => ("Sonarr", 8989, "/tank/data/media/series"),
        };
        let mut set = map(
            json!({"hostname": "<masked>", "port": port, "useSsl": false,
            "baseUrl": "", "is4k": false, "isDefault": true, "syncEnabled": true,
            "preventSearch": false, "externalUrl": ""}),
        );
        if kind == SeerrKind::Radarr {
            set.insert("minimumAvailability".into(), json!("released"));
        } else {
            set.insert("enableSeasonFolders".into(), json!(true));
            set.insert("activeLanguageProfileId".into(), json!(1));
        }
        Servers {
            kind,
            servers: vec![ServerTarget {
                name: name.to_string(),
                set,
                api_key: Secret::new("<masked>".to_string()),
                profile: "Dual Language, sonst Deutsch (1080p)".to_string(),
                root_folder: folder.to_string(),
            }],
        }
    }

    fn with_servers(t: FakeTransport, kind: SeerrKind, list: &str, test: &str) -> FakeTransport {
        t.on_get(&format!("/api/v1/settings/{}", kind.path()), vec![ok(list)])
            .on_put(vec![ok(test)])
    }

    #[test]
    fn both_recorded_servers_match_and_the_test_is_the_only_request_that_writes() {
        for (kind, list, test) in [
            (SeerrKind::Radarr, RADARR_JSON, RADARR_TEST),
            (SeerrKind::Sonarr, SONARR_JSON, SONARR_TEST),
        ] {
            let task = hosts_servers(kind);
            let t = with_servers(seerr(), kind, list, test);
            let current = task.read(&t).unwrap();
            assert_eq!(task.diff(&current).unwrap(), vec![], "{}", kind.path());
            assert_eq!(
                current.profile_ids.values().copied().collect::<Vec<_>>(),
                [7]
            );
            let (path, body) = sent(&t, 0);
            assert_eq!(path, format!("/api/v1/settings/{}/test", kind.path()));
            assert_eq!(
                body["port"],
                json!(if kind == SeerrKind::Radarr {
                    7878
                } else {
                    8989
                })
            );
            assert_eq!(t.written.borrow().len(), 1, "the test, nothing else");
        }
    }

    #[test]
    fn a_renamed_profile_is_one_change_and_the_update_carries_the_whole_entry() {
        let mut task = hosts_servers(SeerrKind::Radarr);
        task.servers[0].profile = "Rarität, Originalsprache (auch SD)".to_string();
        let t = with_servers(seerr(), SeerrKind::Radarr, RADARR_JSON, RADARR_TEST)
            .on_put(vec![ok(RADARR_TEST), Step::Answer(200, "{}".into())]);
        let current = task.read(&t).unwrap();
        let mut shown: Vec<String> = task
            .diff(&current)
            .unwrap()
            .iter()
            .map(|c| c.to_string())
            .collect();
        shown.sort();
        assert_eq!(
            shown,
            [
                r#"Radarr server Radarr: activeProfileId 7 -> 11"#,
                r#"Radarr server Radarr: activeProfileName "Dual Language, sonst Deutsch (1080p)" -> "Rarität, Originalsprache (auch SD)""#
            ]
        );
        task.write(&t, &current).unwrap();
        let (path, body) = sent(&t, 1);
        assert_eq!(path, "/api/v1/settings/radarr/0");
        assert_eq!(body["activeProfileId"], 11);
        assert_eq!(body["id"], 0);
        assert_eq!(body["name"], "Radarr");
        assert_eq!(body["minimumAvailability"], "released");
        assert_eq!(
            body["apiKey"], "<masked>",
            "the key travels with the whole entry"
        );
    }

    #[test]
    fn a_missing_server_is_added_and_a_changed_key_is_hidden() {
        let task = hosts_servers(SeerrKind::Sonarr);
        let t = with_servers(seerr(), SeerrKind::Sonarr, "[]", SONARR_TEST)
            .on_put(vec![ok(SONARR_TEST), Step::Answer(201, "{}".into())]);
        let current = task.read(&t).unwrap();
        assert_eq!(
            task.diff(&current).unwrap()[0].to_string(),
            "Sonarr server Sonarr: (missing) -> (added)"
        );
        task.write(&t, &current).unwrap();
        let (path, body) = sent(&t, 1);
        assert_eq!(path, "/api/v1/settings/sonarr");
        assert_eq!(body["name"], "Sonarr");
        assert_eq!(body["activeProfileId"], 7);
        assert_eq!(body["activeDirectory"], "/tank/data/media/series");
        assert!(body.get("id").is_none(), "Seerr assigns the id");

        let mut task = hosts_servers(SeerrKind::Sonarr);
        task.servers[0].api_key = Secret::new(KEY.to_string());
        let t = with_servers(seerr(), SeerrKind::Sonarr, SONARR_JSON, SONARR_TEST);
        let changes = task.diff(&task.read(&t).unwrap()).unwrap();
        assert_eq!(
            changes[0].to_string(),
            "Sonarr server Sonarr: apiKey (hidden) -> (hidden) from its credential"
        );
        assert!(!format!("{changes:?}").contains(KEY));
    }

    #[test]
    fn an_unknown_profile_or_root_folder_is_not_found_before_anything_is_compared() {
        let mut task = hosts_servers(SeerrKind::Radarr);
        task.servers[0].profile = "Nope".to_string();
        let t = with_servers(seerr(), SeerrKind::Radarr, RADARR_JSON, RADARR_TEST);
        let err = task.read(&t).err().unwrap().to_string();
        assert!(
            err.contains(r#"profile "Nope" of Radarr server Radarr (it has: Dual Language, sonst Deutsch (1080p), "#),
            "{err}"
        );
        let mut task = hosts_servers(SeerrKind::Radarr);
        task.servers[0].root_folder = "/nope".to_string();
        let t = with_servers(seerr(), SeerrKind::Radarr, RADARR_JSON, RADARR_TEST);
        let err = task.read(&t).err().unwrap().to_string();
        assert!(
            err.contains(
                r#"root folder "/nope" of Radarr server Radarr (it has: /tank/data/media/movies)"#
            ),
            "{err}"
        );
    }

    // --- webhook

    fn template() -> Map<String, Value> {
        json!({"notification_type": "{{notification_type}}", "subject": "{{subject}}",
               "{{request}}": {"request_id": "{{request_id}}"}})
        .as_object()
        .unwrap()
        .clone()
    }

    fn hosts_webhook(mark: &str) -> Webhook {
        Webhook {
            set: map(
                json!({"enabled": true, "types": 24, "options.webhookUrl": "<masked>",
                "options.authHeader": ""}),
            ),
            payload: template(),
            headers: [("X-Webhook-Token".to_string(), Secret::new(mark.to_string()))]
                .into_iter()
                .collect(),
        }
    }

    fn with_webhook(t: FakeTransport, body: &str) -> FakeTransport {
        t.on_get(WEBHOOK, vec![ok(body)])
    }

    #[test]
    fn webhook_recorded_state_is_already_desired() {
        let task = hosts_webhook("<masked>");
        let t = with_webhook(seerr(), WEBHOOK_JSON);
        assert_eq!(task.diff(&task.read(&t).unwrap()).unwrap(), vec![]);
    }

    #[test]
    fn the_payload_is_posted_encoded_for_the_agent_and_the_mark_never_shows() {
        let task = hosts_webhook(MARK);
        let t = with_webhook(seerr(), WEBHOOK_JSON).on_put(vec![Step::Answer(200, "{}".into())]);
        let current = task.read(&t).unwrap();
        let changes = task.diff(&current).unwrap();
        assert_eq!(
            changes[0].to_string(),
            "webhook: customHeaders[key=X-Webhook-Token]: value (hidden) -> (hidden) from its credential"
        );
        assert!(!format!("{changes:?}").contains(MARK));
        task.write(&t, &current).unwrap();
        let (path, body) = sent(&t, 0);
        assert_eq!(path, WEBHOOK);
        // What the route stores is base64 of this string; the agent parses
        // the decoded text twice and must arrive at the template.
        let stored = body["options"]["jsonPayload"].as_str().unwrap();
        let once: String = serde_json::from_str(stored).unwrap();
        let twice: Value = serde_json::from_str(&once).unwrap();
        assert_eq!(twice, Value::Object(template()));
        assert_eq!(
            body["options"]["customHeaders"],
            json!([{"key": "X-Webhook-Token", "value": MARK}])
        );
        assert_eq!(body["types"], 24);
        assert_eq!(body["embedPoster"], true, "kept from the answer");
    }

    #[test]
    fn a_payload_the_web_ui_stored_is_a_change_because_the_agent_fails_on_it() {
        let mut current: Value = serde_json::from_str(WEBHOOK_JSON).unwrap();
        current["options"]["jsonPayload"] = Value::Object(template());
        let task = hosts_webhook("<masked>");
        let t = with_webhook(seerr(), &current.to_string());
        let changes = task.diff(&task.read(&t).unwrap()).unwrap();
        assert_eq!(
            changes[0].to_string(),
            "webhook: options.jsonPayload (stored once-encoded; the agent fails on it) -> (the template, encoded for the agent)"
        );

        let mut task = hosts_webhook("<masked>");
        task.payload.insert("extra".to_string(), json!(1));
        let t = with_webhook(seerr(), WEBHOOK_JSON);
        let changes = task.diff(&task.read(&t).unwrap()).unwrap();
        assert!(
            changes[0]
                .to_string()
                .starts_with("webhook: options.jsonPayload {"),
            "{}",
            changes[0]
        );
    }
}
