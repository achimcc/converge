//! Jellyfin: configuration documents whose fields a spec names by path
//! (design §6). The three endpoints below each replace a whole object, so
//! every write sends the object as it was read, with only the named fields
//! changed.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::{
    client::{expect_status, expect_status_at, Transport},
    endpoint::{Endpoint, Shape},
    engine::{shortened, Change, Probe, Task, HIDDEN},
    error::Error,
    paths,
    secret::Secret,
    spec::{ListItems, NamedKey, LIBRARY_IDS},
};

pub const SYSTEM_INFO: Endpoint = Endpoint {
    method: "GET",
    path: "/System/Info",
    request: None,
    response: Some(Shape::One("SystemInfo")),
};
pub const CONFIGURATION_READ: Endpoint = Endpoint {
    method: "GET",
    path: "/System/Configuration",
    request: None,
    response: Some(Shape::Document("ServerConfiguration")),
};
pub const CONFIGURATION_WRITE: Endpoint = Endpoint {
    method: "POST",
    path: "/System/Configuration",
    request: Some(Shape::Document("ServerConfiguration")),
    response: None,
};
/// A named configuration (`network`, `branding`, ...). The description
/// declares the answer as binary and the body without a schema, so only the
/// endpoints are checked here; a spec's paths are checked against the
/// component its key maps to (`NamedKey::component`, design §15).
pub const NAMED_CONFIGURATION_READ: Endpoint = Endpoint {
    method: "GET",
    path: "/System/Configuration/{key}",
    request: None,
    response: None,
};
pub const NAMED_CONFIGURATION_WRITE: Endpoint = Endpoint {
    method: "POST",
    path: "/System/Configuration/{key}",
    request: None,
    response: None,
};
pub const FOLDERS: Endpoint = Endpoint {
    method: "GET",
    path: "/Library/VirtualFolders",
    request: None,
    response: Some(Shape::List("VirtualFolderInfo")),
};
pub const LIBRARY_OPTIONS_WRITE: Endpoint = Endpoint {
    method: "POST",
    path: "/Library/VirtualFolders/LibraryOptions",
    request: Some(Shape::One("UpdateLibraryOptionsDto")),
    response: None,
};
pub const TASKS: Endpoint = Endpoint {
    method: "GET",
    path: "/ScheduledTasks",
    request: None,
    response: Some(Shape::List("TaskInfo")),
};
pub const TRIGGERS_WRITE: Endpoint = Endpoint {
    method: "POST",
    path: "/ScheduledTasks/{taskId}/Triggers",
    request: Some(Shape::List("TaskTriggerInfo")),
    response: None,
};
pub const PLUGINS: Endpoint = Endpoint {
    method: "GET",
    path: "/Plugins",
    request: None,
    response: Some(Shape::List("PluginInfo")),
};
/// The description declares `BasePluginConfiguration` without a single
/// property, and the POST without a body: plugin fields cannot be checked at
/// build time, only against the answer at runtime (design §7).
pub const PLUGIN_CONFIGURATION_READ: Endpoint = Endpoint {
    method: "GET",
    path: "/Plugins/{pluginId}/Configuration",
    request: None,
    response: Some(Shape::Opaque("BasePluginConfiguration")),
};
pub const PLUGIN_CONFIGURATION_WRITE: Endpoint = Endpoint {
    method: "POST",
    path: "/Plugins/{pluginId}/Configuration",
    request: None,
    response: None,
};
pub const USERS: Endpoint = Endpoint {
    method: "GET",
    path: "/Users",
    request: None,
    response: Some(Shape::List("UserDto")),
};
/// The whole policy of one account: Jellyfin replaces it, it does not merge
/// (design §40).
pub const USER_POLICY_WRITE: Endpoint = Endpoint {
    method: "POST",
    path: "/Users/{userId}/Policy",
    request: Some(Shape::Document("UserPolicy")),
    response: None,
};
/// The display preferences of one account under one client. Both the account
/// and the client are query parameters; `displayPreferencesId` is
/// `usersettings` for the settings a client keeps per account.
pub const DISPLAY_PREFERENCES_READ: Endpoint = Endpoint {
    method: "GET",
    path: "/DisplayPreferences/{displayPreferencesId}",
    request: None,
    response: Some(Shape::Document("DisplayPreferencesDto")),
};
pub const DISPLAY_PREFERENCES_WRITE: Endpoint = Endpoint {
    method: "POST",
    path: "/DisplayPreferences/{displayPreferencesId}",
    request: Some(Shape::Document("DisplayPreferencesDto")),
    response: None,
};
pub const ENDPOINTS: [Endpoint; 16] = [
    SYSTEM_INFO,
    CONFIGURATION_READ,
    CONFIGURATION_WRITE,
    NAMED_CONFIGURATION_READ,
    NAMED_CONFIGURATION_WRITE,
    FOLDERS,
    LIBRARY_OPTIONS_WRITE,
    TASKS,
    TRIGGERS_WRITE,
    PLUGINS,
    PLUGIN_CONFIGURATION_READ,
    PLUGIN_CONFIGURATION_WRITE,
    USERS,
    USER_POLICY_WRITE,
    DISPLAY_PREFERENCES_READ,
    DISPLAY_PREFERENCES_WRITE,
];

/// The component each task's spec paths are checked against.
pub const SERVER_CONFIGURATION: &str = "ServerConfiguration";
pub const LIBRARY_OPTIONS: &str = "LibraryOptions";
pub const TASK_TRIGGER_INFO: &str = "TaskTriggerInfo";
pub const USER_POLICY: &str = "UserPolicy";

pub fn wire_types() -> Vec<schemars::Schema> {
    vec![
        schemars::schema_for!(SystemInfo),
        schemars::schema_for!(VirtualFolderInfo),
        schemars::schema_for!(UpdateLibraryOptionsDto),
        schemars::schema_for!(TaskInfo),
        schemars::schema_for!(TaskTriggerInfo),
        schemars::schema_for!(PluginInfo),
        schemars::schema_for!(UserDto),
    ]
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "PascalCase")]
pub struct PluginInfo {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "PascalCase")]
pub struct SystemInfo {
    #[serde(default)]
    pub version: Option<String>,
}

/// `LibraryOptions` is a document: its fields are named by the spec and
/// checked by path, so it carries no wire type of its own here.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "PascalCase")]
pub struct VirtualFolderInfo {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub item_id: Option<String>,
    #[serde(default)]
    #[schemars(skip)]
    pub library_options: Option<Value>,
}

/// One account, as `GET /Users` answers. Only these three fields are kept:
/// everything else the answer carries says when somebody last watched
/// something or which picture they picked, and a change about an account
/// names nothing but its name and the fields the spec asked for.
///
/// `Policy` is a document whose fields a spec names, so it stays a `Value`
/// and is written back as it was read.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "PascalCase")]
pub struct UserDto {
    #[serde(default)]
    pub name: Option<String>,
    pub id: String,
    #[serde(default)]
    #[schemars(skip)]
    pub policy: Option<Value>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "PascalCase")]
pub struct UpdateLibraryOptionsDto {
    pub id: String,
    #[schemars(skip)]
    pub library_options: Value,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "PascalCase")]
pub struct TaskInfo {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub triggers: Option<Vec<TaskTriggerInfo>>,
}

/// Absent stays absent on the way to JSON, so a comparison sees exactly the
/// keys Jellyfin sent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "PascalCase")]
pub struct TaskTriggerInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_of_day_ticks: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval_ticks: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub day_of_week: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_runtime_ticks: Option<i64>,
}

/// Readiness: `/System/Info` answers with a version. A refused key is fatal.
pub fn probe(t: &dyn Transport) -> Result<String, Probe> {
    let reply = t
        .get(SYSTEM_INFO.path)
        .map_err(|e| Probe::NotYet(e.to_string()))?;
    match reply.status {
        200 => {}
        401 | 403 => {
            return Err(Probe::Fatal(Error::Status {
                method: SYSTEM_INFO.method,
                path: SYSTEM_INFO.path.to_string(),
                status: reply.status,
                validation: vec!["the API key was refused".to_string()],
            }))
        }
        other => return Err(Probe::NotYet(format!("HTTP {other}"))),
    }
    let info: SystemInfo = serde_json::from_str(&reply.body)
        .map_err(|e| Probe::NotYet(format!("unexpected answer: {}", crate::error::shape(&e))))?;
    info.version
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

/// The differences between `document` and the named fields. A path the
/// document does not carry goes to `missing`, never into a change.
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
                current: current.to_string(),
                desired: desired.to_string(),
            }),
            Some(_) => {}
        }
    }
    changes
}

/// `document` with the named fields replaced. Only called after
/// `compare_fields` found every path, so a failed `set` is a bug worth
/// reporting rather than hiding.
fn with_fields(
    subject: &str,
    document: &Value,
    set: &BTreeMap<String, Value>,
) -> Result<Value, Error> {
    let mut updated = document.clone();
    let missing: Vec<String> = set
        .iter()
        .filter(|(path, value)| !paths::set(&mut updated, path, (*value).clone()))
        .map(|(path, _)| format!("{subject}: {path}"))
        .collect();
    if missing.is_empty() {
        Ok(updated)
    } else {
        Err(Error::MissingField(missing))
    }
}

// --- server-configuration --------------------------------------------------

pub struct ServerConfiguration {
    pub set: BTreeMap<String, Value>,
}

impl Task for ServerConfiguration {
    type Current = Value;

    fn probe(&self, t: &dyn Transport) -> Result<String, Probe> {
        probe(t)
    }

    fn read(&self, t: &dyn Transport) -> Result<Value, Error> {
        let reply = t.get(CONFIGURATION_READ.path)?;
        expect_status(&CONFIGURATION_READ, &reply, &[200])?;
        let document: Value = decode(CONFIGURATION_READ.path, &reply.body)?;
        if !document.is_object() {
            return Err(Error::Decode {
                path: CONFIGURATION_READ.path.to_string(),
                reason: "not an object".to_string(),
            });
        }
        Ok(document)
    }

    fn diff(&self, current: &Value) -> Result<Vec<Change>, Error> {
        let mut missing = Vec::new();
        let changes = compare_fields(SERVER_CONFIGURATION, current, &self.set, &mut missing);
        if missing.is_empty() {
            Ok(changes)
        } else {
            Err(Error::MissingField(missing))
        }
    }

