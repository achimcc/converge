//! Kavita: its server settings, a document whose fields a spec names by path
//! (design §25). `POST /api/Settings` replaces the whole document, so every
//! write sends the document as it was read, with only the named fields
//! changed -- the same shape as Jellyfin's server configuration (§6).
//!
//! Two values in the answer need care. `oidcConfig.secret` comes back as
//! asterisks of its length, and Kavita puts the stored secret back in when it
//! receives exactly those asterisks (`SettingsService.UpdateOidcSettings`); a
//! round trip therefore keeps it. `smtpConfig.password` comes back in clear
//! text. Neither may be named by a spec (see `spec.rs`), so neither can appear
//! in a change, and no error here carries anything from a settings body.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use crate::{
    client::{expect_status, Reply, Transport},
    endpoint::{Endpoint, Shape},
    engine::{Change, Probe, Task},
    error::Error,
    paths,
};

/// Answers only an administrator, so the probe also proves that the key
/// belongs to one.
pub const SERVER_INFO: Endpoint = Endpoint {
    method: "GET",
    path: "/api/Server/server-info-slim",
    request: None,
    response: Some(Shape::One("ServerInfoSlimDto")),
};
pub const SETTINGS_READ: Endpoint = Endpoint {
    method: "GET",
    path: "/api/Settings",
    request: None,
    response: Some(Shape::Document("ServerSettingDto")),
};
pub const SETTINGS_WRITE: Endpoint = Endpoint {
    method: "POST",
    path: "/api/Settings",
    request: Some(Shape::Document("ServerSettingDto")),
    response: None,
};
/// Every library with its folders (design §26). A library is found by a
/// folder it holds, never by its id: ids are rows, and a rebuilt instance
/// hands out others.
pub const LIBRARIES: Endpoint = Endpoint {
    method: "GET",
    path: "/api/Library/libraries",
    request: None,
    response: Some(Shape::Documents("LibraryDto")),
};
/// Replaces one library whole. Its body is `UpdateLibraryDto`, whose fields
/// the answer of `LIBRARIES` carries under the same names -- except the file
/// types, which it calls `libraryFileTypes` there and `fileGroupTypes` here.
pub const LIBRARY_UPDATE: Endpoint = Endpoint {
    method: "POST",
    path: "/api/Library/update",
    request: Some(Shape::Document("UpdateLibraryDto")),
    response: None,
};
/// Makes a library for a folder none holds (design §26). The description
/// gives it the update's component as its body; it answers the new library,
/// which the task does not read -- it asks `LIBRARIES` again instead.
pub const LIBRARY_CREATE: Endpoint = Endpoint {
    method: "POST",
    path: "/api/Library/create",
    request: Some(Shape::Document("UpdateLibraryDto")),
    response: Some(Shape::Document("LibraryDto")),
};
/// `?libraryId=<id>&force=true`: sent after a change of `type` only.
pub const LIBRARY_SCAN: Endpoint = Endpoint {
    method: "POST",
    path: "/api/Library/scan",
    request: None,
    response: None,
};
pub const ENDPOINTS: [Endpoint; 7] = [
    SERVER_INFO,
    SETTINGS_READ,
    SETTINGS_WRITE,
    LIBRARIES,
    LIBRARY_CREATE,
    LIBRARY_UPDATE,
    LIBRARY_SCAN,
];

/// The components a library spec's fields are checked against: the body it
/// is written with, and the answer it is compared with.
pub const LIBRARY_UPDATE_COMPONENT: &str = "UpdateLibraryDto";
pub const LIBRARY_READ_COMPONENT: &str = "LibraryDto";

/// The fields of `UpdateLibraryDto` a write carries, taken from the answer
/// under the same name. `fileGroupTypes` is added from `libraryFileTypes`.
/// The first list is required by the description (0.9.1.4); a library answer
/// without one of them is an error before anything is sent.
const UPDATE_REQUIRED: [&str; 15] = [
    "allowMetadataMatching",
    "allowScrobbling",
    "enableMetadata",
    "excludePatterns",
    "folders",
    "folderWatching",
    "id",
    "includeInDashboard",
    "includeInSearch",
    "inheritWebLinksFromFirstChapter",
    "manageCollections",
    "manageReadingLists",
    "name",
    "removePrefixForSortName",
    "type",
];
const UPDATE_OPTIONAL: [&str; 2] = ["defaultLanguage", "metadataProvider"];

/// The component a spec's paths are checked against.
pub const SERVER_SETTINGS: &str = "ServerSettingDto";

