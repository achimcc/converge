//! Servarr providers: download clients, notifications and Prowlarr's
//! applications (design §12). Each is a resource with top-level fields and a
//! `fields` list of `{name, value, privacy, …}` entries; the entries depend on
//! the implementation, so the OpenAPI description cannot name them.
//!
//! Hidden values. Servarr answers every field with `privacy` `password` or
//! `apiKey` as `********`, and keeps the stored value when `********` comes
//! back (`SchemaBuilder.ReadFromSchema`). A stale key cannot be seen by
//! reading, so secret fields are handed over on every `apply`: Radarr, Sonarr
//! and Lidarr compare the definition themselves and write only when it
//! changed; Prowlarr writes on every update. Every write sends
//! `forceSave=true`, so no connection test runs -- a test against a client
//! with a stale password is a failed login, and qBittorrent bans the address
//! after a few (the host this was written for, 2026-09-12).

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::{
    client::{expect_status_at, Transport},
    endpoint::{Endpoint, Shape},
    engine::{shortened, Change, Probe, Task},
    error::Error,
    secret::Secret,
    services::servarr,
    spec::Service,
};

/// Which kind of provider a task reconciles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    DownloadClients,
    Notifications,
    Applications,
}

/// The endpoints of one kind of provider behind one API version.
pub struct ProviderApi {
    /// How a provider of this kind is named in output: `download client`.
    pub subject: &'static str,
    pub component: &'static str,
    pub status: Endpoint,
    pub list: Endpoint,
    pub schema: Endpoint,
    pub create: Endpoint,
    pub update: Endpoint,
}

macro_rules! provider_api {
    ($v:literal, $resource:literal, $component:literal, $subject:literal) => {
        ProviderApi {
            subject: $subject,
            component: $component,
            status: Endpoint {
                method: "GET",
                path: concat!("/api/", $v, "/system/status"),
                request: None,
                response: Some(Shape::One("SystemResource")),
            },
            list: Endpoint {
                method: "GET",
                path: concat!("/api/", $v, "/", $resource),
                request: None,
                response: Some(Shape::Documents($component)),
            },
            schema: Endpoint {
                method: "GET",
                path: concat!("/api/", $v, "/", $resource, "/schema"),
                request: None,
                response: Some(Shape::Documents($component)),
            },
            create: Endpoint {
                method: "POST",
                path: concat!("/api/", $v, "/", $resource),
                request: Some(Shape::Document($component)),
                response: None,
            },
            update: Endpoint {
                method: "PUT",
                path: concat!("/api/", $v, "/", $resource, "/{id}"),
                request: Some(Shape::Document($component)),
                response: None,
            },
        }
    };
}

pub static DOWNLOAD_CLIENTS_V3: ProviderApi = provider_api!(
    "v3",
    "downloadclient",
    "DownloadClientResource",
    "download client"
);
pub static DOWNLOAD_CLIENTS_V1: ProviderApi = provider_api!(
    "v1",
    "downloadclient",
    "DownloadClientResource",
    "download client"
);
pub static NOTIFICATIONS_V3: ProviderApi =
    provider_api!("v3", "notification", "NotificationResource", "notification");
pub static NOTIFICATIONS_V1: ProviderApi =
    provider_api!("v1", "notification", "NotificationResource", "notification");
pub static APPLICATIONS_V1: ProviderApi =
    provider_api!("v1", "applications", "ApplicationResource", "application");

