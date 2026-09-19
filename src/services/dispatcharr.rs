//! Dispatcharr: the IPTV proxy in front of a media server's Live TV (design
//! §29). Four tasks: the default stream profile, M3U accounts, the channel
//! groups of an account, and EPG sources.
//!
//! Dispatcharr is a Django REST Framework application whose OpenAPI
//! description drf-spectacular generates at runtime; the vendored copy was
//! fetched from the running container. It is right about the resources and
//! wrong about one action: `PATCH /api/m3u/accounts/{id}/group-settings/` is
//! declared with an `M3UAccount` body, while the view reads
//! `{"group_settings": [...]}` (`apps/m3u/api_views.py`). That endpoint's
//! body is therefore not checked against the description.
//!
//! There is no API key a spec could hold: Dispatcharr generates keys itself
//! and stores them per user. A task logs in as a service account instead
//! (`POST /api/accounts/token/`) and every later request carries the access
//! token, as for SuggestArr (design §19).

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::{
    client::{expect_status, expect_status_at, Transport},
    endpoint::{Endpoint, Shape},
    engine::{shortened, Change, Probe, Task},
    error::Error,
    secret::Secret,
};

pub const TOKEN: Endpoint = Endpoint {
    method: "POST",
    path: "/api/accounts/token/",
    request: Some(Shape::One("TokenObtainPair")),
    response: None,
};
/// Declared without a response schema; only the version is read.
pub const VERSION: Endpoint = Endpoint {
    method: "GET",
    path: "/api/core/version/",
    request: None,
    response: None,
};
pub const STREAM_PROFILES: Endpoint = Endpoint {
    method: "GET",
    path: "/api/core/streamprofiles/",
    request: None,
    response: Some(Shape::List("StreamProfile")),
};
pub const SETTINGS: Endpoint = Endpoint {
    method: "GET",
    path: "/api/core/settings/",
    request: None,
    response: Some(Shape::List("CoreSettings")),
};
pub const SETTING_UPDATE: Endpoint = Endpoint {
    method: "PATCH",
    path: "/api/core/settings/{id}/",
    // `value` is untyped in the description; only the reference is checked.
    request: Some(Shape::Document("PatchedCoreSettings")),
    response: None,
};
pub const M3U_ACCOUNTS: Endpoint = Endpoint {
    method: "GET",
    path: "/api/m3u/accounts/",
    request: None,
    response: Some(Shape::List("M3UAccount")),
};
/// Answers 201; the schema check reads only 200 answers.
pub const M3U_ACCOUNT_CREATE: Endpoint = Endpoint {
    method: "POST",
    path: "/api/m3u/accounts/",
    request: Some(Shape::Document("M3UAccount")),
    response: None,
};
pub const M3U_ACCOUNT_UPDATE: Endpoint = Endpoint {
    method: "PATCH",
    path: "/api/m3u/accounts/{id}/",
    request: Some(Shape::Document("PatchedM3UAccount")),
    response: None,
};
/// The body the description declares is wrong (see the module comment), so
/// no shape is checked; the fields a spec sets are checked against
/// `ChannelGroupM3UAccount`, which is what the account answers with.
pub const GROUP_SETTINGS: Endpoint = Endpoint {
    method: "PATCH",
    path: "/api/m3u/accounts/{id}/group-settings/",
    request: None,
    response: None,
};
pub const CHANNEL_GROUPS: Endpoint = Endpoint {
    method: "GET",
    path: "/api/channels/groups/",
    request: None,
    response: Some(Shape::List("ChannelGroup")),
};
pub const EPG_SOURCES: Endpoint = Endpoint {
    method: "GET",
    path: "/api/epg/sources/",
    request: None,
    response: Some(Shape::List("EPGSource")),
};
pub const EPG_SOURCE_CREATE: Endpoint = Endpoint {
    method: "POST",
    path: "/api/epg/sources/",
    request: Some(Shape::Document("EPGSource")),
    response: None,
};
pub const EPG_SOURCE_UPDATE: Endpoint = Endpoint {
    method: "PATCH",
    path: "/api/epg/sources/{id}/",
    request: Some(Shape::Document("PatchedEPGSource")),
    response: None,
};
pub const ENDPOINTS: [Endpoint; 13] = [
    TOKEN,
    VERSION,
    STREAM_PROFILES,
    SETTINGS,
    SETTING_UPDATE,
    M3U_ACCOUNTS,
    M3U_ACCOUNT_CREATE,
    M3U_ACCOUNT_UPDATE,
    GROUP_SETTINGS,
    CHANNEL_GROUPS,
    EPG_SOURCES,
    EPG_SOURCE_CREATE,
    EPG_SOURCE_UPDATE,
];