pub fn wire_types() -> Vec<schemars::Schema> {
    vec![schemars::schema_for!(ServerInfoSlimDto)]
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfoSlimDto {
    #[serde(default)]
    pub kavita_version: Option<String>,
}

/// Readiness: the slim server info answers with a version. It is an
/// administrator's endpoint, so a refused key -- or the key of an account
/// that is no administrator -- is fatal. (Kavita 0.9.1.4 reports itself as
/// 0.9.1.3; the version here is what Kavita says, not what is installed.)
pub fn probe(t: &dyn Transport) -> Result<String, Probe> {
    let reply = t
        .get(SERVER_INFO.path)
        .map_err(|e| Probe::NotYet(e.to_string()))?;
    match reply.status {
        200 => {}
        401 | 403 => {
            return Err(Probe::Fatal(Error::Status {
                method: SERVER_INFO.method,
                path: SERVER_INFO.path.to_string(),
                status: reply.status,
                validation: vec![
                    "the key was refused, or its account is no administrator".to_string()
                ],
            }))
        }
        other => return Err(Probe::NotYet(format!("HTTP {other}"))),
    }
    let info: ServerInfoSlimDto = serde_json::from_str(&reply.body)
        .map_err(|e| Probe::NotYet(format!("unexpected answer: {}", crate::error::shape(&e))))?;
    info.kavita_version
        .filter(|v| !v.is_empty())
        .ok_or_else(|| Probe::NotYet("the answer has no version".to_string()))
}

/// Kavita refuses a settings write with a plain, translated sentence
/// (`BadRequest(await localizationService.TranslateAsync(...))`). It names a
/// rule, never a value, so a short single line is shown; anything else is
/// left out, as a body that might quote the request would be.
fn refusal(reply: &Reply) -> Vec<String> {
    let text = match serde_json::from_str::<Value>(&reply.body) {
        Ok(Value::String(s)) => s,
        Ok(_) => return Vec::new(),
        Err(_) => reply.body.trim().to_string(),
    };
    if text.is_empty() || text.len() > 200 || text.contains('\n') {
        Vec::new()
    } else {
        vec![text]
    }
}

pub struct ServerSettings {
    pub set: BTreeMap<String, Value>,
}

impl ServerSettings {
    fn missing(&self, document: &Value) -> Vec<String> {
        self.set
            .keys()
            .filter(|path| paths::get(document, path).is_none())
            .map(|path| format!("{SERVER_SETTINGS}: {path}"))
            .collect()
    }
}

impl Task for ServerSettings {
    type Current = Value;

    fn probe(&self, t: &dyn Transport) -> Result<String, Probe> {
        probe(t)
    }

    fn read(&self, t: &dyn Transport) -> Result<Value, Error> {
        let reply = t.get(SETTINGS_READ.path)?;
        expect_status(&SETTINGS_READ, &reply, &[200])?;
        // serde's message could quote a value -- the SMTP password is in
        // this answer in clear text. Say only where it failed.
        let document: Value = serde_json::from_str(&reply.body).map_err(|e| Error::Decode {
            path: SETTINGS_READ.path.to_string(),
            reason: format!(
                "not JSON ({:?} error at line {} column {})",
                e.classify(),
                e.line(),
                e.column()
            ),
        })?;
        if !document.is_object() {
            return Err(Error::Decode {
                path: SETTINGS_READ.path.to_string(),
                reason: "not an object".to_string(),
            });
        }
        Ok(document)
    }

    fn diff(&self, current: &Value) -> Result<Vec<Change>, Error> {
        let missing = self.missing(current);
        if !missing.is_empty() {
            return Err(Error::MissingField(missing));
        }
        Ok(self
            .set
            .iter()
            .filter_map(|(path, desired)| {
                let now = paths::get(current, path)?;
                (now != desired).then(|| Change {
                    subject: SERVER_SETTINGS.to_string(),
                    field: path.clone(),
                    current: now.to_string(),
                    desired: desired.to_string(),
                })
            })
            .collect())
    }

    fn notes(&self, _current: &Value) -> Vec<String> {
        Vec::new()
    }

    fn write(&self, t: &dyn Transport, current: &Value) -> Result<(), Error> {
        let mut updated = current.clone();
        let missing: Vec<String> = self
            .set
            .iter()
            .filter(|(path, value)| !paths::set(&mut updated, path, (*value).clone()))
            .map(|(path, _)| format!("{SERVER_SETTINGS}: {path}"))
            .collect();
        if !missing.is_empty() {
            return Err(Error::MissingField(missing));
        }
        let body = updated.to_string();
        let reply = t.post_json(SETTINGS_WRITE.path, &body)?;
        if reply.status == 200 {
            return Ok(());
        }
        Err(Error::Status {
            method: SETTINGS_WRITE.method,
            path: SETTINGS_WRITE.path.to_string(),
            status: reply.status,
            validation: refusal(&reply),
        })
    }
}

// --- libraries (design §26) ------------------------------------------------

/// Libraries by a folder they hold, with the fields to set on each; a folder
/// no library holds is a library to create, and needs a name and a type.
///
/// Kavita's update does not force a scan after a change of `type` -- its own
/// scan skips files that did not change on disk, so the series would stay
/// parsed by the old type's parser. A change of `type` is therefore followed
/// by a forced scan of that library.
pub struct Libraries {
    pub libraries: BTreeMap<String, BTreeMap<String, Value>>,
}

const LIBRARY_SUBJECT: &str = "library";

fn has_folder(library: &Value, folder: &str) -> bool {
    library
        .get("folders")
        .and_then(Value::as_array)
        .is_some_and(|folders| {
            folders
                .iter()
                .filter_map(Value::as_str)
                .any(|f| f.trim_end_matches('/') == folder.trim_end_matches('/'))
        })
}

impl Libraries {
    /// Each spec folder with the library that holds it, if any. More than
    /// one is an error: converge does not guess between two. None is a
    /// library to create (design §26).
    fn matched<'a>(
        &'a self,
        current: &'a [Value],
    ) -> Result<Vec<(&'a str, Option<&'a Value>)>, Error> {
        let mut found = Vec::new();
        let mut ambiguous = Vec::new();
        for folder in self.libraries.keys() {
            let holders: Vec<&Value> = current.iter().filter(|l| has_folder(l, folder)).collect();
            match holders.as_slice() {
                [] => found.push((folder.as_str(), None)),
                [one] => found.push((folder.as_str(), Some(*one))),
                _ => ambiguous.push(format!(
                    "{} libraries hold the folder {folder}",
                    holders.len()
                )),
            }
        }
        if ambiguous.is_empty() {
            Ok(found)
        } else {
            Err(Error::Mismatch(ambiguous))
        }
    }

    fn label(folder: &str) -> String {
        format!("{LIBRARY_SUBJECT} {folder}")
    }

    /// Writes `library` whole where a spec field differs, and forces a scan
    /// after a change of `type`.
    fn update(
        t: &dyn Transport,
        folder: &str,
        library: &Value,
        set: &BTreeMap<String, Value>,
    ) -> Result<(), Error> {
        if set
            .iter()
            .all(|(field, value)| library.get(field) == Some(value))
        {
            return Ok(());
        }
        let body = update_body(folder, library, set)?;
        let reply = t.post_json(LIBRARY_UPDATE.path, &body.to_string())?;
        if reply.status != 200 {
            return Err(Error::Status {
                method: LIBRARY_UPDATE.method,
                path: LIBRARY_UPDATE.path.to_string(),
                status: reply.status,
                validation: refusal(&reply),
            });
        }
        let type_changed = set
            .get("type")
            .is_some_and(|desired| library.get("type") != Some(desired));
        if type_changed {
            let id = library
                .get("id")
                .and_then(Value::as_i64)
                .ok_or_else(|| Error::MissingField(vec![format!("{}: id", Self::label(folder))]))?;
            let path = format!("{}?libraryId={id}&force=true", LIBRARY_SCAN.path);
            let reply = t.post_json(&path, "")?;
            if reply.status != 200 {
                return Err(Error::Status {
                    method: LIBRARY_SCAN.method,
                    path,
                    status: reply.status,
                    validation: refusal(&reply),
                });
            }
        }
        Ok(())
    }
}