impl ProviderApi {
    pub fn of(service: Service, kind: Kind) -> Option<&'static ProviderApi> {
        let v3 = matches!(service, Service::Radarr | Service::Sonarr);
        let v1 = matches!(service, Service::Lidarr | Service::Prowlarr);
        match kind {
            Kind::DownloadClients if v3 => Some(&DOWNLOAD_CLIENTS_V3),
            Kind::DownloadClients if v1 => Some(&DOWNLOAD_CLIENTS_V1),
            Kind::Notifications if v3 => Some(&NOTIFICATIONS_V3),
            Kind::Notifications if v1 => Some(&NOTIFICATIONS_V1),
            Kind::Applications if service == Service::Prowlarr => Some(&APPLICATIONS_V1),
            _ => None,
        }
    }

    pub fn endpoints(&self) -> [Endpoint; 4] {
        [self.list, self.schema, self.create, self.update]
    }

    /// Every provider endpoint a service's tasks use, without its status
    /// endpoint.
    pub fn endpoints_of(service: Service) -> Vec<Endpoint> {
        [
            Kind::DownloadClients,
            Kind::Notifications,
            Kind::Applications,
        ]
        .into_iter()
        .filter_map(|kind| Self::of(service, kind))
        .flat_map(|api| api.endpoints())
        .collect()
    }
}

/// The `privacy` values Servarr answers with `********`.
const HIDDEN_PRIVACY: [&str; 2] = ["password", "apiKey"];

/// One provider the spec declares, its hidden values read from credentials.
pub struct ProviderTarget {
    pub name: String,
    pub implementation: String,
    pub set: BTreeMap<String, Value>,
    pub fields: BTreeMap<String, Value>,
    pub secret_fields: BTreeMap<String, Secret>,
}

pub struct Providers {
    pub api: &'static ProviderApi,
    pub providers: Vec<ProviderTarget>,
}

pub struct Current {
    pub entries: Vec<Map<String, Value>>,
    /// Read only when a provider is missing: the template to add it from.
    pub templates: Vec<Map<String, Value>>,
}

fn name_of(entry: &Map<String, Value>) -> Option<&str> {
    entry.get("name").and_then(Value::as_str)
}

fn field<'a>(entry: &'a Map<String, Value>, name: &str) -> Option<&'a Map<String, Value>> {
    entry
        .get("fields")?
        .as_array()?
        .iter()
        .filter_map(Value::as_object)
        .find(|f| f.get("name").and_then(Value::as_str) == Some(name))
}

/// A field without a `value` key holds null: Servarr omits null values.
fn value_of(field: &Map<String, Value>) -> Value {
    field.get("value").cloned().unwrap_or(Value::Null)
}

/// Sets `value` on the field called `name`; false if there is none.
fn set_field(entry: &mut Map<String, Value>, name: &str, value: Value) -> bool {
    let Some(list) = entry.get_mut("fields").and_then(Value::as_array_mut) else {
        return false;
    };
    match list
        .iter_mut()
        .filter_map(Value::as_object_mut)
        .find(|f| f.get("name").and_then(Value::as_str) == Some(name))
    {
        Some(f) => {
            f.insert("value".to_string(), value);
            true
        }
        None => false,
    }
}

impl Providers {
    fn subject(&self, name: &str) -> String {
        format!("{} {name}", self.api.subject)
    }