/// The components a spec's fields are checked against: an account's and a
/// source's fields must be writable when they are added and when they are
/// updated; a group's are those the account answers with.
pub const M3U_ACCOUNT_CREATE_COMPONENT: &str = "M3UAccount";
pub const M3U_ACCOUNT_UPDATE_COMPONENT: &str = "PatchedM3UAccount";
pub const GROUP_COMPONENT: &str = "ChannelGroupM3UAccount";
pub const EPG_SOURCE_CREATE_COMPONENT: &str = "EPGSource";
pub const EPG_SOURCE_UPDATE_COMPONENT: &str = "PatchedEPGSource";

/// The fields a group setting may name: what the view reads
/// (`update_group_settings`), and nothing it would drop.
pub const GROUP_FIELDS: [&str; 4] = [
    "enabled",
    "auto_channel_sync",
    "auto_sync_channel_start",
    "auto_sync_channel_end",
];

/// The core setting that holds the default stream profile, and its field.
const STREAM_SETTINGS_KEY: &str = "stream_settings";
const DEFAULT_STREAM_PROFILE: &str = "default_stream_profile";

pub fn wire_types() -> Vec<schemars::Schema> {
    vec![
        schemars::schema_for!(TokenObtainPair),
        schemars::schema_for!(StreamProfile),
        schemars::schema_for!(CoreSettings),
        schemars::schema_for!(M3UAccount),
        schemars::schema_for!(ChannelGroup),
        schemars::schema_for!(EPGSource),
    ]
}

#[derive(Serialize, JsonSchema)]
pub struct TokenObtainPair {
    pub username: String,
    pub password: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct StreamProfile {
    pub id: i64,
    pub name: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct CoreSettings {
    pub id: i64,
    pub key: String,
    /// Untyped in the description: each setting has its own shape.
    #[schemars(skip)]
    pub value: Value,
}

#[derive(Serialize)]
pub struct PatchedCoreSettings {
    pub value: Value,
}

/// An account as it answers. Its other fields stay in `rest`, where a spec's
/// fields are looked up; `password` is among them and is never printed (a
/// spec cannot name it, see `spec.rs`).
#[derive(Deserialize, JsonSchema)]
pub struct M3UAccount {
    pub id: i64,
    pub name: String,
    #[serde(default)]
    #[schemars(skip)]
    pub channel_groups: Vec<GroupMembership>,
    #[serde(flatten)]
    #[schemars(skip)]
    pub rest: Map<String, Value>,
}

/// One entry of an account's `channel_groups` (`ChannelGroupM3UAccount`).
#[derive(Deserialize)]
pub struct GroupMembership {
    pub channel_group: i64,
    #[serde(flatten)]
    pub rest: Map<String, Value>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ChannelGroup {
    pub id: i64,
    pub name: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct EPGSource {
    pub id: i64,
    pub name: String,
    #[serde(flatten)]
    #[schemars(skip)]
    pub rest: Map<String, Value>,
}

/// What a login answered. Only the access token is read.
#[derive(Deserialize)]
struct LoginAnswer {
    access: Secret,
}

/// Exchanges username and password for an access token. Neither value
/// reaches a log line: the body is built here, and a refusal names the
/// account, never the password.
pub fn login(t: &dyn Transport, username: &str, password: &Secret) -> Result<Secret, Error> {
    let body = serialize(
        TOKEN.method,
        TOKEN.path,
        &json!({ "username": username, "password": password.expose() }),
    )?;
    let reply = t.post_json(TOKEN.path, &body)?;
    if reply.status == 400 || reply.status == 401 || reply.status == 403 {
        return Err(Error::Status {
            method: TOKEN.method,
            path: TOKEN.path.to_string(),
            status: reply.status,
            validation: vec![format!(
                "the account {username} or its password was refused"
            )],
        });
    }
    expect_status_at(TOKEN.method, TOKEN.path, &reply, &[200])?;
    let answer: LoginAnswer = decode(TOKEN.path, &reply.body)?;
    Ok(answer.access)
}

#[derive(Deserialize)]
struct VersionAnswer {
    #[serde(default)]
    version: Option<String>,
}

/// Readiness: the version answers. A refused token is fatal.
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
                validation: vec!["the access token was refused".to_string()],
            }))
        }
        other => return Err(Probe::NotYet(format!("HTTP {other}"))),
    }
    let answer: VersionAnswer = serde_json::from_str(&reply.body)
        .map_err(|e| Probe::NotYet(format!("unexpected answer: {e}")))?;
    answer
        .version
        .filter(|v| !v.is_empty())
        .ok_or_else(|| Probe::NotYet("the answer has no version".to_string()))
}