    fn notes(&self, _current: &Value) -> Vec<String> {
        Vec::new()
    }

    fn write(&self, t: &dyn Transport, current: &Value) -> Result<(), Error> {
        let updated = with_fields(SERVER_CONFIGURATION, current, &self.set)?;
        let body = serialize(
            CONFIGURATION_WRITE.method,
            CONFIGURATION_WRITE.path,
            &updated,
        )?;
        let reply = t.post_json(CONFIGURATION_WRITE.path, &body)?;
        expect_status(&CONFIGURATION_WRITE, &reply, &[200, 204])
    }
}

// --- named-configuration ---------------------------------------------------

/// One named configuration, replaced as a whole like `ServerConfiguration`:
/// read, change the named fields, post the object back.
///
/// For `branding` the POST lands on Jellyfin's dedicated route
/// (`/System/Configuration/Branding`, route matching ignores case), which
/// takes `BrandingOptionsDto`; the answer of the GET is the stored
/// `BrandingOptions`, whose extra `SplashscreenLocation` the DTO drops.
pub struct NamedConfiguration {
    pub key: NamedKey,
    pub set: BTreeMap<String, Value>,
}

impl NamedConfiguration {
    fn path(&self, endpoint: &Endpoint) -> String {
        endpoint.path.replace("{key}", self.key.path_segment())
    }
}

impl Task for NamedConfiguration {
    type Current = Value;

    fn probe(&self, t: &dyn Transport) -> Result<String, Probe> {
        probe(t)
    }

    fn read(&self, t: &dyn Transport) -> Result<Value, Error> {
        let path = self.path(&NAMED_CONFIGURATION_READ);
        let reply = t.get(&path)?;
        expect_status_at(NAMED_CONFIGURATION_READ.method, &path, &reply, &[200])?;
        let document: Value = decode(&path, &reply.body)?;
        if !document.is_object() {
            return Err(Error::Decode {
                path,
                reason: "not an object".to_string(),
            });
        }
        Ok(document)
    }

    fn diff(&self, current: &Value) -> Result<Vec<Change>, Error> {
        let mut missing = Vec::new();
        let changes = compare_fields(self.key.component(), current, &self.set, &mut missing);
        if missing.is_empty() {
            Ok(changes)
        } else {
            Err(Error::MissingField(missing))
        }
    }

    fn notes(&self, _current: &Value) -> Vec<String> {
        Vec::new()
    }

    fn write(&self, t: &dyn Transport, current: &Value) -> Result<(), Error> {
        let updated = with_fields(self.key.component(), current, &self.set)?;
        let path = self.path(&NAMED_CONFIGURATION_WRITE);
        let body = serialize(NAMED_CONFIGURATION_WRITE.method, &path, &updated)?;
        let reply = t.post_json(&path, &body)?;
        expect_status_at(NAMED_CONFIGURATION_WRITE.method, &path, &reply, &[200, 204])
    }
}

// --- library-options -------------------------------------------------------

/// Every library, each with a name. An empty answer is an error: no task
/// that names libraries has anything to do without them.
fn read_folders(t: &dyn Transport) -> Result<Vec<VirtualFolderInfo>, Error> {
    let path = FOLDERS.path.to_string();
    let reply = t.get(FOLDERS.path)?;
    expect_status(&FOLDERS, &reply, &[200])?;
    let folders: Vec<VirtualFolderInfo> = decode(FOLDERS.path, &reply.body)?;
    if folders.is_empty() {
        return Err(Error::EmptyList { path });
    }
    if let Some(index) = folders.iter().position(|f| f.name.is_none()) {
        return Err(Error::MissingName { path, index });
    }
    Ok(folders)
}

pub struct LibraryOptions {
    pub libraries: Vec<String>,
    pub set: BTreeMap<String, Value>,
}

impl LibraryOptions {
    fn folder<'a>(
        &self,
        current: &'a [VirtualFolderInfo],
        name: &str,
    ) -> Option<&'a VirtualFolderInfo> {
        current.iter().find(|f| f.name.as_deref() == Some(name))
    }
}

impl Task for LibraryOptions {
    type Current = Vec<VirtualFolderInfo>;

    fn probe(&self, t: &dyn Transport) -> Result<String, Probe> {
        probe(t)
    }

    fn read(&self, t: &dyn Transport) -> Result<Self::Current, Error> {
        read_folders(t)
    }

    fn diff(&self, current: &Self::Current) -> Result<Vec<Change>, Error> {
        let unknown: Vec<String> = self
            .libraries
            .iter()
            .filter(|name| self.folder(current, name).is_none())
            .map(|name| format!("library {name}"))
            .collect();
        if !unknown.is_empty() {
            return Err(Error::NotFound(unknown));
        }
        let mut missing = Vec::new();
        let mut changes = Vec::new();
        for name in &self.libraries {
            let folder = self.folder(current, name).expect("checked above");
            match &folder.library_options {
                Some(options) => {
                    changes.extend(compare_fields(name, options, &self.set, &mut missing))
                }
                None => missing.push(format!("{name}: LibraryOptions")),
            }
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

    fn write(&self, t: &dyn Transport, current: &Self::Current) -> Result<(), Error> {
        for name in &self.libraries {
            let folder = self.folder(current, name).expect("diff checked it");
            let Some(options) = &folder.library_options else {
                continue;
            };
            let mut missing = Vec::new();
            if compare_fields(name, options, &self.set, &mut missing).is_empty() {
                continue;
            }
            let id = folder
                .item_id
                .clone()
                .ok_or_else(|| Error::MissingField(vec![format!("{name}: ItemId")]))?;
            // The endpoint REPLACES LibraryOptions: the whole object goes back,
            // paths included, with only the named fields changed.
            let dto = UpdateLibraryOptionsDto {
                id,
                library_options: with_fields(name, options, &self.set)?,
            };
            let body = serialize(
                LIBRARY_OPTIONS_WRITE.method,
                LIBRARY_OPTIONS_WRITE.path,
                &dto,
            )?;
            let reply = t.post_json(LIBRARY_OPTIONS_WRITE.path, &body)?;
            expect_status(&LIBRARY_OPTIONS_WRITE, &reply, &[200, 204])?;
        }
        Ok(())
    }
}

// --- scheduled-task-triggers ------------------------------------------------

pub struct ScheduledTaskTriggers {
    pub key_prefix: String,
    pub triggers: Vec<Map<String, Value>>,
}

impl ScheduledTaskTriggers {
    fn selected<'a>(&self, current: &'a [TaskInfo]) -> Vec<&'a TaskInfo> {
        current
            .iter()
            .filter(|task| {
                task.key
                    .as_deref()
                    .is_some_and(|key| key.starts_with(&self.key_prefix))
            })
            .collect()
    }

    /// Whether `task`'s triggers are the desired list: same length, and every
    /// key a desired trigger names has that value.
    fn matches(&self, task: &TaskInfo) -> bool {
        let actual: Vec<Value> = task
            .triggers
            .iter()
            .flatten()
            .map(|t| serde_json::to_value(t).unwrap_or(Value::Null))
            .collect();
        actual.len() == self.triggers.len()
            && actual
                .iter()
                .zip(&self.triggers)
                .all(|(have, want)| want.iter().all(|(key, value)| have.get(key) == Some(value)))
    }
}

impl Task for ScheduledTaskTriggers {
    type Current = Vec<TaskInfo>;

    fn probe(&self, t: &dyn Transport) -> Result<String, Probe> {
        probe(t)
    }

    fn read(&self, t: &dyn Transport) -> Result<Self::Current, Error> {
        let reply = t.get(TASKS.path)?;
        expect_status(&TASKS, &reply, &[200])?;
        let tasks: Vec<TaskInfo> = decode(TASKS.path, &reply.body)?;
        if tasks.is_empty() {
            return Err(Error::EmptyList {
                path: TASKS.path.to_string(),
            });
        }
        Ok(tasks)
    }

    fn diff(&self, current: &Self::Current) -> Result<Vec<Change>, Error> {
        let selected = self.selected(current);
        if selected.is_empty() {
            return Err(Error::NotFound(vec![format!(
                "scheduled task with a key starting {:?}",
                self.key_prefix
            )]));
        }
        let desired = Value::Array(self.triggers.iter().cloned().map(Value::Object).collect());
        Ok(selected
            .into_iter()
            .filter(|task| !self.matches(task))
            .map(|task| Change {
                subject: task.key.clone().unwrap_or_default(),
                field: "Triggers".to_string(),
                current: json!(task.triggers.clone().unwrap_or_default()).to_string(),
                desired: desired.to_string(),
            })
            .collect())
    }

    fn notes(&self, _current: &Self::Current) -> Vec<String> {
        Vec::new()
    }

    fn write(&self, t: &dyn Transport, current: &Self::Current) -> Result<(), Error> {
        for task in self.selected(current) {
            if self.matches(task) {
                continue;
            }
            let key = task.key.clone().unwrap_or_default();
            let id = task
                .id
                .clone()
                .ok_or_else(|| Error::MissingField(vec![format!("{key}: Id")]))?;
            let path = TRIGGERS_WRITE.path.replace("{taskId}", &id);
            let body = serialize(TRIGGERS_WRITE.method, &path, &self.triggers)?;
            let reply = t.post_json(&path, &body)?;
            expect_status_at(TRIGGERS_WRITE.method, &path, &reply, &[200, 204])?;
        }
        Ok(())
    }
}

// --- plugin-configurations ---------------------------------------------------

/// One plugin to configure. `secrets` maps a path to the value read from a
/// systemd credential; the value never appears in a change or an error.
/// `lists` names entries of lists other writers share (design §8).
pub struct PluginTarget {
    pub id: String,
    pub name: String,
    pub set: BTreeMap<String, Value>,
    pub secrets: BTreeMap<String, Secret>,
    pub lists: BTreeMap<String, ListItems>,
}

/// `name=value` of a keyed item, for changes and errors.
fn item_label(path: &str, list: &ListItems, item: &Map<String, Value>) -> String {
    let key = item.get(&list.key).and_then(Value::as_str).unwrap_or("");
    format!("{path}[{}={key}]", list.key)
}