    fn find<'a>(current: &'a [Map<String, Value>], name: &str) -> Option<&'a Map<String, Value>> {
        current.iter().find(|e| name_of(e) == Some(name))
    }

    fn template<'a>(
        &self,
        current: &'a Current,
        implementation: &str,
    ) -> Option<&'a Map<String, Value>> {
        current
            .templates
            .iter()
            .find(|t| t.get("implementation").and_then(Value::as_str) == Some(implementation))
    }

    /// Checks that every name the spec uses exists in `entry` (a provider or
    /// a template) and that every secret field is one the service hides.
    fn check_names(
        &self,
        target: &ProviderTarget,
        entry: &Map<String, Value>,
        missing: &mut Vec<String>,
        mismatch: &mut Vec<String>,
    ) {
        let subject = self.subject(&target.name);
        for key in target.set.keys() {
            if !entry.contains_key(key) {
                missing.push(format!("{subject}: {key}"));
            }
        }
        for name in target.fields.keys() {
            if field(entry, name).is_none() {
                missing.push(format!("{subject}: fields.{name}"));
            }
        }
        for name in target.secret_fields.keys() {
            match field(entry, name) {
                None => missing.push(format!("{subject}: fields.{name}")),
                Some(f) => {
                    let privacy = f.get("privacy").and_then(Value::as_str).unwrap_or("normal");
                    if !HIDDEN_PRIVACY.contains(&privacy) {
                        mismatch.push(format!(
                            "{subject}: fields.{name} has privacy {privacy}, so the service shows it -- name it in fields, where it is compared"
                        ));
                    }
                }
            }
        }
    }

    /// Visible differences of an existing provider. Secret fields are never
    /// compared: the service answers them with `********`.
    fn changes_of(&self, target: &ProviderTarget, entry: &Map<String, Value>) -> Vec<Change> {
        let subject = self.subject(&target.name);
        let mut changes = Vec::new();
        for (key, desired) in &target.set {
            if let Some(current) = entry.get(key) {
                if current != desired {
                    changes.push(Change {
                        subject: subject.clone(),
                        field: key.clone(),
                        current: shortened(current),
                        desired: shortened(desired),
                    });
                }
            }
        }
        for (name, desired) in &target.fields {
            if let Some(f) = field(entry, name) {
                let current = value_of(f);
                if &current != desired {
                    changes.push(Change {
                        subject: subject.clone(),
                        field: format!("fields.{name}"),
                        current: shortened(&current),
                        desired: shortened(desired),
                    });
                }
            }
        }
        changes
    }

    /// `base` with the spec's values, hidden ones included.
    fn filled(
        &self,
        target: &ProviderTarget,
        base: &Map<String, Value>,
    ) -> Result<Map<String, Value>, Error> {
        let mut body = base.clone();
        for (key, value) in &target.set {
            body.insert(key.clone(), value.clone());
        }
        let mut missing = Vec::new();
        for (name, value) in &target.fields {
            if !set_field(&mut body, name, value.clone()) {
                missing.push(format!("{}: fields.{name}", self.subject(&target.name)));
            }
        }
        for (name, secret) in &target.secret_fields {
            if !set_field(&mut body, name, Value::from(secret.expose())) {
                missing.push(format!("{}: fields.{name}", self.subject(&target.name)));
            }
        }
        if missing.is_empty() {
            Ok(body)
        } else {
            Err(Error::MissingField(missing))
        }
    }

    fn send(
        &self,
        t: &dyn Transport,
        ep: Endpoint,
        path: &str,
        body: &Map<String, Value>,
    ) -> Result<(), Error> {
        // The body may hold a secret: a serializing error names the path only.
        let text = serde_json::to_string(body).map_err(|_| Error::Request {
            method: ep.method,
            path: path.to_string(),
            reason: "cannot serialize the body".to_string(),
        })?;
        let full = format!("{path}?forceSave=true");
        let reply = if ep.method == "POST" {
            t.post_json(&full, &text)?
        } else {
            t.put_json(&full, &text)?
        };
        // 201 Created for a new provider, 202 Accepted for an update.
        expect_status_at(ep.method, path, &reply, &[200, 201, 202])
    }

    fn id_of(&self, entry: &Map<String, Value>) -> Result<i64, Error> {
        entry
            .get("id")
            .and_then(Value::as_i64)
            .ok_or_else(|| Error::Decode {
                path: self.api.list.path.to_string(),
                reason: "a provider has no integer id".to_string(),
            })
    }
}

impl Task for Providers {
    type Current = Current;

    fn probe(&self, t: &dyn Transport) -> Result<String, Probe> {
        servarr::probe_status(t, self.api.status)
    }

