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
    engine::{Change, Probe, Task},
    error::Error,
    paths,
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
pub const ENDPOINTS: [Endpoint; 7] = [
    SYSTEM_INFO,
    CONFIGURATION_READ,
    CONFIGURATION_WRITE,
    FOLDERS,
    LIBRARY_OPTIONS_WRITE,
    TASKS,
    TRIGGERS_WRITE,
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
    ]
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
