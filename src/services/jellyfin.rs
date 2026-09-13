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
    spec::ListItems,
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
pub const ENDPOINTS: [Endpoint; 10] = [
    SYSTEM_INFO,
    CONFIGURATION_READ,
    CONFIGURATION_WRITE,
    FOLDERS,
    LIBRARY_OPTIONS_WRITE,
    TASKS,
    TRIGGERS_WRITE,
    PLUGINS,
    PLUGIN_CONFIGURATION_READ,
    PLUGIN_CONFIGURATION_WRITE,
];

/// The component each task's spec paths are checked against.
pub const SERVER_CONFIGURATION: &str = "ServerConfiguration";
pub const LIBRARY_OPTIONS: &str = "LibraryOptions";
pub const TASK_TRIGGER_INFO: &str = "TaskTriggerInfo";

pub fn wire_types() -> Vec<schemars::Schema> {
    vec![
        schemars::schema_for!(SystemInfo),
        schemars::schema_for!(VirtualFolderInfo),
        schemars::schema_for!(UpdateLibraryOptionsDto),
        schemars::schema_for!(TaskInfo),
        schemars::schema_for!(TaskTriggerInfo),
        schemars::schema_for!(PluginInfo),
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
        .map_err(|e| Probe::NotYet(format!("unexpected answer: {e}")))?;
    info.version
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

// --- library-options -------------------------------------------------------

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

/// A plugin's configuration as read, keyed like the spec.
pub struct LoadedPlugin {
    pub id: String,
    pub configuration: Value,
}

fn normalized(id: &str) -> String {
    id.replace('-', "").to_ascii_lowercase()
}

impl PluginConfigurations {
    fn configuration_of<'a>(&self, current: &'a [LoadedPlugin], id: &str) -> Option<&'a Value> {
        current
            .iter()
            .find(|p| p.id == id)
            .map(|p| &p.configuration)
    }

    fn changes_of(
        &self,
        target: &PluginTarget,
        configuration: &Value,
        missing: &mut Vec<String>,
    ) -> Vec<Change> {
        let mut changes = compare_fields(&target.name, configuration, &target.set, missing);
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
        let mut loaded = Vec::new();
        for target in &self.plugins {
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
            });
        }
        Ok(loaded)
    }

    fn diff(&self, current: &Self::Current) -> Result<Vec<Change>, Error> {
        let mut missing = Vec::new();
        let mut changes = Vec::new();
        for target in &self.plugins {
            let Some(configuration) = self.configuration_of(current, &target.id) else {
                missing.push(format!("{}: configuration", target.name));
                continue;
            };
            changes.extend(self.changes_of(target, configuration, &mut missing));
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
            let Some(configuration) = self.configuration_of(current, &target.id) else {
                continue;
            };
            let mut missing = Vec::new();
            if self
                .changes_of(target, configuration, &mut missing)
                .is_empty()
            {
                continue;
            }
            // The whole configuration goes back: Ratings alone carries more
            // than a hundred settings made in the web UI.
            let mut updated = with_fields(&target.name, configuration, &target.set)?;
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
}