/// The entry of `entries` whose key equals the item's; the first one if the
/// service carries the key twice.
fn entry_for<'a>(
    entries: &'a [Value],
    list: &ListItems,
    item: &Map<String, Value>,
) -> Option<&'a Map<String, Value>> {
    entries
        .iter()
        .filter_map(Value::as_object)
        .find(|entry| entry.get(&list.key) == item.get(&list.key))
}

/// The differences between the keyed items and the list at `path`. A path
/// that is not a list, or a field an existing entry does not carry, goes to
/// `missing`.
fn compare_list(
    subject: &str,
    document: &Value,
    path: &str,
    list: &ListItems,
    missing: &mut Vec<String>,
) -> Vec<Change> {
    let Some(Value::Array(entries)) = paths::get(document, path) else {
        missing.push(format!("{subject}: {path} (a list)"));
        return Vec::new();
    };
    let mut changes = Vec::new();
    for item in &list.items {
        let label = item_label(path, list, item);
        let Some(entry) = entry_for(entries, list, item) else {
            changes.push(Change {
                subject: subject.to_string(),
                field: label,
                current: "(missing)".to_string(),
                desired: "(added)".to_string(),
            });
            continue;
        };
        for (field, desired) in item {
            match entry.get(field) {
                None => missing.push(format!("{subject}: {label}.{field}")),
                Some(current) if current != desired => changes.push(Change {
                    subject: subject.to_string(),
                    field: format!("{label}.{field}"),
                    current: shortened(current),
                    desired: shortened(desired),
                }),
                Some(_) => {}
            }
        }
    }
    changes
}

/// `document` with the keyed items merged into the list at `path`: fields of
/// a matching entry replaced, a missing item appended, every other entry left
/// where it was.
fn with_list(
    subject: &str,
    document: &mut Value,
    path: &str,
    list: &ListItems,
) -> Result<(), Error> {
    let Some(Value::Array(entries)) = paths::get(document, path) else {
        return Err(Error::MissingField(vec![format!(
            "{subject}: {path} (a list)"
        )]));
    };
    let mut entries = entries.clone();
    for item in &list.items {
        let found = entries
            .iter_mut()
            .filter_map(Value::as_object_mut)
            .find(|entry| entry.get(&list.key) == item.get(&list.key));
        match found {
            Some(entry) => {
                for (field, value) in item {
                    entry.insert(field.clone(), value.clone());
                }
            }
            None => entries.push(Value::Object(item.clone())),
        }
    }
    if paths::set(document, path, Value::Array(entries)) {
        Ok(())
    } else {
        Err(Error::MissingField(vec![format!("{subject}: {path}")]))
    }
}

pub struct PluginConfigurations {
    pub plugins: Vec<PluginTarget>,
}

/// A plugin's configuration as read, keyed like the spec, with the fields
/// the spec sets -- library names already turned into ids.
pub struct LoadedPlugin {
    pub id: String,
    pub configuration: Value,
    pub set: BTreeMap<String, Value>,
}

fn normalized(id: &str) -> String {
    id.replace('-', "").to_ascii_lowercase()
}

/// Whether `value` holds a `{"$library_ids": [...]}` anywhere.
fn names_libraries(value: &Value) -> bool {
    match value {
        Value::Object(map) => map.contains_key(LIBRARY_IDS) || map.values().any(names_libraries),
        Value::Array(items) => items.iter().any(names_libraries),
        _ => false,
    }
}

/// `value` with every `{"$library_ids": [names]}` replaced by the ids of
/// those libraries, in the order named (design §16). The spec has checked the
/// marker's shape; a name the service does not know, or knows twice, goes to
/// `problems` together with `at`.
fn resolve_library_ids(
    value: &Value,
    folders: &[VirtualFolderInfo],
    at: &str,
    problems: &mut Vec<String>,
) -> Value {
    match value {
        Value::Object(map) => match map.get(LIBRARY_IDS).and_then(Value::as_array) {
            Some(names) => Value::Array(
                names
                    .iter()
                    .filter_map(Value::as_str)
                    .filter_map(|name| {
                        let found: Vec<&str> = folders
                            .iter()
                            .filter(|f| f.name.as_deref() == Some(name))
                            .filter_map(|f| f.item_id.as_deref())
                            .collect();
                        match found.as_slice() {
                            [id] => Some(Value::String((*id).to_string())),
                            [] => {
                                problems.push(format!("library {name:?} ({at})"));
                                None
                            }
                            _ => {
                                problems.push(format!(
                                    "library {name:?} ({at}) is there {} times",
                                    found.len()
                                ));
                                None
                            }
                        }
                    })
                    .collect(),
            ),
            None => Value::Object(
                map.iter()
                    .map(|(k, v)| (k.clone(), resolve_library_ids(v, folders, at, problems)))
                    .collect(),
            ),
        },
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|v| resolve_library_ids(v, folders, at, problems))
                .collect(),
        ),
        other => other.clone(),
    }
}

impl PluginConfigurations {
    fn loaded<'a>(&self, current: &'a [LoadedPlugin], id: &str) -> Option<&'a LoadedPlugin> {
        current.iter().find(|p| p.id == id)
    }

    fn changes_of(
        &self,
        target: &PluginTarget,
        loaded: &LoadedPlugin,
        missing: &mut Vec<String>,
    ) -> Vec<Change> {
        let configuration = &loaded.configuration;
        let mut changes = compare_fields(&target.name, configuration, &loaded.set, missing);
        for (path, secret) in &target.secrets {
            match paths::get(configuration, path) {
                None => missing.push(format!("{}: {path}", target.name)),
                Some(Value::String(current)) if current == secret.expose() => {}
                Some(current) => changes.push(Change {
                    subject: target.name.clone(),
                    field: path.clone(),
                    current: if current == &Value::String(String::new()) {
                        "(empty)".to_string()
                    } else {
                        HIDDEN.to_string()
                    },
                    desired: format!("{HIDDEN} from its credential"),
                }),
            }
        }
        for (path, list) in &target.lists {
            changes.extend(compare_list(
                &target.name,
                configuration,
                path,
                list,
                missing,
            ));
        }
        changes
    }
}

impl Task for PluginConfigurations {
    type Current = Vec<LoadedPlugin>;

    fn probe(&self, t: &dyn Transport) -> Result<String, Probe> {
        probe(t)
    }

    fn read(&self, t: &dyn Transport) -> Result<Self::Current, Error> {
        let reply = t.get(PLUGINS.path)?;
        expect_status(&PLUGINS, &reply, &[200])?;
        let installed: Vec<PluginInfo> = decode(PLUGINS.path, &reply.body)?;
        let mut problems = Vec::new();
        for target in &self.plugins {
            let found = installed
                .iter()
                .find(|p| p.id.as_deref().map(normalized) == Some(normalized(&target.id)));
            match found.and_then(|p| p.name.as_deref()) {
                None => problems.push(format!("plugin {} ({})", target.name, target.id)),
                Some(name) if name != target.name => problems.push(format!(
                    "plugin {} is called {name:?}, the spec expects {:?}",
                    target.id, target.name
                )),
                Some(_) => {}
            }
        }
        if !problems.is_empty() {
            return Err(Error::NotFound(problems));
        }
        // Library ids exist only once the libraries do, so they are looked
        // up on every read -- and only when a spec names a library at all.
        let folders = if self
            .plugins
            .iter()
            .any(|p| p.set.values().any(names_libraries))
        {
            read_folders(t)?
        } else {
            Vec::new()
        };
        let mut sets = Vec::new();
        for target in &self.plugins {
            let set: BTreeMap<String, Value> = target
                .set
                .iter()
                .map(|(path, value)| {
                    let at = format!("{}: {path}", target.name);
                    let resolved = resolve_library_ids(value, &folders, &at, &mut problems);
                    (path.clone(), resolved)
                })
                .collect();
            sets.push(set);
        }
        if !problems.is_empty() {
            return Err(Error::NotFound(problems));
        }
        let mut loaded = Vec::new();
        for (target, set) in self.plugins.iter().zip(sets) {
            let path = PLUGIN_CONFIGURATION_READ
                .path
                .replace("{pluginId}", &target.id);
            let reply = t.get(&path)?;
            expect_status_at(PLUGIN_CONFIGURATION_READ.method, &path, &reply, &[200])?;
            let configuration: Value = decode(&path, &reply.body)?;
            if !configuration.is_object() {
                return Err(Error::Decode {
                    path,
                    reason: "not an object".to_string(),
                });
            }
            loaded.push(LoadedPlugin {
                id: target.id.clone(),
                configuration,
                set,
            });
        }
        Ok(loaded)
    }