fn decode<T: for<'de> Deserialize<'de>>(path: &str, body: &str) -> Result<T, Error> {
    serde_json::from_str(body).map_err(|e| Error::Decode {
        path: path.to_string(),
        reason: e.to_string(),
    })
}

fn serialize(method: &'static str, path: &str, value: &impl Serialize) -> Result<String, Error> {
    serde_json::to_string(value).map_err(|e| Error::Request {
        method,
        path: path.to_string(),
        reason: format!("cannot serialize: {e}"),
    })
}

fn get_list<T: for<'de> Deserialize<'de>>(t: &dyn Transport, ep: &Endpoint) -> Result<T, Error> {
    let reply = t.get(ep.path)?;
    expect_status(ep, &reply, &[200])?;
    decode(ep.path, &reply.body)
}

/// The fields of `set` that differ from `current`, and those `current` lacks.
fn compare(
    subject: &str,
    current: &Map<String, Value>,
    set: &BTreeMap<String, Value>,
    missing: &mut Vec<String>,
) -> Vec<Change> {
    let mut changes = Vec::new();
    for (field, desired) in set {
        match current.get(field) {
            None => missing.push(format!("{subject}: {field}")),
            Some(now) if !same(now, desired) => changes.push(Change {
                subject: subject.to_string(),
                field: field.clone(),
                current: shortened(now),
                desired: shortened(desired),
            }),
            Some(_) => {}
        }
    }
    changes
}

/// Equal as JSON, with one allowance: Dispatcharr stores channel numbers as
/// floats (`auto_sync_channel_start` answers `1.0`), so `1` and `1.0` are the
/// same number.
fn same(a: &Value, b: &Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) if a.is_number() && b.is_number() => x == y,
        _ => a == b,
    }
}

// --- stream-settings -------------------------------------------------------

/// The default stream profile, by name. Dispatcharr stores its id in the core
/// setting `stream_settings`; ids differ between instances, names do not.
pub struct StreamSettings {
    pub default_stream_profile: String,
}

pub struct StreamSettingsState {
    pub profiles: Vec<StreamProfile>,
    pub setting: CoreSettings,
}

impl StreamSettings {
    fn wanted_id(&self, profiles: &[StreamProfile]) -> Result<i64, Error> {
        profiles
            .iter()
            .find(|p| p.name == self.default_stream_profile)
            .map(|p| p.id)
            .ok_or_else(|| {
                let names: Vec<&str> = profiles.iter().map(|p| p.name.as_str()).collect();
                Error::NotFound(vec![format!(
                    "stream profile {:?} (Dispatcharr has: {})",
                    self.default_stream_profile,
                    names.join(", ")
                )])
            })
    }
}

impl Task for StreamSettings {
    type Current = StreamSettingsState;

    fn probe(&self, t: &dyn Transport) -> Result<String, Probe> {
        probe(t)
    }

    fn read(&self, t: &dyn Transport) -> Result<Self::Current, Error> {
        let profiles: Vec<StreamProfile> = get_list(t, &STREAM_PROFILES)?;
        if profiles.is_empty() {
            return Err(Error::EmptyList {
                path: STREAM_PROFILES.path.to_string(),
            });
        }
        let settings: Vec<CoreSettings> = get_list(t, &SETTINGS)?;
        let setting = settings
            .into_iter()
            .find(|s| s.key == STREAM_SETTINGS_KEY)
            .ok_or_else(|| Error::NotFound(vec![format!("core setting {STREAM_SETTINGS_KEY}")]))?;
        Ok(StreamSettingsState { profiles, setting })
    }