/// The update body for `library` with `set` applied.
fn update_body(
    folder: &str,
    library: &Value,
    set: &BTreeMap<String, Value>,
) -> Result<Value, Error> {
    let mut body = serde_json::Map::new();
    let mut missing = Vec::new();
    for field in UPDATE_REQUIRED {
        match library.get(field) {
            Some(v) => {
                body.insert(field.to_string(), v.clone());
            }
            None => missing.push(format!("{}: {field}", Libraries::label(folder))),
        }
    }
    match library.get("libraryFileTypes") {
        Some(v) => {
            body.insert("fileGroupTypes".to_string(), v.clone());
        }
        None => missing.push(format!("{}: libraryFileTypes", Libraries::label(folder))),
    }
    for field in UPDATE_OPTIONAL {
        if let Some(v) = library.get(field) {
            body.insert(field.to_string(), v.clone());
        }
    }
    if !missing.is_empty() {
        return Err(Error::MissingField(missing));
    }
    for (field, value) in set {
        body.insert(field.clone(), value.clone());
    }
    Ok(Value::Object(body))
}

/// `LibraryType` of the description (0.9.1.4): Manga, Comic, Book, Image,
/// LightNovel, ComicVine.
const LIBRARY_TYPES: std::ops::RangeInclusive<i64> = 0..=5;
/// `LibraryType.Comic`: the one type `AddLibrary` switches scrobbling off for.
const TYPE_COMIC: i64 = 1;
/// `LibraryType.ComicVine`.
const TYPE_COMIC_VINE: i64 = 5;
/// `MetadataProvider.Mangabaka`, where the web interface's form starts.
const PROVIDER_MANGABAKA: i64 = 3;
/// `MetadataProvider.ComicBookRoundup`, the first provider Kavita lists for
/// the two comic types -- which do not allow Mangabaka.
const PROVIDER_COMIC_BOOK_ROUNDUP: i64 = 4;