    fn diff(&self, current: &Self::Current) -> Result<Vec<Change>, Error> {
        let mut missing = Vec::new();
        let mut changes = Vec::new();
        for target in &self.plugins {
            let Some(loaded) = self.loaded(current, &target.id) else {
                missing.push(format!("{}: configuration", target.name));
                continue;
            };
            changes.extend(self.changes_of(target, loaded, &mut missing));
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

    fn write(&self, t: &dyn Transport, current: &Self::Current) -> Result<(), Error> {
        for target in &self.plugins {
            let Some(loaded) = self.loaded(current, &target.id) else {
                continue;
            };
            let mut missing = Vec::new();
            if self.changes_of(target, loaded, &mut missing).is_empty() {
                continue;
            }
            // The whole configuration goes back: Ratings alone carries more
            // than a hundred settings made in the web UI.
            let mut updated = with_fields(&target.name, &loaded.configuration, &loaded.set)?;
            for (path, secret) in &target.secrets {
                if !paths::set(
                    &mut updated,
                    path,
                    Value::String(secret.expose().to_string()),
                ) {
                    return Err(Error::MissingField(vec![format!(
                        "{}: {path}",
                        target.name
                    )]));
                }
            }
            for (path, list) in &target.lists {
                with_list(&target.name, &mut updated, path, list)?;
            }
            let path = PLUGIN_CONFIGURATION_WRITE
                .path
                .replace("{pluginId}", &target.id);
            let body = serialize(PLUGIN_CONFIGURATION_WRITE.method, &path, &updated)?;
            let reply = t.post_json(&path, &body)?;
            expect_status_at(
                PLUGIN_CONFIGURATION_WRITE.method,
                &path,
                &reply,
                &[200, 204],
            )?;
        }
        Ok(())
    }
}

// --- accounts: policies and display preferences (design §40) ---------------

/// `displayPreferencesId` of the settings a client keeps per account.
pub const USER_SETTINGS: &str = "usersettings";

/// The key of the string map inside a display preferences document.
const CUSTOM_PREFS: &str = "CustomPrefs";

/// Shown where a `CustomPrefs` key is not there at all. Jellyfin's map is
/// sparse by design, so this is a change, not an error -- unlike a policy
/// field, which every policy carries.
const MISSING: &str = "(missing)";

impl UserDto {
    /// How a change and an error name the account. An account whose `Name`
    /// the answer omits cannot be named by a spec, but a change about it
    /// must still say which one it is.
    fn label(&self) -> String {
        format!("account {}", self.name.as_deref().unwrap_or(&self.id))
    }

    /// The account's policy as an object. Jellyfin declares it nullable, and
    /// a spec's fields have nothing to be compared against without it.
    fn policy(&self) -> Result<&Map<String, Value>, Error> {
        self.policy
            .as_ref()
            .and_then(Value::as_object)
            .ok_or_else(|| Error::MissingField(vec![format!("{}: Policy", self.label())]))
    }
}

/// A status these two tasks do not accept. Nothing from the body reaches the
/// message: every answer here is a document about accounts.
fn refuse_account(method: &'static str, path: &str, status: u16) -> Error {
    Error::Status {
        method,
        path: path.to_string(),
        status,
        validation: match status {
            401 => vec!["the API key was refused".to_string()],
            403 => vec!["the key's account may not do this".to_string()],
            _ => Vec::new(),
        },
    }
}

/// Every account the service holds. An empty list is an error: a check over
/// no account would be green for the wrong reason.
fn read_users(t: &dyn Transport) -> Result<Vec<UserDto>, Error> {
    let reply = t.get(USERS.path)?;
    if reply.status != 200 {
        return Err(refuse_account(USERS.method, USERS.path, reply.status));
    }
    let users: Vec<UserDto> = decode(USERS.path, &reply.body)?;
    if users.is_empty() {
        return Err(Error::EmptyList {
            path: USERS.path.to_string(),
        });
    }
    Ok(users)
}

/// The names an `accounts` map holds that the service does not hold yet, in
/// the order the spec names them. Not an error: the host derives the
/// administrators from an authentik group, and a Jellyfin account comes into
/// being at its owner's first sign-in -- a new member would otherwise keep
/// the unit red until then. Nothing is written for such a name, and every
/// other account is reconciled as usual (design §40).
fn absent_accounts<'a>(users: &[UserDto], named: impl Iterator<Item = &'a String>) -> Vec<String> {
    let have: Vec<&str> = users.iter().filter_map(|u| u.name.as_deref()).collect();
    named
        .filter(|n| !have.contains(&n.as_str()))
        .cloned()
        .collect()
}

/// How a note names an account the service does not hold yet.
fn absent_note(name: &str) -> String {
    format!("account {name}: not on the service yet — skipped")
}

/// `all` with the account's own entries on top.
fn merged<T: Clone>(
    all: &BTreeMap<String, T>,
    accounts: &BTreeMap<String, BTreeMap<String, T>>,
    user: &UserDto,
) -> BTreeMap<String, T> {
    let mut set = all.clone();
    if let Some(own) = user.name.as_deref().and_then(|n| accounts.get(n)) {
        set.extend(own.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    set
}

// --- user-policies ----------------------------------------------------------

/// The policy fields every account must carry, and the ones single accounts
/// carry instead. Jellyfin replaces the whole policy on a write, so every
/// field converge does not name goes back as it was read (design §40).
pub struct UserPolicies {
    pub all: BTreeMap<String, Value>,
    pub accounts: BTreeMap<String, BTreeMap<String, Value>>,
}

impl UserPolicies {
    fn set_for(&self, user: &UserDto) -> BTreeMap<String, Value> {
        merged(&self.all, &self.accounts, user)
    }
}

impl Task for UserPolicies {
    type Current = Vec<UserDto>;

    fn probe(&self, t: &dyn Transport) -> Result<String, Probe> {
        probe(t)
    }

    fn read(&self, t: &dyn Transport) -> Result<Self::Current, Error> {
        read_users(t)
    }

    fn diff(&self, current: &Self::Current) -> Result<Vec<Change>, Error> {
        let mut changes = Vec::new();
        let mut missing = Vec::new();
        for user in current {
            let set = self.set_for(user);
            if set.is_empty() {
                continue;
            }
            let policy = user.policy()?;
            for (field, desired) in &set {
                match policy.get(field) {
                    // Unlike a custom pref, a policy field is one Jellyfin
                    // always answers with: a name it does not know is a
                    // misspelling, not something to add.
                    None => missing.push(format!("{}: Policy.{field}", user.label())),
                    Some(now) if now != desired => changes.push(Change {
                        subject: user.label(),
                        field: format!("Policy.{field}"),
                        current: shortened(now),
                        desired: shortened(desired),
                    }),
                    Some(_) => {}
                }
            }
        }
        if missing.is_empty() {
            Ok(changes)
        } else {
            Err(Error::MissingField(missing))
        }
    }

    fn notes(&self, current: &Self::Current) -> Vec<String> {
        absent_accounts(current, self.accounts.keys())
            .iter()
            .map(|name| absent_note(name))
            .collect()
    }

    /// One POST per account that differs, each with that account's whole
    /// policy as it was read and only the named fields changed.
    fn write(&self, t: &dyn Transport, current: &Self::Current) -> Result<(), Error> {
        for user in current {
            let set = self.set_for(user);
            let policy = user.policy()?;
            if set.is_empty() || set.iter().all(|(f, d)| policy.get(f) == Some(d)) {
                continue;
            }
            let mut updated = policy.clone();
            for (field, value) in &set {
                if !updated.contains_key(field) {
                    return Err(Error::MissingField(vec![format!(
                        "{}: Policy.{field}",
                        user.label()
                    )]));
                }
                updated.insert(field.clone(), value.clone());
            }
            let path = USER_POLICY_WRITE.path.replace("{userId}", &user.id);
            let body = serialize(USER_POLICY_WRITE.method, &path, &Value::Object(updated))?;
            let reply = t.post_json(&path, &body)?;
            if !matches!(reply.status, 200 | 204) {
                return Err(refuse_account(
                    USER_POLICY_WRITE.method,
                    &path,
                    reply.status,
                ));
            }
        }
        Ok(())
    }
}

// --- display-preferences ----------------------------------------------------

/// The path one account's display preferences live under. Both parameters
/// are checked by the spec (`client`) or are ids the service handed out
/// (`user_id`), so neither needs escaping here.
fn preferences_path(user_id: &str, client: &str) -> String {
    format!(
        "{}?userId={user_id}&client={client}",
        DISPLAY_PREFERENCES_READ
            .path
            .replace("{displayPreferencesId}", USER_SETTINGS)
    )
}

/// One account and the display preferences document it answered with.
pub struct AccountPreferences {
    user: UserDto,
    document: Value,
}

/// What `read` found: the accounts the spec has something to say about, and
/// the names it names that the service does not hold yet. The second list is
/// kept because `read` asks for no document for such a name, so nothing
/// later could tell it apart from an account the spec is silent about.
pub struct AccountDocuments {
    accounts: Vec<AccountPreferences>,
    absent: Vec<String>,
}

/// `CustomPrefs` keys every account must carry, and the ones single accounts
/// carry instead. The document is written as a whole, like a policy.
pub struct DisplayPreferences {
    pub client: String,
    pub all: BTreeMap<String, String>,
    pub accounts: BTreeMap<String, BTreeMap<String, String>>,
}

impl DisplayPreferences {
    fn set_for(&self, user: &UserDto) -> BTreeMap<String, String> {
        merged(&self.all, &self.accounts, user)
    }
}

impl Task for DisplayPreferences {
    type Current = AccountDocuments;

    fn probe(&self, t: &dyn Transport) -> Result<String, Probe> {
        probe(t)
    }

    /// The accounts first, then one request per account the spec has
    /// something to say about.
    fn read(&self, t: &dyn Transport) -> Result<Self::Current, Error> {
        let users = read_users(t)?;
        let absent = absent_accounts(&users, self.accounts.keys());
        let mut accounts = Vec::new();
        for user in users {
            if self.set_for(&user).is_empty() {
                continue;
            }
            let path = preferences_path(&user.id, &self.client);
            let reply = t.get(&path)?;
            if reply.status != 200 {
                return Err(refuse_account(
                    DISPLAY_PREFERENCES_READ.method,
                    &path,
                    reply.status,
                ));
            }
            let document: Value = decode(&path, &reply.body)?;
            if !document.is_object() {
                return Err(Error::Decode {
                    path,
                    reason: "not an object".to_string(),
                });
            }
            accounts.push(AccountPreferences { user, document });
        }
        Ok(AccountDocuments { accounts, absent })
    }

    fn diff(&self, current: &Self::Current) -> Result<Vec<Change>, Error> {
        let mut changes = Vec::new();
        for account in &current.accounts {
            let prefs = account.document.get(CUSTOM_PREFS);
            for (key, desired) in &self.set_for(&account.user) {
                let desired = Value::String(desired.clone());
                let now = prefs.and_then(|p| p.get(key));
                if now == Some(&desired) {
                    continue;
                }
                changes.push(Change {
                    subject: account.user.label(),
                    field: format!("{CUSTOM_PREFS}.{key}"),
                    // A key the map does not carry is normal: Jellyfin
                    // stores only what a client has written.
                    current: now.map_or_else(|| MISSING.to_string(), shortened),
                    desired: shortened(&desired),
                });
            }
        }
        Ok(changes)
    }

    fn notes(&self, current: &Self::Current) -> Vec<String> {
        current.absent.iter().map(|n| absent_note(n)).collect()
    }

    fn write(&self, t: &dyn Transport, current: &Self::Current) -> Result<(), Error> {
        for account in &current.accounts {
            let set = self.set_for(&account.user);
            let prefs = account.document.get(CUSTOM_PREFS);
            if set
                .iter()
                .all(|(k, v)| prefs.and_then(|p| p.get(k)) == Some(&Value::String(v.clone())))
            {
                continue;
            }
            let mut updated = account.document.clone();
            let map = updated
                .as_object_mut()
                .expect("read refuses a document that is not an object")
                .entry(CUSTOM_PREFS)
                .or_insert_with(|| Value::Object(Map::new()));
            let Some(map) = map.as_object_mut() else {
                return Err(Error::Decode {
                    path: preferences_path(&account.user.id, &self.client),
                    reason: format!("{CUSTOM_PREFS} is not an object"),
                });
            };
            for (key, value) in &set {
                map.insert(key.clone(), Value::String(value.clone()));
            }
            let path = preferences_path(&account.user.id, &self.client);
            let body = serialize(DISPLAY_PREFERENCES_WRITE.method, &path, &updated)?;
            let reply = t.post_json(&path, &body)?;
            if !matches!(reply.status, 200 | 204) {
                return Err(refuse_account(
                    DISPLAY_PREFERENCES_WRITE.method,
                    &path,
                    reply.status,
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{ok, FakeTransport, Step};

    const INFO: &str = include_str!("../../tests/fixtures/jellyfin-10.11.11/system-info.json");
    const CONFIG: &str =
        include_str!("../../tests/fixtures/jellyfin-10.11.11/system-configuration.json");
    const FOLDERS_JSON: &str =
        include_str!("../../tests/fixtures/jellyfin-10.11.11/virtual-folders.json");
    const TASKS_JSON: &str =
        include_str!("../../tests/fixtures/jellyfin-10.11.11/scheduled-tasks.json");
    const NETWORK: &str =
        include_str!("../../tests/fixtures/jellyfin-10.11.11/system-configuration-network.json");
    const BRANDING: &str =
        include_str!("../../tests/fixtures/jellyfin-10.11.11/system-configuration-branding.json");

    const LIVETV: &str =
        include_str!("../../tests/fixtures/jellyfin-10.11.11/system-configuration-livetv.json");

    fn fields(pairs: &[(&str, Value)]) -> BTreeMap<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    fn trickplay() -> BTreeMap<String, Value> {
        fields(&[
            ("TrickplayOptions.EnableKeyFrameOnlyExtraction", json!(true)),
            ("TrickplayOptions.EnableHwAcceleration", json!(true)),
        ])
    }

    fn merge_triggers() -> ScheduledTaskTriggers {
        ScheduledTaskTriggers {
            key_prefix: "Merge".to_string(),
            triggers: vec![
                json!({"Type": "DailyTrigger", "TimeOfDayTicks": 198000000000i64})
                    .as_object()
                    .unwrap()
                    .clone(),
            ],
        }
    }

    fn with(body: &str, edit: impl FnOnce(&mut Value)) -> String {
        let mut v: Value = serde_json::from_str(body).unwrap();
        edit(&mut v);
        v.to_string()
    }

    #[test]
    fn probe_reads_the_version() {
        let t = FakeTransport::default().on_get(SYSTEM_INFO.path, vec![ok(INFO)]);
        assert_eq!(probe(&t).ok().unwrap(), "10.11.11");
        let refused =
            FakeTransport::default().on_get(SYSTEM_INFO.path, vec![Step::Answer(401, "".into())]);
        assert!(matches!(probe(&refused), Err(Probe::Fatal(_))));
    }

    #[test]
    fn server_configuration_recorded_state_is_already_desired() {
        let task = ServerConfiguration { set: trickplay() };
        let t = FakeTransport::default().on_get(CONFIGURATION_READ.path, vec![ok(CONFIG)]);
        assert_eq!(task.diff(&task.read(&t).unwrap()).unwrap(), vec![]);
    }

    #[test]
    fn server_configuration_changes_one_field_and_keeps_the_rest() {
        let off = with(CONFIG, |v| {
            v["TrickplayOptions"]["EnableHwAcceleration"] = json!(false)
        });
        let task = ServerConfiguration { set: trickplay() };
        let t = FakeTransport::default()
            .on_get(CONFIGURATION_READ.path, vec![ok(&off)])
            .on_put(vec![Step::Answer(204, String::new())]);
        let current = task.read(&t).unwrap();
        let changes = task.diff(&current).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(
            changes[0].to_string(),
            "ServerConfiguration: TrickplayOptions.EnableHwAcceleration false -> true"
        );
        task.write(&t, &current).unwrap();
        let written = t.written.borrow();
        assert_eq!(written[0].0, "/System/Configuration");
        let sent: Value = serde_json::from_str(&written[0].1).unwrap();
        assert_eq!(sent, serde_json::from_str::<Value>(CONFIG).unwrap());
    }

    #[test]
    fn named_configuration_reads_and_writes_the_key_it_names() {
        let task = NamedConfiguration {
            key: NamedKey::Network,
            set: fields(&[
                ("KnownProxies", json!(["10.0.20.11"])),
                ("EnableUPnP", json!(false)),
            ]),
        };
        let t = FakeTransport::default()
            .on_get("/System/Configuration/network", vec![ok(NETWORK)])
            .on_put(vec![Step::Answer(204, String::new())]);
        let current = task.read(&t).unwrap();
        let changes = task.diff(&current).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(
            changes[0].to_string(),
            r#"NetworkConfiguration: KnownProxies [] -> ["10.0.20.11"]"#
        );
        task.write(&t, &current).unwrap();
        let written = t.written.borrow();
        assert_eq!(written[0].0, "/System/Configuration/network");
        let sent: Value = serde_json::from_str(&written[0].1).unwrap();
        let expected = with(NETWORK, |v| v["KnownProxies"] = json!(["10.0.20.11"]));
        assert_eq!(sent, serde_json::from_str::<Value>(&expected).unwrap());
    }

    #[test]
    fn named_configuration_live_tv_replaces_the_tuner_list_as_a_whole() {
        let tuners = json!([{
            "Id": "oer", "Type": "m3u", "Url": "/nix/store/x-kanaele.m3u",
            "AllowStreamSharing": true, "TunerCount": 0
        }]);
        let task = NamedConfiguration {
            key: NamedKey::LiveTv,
            set: fields(&[("TunerHosts", tuners.clone())]),
        };
        let t = FakeTransport::default()
            .on_get("/System/Configuration/livetv", vec![ok(LIVETV)])
            .on_put(vec![Step::Answer(204, String::new())]);
        let current = task.read(&t).unwrap();
        let changes = task.diff(&current).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].subject, "LiveTvOptions");
        assert_eq!(changes[0].field, "TunerHosts");
        task.write(&t, &current).unwrap();
        let written = t.written.borrow();
        assert_eq!(written[0].0, "/System/Configuration/livetv");
        let sent: Value = serde_json::from_str(&written[0].1).unwrap();
        let expected = with(LIVETV, |v| v["TunerHosts"] = tuners.clone());
        assert_eq!(sent, serde_json::from_str::<Value>(&expected).unwrap());

        // Read back with the list in place, nothing is left to do.
        let t =
            FakeTransport::default().on_get("/System/Configuration/livetv", vec![ok(&expected)]);
        assert_eq!(task.diff(&task.read(&t).unwrap()).unwrap(), vec![]);
    }

    #[test]
    fn named_configuration_recorded_branding_is_already_desired() {
        let recorded: Value = serde_json::from_str(BRANDING).unwrap();
        let task = NamedConfiguration {
            key: NamedKey::Branding,
            set: fields(&[
                ("LoginDisclaimer", recorded["LoginDisclaimer"].clone()),
                ("SplashscreenEnabled", json!(false)),
            ]),
        };
        let t =
            FakeTransport::default().on_get("/System/Configuration/branding", vec![ok(BRANDING)]);
        assert_eq!(task.diff(&task.read(&t).unwrap()).unwrap(), vec![]);
    }

    #[test]
    fn named_configuration_a_null_jellyfin_omitted_is_missing_not_added() {
        let task = NamedConfiguration {
            key: NamedKey::Branding,
            set: fields(&[("CustomCss", json!(""))]),
        };
        let t =
            FakeTransport::default().on_get("/System/Configuration/branding", vec![ok(BRANDING)]);
        let err = task
            .diff(&task.read(&t).unwrap())
            .err()
            .unwrap()
            .to_string();
        assert_eq!(
            err,
            "the answer has no such field: BrandingOptionsDto: CustomCss"
        );
    }

    #[test]
    fn a_field_the_answer_lacks_is_an_error_not_an_addition() {
        let task = ServerConfiguration {
            set: fields(&[("TrickplayOptions.EnableHwAccel", json!(true))]),
        };
        let t = FakeTransport::default().on_get(CONFIGURATION_READ.path, vec![ok(CONFIG)]);
        let err = task
            .diff(&task.read(&t).unwrap())
            .err()
            .unwrap()
            .to_string();
        assert_eq!(
            err,
            "the answer has no such field: ServerConfiguration: TrickplayOptions.EnableHwAccel"
        );
    }

    #[test]
    fn library_options_recorded_state_is_already_desired() {
        let task = LibraryOptions {
            libraries: vec!["Filme".into(), "Serien".into()],
            set: fields(&[("EnableTrickplayImageExtraction", json!(true))]),
        };
        let t = FakeTransport::default().on_get(FOLDERS.path, vec![ok(FOLDERS_JSON)]);
        assert_eq!(task.diff(&task.read(&t).unwrap()).unwrap(), vec![]);
    }

    #[test]
    fn library_options_posts_only_the_differing_library_with_all_its_options() {
        let off = with(FOLDERS_JSON, |v| {
            for folder in v.as_array_mut().unwrap() {
                if folder["Name"] == "Serien" {
                    folder["LibraryOptions"]["EnableTrickplayImageExtraction"] = json!(false);
                }
            }
        });
        let task = LibraryOptions {
            libraries: vec!["Filme".into(), "Serien".into()],
            set: fields(&[("EnableTrickplayImageExtraction", json!(true))]),
        };
        let t = FakeTransport::default()
            .on_get(FOLDERS.path, vec![ok(&off)])
            .on_put(vec![Step::Answer(204, String::new())]);
        let current = task.read(&t).unwrap();
        let changes = task.diff(&current).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(
            changes[0].to_string(),
            "Serien: EnableTrickplayImageExtraction false -> true"
        );
        task.write(&t, &current).unwrap();
        let written = t.written.borrow();
        assert_eq!(written.len(), 1, "Filme already had it");
        let sent: Value = serde_json::from_str(&written[0].1).unwrap();
        let recorded: Value = serde_json::from_str(FOLDERS_JSON).unwrap();
        let serien = recorded
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["Name"] == "Serien")
            .unwrap();
        assert_eq!(sent["Id"], serien["ItemId"]);
        // Byte for byte the recorded options, paths included: the endpoint
        // replaces the object, so nothing may go missing on the way back.
        assert_eq!(sent["LibraryOptions"], serien["LibraryOptions"]);
    }

    #[test]
    fn an_unknown_library_is_not_found() {
        let task = LibraryOptions {
            libraries: vec!["Filme".into(), "Hörspiele".into()],
            set: fields(&[("EnableTrickplayImageExtraction", json!(true))]),
        };
        let t = FakeTransport::default().on_get(FOLDERS.path, vec![ok(FOLDERS_JSON)]);
        let err = task
            .diff(&task.read(&t).unwrap())
            .err()
            .unwrap()
            .to_string();
        assert_eq!(err, "not found on the service: library Hörspiele");
    }

    #[test]
    fn triggers_recorded_state_is_already_desired() {
        let task = merge_triggers();
        let t = FakeTransport::default().on_get(TASKS.path, vec![ok(TASKS_JSON)]);
        assert_eq!(task.diff(&task.read(&t).unwrap()).unwrap(), vec![]);
    }

    #[test]
    fn a_task_with_a_different_trigger_gets_the_desired_list() {
        let changed = with(TASKS_JSON, |v| {
            for task in v.as_array_mut().unwrap() {
                if task["Key"] == "MergeMoviesTask" {
                    task["Triggers"] =
                        json!([{"Type": "IntervalTrigger", "IntervalTicks": 864000000000i64}]);
                }
            }
        });
        let task = merge_triggers();
        let t = FakeTransport::default()
            .on_get(TASKS.path, vec![ok(&changed)])
            .on_put(vec![Step::Answer(204, String::new())]);
        let current = task.read(&t).unwrap();
        let changes = task.diff(&current).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].subject, "MergeMoviesTask");
        task.write(&t, &current).unwrap();
        let written = t.written.borrow();
        let recorded: Value = serde_json::from_str(TASKS_JSON).unwrap();
        let id = recorded
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["Key"] == "MergeMoviesTask")
            .unwrap()["Id"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(written[0].0, format!("/ScheduledTasks/{id}/Triggers"));
        assert_eq!(
            serde_json::from_str::<Value>(&written[0].1).unwrap(),
            json!([{"Type": "DailyTrigger", "TimeOfDayTicks": 198000000000i64}])
        );
    }

    const PLUGINS_JSON: &str = include_str!("../../tests/fixtures/jellyfin-10.11.11/plugins.json");
    const OSCARS: &str = "c531afa3de204055aca5a7cc43adf783";
    const MEDIATHEK: &str = "a31b415a5264419db1528c8192a54994";

    fn plugin_file(id: &str) -> String {
        std::fs::read_to_string(format!(
            "{}/tests/fixtures/jellyfin-10.11.11/plugins/{id}.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    fn plugins_transport(oscars: &str, mediathek: &str) -> FakeTransport {
        FakeTransport::default()
            .on_get(PLUGINS.path, vec![ok(PLUGINS_JSON)])
            .on_get(
                &format!("/Plugins/{OSCARS}/Configuration"),
                vec![ok(oscars)],
            )
            .on_get(
                &format!("/Plugins/{MEDIATHEK}/Configuration"),
                vec![ok(mediathek)],
            )
    }

    /// The fixtures carry `<masked>` where the key was; a test credential
    /// with exactly that value is "already set".
    fn targets(oscars_key: &str) -> PluginConfigurations {
        PluginConfigurations {
            plugins: vec![
                PluginTarget {
                    id: OSCARS.to_string(),
                    name: "Jellyfin Oscars".to_string(),
                    set: BTreeMap::new(),
                    secrets: [(
                        "OmdbApiKey".to_string(),
                        Secret::new(oscars_key.to_string()),
                    )]
                    .into_iter()
                    .collect(),
                    lists: BTreeMap::new(),
                },
                PluginTarget {
                    id: MEDIATHEK.to_string(),
                    name: "Mediathek Downloader".to_string(),
                    set: fields(&[
                        ("Network.AllowUnknownDomains", json!(false)),
                        ("WizardCompleted", json!(true)),
                    ]),
                    secrets: BTreeMap::new(),
                    lists: BTreeMap::new(),
                },
            ],
        }
    }

    #[test]
    fn plugins_recorded_state_is_already_desired() {
        let task = targets("<masked>");
        let t = plugins_transport(&plugin_file(OSCARS), &plugin_file(MEDIATHEK));
        assert_eq!(task.diff(&task.read(&t).unwrap()).unwrap(), vec![]);
    }

    #[test]
    fn a_changed_secret_is_written_but_never_shown() {
        let key = "s3cr3t-omdb-value";
        let task = targets(key);
        let t = plugins_transport(&plugin_file(OSCARS), &plugin_file(MEDIATHEK))
            .on_put(vec![Step::Answer(204, String::new())]);
        let current = task.read(&t).unwrap();
        let changes = task.diff(&current).unwrap();
        assert_eq!(changes.len(), 1);
        let shown = changes[0].to_string();
        assert_eq!(
            shown,
            "Jellyfin Oscars: OmdbApiKey (hidden) -> (hidden) from its credential"
        );
        assert!(!format!("{changes:?}").contains(key));
        assert!(
            !format!("{changes:?}").contains("<masked>"),
            "not even the old value"
        );

        task.write(&t, &current).unwrap();
        let written = t.written.borrow();
        assert_eq!(written.len(), 1, "Mediathek already matched");
        assert_eq!(written[0].0, format!("/Plugins/{OSCARS}/Configuration"));
        let sent: Value = serde_json::from_str(&written[0].1).unwrap();
        let mut expected: Value = serde_json::from_str(&plugin_file(OSCARS)).unwrap();
        expected["OmdbApiKey"] = json!(key);
        assert_eq!(sent, expected, "every other setting goes back untouched");
    }

    #[test]
    fn a_plain_field_change_keeps_the_rest_of_the_configuration() {
        let mut mediathek: Value = serde_json::from_str(&plugin_file(MEDIATHEK)).unwrap();
        mediathek["Network"]["AllowUnknownDomains"] = json!(true);
        let task = targets("<masked>");
        let t = plugins_transport(&plugin_file(OSCARS), &mediathek.to_string())
            .on_put(vec![Step::Answer(204, String::new())]);
        let current = task.read(&t).unwrap();
        assert_eq!(
            task.diff(&current).unwrap()[0].to_string(),
            "Mediathek Downloader: Network.AllowUnknownDomains true -> false"
        );
        task.write(&t, &current).unwrap();
        let sent: Value = serde_json::from_str(&t.written.borrow()[0].1).unwrap();
        assert_eq!(
            sent,
            serde_json::from_str::<Value>(&plugin_file(MEDIATHEK)).unwrap()
        );
    }

    #[test]
    fn a_wrong_plugin_id_or_name_is_not_found() {
        let mut task = targets("<masked>");
        task.plugins[0].name = "Oscars".to_string();
        let t = plugins_transport(&plugin_file(OSCARS), &plugin_file(MEDIATHEK));
        let err = task.read(&t).err().unwrap().to_string();
        assert!(
            err.contains(r#"is called "Jellyfin Oscars", the spec expects "Oscars""#),
            "{err}"
        );

        let mut task = targets("<masked>");
        task.plugins[0].id = "00000000000000000000000000000000".to_string();
        let err = task.read(&t).err().unwrap().to_string();
        assert!(
            err.starts_with("not found on the service: plugin Jellyfin Oscars"),
            "{err}"
        );
    }

    #[test]
    fn a_secret_field_the_plugin_lacks_is_an_error_without_the_value() {
        let mut task = targets("s3cr3t");
        let secret = task.plugins[0].secrets.remove("OmdbApiKey").unwrap();
        task.plugins[0]
            .secrets
            .insert("OmdbKey".to_string(), secret);
        let t = plugins_transport(&plugin_file(OSCARS), &plugin_file(MEDIATHEK));
        let err = task
            .diff(&task.read(&t).unwrap())
            .err()
            .unwrap()
            .to_string();
        assert_eq!(
            err,
            "the answer has no such field: Jellyfin Oscars: OmdbKey"
        );
    }

    const INJECTOR: &str = "f5a34f7b2e8a4e6aa7223a216a81b374";

    fn injector(items: Vec<Value>) -> PluginConfigurations {
        PluginConfigurations {
            plugins: vec![PluginTarget {
                id: INJECTOR.to_string(),
                name: "JavaScript Injector".to_string(),
                set: fields(&[("DisableScriptInjectionMiddleware", json!(false))]),
                secrets: BTreeMap::new(),
                lists: [(
                    "CustomJavaScripts".to_string(),
                    ListItems {
                        key: "Name".to_string(),
                        items: items
                            .into_iter()
                            .map(|v| v.as_object().unwrap().clone())
                            .collect(),
                    },
                )]
                .into_iter()
                .collect(),
            }],
        }
    }

    fn injector_transport(configuration: &str) -> FakeTransport {
        FakeTransport::default()
            .on_get(PLUGINS.path, vec![ok(PLUGINS_JSON)])
            .on_get(
                &format!("/Plugins/{INJECTOR}/Configuration"),
                vec![ok(configuration)],
            )
            .on_put(vec![Step::Answer(204, String::new())])
    }

    /// The recorded host entry, as the spec would name it.
    fn recorded_entry() -> Value {
        let recorded: Value = serde_json::from_str(&plugin_file(INJECTOR)).unwrap();
        recorded["CustomJavaScripts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["Name"] == "Skin-Manager-Vorgabe")
            .unwrap()
            .clone()
    }

    #[test]
    fn a_keyed_list_item_that_already_matches_is_unchanged() {
        let task = injector(vec![recorded_entry()]);
        let t = injector_transport(&plugin_file(INJECTOR));
        assert_eq!(task.diff(&task.read(&t).unwrap()).unwrap(), vec![]);
    }

    #[test]
    fn a_changed_field_of_the_own_item_is_one_change_and_foreign_items_stay() {
        let mut wanted = recorded_entry();
        wanted["Enabled"] = json!(false);
        let task = injector(vec![wanted]);
        let t = injector_transport(&plugin_file(INJECTOR));
        let current = task.read(&t).unwrap();
        let changes = task.diff(&current).unwrap();
        assert_eq!(changes.len(), 1, "{changes:?}");
        assert_eq!(
            changes[0].to_string(),
            "JavaScript Injector: CustomJavaScripts[Name=Skin-Manager-Vorgabe].Enabled true -> false"
        );
        task.write(&t, &current).unwrap();
        let sent: Value = serde_json::from_str(&t.written.borrow()[0].1).unwrap();
        let mut expected: Value = serde_json::from_str(&plugin_file(INJECTOR)).unwrap();
        for e in expected["CustomJavaScripts"].as_array_mut().unwrap() {
            if e["Name"] == "Skin-Manager-Vorgabe" {
                e["Enabled"] = json!(false);
            }
        }
        // Everything else byte for byte: PluginJavaScripts belong to the
        // plugins and must survive, in their order.
        assert_eq!(sent, expected);
    }

    #[test]
    fn a_missing_item_is_appended_after_the_foreign_ones() {
        let mut recorded: Value = serde_json::from_str(&plugin_file(INJECTOR)).unwrap();
        recorded["CustomJavaScripts"] = json!([{"Name": "Fremd", "Script": "x", "Enabled": true, "RequiresAuthentication": false}]);
        let task = injector(vec![recorded_entry()]);
        let t = injector_transport(&recorded.to_string());
        let current = task.read(&t).unwrap();
        let changes = task.diff(&current).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(
            changes[0].to_string(),
            "JavaScript Injector: CustomJavaScripts[Name=Skin-Manager-Vorgabe] (missing) -> (added)"
        );
        task.write(&t, &current).unwrap();
        let sent: Value = serde_json::from_str(&t.written.borrow()[0].1).unwrap();
        let names: Vec<&str> = sent["CustomJavaScripts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["Name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["Fremd", "Skin-Manager-Vorgabe"]);
    }

    #[test]
    fn a_list_path_that_is_not_a_list_is_an_error() {
        let mut task = injector(vec![recorded_entry()]);
        let list = task.plugins[0].lists.remove("CustomJavaScripts").unwrap();
        task.plugins[0]
            .lists
            .insert("DisableScriptInjectionMiddleware".to_string(), list);
        task.plugins[0].set.clear();
        let t = injector_transport(&plugin_file(INJECTOR));
        let err = task
            .diff(&task.read(&t).unwrap())
            .err()
            .unwrap()
            .to_string();
        assert_eq!(
            err,
            "the answer has no such field: JavaScript Injector: DisableScriptInjectionMiddleware (a list)"
        );
    }

    #[test]
    fn a_long_value_is_shortened_in_the_change() {
        let mut wanted = recorded_entry();
        wanted["Script"] = json!("x".repeat(500));
        let task = injector(vec![wanted]);
        let t = injector_transport(&plugin_file(INJECTOR));
        let changes = task.diff(&task.read(&t).unwrap()).unwrap();
        let shown = changes[0].to_string();
        assert!(shown.len() < 260, "{shown}");
        assert!(shown.contains("…"), "{shown}");
    }

    #[test]
    fn a_prefix_matching_no_task_is_not_found() {
        let task = ScheduledTaskTriggers {
            key_prefix: "Merg3".to_string(),
            ..merge_triggers()
        };
        let t = FakeTransport::default().on_get(TASKS.path, vec![ok(TASKS_JSON)]);
        let err = task
            .diff(&task.read(&t).unwrap())
            .err()
            .unwrap()
            .to_string();
        assert!(err.starts_with("not found on the service"), "{err}");
    }

    // --- library ids by name (design §16) ---------------------------------

    const LDAP: &str = "958aad6637844d2ab89aa7b6fab6e25c";
    const SSO: &str = "505ce9d1d91642fa86ca673ef241d7df";

    fn library_ids(names: &[&str]) -> Value {
        json!({ LIBRARY_IDS: names })
    }

    const BASIC: [&str; 5] = [
        "Filme",
        "Musik",
        "Serien",
        "Mediathek Serien",
        "Mediathek Filme",
    ];

    /// The recorded state, named by library instead of by id.
    fn sign_in(basic: &[&str]) -> PluginConfigurations {
        PluginConfigurations {
            plugins: vec![
                PluginTarget {
                    id: LDAP.to_string(),
                    name: "LDAP-Auth".to_string(),
                    set: fields(&[
                        ("EnabledFolders", library_ids(basic)),
                        ("UseSsl", json!(true)),
                    ]),
                    secrets: BTreeMap::new(),
                    lists: BTreeMap::new(),
                },
                PluginTarget {
                    id: SSO.to_string(),
                    name: "SSO-Auth".to_string(),
                    set: fields(&[
                        ("OidConfigs.authentik.EnabledFolders", library_ids(basic)),
                        (
                            "OidConfigs.authentik.FolderRoleMapping",
                            json!([
                                { "Role": "Medien", "Folders": library_ids(&[
                                    "Filme", "Mediathek Filme", "Mediathek Serien", "Musik", "Serien"]) },
                                { "Role": "Privat", "Folders": library_ids(&["Privat"]) },
                            ]),
                        ),
                    ]),
                    secrets: BTreeMap::new(),
                    lists: BTreeMap::new(),
                },
            ],
        }
    }

    fn sign_in_transport(ldap: &str) -> FakeTransport {
        FakeTransport::default()
            .on_get(PLUGINS.path, vec![ok(PLUGINS_JSON)])
            .on_get(FOLDERS.path, vec![ok(FOLDERS_JSON)])
            .on_get(&format!("/Plugins/{LDAP}/Configuration"), vec![ok(ldap)])
            .on_get(
                &format!("/Plugins/{SSO}/Configuration"),
                vec![ok(&plugin_file(SSO))],
            )
            .on_put(vec![Step::Answer(204, String::new())])
    }

    #[test]
    fn library_names_resolve_to_the_recorded_ids() {
        let task = sign_in(&BASIC);
        let t = sign_in_transport(&plugin_file(LDAP));
        assert_eq!(task.diff(&task.read(&t).unwrap()).unwrap(), vec![]);
    }

    #[test]
    fn a_library_list_in_another_order_is_written_as_ids() {
        let reordered = [
            "Filme",
            "Serien",
            "Musik",
            "Mediathek Serien",
            "Mediathek Filme",
        ];
        let task = sign_in(&reordered);
        let t = sign_in_transport(&plugin_file(LDAP));
        let current = task.read(&t).unwrap();
        let changes = task.diff(&current).unwrap();
        let fields: Vec<String> = changes
            .iter()
            .map(|c| format!("{}: {}", c.subject, c.field))
            .collect();
        assert_eq!(
            fields,
            [
                "LDAP-Auth: EnabledFolders",
                "SSO-Auth: OidConfigs.authentik.EnabledFolders"
            ]
        );
        assert!(
            !changes[0].desired.contains(LIBRARY_IDS),
            "the change shows ids, not the marker: {}",
            changes[0].desired
        );

        task.write(&t, &current).unwrap();
        let written = t.written.borrow();
        assert_eq!(written.len(), 2);
        let sent: Value = serde_json::from_str(&written[0].1).unwrap();
        let mut expected: Value = serde_json::from_str(&plugin_file(LDAP)).unwrap();
        expected["EnabledFolders"] = json!([
            "7a2175bccb1f1a94152cbd2b2bae8f6d",
            "43cfe12fe7d9d8d21251e0964e0232e2",
            "8a05b0252259a1dbd62df97522638439",
            "c70af6a113b51f9aab8594b81d74f1b7",
            "7ee571d485deaa484dcede65597cd699"
        ]);
        assert_eq!(sent, expected, "ids in the order named, the rest untouched");
        let sso: Value = serde_json::from_str(&written[1].1).unwrap();
        let recorded: Value = serde_json::from_str(&plugin_file(SSO)).unwrap();
        assert_eq!(
            sso["OidConfigs"]["authentik"]["FolderRoleMapping"],
            recorded["OidConfigs"]["authentik"]["FolderRoleMapping"],
            "a marker nested in a list resolves too"
        );
        assert_eq!(
            sso["OidConfigs"]["authentik"]["CanonicalLinks"],
            recorded["OidConfigs"]["authentik"]["CanonicalLinks"],
            "the plugin's own links go back as read"
        );
    }

    #[test]
    fn an_unknown_library_is_not_found_before_anything_is_written() {
        let task = sign_in(&["Filme", "Filem"]);
        let t = sign_in_transport(&plugin_file(LDAP));
        let err = task.read(&t).err().unwrap().to_string();
        assert_eq!(
            err,
            "not found on the service: library \"Filem\" (LDAP-Auth: EnabledFolders); \
             library \"Filem\" (SSO-Auth: OidConfigs.authentik.EnabledFolders)"
        );
        assert!(t.written.borrow().is_empty());
    }

    #[test]
    fn an_empty_library_list_from_the_service_is_an_error() {
        let task = sign_in(&BASIC);
        let t = sign_in_transport(&plugin_file(LDAP)).on_get(FOLDERS.path, vec![ok("[]")]);
        let err = task.read(&t).err().unwrap().to_string();
        assert_eq!(err, "/Library/VirtualFolders returned an empty list");
    }

    #[test]
    fn a_library_name_the_service_carries_twice_is_not_guessed() {
        let mut folders: Vec<Value> = serde_json::from_str(FOLDERS_JSON).unwrap();
        let mut copy = folders
            .iter()
            .find(|f| f["Name"] == "Musik")
            .unwrap()
            .clone();
        copy["ItemId"] = json!("00000000000000000000000000000000");
        folders.push(copy);
        let task = sign_in(&BASIC);
        let t = sign_in_transport(&plugin_file(LDAP))
            .on_get(FOLDERS.path, vec![ok(&Value::Array(folders).to_string())]);
        let err = task.read(&t).err().unwrap().to_string();
        assert!(
            err.contains(r#"library "Musik" (LDAP-Auth: EnabledFolders) is there 2 times"#),
            "{err}"
        );
    }

    #[test]
    fn without_a_marker_the_libraries_are_not_asked() {
        // `plugins_transport` scripts no /Library/VirtualFolders: a request
        // there would panic.
        let task = targets("<masked>");
        let t = plugins_transport(&plugin_file(OSCARS), &plugin_file(MEDIATHEK));
        assert_eq!(task.diff(&task.read(&t).unwrap()).unwrap(), vec![]);
    }

    // --- user-policies and display-preferences (design §40) ----------------

    const USERS_JSON: &str = include_str!("../../tests/fixtures/jellyfin-10.11.11/users.json");
    const PREFERENCES: &str =
        include_str!("../../tests/fixtures/jellyfin-10.11.11/displaypreferences-usersettings.json");

    const LDAP_PROVIDER: &str = "Jellyfin.Plugin.LDAP_Auth.LdapAuthenticationProviderPlugin";

    /// What the host's three shell units set on every account, plus the one
    /// field nobody set: one administrator, everybody else not.
    fn host_policies() -> UserPolicies {
        UserPolicies {
            all: fields(&[
                ("AuthenticationProviderId", json!(LDAP_PROVIDER)),
                ("EnableAllFolders", json!(false)),
                ("EnableSubtitleManagement", json!(true)),
                ("EnableLiveTvAccess", json!(true)),
                ("EnableLiveTvManagement", json!(false)),
                ("IsAdministrator", json!(false)),
            ]),
            accounts: [(
                "konto1".to_string(),
                fields(&[("IsAdministrator", json!(true))]),
            )]
            .into_iter()
            .collect(),
        }
    }

    fn recorded_users() -> Vec<Value> {
        serde_json::from_str(USERS_JSON).unwrap()
    }

    fn account(name: &str) -> Value {
        recorded_users()
            .into_iter()
            .find(|u| u["Name"] == json!(name))
            .expect("the fixture holds that account")
    }

    fn account_id(name: &str) -> String {
        account(name)["Id"].as_str().unwrap().to_string()
    }

    fn users_transport(body: &str) -> FakeTransport {
        FakeTransport::default()
            .on_get(USERS.path, vec![ok(body)])
            .on_put(vec![Step::Answer(204, String::new())])
    }

    fn demoted() -> String {
        with(USERS_JSON, |v| {
            for user in v.as_array_mut().unwrap() {
                if user["Name"] == json!("konto1") {
                    user["Policy"]["IsAdministrator"] = json!(false);
                }
            }
        })
    }

    #[test]
    fn user_policies_recorded_state_is_already_desired() {
        let task = host_policies();
        let t = users_transport(USERS_JSON);
        let current = task.read(&t).unwrap();
        assert_eq!(current.len(), 9);
        assert_eq!(task.diff(&current).unwrap(), vec![]);
    }

    #[test]
    fn user_policies_write_the_whole_policy_of_the_account_that_differs() {
        let task = host_policies();
        let t = users_transport(&demoted());
        let current = task.read(&t).unwrap();
        let changes = task.diff(&current).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(
            changes[0].to_string(),
            "account konto1: Policy.IsAdministrator false -> true"
        );
        task.write(&t, &current).unwrap();
        let written = t.written.borrow();
        assert_eq!(written.len(), 1, "one POST, for the one account");
        assert_eq!(
            written[0].0,
            format!("/Users/{}/Policy", account_id("konto1"))
        );
        let sent: Value = serde_json::from_str(&written[0].1).unwrap();
        assert_eq!(
            sent,
            account("konto1")["Policy"],
            "the whole policy as read, with only IsAdministrator different"
        );
    }

    /// The host derives its administrators from an authentik group, and a
    /// Jellyfin account exists only after its owner has signed in once. Such
    /// a name is a note, not an error: nothing is written for it, and every
    /// other account is reconciled as before (design §40).
    #[test]
    fn an_account_the_answer_does_not_hold_is_a_note_and_nothing_is_written_for_it() {
        let mut task = host_policies();
        task.accounts
            .insert("konto99".to_string(), fields(&[("IsHidden", json!(false))]));
        let t = users_transport(USERS_JSON);
        let current = task.read(&t).unwrap();
        assert_eq!(current.len(), 9, "the nine accounts the service holds");
        assert_eq!(
            task.notes(&current),
            vec!["account konto99: not on the service yet — skipped".to_string()]
        );
        // The other accounts are as desired, so the run is unchanged -- the
        // note has not turned into a change either.
        assert_eq!(task.diff(&current).unwrap(), vec![]);
        task.write(&t, &current).unwrap();
        assert!(t.written.borrow().is_empty());
    }

    /// The same name, next to an account that really differs: the absent one
    /// is skipped and the present one is still written.
    #[test]
    fn an_absent_account_does_not_stop_the_accounts_the_service_does_hold() {
        let mut task = host_policies();
        task.accounts
            .insert("konto99".to_string(), fields(&[("IsHidden", json!(false))]));
        let t = users_transport(&demoted());
        let current = task.read(&t).unwrap();
        assert_eq!(
            task.notes(&current),
            vec!["account konto99: not on the service yet — skipped".to_string()]
        );
        let changes = task.diff(&current).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(
            changes[0].to_string(),
            "account konto1: Policy.IsAdministrator false -> true"
        );
        task.write(&t, &current).unwrap();
        let written = t.written.borrow();
        assert_eq!(written.len(), 1, "one POST, for the account that is there");
        assert_eq!(
            written[0].0,
            format!("/Users/{}/Policy", account_id("konto1"))
        );
    }

    /// `all` is unchanged by this: it says nothing about single accounts, so
    /// there is no name in it that could be absent.
    #[test]
    fn display_preferences_name_the_service_does_not_hold_is_a_note_without_a_write() {
        let mut task = favourites_off();
        task.accounts.insert(
            "konto99".to_string(),
            [(
                "livetv-favoritechannelsattop".to_string(),
                "true".to_string(),
            )]
            .into_iter()
            .collect(),
        );
        let t = preferences_transport(PREFERENCES);
        let current = task.read(&t).unwrap();
        assert_eq!(current.accounts.len(), 9);
        assert_eq!(
            task.notes(&current),
            vec!["account konto99: not on the service yet — skipped".to_string()]
        );
        assert_eq!(task.diff(&current).unwrap(), vec![]);
        task.write(&t, &current).unwrap();
        assert!(t.written.borrow().is_empty());
    }

    #[test]
    fn a_policy_field_the_answer_does_not_carry_is_an_error_before_the_write() {
        let task = UserPolicies {
            all: BTreeMap::new(),
            accounts: [(
                "konto1".to_string(),
                fields(&[("EnableAllFoldrs", json!(false))]),
            )]
            .into_iter()
            .collect(),
        };
        let t = users_transport(USERS_JSON);
        let current = task.read(&t).unwrap();
        let err = task.diff(&current).err().unwrap().to_string();
        assert_eq!(
            err,
            "the answer has no such field: account konto1: Policy.EnableAllFoldrs"
        );
        assert!(t.written.borrow().is_empty());
    }

    #[test]
    fn a_refused_policy_write_says_nothing_from_the_body() {
        const FROM_THE_BODY: &str = "ERFUNDENES-GEHEIMNIS-XYZ";
        for status in [401u16, 403] {
            let task = host_policies();
            let t = FakeTransport::default()
                .on_get(USERS.path, vec![ok(&demoted())])
                .on_put(vec![Step::Answer(
                    status,
                    format!(r#"[{{"errorMessage":"{FROM_THE_BODY}"}}]"#),
                )]);
            let current = task.read(&t).unwrap();
            let err = task.write(&t, &current).err().unwrap().to_string();
            assert!(!err.contains(FROM_THE_BODY), "{err}");
            assert!(err.contains(&format!("HTTP {status}")), "{err}");
            assert!(err.contains(&account_id("konto1")), "{err}");
        }
    }

    fn favourites_off() -> DisplayPreferences {
        DisplayPreferences {
            client: "emby".to_string(),
            all: [(
                "livetv-favoritechannelsattop".to_string(),
                "false".to_string(),
            )]
            .into_iter()
            .collect(),
            accounts: BTreeMap::new(),
        }
    }

    fn preferences_transport(document: &str) -> FakeTransport {
        let mut t = FakeTransport::default().on_get(USERS.path, vec![ok(USERS_JSON)]);
        for user in recorded_users() {
            t = t.on_get(
                &preferences_path(user["Id"].as_str().unwrap(), "emby"),
                vec![ok(document)],
            );
        }
        t.on_put(vec![Step::Answer(204, String::new())])
    }

    #[test]
    fn display_preferences_recorded_state_is_already_desired() {
        let task = favourites_off();
        let t = preferences_transport(PREFERENCES);
        let current = task.read(&t).unwrap();
        assert_eq!(current.accounts.len(), 9);
        assert_eq!(task.diff(&current).unwrap(), vec![]);
    }

    #[test]
    fn a_custom_pref_the_account_does_not_carry_is_added_to_the_whole_document() {
        let without = with(PREFERENCES, |v| {
            v["CustomPrefs"]
                .as_object_mut()
                .unwrap()
                .remove("livetv-favoritechannelsattop");
        });
        let task = favourites_off();
        let t = preferences_transport(&without);
        let current = task.read(&t).unwrap();
        let changes = task.diff(&current).unwrap();
        assert_eq!(changes.len(), 9, "every account, the one key each");
        assert_eq!(
            changes[0].to_string(),
            r#"account konto1: CustomPrefs.livetv-favoritechannelsattop (missing) -> "false""#
        );
        task.write(&t, &current).unwrap();
        let written = t.written.borrow();
        assert_eq!(written.len(), 9);
        assert_eq!(
            written[0].0,
            preferences_path(&account_id("konto1"), "emby")
        );
        let sent: Value = serde_json::from_str(&written[0].1).unwrap();
        assert_eq!(
            sent,
            serde_json::from_str::<Value>(PREFERENCES).unwrap(),
            "the whole document, with the key back"
        );
    }

    #[test]
    fn a_display_preference_of_one_account_overrides_the_one_for_all() {
        let mut task = favourites_off();
        task.accounts.insert(
            "konto2".to_string(),
            [(
                "livetv-favoritechannelsattop".to_string(),
                "true".to_string(),
            )]
            .into_iter()
            .collect(),
        );
        let t = preferences_transport(PREFERENCES);
        let current = task.read(&t).unwrap();
        let changes = task.diff(&current).unwrap();
        assert_eq!(changes.len(), 1, "only the account with its own value");
        assert_eq!(
            changes[0].to_string(),
            r#"account konto2: CustomPrefs.livetv-favoritechannelsattop "false" -> "true""#
        );
    }
}