    fn diff(&self, current: &Self::Current) -> Result<Vec<Change>, Error> {
        let wanted = self.wanted_id(&current.profiles)?;
        let now = current.setting.value.get(DEFAULT_STREAM_PROFILE);
        if now.and_then(Value::as_i64) == Some(wanted) {
            return Ok(Vec::new());
        }
        let name_of = |id: Option<i64>| match id {
            Some(id) => current
                .profiles
                .iter()
                .find(|p| p.id == id)
                .map_or_else(|| format!("#{id}"), |p| p.name.clone()),
            None => "(none)".to_string(),
        };
        Ok(vec![Change {
            subject: STREAM_SETTINGS_KEY.to_string(),
            field: DEFAULT_STREAM_PROFILE.to_string(),
            current: name_of(now.and_then(Value::as_i64)),
            desired: self.default_stream_profile.clone(),
        }])
    }

    fn notes(&self, _current: &Self::Current) -> Vec<String> {
        Vec::new()
    }

    /// The setting's whole `value` goes back with the one field changed: the
    /// PATCH replaces `value` as a whole.
    fn write(&self, t: &dyn Transport, current: &Self::Current) -> Result<(), Error> {
        let wanted = self.wanted_id(&current.profiles)?;
        let mut value = current.setting.value.clone();
        let Some(object) = value.as_object_mut() else {
            return Err(Error::Decode {
                path: SETTINGS.path.to_string(),
                reason: format!("{STREAM_SETTINGS_KEY}.value is not an object"),
            });
        };
        object.insert(DEFAULT_STREAM_PROFILE.to_string(), json!(wanted));
        let path = SETTING_UPDATE
            .path
            .replace("{id}", &current.setting.id.to_string());
        let body = serialize(SETTING_UPDATE.method, &path, &PatchedCoreSettings { value })?;
        let reply = t.patch_json(&path, &body)?;
        expect_status_at(SETTING_UPDATE.method, &path, &reply, &[200])
    }
}

// --- m3u-accounts and epg-sources ------------------------------------------

/// Entries by name, fields by name. A missing entry is added with its fields;
/// converge never removes one. The same shape for M3U accounts and EPG
/// sources, which differ only in their endpoints.
pub struct Entries {
    pub kind: EntryKind,
    pub entries: BTreeMap<String, BTreeMap<String, Value>>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EntryKind {
    M3uAccount,
    EpgSource,
}

impl EntryKind {
    fn label(self) -> &'static str {
        match self {
            EntryKind::M3uAccount => "M3U account",
            EntryKind::EpgSource => "EPG source",
        }
    }

    fn endpoints(self) -> (Endpoint, Endpoint, Endpoint) {
        match self {
            EntryKind::M3uAccount => (M3U_ACCOUNTS, M3U_ACCOUNT_CREATE, M3U_ACCOUNT_UPDATE),
            EntryKind::EpgSource => (EPG_SOURCES, EPG_SOURCE_CREATE, EPG_SOURCE_UPDATE),
        }
    }
}

/// An entry as both lists answer it: id, name and everything else.
#[derive(Deserialize)]
pub struct Entry {
    pub id: i64,
    pub name: String,
    #[serde(flatten)]
    pub rest: Map<String, Value>,
}

impl Entries {
    fn subject(&self, name: &str) -> String {
        format!("{} {name}", self.kind.label())
    }

    fn changes_of(
        &self,
        name: &str,
        set: &BTreeMap<String, Value>,
        entry: &Entry,
        missing: &mut Vec<String>,
    ) -> Vec<Change> {
        compare(&self.subject(name), &entry.rest, set, missing)
    }
}

impl Task for Entries {
    type Current = Vec<Entry>;

    fn probe(&self, t: &dyn Transport) -> Result<String, Probe> {
        probe(t)
    }

    /// An empty list is a valid answer: a fresh Dispatcharr has no EPG
    /// source, and every entry the spec names is then a change.
    fn read(&self, t: &dyn Transport) -> Result<Self::Current, Error> {
        get_list(t, &self.kind.endpoints().0)
    }