/// The body of `POST /api/Library/create` for a folder no library holds: an
/// `UpdateLibraryDto` like the update's, but with no answer to take its
/// sixteen required fields from. `name` and `type` must come from the spec;
/// everything else the spec does not name is what Kavita's own web interface
/// would send for a new library of that type (design §26, read in the source
/// at the tag v0.9.1.4). A spec field wins over a default.
///
/// `metadataProvider` is optional in the description, but left out it is 0,
/// which no library type allows, and Kavita refuses the request.
///
/// Called in `diff` as well, so a spec that cannot create fails before
/// anything is written.
pub fn create_body(folder: &str, set: &BTreeMap<String, Value>) -> Result<Value, Error> {
    let label = Libraries::label(folder);
    let mut problems = Vec::new();
    if !matches!(set.get("name"), Some(Value::String(name)) if !name.is_empty()) {
        problems.push(format!(
            "{label}: no library holds this folder, and the spec gives no name to create one with"
        ));
    }
    let kind = set
        .get("type")
        .and_then(Value::as_i64)
        .filter(|kind| LIBRARY_TYPES.contains(kind));
    if kind.is_none() {
        problems.push(format!(
            "{label}: no library holds this folder, and the spec gives no type (0 to 5) to create one with"
        ));
    }
    let Some(kind) = kind.filter(|_| problems.is_empty()) else {
        return Err(Error::Mismatch(problems));
    };
    let provider = if kind == TYPE_COMIC || kind == TYPE_COMIC_VINE {
        PROVIDER_COMIC_BOOK_ROUNDUP
    } else {
        PROVIDER_MANGABAKA
    };
    let mut body = serde_json::json!({
        "id": 0,
        "folders": [folder],
        "folderWatching": true,
        "includeInDashboard": true,
        "includeInSearch": true,
        "manageCollections": false,
        "manageReadingLists": false,
        "allowScrobbling": kind != TYPE_COMIC,
        "allowMetadataMatching": true,
        "enableMetadata": true,
        "removePrefixForSortName": false,
        "inheritWebLinksFromFirstChapter": false,
        "defaultLanguage": "",
        "metadataProvider": provider,
        "fileGroupTypes": [1, 2, 3, 4],
        "excludePatterns": [],
    });
    let fields = body.as_object_mut().expect("an object literal");
    for (field, value) in set {
        fields.insert(field.clone(), value.clone());
    }
    Ok(body)
}

impl Task for Libraries {
    type Current = Vec<Value>;

    fn probe(&self, t: &dyn Transport) -> Result<String, Probe> {
        probe(t)
    }

    fn read(&self, t: &dyn Transport) -> Result<Vec<Value>, Error> {
        let reply = t.get(LIBRARIES.path)?;
        expect_status(&LIBRARIES, &reply, &[200])?;
        let list: Value = serde_json::from_str(&reply.body).map_err(|e| Error::Decode {
            path: LIBRARIES.path.to_string(),
            reason: format!(
                "not JSON ({:?} error at line {} column {})",
                e.classify(),
                e.line(),
                e.column()
            ),
        })?;
        match list {
            Value::Array(items) if items.iter().all(Value::is_object) => Ok(items),
            _ => Err(Error::Decode {
                path: LIBRARIES.path.to_string(),
                reason: "not a list of libraries".to_string(),
            }),
        }
    }

