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
/// The description declares a page (`PaginatedChannelList`); without
/// `page_size` the view answers a plain list -- recorded, and that is how it
/// is read. So no response shape is checked here; `Channel` is still checked
/// as a wire type against its component.
pub const CHANNELS: Endpoint = Endpoint {
    method: "GET",
    path: "/api/channels/channels/",
    request: None,
    response: None,
};
/// A list of partial channels; an `override` entry writes the channel's
/// `ChannelOverride`, which the channel sync leaves alone (design §34). The
/// description declares no body for it.
pub const CHANNELS_BULK: Endpoint = Endpoint {
    method: "PATCH",
    path: "/api/channels/channels/edit/bulk/",
    request: None,
    response: None,
};
pub const EPG_DATA: Endpoint = Endpoint {
    method: "GET",
    path: "/api/epg/epgdata/",
    request: None,
    response: Some(Shape::List("EPGData")),
};

pub const ENDPOINTS: [Endpoint; 16] = [
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
    CHANNELS,
    CHANNELS_BULK,
    EPG_DATA,
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
/// What the view writes of a membership. It REPLACES the row, so every one of
/// these travels in each write, the current value where the spec names none.
const GROUP_WRITTEN: [&str; 5] = [
    "enabled",
    "auto_channel_sync",
    "auto_sync_channel_start",
    "auto_sync_channel_end",
    "custom_properties",
];
/// A group setting that is not a field of the view: the stream profile, by
/// name, which the sync gives every channel of the group
/// (`custom_properties.stream_profile_id`, design §32).
pub const GROUP_STREAM_PROFILE: &str = "stream_profile";
/// Group settings the view keeps in `custom_properties` and the channel sync
/// reads by these names (`apps/m3u/tasks.py`): which streams become channels
/// and how they are renamed (design §33). Compared and written as text.
pub const GROUP_CUSTOM_FIELDS: [&str; 4] = [
    "name_match_regex",
    "name_match_exclude_regex",
    "name_regex_pattern",
    "name_replace_pattern",
];
const STREAM_PROFILE_ID: &str = "stream_profile_id";

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
        schemars::schema_for!(Channel),
        schemars::schema_for!(EPGData),
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

/// A channel as the list answers it. The `effective_*` fields are what the
/// output uses: the channel's own value, or its override.
#[derive(Deserialize, JsonSchema)]
pub struct Channel {
    pub id: i64,
    pub name: String,
    pub effective_name: Option<String>,
    pub effective_epg_data_id: Option<i64>,
}

/// One channel of one EPG source.
#[derive(Deserialize, JsonSchema)]
#[allow(clippy::upper_case_acronyms)]
pub struct EPGData {
    pub id: i64,
    pub tvg_id: Option<String>,
    pub epg_source: Option<i64>,
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
        .map_err(|e| Probe::NotYet(format!("unexpected answer: {}", crate::error::shape(&e))))?;
    answer
        .version
        .filter(|v| !v.is_empty())
        .ok_or_else(|| Probe::NotYet("the answer has no version".to_string()))
}

fn decode<T: for<'de> Deserialize<'de>>(path: &str, body: &str) -> Result<T, Error> {
    serde_json::from_str(body).map_err(|e| Error::Decode {
        path: path.to_string(),
        reason: crate::error::shape(&e),
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
///
/// An M3U account may also carry `secrets` from credentials (design §30),
/// and an EPG source its `url` (§31, compared like `server_url`):
/// `server_url` and `username`, which Dispatcharr answers in the clear and
/// which are compared without being shown, and `password`, which it answers
/// as `""` and which is therefore handed over on every `apply`.
pub struct Entries {
    pub kind: EntryKind,
    pub entries: BTreeMap<String, BTreeMap<String, Value>>,
    /// Entry name -> field -> value, read from credentials.
    pub secrets: BTreeMap<String, BTreeMap<String, Secret>>,
}

/// The secret fields an M3U account may take from a credential, and the one
/// among them Dispatcharr never answers with (`write_only`).
pub const SECRET_FIELDS: [&str; 3] = ["server_url", "username", "password"];
pub const WRITE_ONLY: &str = "password";
/// An EPG source's one secret field: an Xtream provider's guide URL carries
/// the account's user name and password in its query (design §31).
pub const SOURCE_SECRET_FIELDS: [&str; 1] = ["url"];

impl EntryKind {
    /// The fields this kind may take from a credential.
    pub fn secret_fields(self) -> &'static [&'static str] {
        match self {
            EntryKind::M3uAccount => &SECRET_FIELDS,
            EntryKind::EpgSource => &SOURCE_SECRET_FIELDS,
        }
    }
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
        let subject = self.subject(name);
        let mut changes = compare(&subject, &entry.rest, set, missing);
        for (field, secret) in self.secrets.get(name).into_iter().flatten() {
            if field == WRITE_ONLY {
                continue;
            }
            match entry.rest.get(field) {
                None => missing.push(format!("{subject}: {field}")),
                Some(now) if now.as_str() != Some(secret.expose()) => changes.push(Change {
                    subject: subject.clone(),
                    field: field.clone(),
                    current: "(another value, not shown)".to_string(),
                    desired: "(the credential's, not shown)".to_string(),
                }),
                Some(_) => {}
            }
        }
        changes
    }

    /// The spec's fields and, for an entry being written, every secret: a
    /// changed user name without its password would be half an account.
    fn body_of(&self, name: &str, set: &BTreeMap<String, Value>) -> Map<String, Value> {
        let mut body: Map<String, Value> = set.clone().into_iter().collect();
        for (field, secret) in self.secrets.get(name).into_iter().flatten() {
            body.insert(field.clone(), json!(secret.expose()));
        }
        body
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
            let mut body = self.body_of(name, set);
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

    /// The write-only password, on every `apply`: a `PATCH` with nothing
    /// else, which Dispatcharr saves without a refresh (`refresh_account_on_save`
    /// acts only on a created account, the schedule only on its own fields).
    fn hand_over(&self, t: &dyn Transport, current: &Self::Current) -> Result<Vec<String>, Error> {
        let (_, _, update) = self.kind.endpoints();
        let mut lines = Vec::new();
        for (name, secrets) in &self.secrets {
            let Some(secret) = secrets.get(WRITE_ONLY) else {
                continue;
            };
            let Some(entry) = current.iter().find(|e| &e.name == name) else {
                return Err(Error::NotFound(vec![self.subject(name)]));
            };
            let path = update.path.replace("{id}", &entry.id.to_string());
            let body = serialize(
                update.method,
                &path,
                &json!({ WRITE_ONLY: secret.expose() }),
            )?;
            let reply = t.patch_json(&path, &body)?;
            expect_status_at(update.method, &path, &reply, &[200])?;
            lines.push(format!(
                "{}: {WRITE_ONLY} handed over from credentials (write-only; the rest of the account stays)",
                self.subject(name)
            ));
        }
        Ok(lines)
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
    /// Read only when a group names a stream profile.
    pub profiles: Vec<StreamProfile>,
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

    fn read_state(&self, t: &dyn Transport) -> Result<GroupsState, Error> {
        let wants_profiles = self
            .accounts
            .values()
            .flat_map(|g| g.values())
            .any(|set| set.contains_key(GROUP_STREAM_PROFILE));
        Ok(GroupsState {
            accounts: get_list(t, &M3U_ACCOUNTS)?,
            groups: get_list(t, &CHANNEL_GROUPS)?,
            profiles: if wants_profiles {
                get_list(t, &STREAM_PROFILES)?
            } else {
                Vec::new()
            },
        })
    }

    /// The id of the named profile, or an error that lists the names.
    fn profile_id(state: &GroupsState, name: &str) -> Result<i64, Error> {
        state
            .profiles
            .iter()
            .find(|p| p.name == name)
            .map(|p| p.id)
            .ok_or_else(|| {
                let names: Vec<&str> = state.profiles.iter().map(|p| p.name.as_str()).collect();
                Error::NotFound(vec![format!(
                    "stream profile {name} (there are: {})",
                    names.join(", ")
                )])
            })
    }

    /// The profile a membership holds: the web UI stores its id as a string,
    /// the sync reads `int(...)`, so both forms are the same id.
    fn stored_profile(membership: &GroupMembership) -> Option<i64> {
        let v = membership
            .rest
            .get("custom_properties")?
            .get(STREAM_PROFILE_ID)?;
        v.as_i64().or_else(|| v.as_str()?.trim().parse().ok())
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
        let state = self
            .read_state(t)
            .map_err(|e| Probe::NotYet(e.to_string()))?;
        for (account, groups) in &self.accounts {
            for group in groups.keys() {
                Self::locate(&state, account, group).map_err(Probe::NotYet)?;
            }
        }
        Ok(version)
    }

    fn read(&self, t: &dyn Transport) -> Result<Self::Current, Error> {
        self.read_state(t)
    }

    fn diff(&self, current: &Self::Current) -> Result<Vec<Change>, Error> {
        let mut missing = Vec::new();
        let mut absent = Vec::new();
        let mut changes = Vec::new();
        for (account, groups) in &self.accounts {
            for (group, set) in groups {
                match Self::locate(current, account, group) {
                    Ok((_, membership)) => {
                        let subject = Self::subject(account, group);
                        let mut plain = set.clone();
                        if let Some(name) = plain.remove(GROUP_STREAM_PROFILE) {
                            let name = name.as_str().unwrap_or_default();
                            let wanted = Self::profile_id(current, name)?;
                            let stored = Self::stored_profile(membership);
                            if stored != Some(wanted) {
                                let now = match stored {
                                    None => "(the default)".to_string(),
                                    Some(id) => current
                                        .profiles
                                        .iter()
                                        .find(|p| p.id == id)
                                        .map_or_else(|| format!("(id {id})"), |p| p.name.clone()),
                                };
                                changes.push(Change {
                                    subject: subject.clone(),
                                    field: GROUP_STREAM_PROFILE.to_string(),
                                    current: now,
                                    desired: name.to_string(),
                                });
                            }
                        }
                        let custom = membership.rest.get("custom_properties");
                        for key in GROUP_CUSTOM_FIELDS {
                            let Some(desired) = plain.remove(key) else {
                                continue;
                            };
                            let now = custom
                                .and_then(|c| c.get(key))
                                .cloned()
                                .unwrap_or(Value::Null);
                            if !same(&now, &desired) {
                                changes.push(Change {
                                    subject: subject.clone(),
                                    field: key.to_string(),
                                    current: shortened(&now),
                                    desired: shortened(&desired),
                                });
                            }
                        }
                        changes.extend(compare(&subject, &membership.rest, &plain, &mut missing));
                    }
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
    /// view leaves every other group of the account as it is. Each named
    /// group goes out WHOLE (`GROUP_WRITTEN`): the view replaces the row, and
    /// until v0.28.0 a write set `custom_properties` back to `{}`.
    fn write(&self, t: &dyn Transport, current: &Self::Current) -> Result<(), Error> {
        for (account, groups) in &self.accounts {
            let mut settings = Vec::new();
            let mut account_id = None;
            for (group, set) in groups {
                let (a, membership) = Self::locate(current, account, group)
                    .map_err(|why| Error::NotFound(vec![why]))?;
                account_id = Some(a.id);
                let mut entry: Map<String, Value> = GROUP_WRITTEN
                    .iter()
                    .filter_map(|f| membership.rest.get(*f).map(|v| (f.to_string(), v.clone())))
                    .collect();
                let mut plain = set.clone();
                if let Some(name) = plain.remove(GROUP_STREAM_PROFILE) {
                    let id = Self::profile_id(current, name.as_str().unwrap_or_default())?;
                    let custom = entry
                        .entry("custom_properties".to_string())
                        .or_insert_with(|| json!({}));
                    if !custom.is_object() {
                        *custom = json!({});
                    }
                    custom[STREAM_PROFILE_ID] = json!(id);
                }
                for key in GROUP_CUSTOM_FIELDS {
                    if let Some(value) = plain.remove(key) {
                        let custom = entry
                            .entry("custom_properties".to_string())
                            .or_insert_with(|| json!({}));
                        if !custom.is_object() {
                            *custom = json!({});
                        }
                        custom[key] = value;
                    }
                }
                entry.extend(plain);
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

// --- channel-epg -----------------------------------------------------------

/// The guide entry a channel shows: an EPG source by name, a channel of it by
/// `tvg_id`.
#[derive(Debug, Clone, PartialEq)]
pub struct EpgTarget {
    pub source: String,
    pub tvg_id: String,
}

/// Channels by name (their effective name), each with the guide entry it is
/// to show (design §34). Written as the channel's OVERRIDE: the channel sync
/// sets `epg_data` of an auto-created channel from its stream's tvg-id on
/// every refresh and would undo a plain assignment by the next morning; an
/// override it leaves alone.
pub struct ChannelEpg {
    pub channels: BTreeMap<String, EpgTarget>,
}

/// A named channel as `wanted` finds it: channel id, the entry it shows now,
/// the entry it should show, its name.
type Wanted = (i64, Option<i64>, i64, String);

pub struct ChannelEpgState {
    pub channels: Vec<Channel>,
    pub sources: Vec<EPGSource>,
    pub data: Vec<EPGData>,
}

impl ChannelEpg {
    fn read_state(t: &dyn Transport) -> Result<ChannelEpgState, Error> {
        Ok(ChannelEpgState {
            channels: get_list(t, &CHANNELS)?,
            sources: get_list(t, &EPG_SOURCES)?,
            data: get_list(t, &EPG_DATA)?,
        })
    }

    fn channel<'a>(state: &'a ChannelEpgState, name: &str) -> Option<&'a Channel> {
        state
            .channels
            .iter()
            .find(|c| c.effective_name.as_deref().unwrap_or(&c.name) == name)
    }

    /// The guide entry's id, or why it is not there (yet).
    fn entry(state: &ChannelEpgState, target: &EpgTarget) -> Result<i64, String> {
        let Some(source) = state.sources.iter().find(|s| s.name == target.source) else {
            let names: Vec<&str> = state.sources.iter().map(|s| s.name.as_str()).collect();
            return Err(format!(
                "EPG source {} does not exist (there are: {})",
                target.source,
                names.join(", ")
            ));
        };
        state
            .data
            .iter()
            .find(|e| {
                e.epg_source == Some(source.id) && e.tvg_id.as_deref() == Some(&target.tvg_id)
            })
            .map(|e| e.id)
            .ok_or_else(|| {
                format!(
                    "EPG source {} has no channel {} (yet)",
                    target.source, target.tvg_id
                )
            })
    }

    /// `source tvg_id` of an entry, for a change line.
    fn label(state: &ChannelEpgState, id: Option<i64>) -> String {
        let Some(id) = id else {
            return "(none)".to_string();
        };
        let Some(e) = state.data.iter().find(|e| e.id == id) else {
            return format!("(entry {id})");
        };
        let source = state
            .sources
            .iter()
            .find(|s| Some(s.id) == e.epg_source)
            .map_or_else(|| "(no source)".to_string(), |s| s.name.clone());
        format!("{source} {}", e.tvg_id.as_deref().unwrap_or("(no tvg-id)"))
    }

    /// Every named channel with the entry it should show, or what is missing.
    fn wanted(&self, state: &ChannelEpgState) -> Result<Vec<Wanted>, Vec<String>> {
        let mut out = Vec::new();
        let mut absent = Vec::new();
        for (name, target) in &self.channels {
            let Some(channel) = Self::channel(state, name) else {
                absent.push(format!("channel {name} does not exist (yet)"));
                continue;
            };
            match Self::entry(state, target) {
                Ok(id) => out.push((channel.id, channel.effective_epg_data_id, id, name.clone())),
                Err(why) => absent.push(why),
            }
        }
        if absent.is_empty() {
            Ok(out)
        } else {
            Err(absent)
        }
    }
}

impl Task for ChannelEpg {
    type Current = ChannelEpgState;

    /// Ready when every named channel exists and every named guide entry has
    /// been read from its source -- a source added in the same run is parsed
    /// asynchronously.
    fn probe(&self, t: &dyn Transport) -> Result<String, Probe> {
        let version = probe(t)?;
        let state = Self::read_state(t).map_err(|e| Probe::NotYet(e.to_string()))?;
        self.wanted(&state)
            .map_err(|absent| Probe::NotYet(absent.join("; ")))?;
        Ok(version)
    }

    fn read(&self, t: &dyn Transport) -> Result<Self::Current, Error> {
        Self::read_state(t)
    }

    fn diff(&self, current: &Self::Current) -> Result<Vec<Change>, Error> {
        let wanted = self.wanted(current).map_err(Error::NotFound)?;
        Ok(wanted
            .into_iter()
            .filter(|(_, now, want, _)| *now != Some(*want))
            .map(|(_, now, want, name)| Change {
                subject: format!("channel {name}"),
                field: "epg".to_string(),
                current: Self::label(current, now),
                desired: Self::label(current, Some(want)),
            })
            .collect())
    }

    fn notes(&self, _current: &Self::Current) -> Vec<String> {
        Vec::new()
    }

    /// One bulk PATCH with an override per channel that differs.
    fn write(&self, t: &dyn Transport, current: &Self::Current) -> Result<(), Error> {
        let wanted = self.wanted(current).map_err(Error::NotFound)?;
        let body: Vec<Value> = wanted
            .into_iter()
            .filter(|(_, now, want, _)| *now != Some(*want))
            .map(|(id, _, want, _)| json!({"id": id, "override": {"epg_data": want}}))
            .collect();
        if body.is_empty() {
            return Ok(());
        }
        let text = serialize(CHANNELS_BULK.method, CHANNELS_BULK.path, &body)?;
        let reply = t.patch_json(CHANNELS_BULK.path, &text)?;
        expect_status_at(CHANNELS_BULK.method, CHANNELS_BULK.path, &reply, &[200])
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
            secrets: BTreeMap::new(),
        }
    }

    const PASSWORD: &str = "xtream-password-never-print";
    const USERNAME: &str = "xtream-user-never-print";
    const SERVER: &str = "http://xtream.example:8080";

    /// The recorded account turned into an Xtream Codes account, as a
    /// provider's would answer: `server_url` and `username` in the clear,
    /// `password` as an empty string (write-only, `M3UAccountSerializer`).
    fn xtream_answer(server: &str, user: &str) -> String {
        with(ACCOUNTS_JSON, |v| {
            let account = &mut v[1];
            account["account_type"] = json!("XC");
            account["server_url"] = json!(server);
            account["username"] = json!(user);
            account["password"] = json!("");
        })
    }

    fn xtream(set: &[(&str, Value)]) -> Entries {
        let mut task = accounts(set);
        task.secrets = BTreeMap::from([(
            "Oeffentlich-rechtlich".to_string(),
            BTreeMap::from([
                ("server_url".to_string(), Secret::new(SERVER.to_string())),
                ("username".to_string(), Secret::new(USERNAME.to_string())),
                ("password".to_string(), Secret::new(PASSWORD.to_string())),
            ]),
        )]);
        task
    }

    fn never_printed(text: &str) {
        for secret in [PASSWORD, USERNAME, SERVER, "xtream.example", "old-user"] {
            assert!(!text.contains(secret), "a secret was printed: {text}");
        }
    }

    #[test]
    fn readable_secrets_are_compared_without_being_shown() {
        let task = xtream(&[("account_type", json!("XC"))]);
        let same = FakeTransport::default().on_get(
            M3U_ACCOUNTS.path,
            vec![ok(&xtream_answer(SERVER, USERNAME))],
        );
        assert_eq!(task.diff(&task.read(&same).unwrap()).unwrap(), vec![]);

        let other = FakeTransport::default().on_get(
            M3U_ACCOUNTS.path,
            vec![ok(&xtream_answer("http://old.xtream.example", "old-user"))],
        );
        let changes = task.diff(&task.read(&other).unwrap()).unwrap();
        let lines: Vec<String> = changes.iter().map(ToString::to_string).collect();
        assert_eq!(
            lines,
            [
                "M3U account Oeffentlich-rechtlich: server_url (another value, not shown) -> (the credential's, not shown)",
                "M3U account Oeffentlich-rechtlich: username (another value, not shown) -> (the credential's, not shown)",
            ]
        );
        never_printed(&lines.join("\n"));
    }

    #[test]
    fn a_missing_xtream_account_is_added_with_every_secret() {
        let task = xtream(&[("account_type", json!("XC")), ("is_active", json!(true))]);
        let t = FakeTransport::default()
            .on_get(M3U_ACCOUNTS.path, vec![ok("[]")])
            .on_put(vec![Step::Answer(201, "{}".to_string())]);
        let current = task.read(&t).unwrap();
        let changes = task.diff(&current).unwrap();
        never_printed(&changes[0].to_string());
        task.write(&t, &current).unwrap();
        let written = t.written.borrow();
        assert_eq!(written[0].0, "/api/m3u/accounts/");
        let sent: Value = serde_json::from_str(&written[0].1).unwrap();
        assert_eq!(
            sent,
            json!({
                "name": "Oeffentlich-rechtlich",
                "account_type": "XC",
                "is_active": true,
                "server_url": SERVER,
                "username": USERNAME,
                "password": PASSWORD,
            })
        );
    }

    #[test]
    fn a_changed_readable_secret_is_patched_with_the_password() {
        // Dispatcharr's `XCClient` logs in with all three; a new user name
        // with the old password would be a half-written account.
        let task = xtream(&[("account_type", json!("XC"))]);
        let t = FakeTransport::default()
            .on_get(
                M3U_ACCOUNTS.path,
                vec![ok(&xtream_answer(SERVER, "old-user"))],
            )
            .on_put(vec![Step::Answer(200, "{}".to_string())]);
        let current = task.read(&t).unwrap();
        task.write(&t, &current).unwrap();
        let written = t.written.borrow();
        assert_eq!(written[0].0, "/api/m3u/accounts/2/");
        let sent: Value = serde_json::from_str(&written[0].1).unwrap();
        assert_eq!(
            sent,
            json!({
                "account_type": "XC",
                "server_url": SERVER,
                "username": USERNAME,
                "password": PASSWORD,
            })
        );
    }

    #[test]
    fn the_password_is_handed_over_alone_on_every_apply() {
        let task = xtream(&[("account_type", json!("XC"))]);
        let t = FakeTransport::default()
            .on_get(
                M3U_ACCOUNTS.path,
                vec![ok(&xtream_answer(SERVER, USERNAME))],
            )
            .on_put(vec![Step::Answer(200, "{}".to_string())]);
        let current = task.read(&t).unwrap();
        assert_eq!(task.diff(&current).unwrap(), vec![]);
        let lines = task.hand_over(&t, &current).unwrap();
        assert_eq!(
            lines,
            ["M3U account Oeffentlich-rechtlich: password handed over from credentials (write-only; the rest of the account stays)"]
        );
        never_printed(&lines.join("\n"));
        let written = t.written.borrow();
        assert_eq!(written.len(), 1);
        assert_eq!(written[0].0, "/api/m3u/accounts/2/");
        let sent: Value = serde_json::from_str(&written[0].1).unwrap();
        assert_eq!(sent, json!({"password": PASSWORD}));
    }

    const EPG_URL: &str =
        "http://xtream.example:8080/xmltv.php?username=u&password=xtream-password-never-print";

    fn secret_source() -> Entries {
        Entries {
            kind: EntryKind::EpgSource,
            entries: BTreeMap::from([(
                "epgshare01-de".to_string(),
                fields(&[("source_type", json!("xmltv"))]),
            )]),
            secrets: BTreeMap::from([(
                "epgshare01-de".to_string(),
                BTreeMap::from([("url".to_string(), Secret::new(EPG_URL.to_string()))]),
            )]),
        }
    }

    #[test]
    fn a_source_url_from_a_credential_is_compared_unseen_and_nothing_is_handed_over() {
        let task = secret_source();
        let same = FakeTransport::default().on_get(
            EPG_SOURCES.path,
            vec![ok(&with(SOURCES_JSON, |v| v[0]["url"] = json!(EPG_URL)))],
        );
        let current = task.read(&same).unwrap();
        assert_eq!(task.diff(&current).unwrap(), vec![]);
        assert!(task.hand_over(&same, &current).unwrap().is_empty());
        assert!(same.written.borrow().is_empty());

        let other = FakeTransport::default()
            .on_get(EPG_SOURCES.path, vec![ok(SOURCES_JSON)])
            .on_put(vec![Step::Answer(200, "{}".to_string())]);
        let current = task.read(&other).unwrap();
        let lines: Vec<String> = task
            .diff(&current)
            .unwrap()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(
            lines,
            ["EPG source epgshare01-de: url (another value, not shown) -> (the credential's, not shown)"]
        );
        never_printed(&lines.join("\n"));
        assert!(!lines.join("\n").contains("xmltv.php"));
        task.write(&other, &current).unwrap();
        let sent: Value = serde_json::from_str(&other.written.borrow()[0].1).unwrap();
        assert_eq!(sent, json!({"source_type": "xmltv", "url": EPG_URL}));
    }

    #[test]
    fn an_account_without_secrets_hands_nothing_over() {
        let task = accounts(&[("is_active", json!(true))]);
        let t = FakeTransport::default().on_get(M3U_ACCOUNTS.path, vec![ok(ACCOUNTS_JSON)]);
        let current = task.read(&t).unwrap();
        assert!(task.hand_over(&t, &current).unwrap().is_empty());
        assert!(t.written.borrow().is_empty());
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
            secrets: BTreeMap::new(),
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
            secrets: BTreeMap::new(),
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

    /// The recorded account with a custom property on its group, as the
    /// web UI leaves one (a name filter).
    fn accounts_with_custom(custom: Value) -> String {
        with(ACCOUNTS_JSON, |v| {
            // The membership of `Öffentlich-rechtlich`, channel group 2.
            for m in v[1]["channel_groups"].as_array_mut().unwrap() {
                if m["channel_group"] == 2 {
                    m["custom_properties"] = custom.clone();
                }
            }
        })
    }

    #[test]
    fn group_settings_go_out_whole_so_the_view_keeps_what_the_spec_does_not_name() {
        // The view REPLACES a membership (`bulk_create` with
        // `update_conflicts`): a field left out falls back to its default,
        // `custom_properties` to `{}`. So the current values travel along.
        let task = group_task(&[("auto_sync_channel_end", json!(50))]);
        let t = FakeTransport::default()
            .on_get(
                M3U_ACCOUNTS.path,
                vec![ok(&accounts_with_custom(json!({"name_regex": "^DE"})))],
            )
            .on_get(CHANNEL_GROUPS.path, vec![ok(GROUPS_JSON)])
            .on_put(vec![Step::Answer(200, "{}".to_string())]);
        let current = task.read(&t).unwrap();
        task.write(&t, &current).unwrap();
        let written = t.written.borrow();
        assert_eq!(written[0].0, "/api/m3u/accounts/2/group-settings/");
        let sent: Value = serde_json::from_str(&written[0].1).unwrap();
        assert_eq!(
            sent,
            json!({"group_settings": [{
                "channel_group": 2,
                "enabled": true,
                "auto_channel_sync": true,
                "auto_sync_channel_start": 1.0,
                "auto_sync_channel_end": 50,
                "custom_properties": {"name_regex": "^DE"},
            }]})
        );
    }

    fn profile_task(profile: &str) -> Groups {
        group_task(&[("stream_profile", json!(profile))])
    }

    fn profile_transport(custom: Value) -> FakeTransport {
        FakeTransport::default()
            .on_get(M3U_ACCOUNTS.path, vec![ok(&accounts_with_custom(custom))])
            .on_get(CHANNEL_GROUPS.path, vec![ok(GROUPS_JSON)])
            .on_get(STREAM_PROFILES.path, vec![ok(PROFILES_JSON)])
            .on_put(vec![Step::Answer(200, "{}".to_string())])
    }

    #[test]
    fn a_group_stream_profile_is_named_and_set_in_its_custom_properties() {
        let task = profile_task("Proxy");
        let t = profile_transport(json!({"name_regex": "^DE"}));
        let current = task.read(&t).unwrap();
        let lines: Vec<String> = task
            .diff(&current)
            .unwrap()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(
            lines,
            ["group Öffentlich-rechtlich of M3U account Oeffentlich-rechtlich: stream_profile (the default) -> Proxy"]
        );
        task.write(&t, &current).unwrap();
        let sent: Value = serde_json::from_str(&t.written.borrow()[0].1).unwrap();
        let entry = &sent["group_settings"][0];
        assert_eq!(
            entry["custom_properties"],
            json!({"name_regex": "^DE", "stream_profile_id": 3})
        );
        assert!(
            entry.get("stream_profile").is_none(),
            "not a field of the view"
        );
    }

    #[test]
    fn a_stored_profile_id_is_the_same_as_a_string_or_a_number() {
        // The web UI stores the id as a string; the sync reads `int(...)`.
        let task = profile_task("Proxy");
        for stored in [json!("3"), json!(3)] {
            let t = profile_transport(json!({ "stream_profile_id": stored }));
            assert_eq!(task.diff(&task.read(&t).unwrap()).unwrap(), vec![]);
        }
        let t = profile_transport(json!({"stream_profile_id": "2"}));
        assert_eq!(
            task.diff(&task.read(&t).unwrap()).unwrap()[0].to_string(),
            "group Öffentlich-rechtlich of M3U account Oeffentlich-rechtlich: stream_profile streamlink -> Proxy"
        );
    }

    #[test]
    fn an_unknown_profile_names_the_profiles_there_are() {
        let task = profile_task("Direkt");
        let t = profile_transport(json!({}));
        let e = task.diff(&task.read(&t).unwrap()).unwrap_err().to_string();
        assert!(e.contains("Direkt"), "{e}");
        assert!(e.contains("Proxy"), "{e}");
    }

    #[test]
    fn name_filters_live_in_custom_properties_next_to_the_profile() {
        let task = group_task(&[
            ("stream_profile", json!("Proxy")),
            ("name_match_regex", json!(" 4K$")),
            ("name_regex_pattern", json!("^SKYGO: | 4K$")),
        ]);
        let t =
            profile_transport(json!({"stream_profile_id": "3", "name_regex_pattern": "^SKYGO: "}));
        let current = task.read(&t).unwrap();
        let lines: Vec<String> = task
            .diff(&current)
            .unwrap()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(
            lines,
            [
                "group Öffentlich-rechtlich of M3U account Oeffentlich-rechtlich: name_match_regex null -> \" 4K$\"",
                "group Öffentlich-rechtlich of M3U account Oeffentlich-rechtlich: name_regex_pattern \"^SKYGO: \" -> \"^SKYGO: | 4K$\"",
            ]
        );
        task.write(&t, &current).unwrap();
        let sent: Value = serde_json::from_str(&t.written.borrow()[0].1).unwrap();
        let entry = &sent["group_settings"][0];
        assert_eq!(
            entry["custom_properties"],
            json!({"stream_profile_id": 3, "name_match_regex": " 4K$", "name_regex_pattern": "^SKYGO: | 4K$"})
        );
        for key in ["name_match_regex", "name_regex_pattern", "stream_profile"] {
            assert!(entry.get(key).is_none(), "{key} is not a field of the view");
        }
        // Set as asked, the group is unchanged.
        let done = profile_transport(
            json!({"stream_profile_id": 3, "name_match_regex": " 4K$", "name_regex_pattern": "^SKYGO: | 4K$"}),
        );
        assert_eq!(task.diff(&task.read(&done).unwrap()).unwrap(), vec![]);
    }

    const CHANNELS_JSON: &str =
        include_str!("../../tests/fixtures/dispatcharr-0.31.0/channels-channels.json");
    const EPGDATA_JSON: &str =
        include_str!("../../tests/fixtures/dispatcharr-0.31.0/epg-epgdata-trimmed.json");

    /// The recorded source list plus source 2, the provider's guide, which
    /// was not recorded again because its URL carries a password.
    fn sources_with_provider() -> String {
        with(SOURCES_JSON, |v| {
            let mut second = v[0].clone();
            second["id"] = json!(2);
            second["name"] = json!("Xtream");
            second["url"] = json!("http://provider.example/xmltv.php");
            v.as_array_mut().unwrap().push(second);
        })
    }

    fn channel_epg(pairs: &[(&str, &str, &str)]) -> ChannelEpg {
        ChannelEpg {
            channels: pairs
                .iter()
                .map(|(c, s, t)| {
                    (
                        c.to_string(),
                        EpgTarget {
                            source: s.to_string(),
                            tvg_id: t.to_string(),
                        },
                    )
                })
                .collect(),
        }
    }

    fn epg_transport() -> FakeTransport {
        FakeTransport::default()
            .on_get(VERSION.path, vec![ok(VERSION_JSON)])
            .on_get(CHANNELS.path, vec![ok(CHANNELS_JSON)])
            .on_get(EPG_SOURCES.path, vec![ok(&sources_with_provider())])
            .on_get(EPG_DATA.path, vec![ok(EPGDATA_JSON)])
            .on_put(vec![Step::Answer(200, "[]".to_string())])
    }

    #[test]
    fn a_channel_already_on_its_guide_entry_is_unchanged() {
        let task = channel_epg(&[("Das Erste", "epgshare01-de", "Das.Erste.de")]);
        let t = epg_transport();
        assert_eq!(task.diff(&task.read(&t).unwrap()).unwrap(), vec![]);
    }

    #[test]
    fn a_channel_is_moved_to_another_guide_by_an_override() {
        let task = channel_epg(&[(
            "SKY CINEMA ACTION",
            "epgshare01-de",
            "Sky.Cinema.Action.HD.de",
        )]);
        let t = epg_transport();
        let current = task.read(&t).unwrap();
        let lines: Vec<String> = task
            .diff(&current)
            .unwrap()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(
            lines,
            ["channel SKY CINEMA ACTION: epg Xtream SkyAction.de -> epgshare01-de Sky.Cinema.Action.HD.de"]
        );
        task.write(&t, &current).unwrap();
        let written = t.written.borrow();
        assert_eq!(written[0].0, "/api/channels/channels/edit/bulk/");
        let sent: Value = serde_json::from_str(&written[0].1).unwrap();
        let wanted = current
            .data
            .iter()
            .find(|e| {
                e.tvg_id.as_deref() == Some("Sky.Cinema.Action.HD.de") && e.epg_source == Some(1)
            })
            .unwrap()
            .id;
        let channel = current
            .channels
            .iter()
            .find(|c| c.effective_name.as_deref() == Some("SKY CINEMA ACTION"))
            .unwrap()
            .id;
        assert_eq!(
            sent,
            json!([{"id": channel, "override": {"epg_data": wanted}}])
        );
    }

    #[test]
    fn readiness_waits_for_the_channel_and_for_the_guide_entry() {
        let missing_entry = channel_epg(&[("SKY CINEMA ACTION", "epgshare01-de", "Nicht.Da.de")]);
        match missing_entry.probe(&epg_transport()) {
            Err(Probe::NotYet(why)) => assert!(why.contains("Nicht.Da.de"), "{why}"),
            _ => panic!("ready without the guide entry"),
        }
        let missing_channel = channel_epg(&[("GIBT ES NICHT", "epgshare01-de", "Das.Erste.de")]);
        match missing_channel.probe(&epg_transport()) {
            Err(Probe::NotYet(why)) => assert!(why.contains("GIBT ES NICHT"), "{why}"),
            _ => panic!("ready without the channel"),
        }
        let ready = channel_epg(&[("Das Erste", "epgshare01-de", "Das.Erste.de")]);
        assert!(ready.probe(&epg_transport()).is_ok());
    }

    #[test]
    fn an_unknown_source_names_the_sources_there_are() {
        let task = channel_epg(&[("Das Erste", "epgshare01-uk", "Das.Erste.de")]);
        let e = match task.probe(&epg_transport()) {
            Err(Probe::NotYet(why)) => why,
            _ => panic!("ready with an unknown source"),
        };
        assert!(e.contains("epgshare01-uk") && e.contains("Xtream"), "{e}");
    }

    #[test]
    fn groups_without_a_profile_do_not_read_the_profiles() {
        // The recorded transport has no answer for the profile list.
        let task = group_task(&[("enabled", json!(true))]);
        let t = FakeTransport::default()
            .on_get(M3U_ACCOUNTS.path, vec![ok(ACCOUNTS_JSON)])
            .on_get(CHANNEL_GROUPS.path, vec![ok(GROUPS_JSON)]);
        assert_eq!(task.diff(&task.read(&t).unwrap()).unwrap(), vec![]);
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