    /// An empty list is a valid answer: a fresh service has no provider, and
    /// every provider the spec names then shows up as a change.
    fn read(&self, t: &dyn Transport) -> Result<Self::Current, Error> {
        let read_list = |ep: Endpoint| -> Result<Vec<Map<String, Value>>, Error> {
            let reply = t.get(ep.path)?;
            expect_status_at(ep.method, ep.path, &reply, &[200])?;
            serde_json::from_str(&reply.body).map_err(|e| Error::Decode {
                path: ep.path.to_string(),
                reason: e.to_string(),
            })
        };
        let entries = read_list(self.api.list)?;
        if let Some(index) = entries.iter().position(|e| name_of(e).is_none()) {
            return Err(Error::MissingName {
                path: self.api.list.path.to_string(),
                index,
            });
        }
        let lacking = self
            .providers
            .iter()
            .any(|p| Self::find(&entries, &p.name).is_none());
        let templates = if lacking {
            let templates = read_list(self.api.schema)?;
            if templates.is_empty() {
                return Err(Error::EmptyList {
                    path: self.api.schema.path.to_string(),
                });
            }
            templates
        } else {
            Vec::new()
        };
        Ok(Current { entries, templates })
    }

    fn diff(&self, current: &Self::Current) -> Result<Vec<Change>, Error> {
        let (mut missing, mut mismatch, mut changes) = (Vec::new(), Vec::new(), Vec::new());
        for target in &self.providers {
            let subject = self.subject(&target.name);
            match Self::find(&current.entries, &target.name) {
                Some(entry) => {
                    let implementation = entry.get("implementation").and_then(Value::as_str);
                    if implementation != Some(target.implementation.as_str()) {
                        mismatch.push(format!(
                            "{subject} is {} on the service, the spec says {} -- converge does not change an implementation",
                            implementation.unwrap_or("(none)"),
                            target.implementation
                        ));
                        continue;
                    }
                    self.check_names(target, entry, &mut missing, &mut mismatch);
                    changes.extend(self.changes_of(target, entry));
                }
                None => {
                    match self.template(current, &target.implementation) {
                        Some(template) => {
                            self.check_names(target, template, &mut missing, &mut mismatch)
                        }
                        None => mismatch.push(format!(
                            "{subject}: the service has no implementation {} to add it from",
                            target.implementation
                        )),
                    }
                    changes.push(Change {
                        subject,
                        field: String::new(),
                        current: "(missing)".to_string(),
                        desired: "(added)".to_string(),
                    });
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

    fn notes(&self, current: &Self::Current) -> Vec<String> {
        current
            .entries
            .iter()
            .filter_map(name_of)
            .filter(|name| !self.providers.iter().any(|p| p.name == *name))
            .map(|name| format!("not in the spec: {}", self.subject(name)))
            .collect()
    }

    fn write(&self, t: &dyn Transport, current: &Self::Current) -> Result<(), Error> {
        for target in &self.providers {
            match Self::find(&current.entries, &target.name) {
                None => {
                    let Some(template) = self.template(current, &target.implementation) else {
                        return Err(Error::Mismatch(vec![format!(
                            "{}: the service has no implementation {} to add it from",
                            self.subject(&target.name),
                            target.implementation
                        )]));
                    };
                    let mut body = self.filled(target, template)?;
                    body.insert("name".to_string(), Value::from(target.name.clone()));
                    body.remove("id");
                    self.send(t, self.api.create, self.api.create.path, &body)?;
                }
                Some(entry) => {
                    if self.changes_of(target, entry).is_empty() {
                        continue;
                    }
                    let body = self.filled(target, entry)?;
                    let path = self
                        .api
                        .update
                        .path
                        .replace("{id}", &self.id_of(entry)?.to_string());
                    self.send(t, self.api.update, &path, &body)?;
                }
            }
        }
        Ok(())
    }

    fn hand_over(&self, t: &dyn Transport, current: &Self::Current) -> Result<Vec<String>, Error> {
        let mut lines = Vec::new();
        for target in self
            .providers
            .iter()
            .filter(|p| !p.secret_fields.is_empty())
        {
            let Some(entry) = Self::find(&current.entries, &target.name) else {
                return Err(Error::NotFound(vec![self.subject(&target.name)]));
            };
            let body = self.filled(target, entry)?;
            let path = self
                .api
                .update
                .path
                .replace("{id}", &self.id_of(entry)?.to_string());
            self.send(t, self.api.update, &path, &body)?;
            let names: Vec<&str> = target.secret_fields.keys().map(String::as_str).collect();
            lines.push(format!(
                "{}: {} handed over from credentials (hidden; the service compares)",
                self.subject(&target.name),
                names.join(", ")
            ));
        }
        Ok(lines)
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

    const RADARR_STATUS: &str =
        include_str!("../../tests/fixtures/radarr-6.3.0.10514/system-status.json");
    const RADARR_CLIENTS: &str =
        include_str!("../../tests/fixtures/radarr-6.3.0.10514/downloadclient.json");
    const RADARR_CLIENT_SCHEMA: &str =
        include_str!("../../tests/fixtures/radarr-6.3.0.10514/downloadclient-schema.json");
    const LIDARR_NOTIFICATIONS: &str =
        include_str!("../../tests/fixtures/lidarr-3.1.0.4875/notification.json");
    const PROWLARR_STATUS: &str =
        include_str!("../../tests/fixtures/prowlarr-2.5.2.5491/system-status.json");
    const PROWLARR_APPS: &str =
        include_str!("../../tests/fixtures/prowlarr-2.5.2.5491/applications.json");

    /// A password nobody would type, so a leak is easy to search for.
    const PASSWORD: &str = "pw-7f3a9c-never-print-me";

    fn map(value: Value) -> BTreeMap<String, Value> {
        value.as_object().unwrap().clone().into_iter().collect()
    }

    fn qbittorrent(port: i64) -> ProviderTarget {
        ProviderTarget {
            name: "qBittorrent".to_string(),
            implementation: "QBittorrent".to_string(),
            set: map(json!({"enable": true, "priority": 1})),
            // The recording masks the user name on the host; the spec uses it.
            fields: map(json!({"host": "10.0.10.11", "port": port,
                               "username": "<masked>", "movieCategory": "radarr"})),
            secret_fields: [("password".to_string(), Secret::new(PASSWORD.to_string()))]
                .into_iter()
                .collect(),
        }
    }

    fn sabnzbd() -> ProviderTarget {
        ProviderTarget {
            name: "SABnzbd".to_string(),
            implementation: "Sabnzbd".to_string(),
            set: map(json!({"enable": true, "priority": 1})),
            fields: map(json!({"host": "10.0.10.10", "port": 8080, "movieCategory": "radarr"})),
            secret_fields: [(
                "apiKey".to_string(),
                Secret::new("sab-key-never-print".to_string()),
            )]
            .into_iter()
            .collect(),
        }
    }

    fn radarr_clients(targets: Vec<ProviderTarget>) -> Providers {
        Providers {
            api: &DOWNLOAD_CLIENTS_V3,
            providers: targets,
        }
    }

    fn radarr(lists: Vec<Step>) -> FakeTransport {
        FakeTransport::default()
            .on_get("/api/v3/system/status", vec![ok(RADARR_STATUS)])
            .on_get("/api/v3/downloadclient", lists)
            .on_get(
                "/api/v3/downloadclient/schema",
                vec![ok(RADARR_CLIENT_SCHEMA)],
            )
    }

    fn sent(t: &FakeTransport, index: usize) -> (String, Value) {
        let written = t.written.borrow();
        (
            written[index].0.clone(),
            serde_json::from_str(&written[index].1).unwrap(),
        )
    }

    fn field_value(body: &Value, name: &str) -> Value {
        body["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["name"] == name)
            .map(|f| f.get("value").cloned().unwrap_or(Value::Null))
            .unwrap()
    }

    #[test]
    fn the_paths_and_kinds_per_service() {
        assert_eq!(
            DOWNLOAD_CLIENTS_V3.update.path,
            "/api/v3/downloadclient/{id}"
        );
        assert_eq!(APPLICATIONS_V1.schema.path, "/api/v1/applications/schema");
        let of = |s, k| ProviderApi::of(s, k).map(|a| a.list.path);
        assert_eq!(
            of(Service::Lidarr, Kind::Notifications),
            Some("/api/v1/notification")
        );
        assert_eq!(
            of(Service::Prowlarr, Kind::Applications),
            Some("/api/v1/applications")
        );
        assert_eq!(of(Service::Radarr, Kind::Applications), None);
        assert_eq!(of(Service::Jellyfin, Kind::DownloadClients), None);
        assert_eq!(ProviderApi::endpoints_of(Service::Sonarr).len(), 8);
        assert_eq!(ProviderApi::endpoints_of(Service::Prowlarr).len(), 12);
    }

    #[test]
    fn the_hosts_clients_match_and_every_run_hands_the_secrets_over() {
        let task = radarr_clients(vec![qbittorrent(8080), sabnzbd()]);
        let t = radarr(vec![ok(RADARR_CLIENTS)]).on_put(vec![Step::Answer(202, String::new())]);
        let report = run(Mode::Apply, &task, &t, &FakeClock::new(), Timing::default()).unwrap();
        assert_eq!(report.outcome, Outcome::Unchanged);
        assert_eq!(
            report.handed_over,
            [
                "download client qBittorrent: password handed over from credentials (hidden; the service compares)",
                "download client SABnzbd: apiKey handed over from credentials (hidden; the service compares)"
            ]
        );
        // One PUT per provider with a secret, the recorded entry with the
        // secret filled in and forceSave, so no connection test runs.
        assert_eq!(t.written.borrow().len(), 2);
        let (path, body) = sent(&t, 0);
        assert_eq!(path, "/api/v3/downloadclient/1?forceSave=true");
        assert_eq!(field_value(&body, "password"), PASSWORD);
        assert_eq!(field_value(&body, "movieCategory"), "radarr");
        assert_eq!(body["implementation"], "QBittorrent");
        let recorded: Value = serde_json::from_str(RADARR_CLIENTS).unwrap();
        assert_eq!(
            body["fields"].as_array().unwrap().len(),
            recorded[0]["fields"].as_array().unwrap().len()
        );
        let (path, body) = sent(&t, 1);
        assert_eq!(path, "/api/v3/downloadclient/2?forceSave=true");
        assert_eq!(field_value(&body, "apiKey"), "sab-key-never-print");
        // Nothing the service sees as a template was read: nothing was missing.
    }

    #[test]
    fn plan_neither_writes_nor_hands_over_and_says_nothing_about_the_secret() {
        let task = radarr_clients(vec![qbittorrent(8080)]);
        let t = radarr(vec![ok(RADARR_CLIENTS)]);
        let report = run(Mode::Plan, &task, &t, &FakeClock::new(), Timing::default()).unwrap();
        assert_eq!(report.outcome, Outcome::Unchanged);
        assert!(report.handed_over.is_empty());
        assert!(t.written.borrow().is_empty());
        assert_eq!(report.notes, ["not in the spec: download client SABnzbd"]);
    }

    #[test]
    fn a_differing_field_is_one_change_and_the_write_carries_it_and_the_secret() {
        let task = radarr_clients(vec![qbittorrent(8081)]);
        let changed = {
            let mut v: Value = serde_json::from_str(RADARR_CLIENTS).unwrap();
            for f in v[0]["fields"].as_array_mut().unwrap() {
                if f["name"] == "port" {
                    f["value"] = 8081.into();
                }
            }
            v.to_string()
        };
        let t = radarr(vec![ok(RADARR_CLIENTS), ok(&changed)])
            .on_put(vec![Step::Answer(202, String::new())]);
        let report = run(Mode::Apply, &task, &t, &FakeClock::new(), Timing::default()).unwrap();
        match &report.outcome {
            Outcome::Changed(changes) => {
                assert_eq!(changes.len(), 1);
                assert_eq!(
                    changes[0].to_string(),
                    "download client qBittorrent: fields.port 8080 -> 8081"
                );
            }
            other => panic!("expected Changed, got {other:?}"),
        }
        let (path, body) = sent(&t, 0);
        assert_eq!(path, "/api/v3/downloadclient/1?forceSave=true");
        assert_eq!(field_value(&body, "port"), 8081);
        assert_eq!(field_value(&body, "password"), PASSWORD);
        // The write, then the hand-over after reading back.
        assert_eq!(t.written.borrow().len(), 2);
        for change in match report.outcome {
            Outcome::Changed(c) => c,
            _ => unreachable!(),
        } {
            assert!(!change.to_string().contains(PASSWORD));
        }
        assert!(report.handed_over.iter().all(|l| !l.contains(PASSWORD)));
    }

    #[test]
    fn a_missing_provider_is_added_from_the_template_without_id() {
        let task = radarr_clients(vec![qbittorrent(8080)]);
        let without = {
            let v: Value = serde_json::from_str(RADARR_CLIENTS).unwrap();
            Value::Array(
                v.as_array()
                    .unwrap()
                    .iter()
                    .filter(|e| e["name"] != "qBittorrent")
                    .cloned()
                    .collect(),
            )
            .to_string()
        };
        let t = radarr(vec![ok(&without), ok(RADARR_CLIENTS)])
            .on_put(vec![Step::Answer(201, "{}".into())]);
        let report = run(Mode::Apply, &task, &t, &FakeClock::new(), Timing::default()).unwrap();
        match &report.outcome {
            Outcome::Changed(changes) => {
                assert_eq!(
                    changes[0].to_string(),
                    "download client qBittorrent: (missing) -> (added)"
                )
            }
            other => panic!("expected Changed, got {other:?}"),
        }
        let (path, body) = sent(&t, 0);
        assert_eq!(path, "/api/v3/downloadclient?forceSave=true");
        assert_eq!(body["name"], "qBittorrent");
        assert_eq!(body["implementation"], "QBittorrent");
        assert!(body.get("id").is_none());
        assert_eq!(field_value(&body, "host"), "10.0.10.11");
        assert_eq!(field_value(&body, "password"), PASSWORD);
        assert_eq!(body["priority"], 1);
    }

    #[test]
    fn names_the_answer_lacks_and_hidden_fields_that_are_not_hidden_are_errors() {
        let mut target = qbittorrent(8080);
        target
            .fields
            .insert("tvCategory".to_string(), json!("sonarr"));
        target.set.insert("onDownload".to_string(), json!(true));
        let task = radarr_clients(vec![target]);
        let t = radarr(vec![ok(RADARR_CLIENTS)]);
        let err = task
            .diff(&task.read(&t).unwrap())
            .err()
            .unwrap()
            .to_string();
        assert_eq!(
            err,
            "the answer has no such field: download client qBittorrent: onDownload; download client qBittorrent: fields.tvCategory"
        );

        let mut target = qbittorrent(8080);
        target.fields.remove("host");
        target
            .secret_fields
            .insert("host".to_string(), Secret::new("x".to_string()));
        let task = radarr_clients(vec![target]);
        let err = task
            .diff(&task.read(&t).unwrap())
            .err()
            .unwrap()
            .to_string();
        assert!(err.starts_with("the spec does not fit the service: download client qBittorrent: fields.host has privacy normal"), "{err}");
    }

    #[test]
    fn another_implementation_or_one_the_service_lacks_is_an_error() {
        let mut target = qbittorrent(8080);
        target.implementation = "Transmission".to_string();
        let task = radarr_clients(vec![target]);
        let t = radarr(vec![ok(RADARR_CLIENTS)]);
        let err = task
            .diff(&task.read(&t).unwrap())
            .err()
            .unwrap()
            .to_string();
        assert!(
            err.contains("is QBittorrent on the service, the spec says Transmission"),
            "{err}"
        );

        let mut target = qbittorrent(8080);
        target.name = "Deluge".to_string();
        target.implementation = "Deluge".to_string();
        let task = radarr_clients(vec![target]);
        let err = task
            .diff(&task.read(&t).unwrap())
            .err()
            .unwrap()
            .to_string();
        assert!(
            err.contains("the service has no implementation Deluge to add it from"),
            "{err}"
        );
    }

    #[test]
    fn lidarr_notifications_and_prowlarr_applications_match_the_hosts_spec() {
        let task = Providers {
            api: &NOTIFICATIONS_V1,
            providers: vec![ProviderTarget {
                name: "Autopulse".to_string(),
                implementation: "Webhook".to_string(),
                set: map(
                    json!({"onReleaseImport": true, "onUpgrade": true, "onRename": true,
                                "onGrab": false, "onHealthIssue": false, "includeHealthWarnings": false}),
                ),
                fields: map(
                    json!({"url": "http://10.0.10.10:2875/triggers/lidarr", "method": 1, "username": "<masked>"}),
                ),
                secret_fields: [("password".to_string(), Secret::new(PASSWORD.to_string()))]
                    .into_iter()
                    .collect(),
            }],
        };
        let t =
            FakeTransport::default().on_get("/api/v1/notification", vec![ok(LIDARR_NOTIFICATIONS)]);
        assert_eq!(task.diff(&task.read(&t).unwrap()).unwrap(), vec![]);

        let apps = Providers {
            api: &APPLICATIONS_V1,
            providers: vec![ProviderTarget {
                name: "Lidarr".to_string(),
                implementation: "Lidarr".to_string(),
                set: map(json!({"syncLevel": "fullSync"})),
                fields: map(
                    json!({"prowlarrUrl": "http://10.0.10.10:9696", "baseUrl": "http://10.0.80.10:8686",
                                   "syncCategories": [3000, 3010, 3020, 3030, 3040]}),
                ),
                secret_fields: [("apiKey".to_string(), Secret::new("lidarr-key".to_string()))]
                    .into_iter()
                    .collect(),
            }],
        };
        let t = FakeTransport::default()
            .on_get("/api/v1/system/status", vec![ok(PROWLARR_STATUS)])
            .on_get("/api/v1/applications", vec![ok(PROWLARR_APPS)])
            .on_put(vec![Step::Answer(202, String::new())]);
        assert_eq!(apps.probe(&t).ok().unwrap(), "2.5.2.5491");
        let current = apps.read(&t).unwrap();
        assert_eq!(apps.diff(&current).unwrap(), vec![]);
        assert_eq!(
            apps.notes(&current),
            [
                "not in the spec: application Radarr",
                "not in the spec: application Sonarr"
            ]
        );
        // A null value and an absent one are the same: authUsername is null.
        let mut with_null = apps.providers.into_iter().next().unwrap();
        with_null
            .fields
            .insert("authUsername".to_string(), Value::Null);
        let apps = Providers {
            api: &APPLICATIONS_V1,
            providers: vec![with_null],
        };
        assert_eq!(apps.diff(&current).unwrap(), vec![]);
        let lines = apps.hand_over(&t, &current).unwrap();
        assert_eq!(lines, ["application Lidarr: apiKey handed over from credentials (hidden; the service compares)"]);
        let (path, body) = sent(&t, 0);
        assert_eq!(path, "/api/v1/applications/3?forceSave=true");
        assert_eq!(field_value(&body, "apiKey"), "lidarr-key");
        assert_eq!(body["syncLevel"], "fullSync");
    }

    #[test]
    fn a_nameless_entry_and_an_empty_template_list_are_errors() {
        let task = radarr_clients(vec![qbittorrent(8080)]);
        let t = radarr(vec![ok(r#"[{"id":1,"fields":[]}]"#)]);
        assert!(task
            .read(&t)
            .err()
            .unwrap()
            .to_string()
            .contains("has no name"));
        let t = FakeTransport::default()
            .on_get("/api/v3/downloadclient", vec![ok("[]")])
            .on_get("/api/v3/downloadclient/schema", vec![ok("[]")]);
        assert!(task
            .read(&t)
            .err()
            .unwrap()
            .to_string()
            .contains("empty list"));
    }
}