    fn diff(&self, current: &Vec<Value>) -> Result<Vec<Change>, Error> {
        let mut changes = Vec::new();
        let mut missing = Vec::new();
        for (folder, library) in self.matched(current)? {
            let set = &self.libraries[folder];
            let Some(library) = library else {
                // Built here although nothing is sent: a spec that cannot
                // create has to fail before the first write.
                create_body(folder, set)?;
                changes.push(Change {
                    subject: Self::label(folder),
                    field: String::new(),
                    current: "(missing)".to_string(),
                    desired: "(added)".to_string(),
                });
                continue;
            };
            for (field, desired) in set {
                match library.get(field) {
                    None => missing.push(format!("{}: {field}", Self::label(folder))),
                    Some(now) if now != desired => changes.push(Change {
                        subject: Self::label(folder),
                        field: field.clone(),
                        current: now.to_string(),
                        desired: desired.to_string(),
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

    fn notes(&self, current: &Vec<Value>) -> Vec<String> {
        let named = current
            .iter()
            .filter(|l| !self.libraries.keys().any(|f| has_folder(l, f)))
            .filter_map(|l| l.get("name").and_then(Value::as_str))
            .map(|name| format!("library {name} is not in the spec and stays as it is"))
            .collect();
        named
    }

    /// Creates what no library holds, then writes every library that
    /// differs. `AddLibrary` does not take every field it is sent
    /// (`includeInSearch` is not among them, and scrobbling is switched off
    /// for a Comic library), so a created library is read back from the list
    /// and goes through the same whole-library update as one that was there.
    fn write(&self, t: &dyn Transport, current: &Vec<Value>) -> Result<(), Error> {
        let matched = self.matched(current)?;
        let mut bodies = Vec::new();
        for (folder, library) in &matched {
            if library.is_none() {
                bodies.push(create_body(folder, &self.libraries[*folder])?);
            }
        }
        for body in &bodies {
            let reply = t.post_json(LIBRARY_CREATE.path, &body.to_string())?;
            if reply.status != 200 {
                return Err(Error::Status {
                    method: LIBRARY_CREATE.method,
                    path: LIBRARY_CREATE.path.to_string(),
                    status: reply.status,
                    validation: refusal(&reply),
                });
            }
        }
        // 200 is not "there": ask the list. Kavita commits before it
        // answers, so a library the list does not show now is an error and
        // nothing to wait for.
        let after;
        let matched = if bodies.is_empty() {
            matched
        } else {
            after = self.read(t)?;
            self.matched(&after)?
        };
        for (folder, library) in matched {
            let library = library.ok_or_else(|| {
                Error::NotFound(vec![format!(
                    "no library holds the folder {folder}, although {} {} answered 200",
                    LIBRARY_CREATE.method, LIBRARY_CREATE.path
                )])
            })?;
            Self::update(t, folder, library, &self.libraries[folder])?;
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

    const SETTINGS_JSON: &str = include_str!("../../tests/fixtures/kavita-0.9.1.4/settings.json");
    const INFO_JSON: &str =
        include_str!("../../tests/fixtures/kavita-0.9.1.4/server-info-slim.json");

    /// What the host declares (design §25).
    fn declared() -> ServerSettings {
        let set = [
            ("oidcConfig.provisionAccounts", json!(true)),
            ("oidcConfig.syncUserSettings", json!(true)),
            ("oidcConfig.requireVerifiedEmail", json!(false)),
            ("oidcConfig.disablePasswordAuthentication", json!(true)),
            ("oidcConfig.autoLogin", json!(true)),
            ("oidcConfig.rolesClaim", json!("kavita_roles")),
            ("oidcConfig.rolesPrefix", json!("")),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        ServerSettings { set }
    }

    fn settings(body: &str) -> FakeTransport {
        FakeTransport::default()
            .on_get(SETTINGS_READ.path, vec![ok(body)])
            .on_put(vec![ok(body)])
    }

    #[test]
    fn probe_reads_the_version_and_a_refused_key_is_fatal() {
        let t = FakeTransport::default().on_get(SERVER_INFO.path, vec![ok(INFO_JSON)]);
        assert_eq!(probe(&t).ok().unwrap(), "0.9.1.3");
        for status in [401, 403] {
            let t = FakeTransport::default()
                .on_get(SERVER_INFO.path, vec![Step::Answer(status, String::new())]);
            match probe(&t) {
                Err(Probe::Fatal(e)) => {
                    assert!(e.to_string().contains("no administrator"), "{status}: {e}")
                }
                _ => panic!("{status} is not fatal"),
            }
        }
        let t = FakeTransport::default()
            .on_get(SERVER_INFO.path, vec![Step::Answer(502, String::new())]);
        assert!(matches!(probe(&t), Err(Probe::NotYet(_))));
    }

    #[test]
    fn the_recorded_settings_already_match() {
        let task = declared();
        let current = task.read(&settings(SETTINGS_JSON)).unwrap();
        assert_eq!(task.diff(&current).unwrap(), vec![]);
    }

    #[test]
    fn a_switch_off_is_one_change_and_the_write_keeps_everything_else() {
        let mut recorded: Value = serde_json::from_str(SETTINGS_JSON).unwrap();
        recorded["oidcConfig"]["autoLogin"] = json!(false);
        let body = recorded.to_string();
        let task = declared();
        let t = settings(&body);
        let current = task.read(&t).unwrap();
        let changes = task.diff(&current).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(
            changes[0].to_string(),
            "ServerSettingDto: oidcConfig.autoLogin false -> true"
        );
        task.write(&t, &current).unwrap();
        let written = t.written.borrow();
        assert_eq!(written.len(), 1);
        assert_eq!(written[0].0, "/api/Settings");
        let sent: Value = serde_json::from_str(&written[0].1).unwrap();
        let mut expected = recorded.clone();
        expected["oidcConfig"]["autoLogin"] = json!(true);
        // Everything else travels back as it came -- the masked secret too,
        // which is how Kavita knows to keep the stored one.
        assert_eq!(sent, expected);
        assert_eq!(sent["oidcConfig"]["secret"], json!("********"));
    }

    #[test]
    fn a_path_kavita_does_not_answer_is_an_error_before_any_write() {
        let mut task = declared();
        task.set
            .insert("oidcConfig.autoLogn".to_string(), json!(true));
        let current = task.read(&settings(SETTINGS_JSON)).unwrap();
        let err = task.diff(&current).err().unwrap().to_string();
        assert!(err.contains("oidcConfig.autoLogn"), "{err}");
    }

    #[test]
    fn a_refused_write_shows_kavitas_sentence_but_never_a_long_body() {
        let task = declared();
        let t = FakeTransport::default()
            .on_get(SETTINGS_READ.path, vec![ok(SETTINGS_JSON)])
            .on_put(vec![Step::Answer(
                400,
                "\"The Authority is not valid\"".to_string(),
            )]);
        let current = task.read(&t).unwrap();
        let err = task.write(&t, &current).err().unwrap().to_string();
        assert!(err.contains("HTTP 400"), "{err}");
        assert!(err.contains("The Authority is not valid"), "{err}");

        let long = format!("\"{}\"", "x".repeat(300));
        let t = FakeTransport::default()
            .on_get(SETTINGS_READ.path, vec![ok(SETTINGS_JSON)])
            .on_put(vec![Step::Answer(400, long)]);
        let err = task.write(&t, &current).err().unwrap().to_string();
        assert!(!err.contains("xxx"), "{err}");
    }

    const LIBRARIES_JSON: &str = include_str!("../../tests/fixtures/kavita-0.9.1.4/libraries.json");
    const BOOKS: &str = "/tank/data/media/books";

    fn libraries(fields: Value) -> Libraries {
        let set: BTreeMap<String, Value> = serde_json::from_value(fields).expect("an object");
        Libraries {
            libraries: [(BOOKS.to_string(), set)].into_iter().collect(),
        }
    }

    fn library_answer(body: &str) -> FakeTransport {
        FakeTransport::default()
            .on_get(LIBRARIES.path, vec![ok(body)])
            .on_put(vec![ok("{}"), ok("")])
    }

    #[test]
    fn the_recorded_library_already_matches() {
        let task = libraries(json!({"name": "Buecher", "type": 2}));
        let current = task.read(&library_answer(LIBRARIES_JSON)).unwrap();
        assert_eq!(task.diff(&current).unwrap(), vec![]);
        assert_eq!(task.notes(&current), Vec::<String>::new());
    }

    #[test]
    fn a_wrong_type_is_written_whole_and_followed_by_a_forced_scan() {
        let mut recorded: Value = serde_json::from_str(LIBRARIES_JSON).unwrap();
        recorded[0]["type"] = json!(0);
        recorded[0]["name"] = json!("Books");
        let task = libraries(json!({"name": "Buecher", "type": 2}));
        let t = library_answer(&recorded.to_string());
        let current = task.read(&t).unwrap();
        let changes: Vec<String> = task
            .diff(&current)
            .unwrap()
            .iter()
            .map(|c| c.to_string())
            .collect();
        assert_eq!(
            changes,
            [
                "library /tank/data/media/books: name \"Books\" -> \"Buecher\"",
                "library /tank/data/media/books: type 0 -> 2"
            ]
        );
        task.write(&t, &current).unwrap();
        let written = t.written.borrow();
        assert_eq!(written.len(), 2);
        assert_eq!(written[0].0, "/api/Library/update");
        let body: Value = serde_json::from_str(&written[0].1).unwrap();
        assert_eq!(body["name"], json!("Buecher"));
        assert_eq!(body["type"], json!(2));
        assert_eq!(body["id"], json!(1));
        assert_eq!(body["folders"], json!([BOOKS]));
        // The file types travel under the name the update expects.
        assert_eq!(body["fileGroupTypes"], json!([1, 2, 3, 4]));
        assert!(body.get("libraryFileTypes").is_none());
        // Fields the answer carries but the update does not take stay out.
        assert!(body.get("lastScanned").is_none());
        assert!(body.get("coverImage").is_none());
        assert_eq!(written[1].0, "/api/Library/scan?libraryId=1&force=true");
    }

    #[test]
    fn a_rename_alone_does_not_scan() {
        let mut recorded: Value = serde_json::from_str(LIBRARIES_JSON).unwrap();
        recorded[0]["name"] = json!("Books");
        let task = libraries(json!({"name": "Buecher", "type": 2}));
        let t = library_answer(&recorded.to_string());
        let current = task.read(&t).unwrap();
        task.write(&t, &current).unwrap();
        let written = t.written.borrow();
        assert_eq!(written.len(), 1);
        assert_eq!(written[0].0, "/api/Library/update");
    }

    const AFTER_CREATE_JSON: &str =
        include_str!("../../tests/fixtures/kavita-0.9.1.4/constructed-libraries-after-create.json");
    const PAPERS: &str = "/tank/data/media/zeitungen";

    /// The recorded library as declared, and a second one nothing holds yet.
    fn with_papers(fields: Value) -> Libraries {
        let mut task = libraries(json!({"name": "Buecher", "type": 2}));
        task.libraries.insert(
            PAPERS.to_string(),
            serde_json::from_value(fields).expect("an object"),
        );
        task
    }

    fn scripted(lists: Vec<Step>, writes: Vec<Step>) -> FakeTransport {
        FakeTransport::default()
            .on_get(SERVER_INFO.path, vec![ok(INFO_JSON)])
            .on_get(LIBRARIES.path, lists)
            .on_put(writes)
    }

    #[test]
    fn a_folder_no_library_holds_is_a_change_and_plan_writes_nothing() {
        let task = with_papers(json!({"name": "Zeitungen", "type": 1}));
        let t = scripted(vec![ok(LIBRARIES_JSON)], vec![]);
        let report = run(Mode::Plan, &task, &t, &FakeClock::new(), Timing::default()).unwrap();
        let Outcome::Differs(changes) = report.outcome else {
            panic!("plan saw no difference");
        };
        let changes: Vec<String> = changes.iter().map(|c| c.to_string()).collect();
        assert_eq!(
            changes,
            ["library /tank/data/media/zeitungen: (missing) -> (added)"]
        );
        assert!(t.written.borrow().is_empty());
        // The library that is there is no note: the spec names it.
        assert_eq!(report.notes, Vec::<String>::new());
    }

    #[test]
    fn apply_creates_reads_back_and_then_writes_what_create_did_not_take() {
        // `AddLibrary` (0.9.1.4) does not read `includeInSearch`; the library
        // comes back with the entity's default, and the update sets it.
        let task = with_papers(json!({
            "name": "Zeitungen", "type": 1, "folderWatching": true, "includeInSearch": false
        }));
        let mut converged: Value = serde_json::from_str(AFTER_CREATE_JSON).unwrap();
        converged[1]["includeInSearch"] = json!(false);
        let t = scripted(
            vec![
                ok(LIBRARIES_JSON),
                ok(AFTER_CREATE_JSON),
                ok(&converged.to_string()),
            ],
            vec![ok("{}"), ok("")],
        );
        let report = run(Mode::Apply, &task, &t, &FakeClock::new(), Timing::default()).unwrap();
        let Outcome::Changed(changes) = report.outcome else {
            panic!("apply changed nothing");
        };
        assert_eq!(changes.len(), 1);
        assert_eq!(
            changes[0].to_string(),
            "library /tank/data/media/zeitungen: (missing) -> (added)"
        );
        let written = t.written.borrow();
        assert_eq!(written.len(), 2, "{written:?}");
        assert_eq!(written[0].0, "/api/Library/create");
        assert_eq!(
            serde_json::from_str::<Value>(&written[0].1).unwrap(),
            json!({
                "id": 0,
                "name": "Zeitungen",
                "type": 1,
                "folders": [PAPERS],
                "folderWatching": true,
                "includeInDashboard": true,
                "includeInSearch": false,
                "manageCollections": false,
                "manageReadingLists": false,
                "allowScrobbling": false,
                "allowMetadataMatching": true,
                "enableMetadata": true,
                "removePrefixForSortName": false,
                "inheritWebLinksFromFirstChapter": false,
                "defaultLanguage": "",
                "metadataProvider": 4,
                "fileGroupTypes": [1, 2, 3, 4],
                "excludePatterns": []
            })
        );
        // The library the list now shows, written whole; no scan, since the
        // type is the one it was created with.
        assert_eq!(written[1].0, "/api/Library/update");
        let update: Value = serde_json::from_str(&written[1].1).unwrap();
        assert_eq!(update["id"], json!(2));
        assert_eq!(update["name"], json!("Zeitungen"));
        assert_eq!(update["folders"], json!([PAPERS]));
        assert_eq!(update["includeInSearch"], json!(false));
    }

    #[test]
    fn a_created_library_that_already_is_as_declared_is_not_updated() {
        let task = with_papers(json!({"name": "Zeitungen", "type": 1}));
        let t = scripted(
            vec![ok(LIBRARIES_JSON), ok(AFTER_CREATE_JSON)],
            vec![ok("{}")],
        );
        run(Mode::Apply, &task, &t, &FakeClock::new(), Timing::default()).unwrap();
        let written = t.written.borrow();
        assert_eq!(written.len(), 1, "{written:?}");
        assert_eq!(written[0].0, "/api/Library/create");
    }

    #[test]
    fn a_library_to_be_created_without_a_name_or_a_type_fails_before_any_write() {
        for (fields, word) in [
            (json!({"type": 1}), "name"),
            (json!({"name": "", "type": 1}), "name"),
            (json!({"name": "Zeitungen"}), "type"),
            (json!({"name": "Zeitungen", "type": 9}), "type"),
            (json!({"name": "Zeitungen", "type": "1"}), "type"),
            (json!({"folderWatching": true}), "type"),
        ] {
            let task = with_papers(fields.clone());
            let t = scripted(vec![ok(LIBRARIES_JSON)], vec![ok("{}")]);
            let current = task.read(&t).unwrap();
            let err = task.diff(&current).err().unwrap().to_string();
            assert!(
                err.contains(PAPERS) && err.contains(word),
                "{fields}: {err}"
            );
            assert!(task.write(&t, &current).is_err(), "{fields}");
            assert!(t.written.borrow().is_empty(), "{fields}");
        }
    }

    #[test]
    fn a_created_library_the_list_does_not_show_is_an_error() {
        let task = with_papers(json!({"name": "Zeitungen", "type": 1}));
        let t = scripted(vec![ok(LIBRARIES_JSON)], vec![ok("{}")]);
        let current = task.read(&t).unwrap();
        let err = task.write(&t, &current).err().unwrap().to_string();
        assert!(err.contains(PAPERS) && err.contains("create"), "{err}");
        assert_eq!(t.written.borrow().len(), 1);
    }

    #[test]
    fn a_refused_create_shows_kavitas_sentence_and_stops() {
        let task = with_papers(json!({"name": "Buecher", "type": 1}));
        let t = scripted(
            vec![ok(LIBRARIES_JSON)],
            vec![Step::Answer(
                400,
                "\"Library name already exists\"".to_string(),
            )],
        );
        let current = task.read(&t).unwrap();
        let err = task.write(&t, &current).err().unwrap().to_string();
        assert!(err.contains("/api/Library/create"), "{err}");
        assert!(err.contains("HTTP 400"), "{err}");
        assert!(err.contains("Library name already exists"), "{err}");
        assert_eq!(t.written.borrow().len(), 1);
    }

    #[test]
    fn the_create_body_fills_what_the_spec_leaves_out_and_the_spec_wins() {
        let set = |v: Value| -> BTreeMap<String, Value> { serde_json::from_value(v).unwrap() };
        // The provider the web interface would end up with, per type.
        for (kind, provider) in [(0, 3), (1, 4), (2, 3), (3, 3), (4, 3), (5, 4)] {
            let body = create_body(PAPERS, &set(json!({"name": "N", "type": kind}))).unwrap();
            assert_eq!(body["metadataProvider"], json!(provider), "type {kind}");
            assert_eq!(body["allowScrobbling"], json!(kind != 1), "type {kind}");
        }
        let body = create_body(
            PAPERS,
            &set(json!({
                "name": "N", "type": 1, "metadataProvider": 2,
                "excludePatterns": ["**/tmp/**"], "manageCollections": true
            })),
        )
        .unwrap();
        assert_eq!(body["metadataProvider"], json!(2));
        assert_eq!(body["excludePatterns"], json!(["**/tmp/**"]));
        assert_eq!(body["manageCollections"], json!(true));
        assert_eq!(body["folders"], json!([PAPERS]));
    }

    #[test]
    fn other_libraries_are_notes_and_a_trailing_slash_names_the_same_folder() {
        let task = Libraries {
            libraries: [(
                "/tank/elsewhere".to_string(),
                serde_json::from_value(json!({"name": "Elsewhere", "type": 0})).unwrap(),
            )]
            .into_iter()
            .collect(),
        };
        let current = task.read(&library_answer(LIBRARIES_JSON)).unwrap();
        assert_eq!(
            task.notes(&current),
            ["library Buecher is not in the spec and stays as it is"]
        );
        // A trailing slash names the same folder.
        let task = libraries(json!({"type": 2}));
        let mut t = task;
        let set = t.libraries.remove(BOOKS).unwrap();
        t.libraries.insert(format!("{BOOKS}/"), set);
        let current = t.read(&library_answer(LIBRARIES_JSON)).unwrap();
        assert_eq!(t.diff(&current).unwrap(), vec![]);
    }

    #[test]
    fn two_libraries_holding_the_folder_is_an_error() {
        let recorded: Value = serde_json::from_str(LIBRARIES_JSON).unwrap();
        let mut second = recorded[0].clone();
        second["id"] = json!(2);
        let both = Value::Array(vec![recorded[0].clone(), second]).to_string();
        let task = libraries(json!({"type": 2}));
        let current = task.read(&library_answer(&both)).unwrap();
        let err = task.diff(&current).err().unwrap().to_string();
        assert!(err.contains("2 libraries hold"), "{err}");
    }

    #[test]
    fn an_answer_that_is_no_json_quotes_nothing() {
        let task = declared();
        let err = task
            .read(&settings("password=hunter2"))
            .err()
            .unwrap()
            .to_string();
        assert!(!err.contains("hunter2"), "{err}");
        let err = task.read(&settings("[]")).err().unwrap().to_string();
        assert!(err.contains("not an object"), "{err}");
    }
}