    fn diff(&self, current: &Self::Current) -> Result<Vec<Change>, Error> {
        let mut missing = Vec::new();
        let mut changes = Vec::new();
        for (name, set) in &self.entries {
            match current.iter().find(|e| &e.name == name) {
                Some(entry) => changes.extend(self.changes_of(name, set, entry, &mut missing)),
                None => changes.push(Change {
                    subject: self.subject(name),
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
            .filter(|e| !self.entries.contains_key(&e.name))
            .map(|e| format!("not in the spec: {}", self.subject(&e.name)))
            .collect()
    }

    fn write(&self, t: &dyn Transport, current: &Self::Current) -> Result<(), Error> {
        let (_, create, update) = self.kind.endpoints();
        for (name, set) in &self.entries {
            let mut body: Map<String, Value> = set.clone().into_iter().collect();
            match current.iter().find(|e| &e.name == name) {
                None => {
                    body.insert("name".to_string(), json!(name));
                    let body = serialize(create.method, create.path, &body)?;
                    let reply = t.post_json(create.path, &body)?;
                    expect_status(&create, &reply, &[200, 201])?;
                }
                Some(entry) => {
                    let mut missing = Vec::new();
                    if self.changes_of(name, set, entry, &mut missing).is_empty() {
                        continue;
                    }
                    let path = update.path.replace("{id}", &entry.id.to_string());
                    let body = serialize(update.method, &path, &body)?;
                    let reply = t.patch_json(&path, &body)?;
                    expect_status_at(update.method, &path, &reply, &[200])?;
                }
            }
        }
        Ok(())
    }
}

// --- m3u-groups ------------------------------------------------------------

/// The settings of channel groups within M3U accounts: account name -> group
/// name -> fields. A group exists only once Dispatcharr has read the
/// account's playlist, which it does asynchronously after the account is
/// added; readiness therefore waits until every named group is there.
pub struct Groups {
    pub accounts: BTreeMap<String, BTreeMap<String, BTreeMap<String, Value>>>,
}

pub struct GroupsState {
    pub accounts: Vec<M3UAccount>,
    pub groups: Vec<ChannelGroup>,
}

impl Groups {
    /// The named account and group, or what is not there yet.
    fn locate<'a>(
        state: &'a GroupsState,
        account: &str,
        group: &str,
    ) -> Result<(&'a M3UAccount, &'a GroupMembership), String> {
        let Some(a) = state.accounts.iter().find(|a| a.name == account) else {
            return Err(format!("M3U account {account} does not exist"));
        };
        let Some(g) = state.groups.iter().find(|g| g.name == group) else {
            return Err(format!("group {group} is not loaded yet"));
        };
        a.channel_groups
            .iter()
            .find(|m| m.channel_group == g.id)
            .map(|m| (a, m))
            .ok_or_else(|| format!("group {group} is not part of M3U account {account} yet"))
    }

    fn read_state(t: &dyn Transport) -> Result<GroupsState, Error> {
        Ok(GroupsState {
            accounts: get_list(t, &M3U_ACCOUNTS)?,
            groups: get_list(t, &CHANNEL_GROUPS)?,
        })
    }

    fn subject(account: &str, group: &str) -> String {
        format!("group {group} of M3U account {account}")
    }
}

impl Task for Groups {
    type Current = GroupsState;

    /// Ready when the service answers AND every group the spec names has
    /// been loaded into its account.
    fn probe(&self, t: &dyn Transport) -> Result<String, Probe> {
        let version = probe(t)?;
        let state = Self::read_state(t).map_err(|e| Probe::NotYet(e.to_string()))?;
        for (account, groups) in &self.accounts {
            for group in groups.keys() {
                Self::locate(&state, account, group).map_err(Probe::NotYet)?;
            }
        }
        Ok(version)
    }

    fn read(&self, t: &dyn Transport) -> Result<Self::Current, Error> {
        Self::read_state(t)
    }

    fn diff(&self, current: &Self::Current) -> Result<Vec<Change>, Error> {
        let mut missing = Vec::new();
        let mut absent = Vec::new();
        let mut changes = Vec::new();
        for (account, groups) in &self.accounts {
            for (group, set) in groups {
                match Self::locate(current, account, group) {
                    Ok((_, membership)) => changes.extend(compare(
                        &Self::subject(account, group),
                        &membership.rest,
                        set,
                        &mut missing,
                    )),
                    Err(why) => absent.push(why),
                }
            }
        }
        if !absent.is_empty() {
            return Err(Error::NotFound(absent));
        }
        if missing.is_empty() {
            Ok(changes)
        } else {
            Err(Error::MissingField(missing))
        }
    }

    fn notes(&self, _current: &Self::Current) -> Vec<String> {
        Vec::new()
    }

    /// One PATCH per account, naming only the groups the spec names; the
    /// view leaves every other group of the account as it is.
    fn write(&self, t: &dyn Transport, current: &Self::Current) -> Result<(), Error> {
        for (account, groups) in &self.accounts {
            let mut settings = Vec::new();
            let mut account_id = None;
            for (group, set) in groups {
                let (a, membership) = Self::locate(current, account, group)
                    .map_err(|why| Error::NotFound(vec![why]))?;
                account_id = Some(a.id);
                let mut entry: Map<String, Value> = set.clone().into_iter().collect();
                entry.insert("channel_group".to_string(), json!(membership.channel_group));
                settings.push(Value::Object(entry));
            }
            let Some(id) = account_id else { continue };
            let path = GROUP_SETTINGS.path.replace("{id}", &id.to_string());
            let body = serialize(
                GROUP_SETTINGS.method,
                &path,
                &json!({ "group_settings": settings }),
            )?;
            let reply = t.patch_json(&path, &body)?;
            expect_status_at(GROUP_SETTINGS.method, &path, &reply, &[200])?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{ok, FakeTransport, Step};

    const VERSION_JSON: &str =
        include_str!("../../tests/fixtures/dispatcharr-0.31.0/core-version.json");
    const PROFILES_JSON: &str =
        include_str!("../../tests/fixtures/dispatcharr-0.31.0/core-streamprofiles.json");
    const SETTINGS_JSON: &str =
        include_str!("../../tests/fixtures/dispatcharr-0.31.0/core-settings.json");
    const ACCOUNTS_JSON: &str =
        include_str!("../../tests/fixtures/dispatcharr-0.31.0/m3u-accounts.json");
    const GROUPS_JSON: &str =
        include_str!("../../tests/fixtures/dispatcharr-0.31.0/channels-groups.json");
    const SOURCES_JSON: &str =
        include_str!("../../tests/fixtures/dispatcharr-0.31.0/epg-sources.json");

    fn fields(pairs: &[(&str, Value)]) -> BTreeMap<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    fn with(body: &str, edit: impl FnOnce(&mut Value)) -> String {
        let mut v: Value = serde_json::from_str(body).unwrap();
        edit(&mut v);
        v.to_string()
    }

    #[test]
    fn probe_reads_the_version_and_a_refused_token_is_fatal() {
        let t = FakeTransport::default().on_get(VERSION.path, vec![ok(VERSION_JSON)]);
        assert!(matches!(probe(&t), Ok(v) if v == "0.31.0"));
        let t = FakeTransport::default().on_get(
            VERSION.path,
            vec![Step::Answer(401, r#"{"detail":"x"}"#.to_string())],
        );
        assert!(matches!(probe(&t), Err(Probe::Fatal(_))));
    }

    #[test]
    fn login_returns_the_access_token_and_a_refusal_names_no_password() {
        let t = FakeTransport::default().on_put(vec![Step::Answer(
            200,
            r#"{"access":"tok-a","refresh":"tok-r"}"#.to_string(),
        )]);
        let token = login(&t, "converge", &Secret::new("pw".to_string())).unwrap();
        assert_eq!(token.expose(), "tok-a");
        let sent: Value = serde_json::from_str(&t.written.borrow()[0].1).unwrap();
        assert_eq!(sent, json!({"username": "converge", "password": "pw"}));

        let t = FakeTransport::default().on_put(vec![Step::Answer(
            401,
            r#"{"detail":"No active account"}"#.to_string(),
        )]);
        let Err(e) = login(&t, "converge", &Secret::new("geheim".to_string())) else {
            panic!("a refused login succeeded")
        };
        let e = e.to_string();
        assert!(e.contains("converge"), "{e}");
        assert!(!e.contains("geheim"), "{e}");
    }

    #[test]
    fn the_recorded_stream_setting_already_names_streamlink() {
        let task = StreamSettings {
            default_stream_profile: "streamlink".to_string(),
        };
        let t = FakeTransport::default()
            .on_get(STREAM_PROFILES.path, vec![ok(PROFILES_JSON)])
            .on_get(SETTINGS.path, vec![ok(SETTINGS_JSON)]);
        assert_eq!(task.diff(&task.read(&t).unwrap()).unwrap(), vec![]);
    }

    #[test]
    fn a_default_profile_is_set_by_name_and_the_rest_of_the_value_stays() {
        let task = StreamSettings {
            default_stream_profile: "ffmpeg".to_string(),
        };
        let t = FakeTransport::default()
            .on_get(STREAM_PROFILES.path, vec![ok(PROFILES_JSON)])
            .on_get(SETTINGS.path, vec![ok(SETTINGS_JSON)])
            .on_put(vec![Step::Answer(200, "{}".to_string())]);
        let current = task.read(&t).unwrap();
        let changes = task.diff(&current).unwrap();
        assert_eq!(
            changes[0].to_string(),
            "stream_settings: default_stream_profile streamlink -> ffmpeg"
        );
        task.write(&t, &current).unwrap();
        let written = t.written.borrow();
        assert_eq!(written[0].0, "/api/core/settings/13/");
        let sent: Value = serde_json::from_str(&written[0].1).unwrap();
        assert_eq!(
            sent,
            json!({"value": {"m3u_hash_key": "url", "default_user_agent": 1, "default_stream_profile": 1}})
        );
    }

    #[test]
    fn an_unknown_profile_names_the_ones_there_are() {
        let task = StreamSettings {
            default_stream_profile: "Streamlink".to_string(),
        };
        let t = FakeTransport::default()
            .on_get(STREAM_PROFILES.path, vec![ok(PROFILES_JSON)])
            .on_get(SETTINGS.path, vec![ok(SETTINGS_JSON)]);
        let e = task.diff(&task.read(&t).unwrap()).unwrap_err().to_string();
        assert!(e.contains("\"Streamlink\""), "{e}");
        assert!(e.contains("streamlink"), "{e}");
    }

    fn accounts(set: &[(&str, Value)]) -> Entries {
        Entries {
            kind: EntryKind::M3uAccount,
            entries: BTreeMap::from([("Oeffentlich-rechtlich".to_string(), fields(set))]),
        }
    }

    #[test]
    fn the_recorded_account_is_already_desired() {
        let task = accounts(&[
            ("file_path", json!("/data/sender.m3u")),
            ("is_active", json!(true)),
        ]);
        let t = FakeTransport::default().on_get(M3U_ACCOUNTS.path, vec![ok(ACCOUNTS_JSON)]);
        let current = task.read(&t).unwrap();
        assert_eq!(task.diff(&current).unwrap(), vec![]);
        assert_eq!(
            task.notes(&current),
            vec!["not in the spec: M3U account custom"]
        );
    }

    #[test]
    fn a_changed_account_field_is_patched_with_only_the_spec_fields() {
        let task = accounts(&[
            ("file_path", json!("/m3u/sender.m3u")),
            ("refresh_interval", json!(24)),
        ]);
        let t = FakeTransport::default()
            .on_get(M3U_ACCOUNTS.path, vec![ok(ACCOUNTS_JSON)])
            .on_put(vec![Step::Answer(200, "{}".to_string())]);
        let current = task.read(&t).unwrap();
        assert_eq!(task.diff(&current).unwrap().len(), 2);
        task.write(&t, &current).unwrap();
        let written = t.written.borrow();
        assert_eq!(written[0].0, "/api/m3u/accounts/2/");
        let sent: Value = serde_json::from_str(&written[0].1).unwrap();
        assert_eq!(
            sent,
            json!({"file_path": "/m3u/sender.m3u", "refresh_interval": 24})
        );
    }

    #[test]
    fn a_missing_source_is_added_with_its_name() {
        let task = Entries {
            kind: EntryKind::EpgSource,
            entries: BTreeMap::from([(
                "epgshare01-de".to_string(),
                fields(&[
                    ("source_type", json!("xmltv")),
                    ("url", json!("https://epg.example/de.xml.gz")),
                    ("refresh_interval", json!(12)),
                ]),
            )]),
        };
        let empty = FakeTransport::default()
            .on_get(EPG_SOURCES.path, vec![ok("[]")])
            .on_put(vec![Step::Answer(201, "{}".to_string())]);
        let current = task.read(&empty).unwrap();
        let changes = task.diff(&current).unwrap();
        assert_eq!(
            changes[0].to_string(),
            "EPG source epgshare01-de: (missing) -> (added)"
        );
        task.write(&empty, &current).unwrap();
        let written = empty.written.borrow();
        assert_eq!(written[0].0, "/api/epg/sources/");
        let sent: Value = serde_json::from_str(&written[0].1).unwrap();
        assert_eq!(sent["name"], "epgshare01-de");
        assert_eq!(sent["refresh_interval"], 12);

        // The recorded source differs only in its refresh interval.
        let t = FakeTransport::default().on_get(EPG_SOURCES.path, vec![ok(SOURCES_JSON)]);
        let task = Entries {
            kind: EntryKind::EpgSource,
            entries: BTreeMap::from([(
                "epgshare01-de".to_string(),
                fields(&[("refresh_interval", json!(12))]),
            )]),
        };
        assert_eq!(
            task.diff(&task.read(&t).unwrap()).unwrap()[0].to_string(),
            "EPG source epgshare01-de: refresh_interval 0 -> 12"
        );
    }

    fn group_task(set: &[(&str, Value)]) -> Groups {
        Groups {
            accounts: BTreeMap::from([(
                "Oeffentlich-rechtlich".to_string(),
                BTreeMap::from([("Öffentlich-rechtlich".to_string(), fields(set))]),
            )]),
        }
    }

    #[test]
    fn the_recorded_group_is_already_desired_and_one_equals_one_point_zero() {
        let task = group_task(&[
            ("enabled", json!(true)),
            ("auto_channel_sync", json!(true)),
            ("auto_sync_channel_start", json!(1)),
            ("auto_sync_channel_end", json!(99)),
        ]);
        let t = FakeTransport::default()
            .on_get(M3U_ACCOUNTS.path, vec![ok(ACCOUNTS_JSON)])
            .on_get(CHANNEL_GROUPS.path, vec![ok(GROUPS_JSON)]);
        assert_eq!(task.diff(&task.read(&t).unwrap()).unwrap(), vec![]);
    }

    #[test]
    fn group_settings_go_out_as_the_list_the_view_reads() {
        let task = group_task(&[("auto_sync_channel_end", json!(50))]);
        let t = FakeTransport::default()
            .on_get(M3U_ACCOUNTS.path, vec![ok(ACCOUNTS_JSON)])
            .on_get(CHANNEL_GROUPS.path, vec![ok(GROUPS_JSON)])
            .on_put(vec![Step::Answer(200, "{}".to_string())]);
        let current = task.read(&t).unwrap();
        task.write(&t, &current).unwrap();
        let written = t.written.borrow();
        assert_eq!(written[0].0, "/api/m3u/accounts/2/group-settings/");
        let sent: Value = serde_json::from_str(&written[0].1).unwrap();
        assert_eq!(
            sent,
            json!({"group_settings": [{"channel_group": 2, "auto_sync_channel_end": 50}]})
        );
    }

    #[test]
    fn readiness_waits_until_the_group_is_loaded() {
        let task = group_task(&[("enabled", json!(true))]);
        // The account exists, its playlist is not read yet: no groups.
        let unloaded = with(ACCOUNTS_JSON, |v| {
            for a in v.as_array_mut().unwrap() {
                a["channel_groups"] = json!([]);
            }
        });
        let t = FakeTransport::default()
            .on_get(VERSION.path, vec![ok(VERSION_JSON)])
            .on_get(M3U_ACCOUNTS.path, vec![ok(&unloaded)])
            .on_get(
                CHANNEL_GROUPS.path,
                vec![ok(r#"[{"id":1,"name":"Default Group"}]"#)],
            );
        match task.probe(&t) {
            Err(Probe::NotYet(why)) => assert!(why.contains("Öffentlich-rechtlich"), "{why}"),
            _ => panic!("ready too early"),
        }
        let t = FakeTransport::default()
            .on_get(VERSION.path, vec![ok(VERSION_JSON)])
            .on_get(M3U_ACCOUNTS.path, vec![ok(ACCOUNTS_JSON)])
            .on_get(CHANNEL_GROUPS.path, vec![ok(GROUPS_JSON)]);
        assert!(matches!(task.probe(&t), Ok(v) if v == "0.31.0"));
    }
}
