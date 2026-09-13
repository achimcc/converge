//! Radarr, Sonarr and Lidarr share one code base (Servarr): the same
//! configuration documents under `config/`, the same root folders -- behind
//! API v3 for the first two and API v1 for Lidarr (design §11).

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::{
    client::{expect_status, expect_status_at, Transport},
    endpoint::{Endpoint, Shape},
    engine::{shortened, Change, Probe, Task},
    error::Error,
    spec::Service,
};

/// The endpoints one API version offers for the tasks in this module.
pub struct Api {
    pub status: Endpoint,
    pub naming_read: Endpoint,
    pub naming_write: Endpoint,
    pub media_management_read: Endpoint,
    pub media_management_write: Endpoint,
    pub root_folders: Endpoint,
    pub root_folder_create: Endpoint,
    /// Lidarr only: Radarr's and Sonarr's root folders are a path and
    /// nothing else, and their API has no update.
    pub root_folder_update: Option<Endpoint>,
    /// Lidarr only: a root folder names default profiles by id.
    pub quality_profiles: Option<Endpoint>,
    pub metadata_profiles: Option<Endpoint>,
}

pub const NAMING: &str = "NamingConfigResource";
pub const MEDIA_MANAGEMENT: &str = "MediaManagementConfigResource";
pub const ROOT_FOLDER: &str = "RootFolderResource";

macro_rules! api {
    ($v:literal, $lidarr:expr) => {
        Api {
            status: Endpoint {
                method: "GET",
                path: concat!("/api/", $v, "/system/status"),
                request: None,
                response: Some(Shape::One("SystemResource")),
            },
            naming_read: Endpoint {
                method: "GET",
                path: concat!("/api/", $v, "/config/naming"),
                request: None,
                response: Some(Shape::Document(NAMING)),
            },
            naming_write: Endpoint {
                method: "PUT",
                path: concat!("/api/", $v, "/config/naming/{id}"),
                request: Some(Shape::Document(NAMING)),
                response: None,
            },
            media_management_read: Endpoint {
                method: "GET",
                path: concat!("/api/", $v, "/config/mediamanagement"),
                request: None,
                response: Some(Shape::Document(MEDIA_MANAGEMENT)),
            },
            media_management_write: Endpoint {
                method: "PUT",
                path: concat!("/api/", $v, "/config/mediamanagement/{id}"),
                request: Some(Shape::Document(MEDIA_MANAGEMENT)),
                response: None,
            },
            root_folders: Endpoint {
                method: "GET",
                path: concat!("/api/", $v, "/rootfolder"),
                request: None,
                response: Some(Shape::Documents(ROOT_FOLDER)),
            },
            root_folder_create: Endpoint {
                method: "POST",
                path: concat!("/api/", $v, "/rootfolder"),
                request: Some(Shape::Document(ROOT_FOLDER)),
                response: None,
            },
            root_folder_update: if $lidarr {
                Some(Endpoint {
                    method: "PUT",
                    path: concat!("/api/", $v, "/rootfolder/{id}"),
                    request: Some(Shape::Document(ROOT_FOLDER)),
                    response: None,
                })
            } else {
                None
            },
            quality_profiles: if $lidarr {
                Some(Endpoint {
                    method: "GET",
                    path: concat!("/api/", $v, "/qualityprofile"),
                    request: None,
                    response: Some(Shape::List("QualityProfileResource")),
                })
            } else {
                None
            },
            metadata_profiles: if $lidarr {
                Some(Endpoint {
                    method: "GET",
                    path: concat!("/api/", $v, "/metadataprofile"),
                    request: None,
                    response: Some(Shape::List("MetadataProfileResource")),
                })
            } else {
                None
            },
        }
    };
}

/// Radarr and Sonarr.
pub static V3: Api = api!("v3", false);
/// Lidarr.
pub static V1: Api = api!("v1", true);

impl Api {
    pub fn of(service: Service) -> Option<&'static Api> {
        match service {
            Service::Radarr | Service::Sonarr => Some(&V3),
            Service::Lidarr => Some(&V1),
            _ => None,
        }
    }

    /// The endpoints of this module's tasks, without the status endpoint
    /// (Radarr and Sonarr already list it for their quality tasks).
    pub fn task_endpoints(&self) -> Vec<Endpoint> {
        let mut endpoints = vec![
            self.naming_read,
            self.naming_write,
            self.media_management_read,
            self.media_management_write,
            self.root_folders,
            self.root_folder_create,
        ];
        endpoints.extend(
            [
                self.root_folder_update,
                self.quality_profiles,
                self.metadata_profiles,
            ]
            .into_iter()
            .flatten(),
        );
        endpoints
    }
}

/// Lidarr's wire types. The status answer shares its component with Radarr
/// and Sonarr; the profile lists are read for their names only.
pub fn lidarr_wire_types() -> Vec<schemars::Schema> {
    vec![
        schemars::schema_for!(crate::services::arr::SystemResource),
        schemars::schema_for!(QualityProfileName),
        schemars::schema_for!(MetadataProfileName),
    ]
}

/// Prowlarr's: only its status answer is typed; providers are documents.
pub fn prowlarr_wire_types() -> Vec<schemars::Schema> {
    vec![schemars::schema_for!(crate::services::arr::SystemResource)]
}

/// A quality profile, read for its id and name. Named after the component so
/// `schema-check` compares exactly these two fields.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[schemars(rename = "QualityProfileResource")]
pub struct QualityProfileName {
    pub id: i64,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[schemars(rename = "MetadataProfileResource")]
pub struct MetadataProfileName {
    pub id: i64,
    #[serde(default)]
    pub name: Option<String>,
}

/// Readiness: the status endpoint answers with a version. A refused key is
/// fatal; anything else is worth waiting for.
pub fn probe(t: &dyn Transport, api: &Api) -> Result<String, Probe> {
    probe_status(t, api.status)
}

/// The same against a status endpoint of any Servarr API (Prowlarr's too).
pub fn probe_status(t: &dyn Transport, ep: Endpoint) -> Result<String, Probe> {
    let reply = t.get(ep.path).map_err(|e| Probe::NotYet(e.to_string()))?;
    match reply.status {
        200 => {}
        401 | 403 => {
            return Err(Probe::Fatal(Error::Status {
                method: ep.method,
                path: ep.path.to_string(),
                status: reply.status,
                validation: vec!["the API key was refused".to_string()],
            }))
        }
        other => return Err(Probe::NotYet(format!("HTTP {other}"))),
    }
    let status: crate::services::arr::SystemResource = serde_json::from_str(&reply.body)
        .map_err(|e| Probe::NotYet(format!("unexpected answer: {e}")))?;
    status
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

fn serialize(method: &'static str, path: &str, value: &Value) -> Result<String, Error> {
    serde_json::to_string(value).map_err(|e| Error::Request {
        method,
        path: path.to_string(),
        reason: format!("cannot serialize: {e}"),
    })
}

/// The id of an object as the service sent it.
fn id_of(object: &Map<String, Value>, path: &str) -> Result<i64, Error> {
    object
        .get("id")
        .and_then(Value::as_i64)
        .ok_or_else(|| Error::Decode {
            path: path.to_string(),
            reason: "no integer id".to_string(),
        })
}

/// One change per named field that differs; a field the object does not
/// carry goes to `missing`.
fn compare(
    subject: &str,
    object: &Map<String, Value>,
    set: &BTreeMap<String, Value>,
    missing: &mut Vec<String>,
) -> Vec<Change> {
    let mut changes = Vec::new();
    for (field, desired) in set {
        match object.get(field) {
            None => missing.push(format!("{subject}: {field}")),
            Some(current) if current != desired => changes.push(Change {
                subject: subject.to_string(),
                field: field.clone(),
                current: shortened(current),
                desired: shortened(desired),
            }),
            Some(_) => {}
        }
    }
    changes
}

// --- naming, media-management ------------------------------------------------

/// Which configuration document a `Document` task reconciles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Naming,
    MediaManagement,
}

/// A configuration document the service keeps as one object with an id:
/// read it, change the named fields, `PUT` it back whole.
pub struct Document {
    pub api: &'static Api,
    pub kind: Kind,
    pub set: BTreeMap<String, Value>,
}

impl Document {
    fn endpoints(&self) -> (Endpoint, Endpoint, &'static str) {
        match self.kind {
            Kind::Naming => (self.api.naming_read, self.api.naming_write, NAMING),
            Kind::MediaManagement => (
                self.api.media_management_read,
                self.api.media_management_write,
                MEDIA_MANAGEMENT,
            ),
        }
    }
}

impl Task for Document {
    type Current = Map<String, Value>;

    fn probe(&self, t: &dyn Transport) -> Result<String, Probe> {
        probe(t, self.api)
    }

    fn read(&self, t: &dyn Transport) -> Result<Self::Current, Error> {
        let (read, _, _) = self.endpoints();
        let reply = t.get(read.path)?;
        expect_status(&read, &reply, &[200])?;
        let document: Value = decode(read.path, &reply.body)?;
        let Value::Object(document) = document else {
            return Err(Error::Decode {
                path: read.path.to_string(),
                reason: "not an object".to_string(),
            });
        };
        id_of(&document, read.path)?;
        Ok(document)
    }

    fn diff(&self, current: &Self::Current) -> Result<Vec<Change>, Error> {
        let (_, _, component) = self.endpoints();
        let mut missing = Vec::new();
        let changes = compare(component, current, &self.set, &mut missing);
        if missing.is_empty() {
            Ok(changes)
        } else {
            Err(Error::MissingField(missing))
        }
    }

    fn notes(&self, _current: &Self::Current) -> Vec<String> {
        Vec::new()
    }

    /// The whole document goes back with only the named fields changed.
    /// Servarr answers `202 Accepted`; the engine reads back.
    fn write(&self, t: &dyn Transport, current: &Self::Current) -> Result<(), Error> {
        let (read, write, _) = self.endpoints();
        let id = id_of(current, read.path)?;
        let path = write.path.replace("{id}", &id.to_string());
        let mut updated = current.clone();
        for (field, value) in &self.set {
            updated.insert(field.clone(), value.clone());
        }
        let body = serialize(write.method, &path, &Value::Object(updated))?;
        let reply = t.put_json(&path, &body)?;
        expect_status_at(write.method, &path, &reply, &[200, 202])
    }
}

// --- root-folders ------------------------------------------------------------

/// One root folder the spec declares, its profiles still by name.
pub struct FolderTarget {
    pub path: String,
    pub set: BTreeMap<String, Value>,
    pub profiles: BTreeMap<String, String>,
}

pub struct RootFolders {
    pub api: &'static Api,
    pub folders: Vec<FolderTarget>,
}

/// The root folders and, if the spec names profiles, the profiles by name.
pub struct Folders {
    pub folders: Vec<Map<String, Value>>,
    pub quality_profiles: Vec<(i64, String)>,
    pub metadata_profiles: Vec<(i64, String)>,
}

fn subject(path: &str) -> String {
    format!("root folder {path}")
}

impl RootFolders {
    fn wants(&self, field: &str) -> bool {
        self.folders.iter().any(|f| f.profiles.contains_key(field))
    }

    fn find<'a>(current: &'a Folders, path: &str) -> Option<&'a Map<String, Value>> {
        current
            .folders
            .iter()
            .find(|f| f.get("path").and_then(Value::as_str) == Some(path))
    }

    /// The spec's `set` plus its profiles resolved to ids. Names that match
    /// no profile go to `unknown`.
    fn resolved(
        target: &FolderTarget,
        current: &Folders,
        unknown: &mut Vec<String>,
    ) -> BTreeMap<String, Value> {
        let mut fields = target.set.clone();
        for (field, name) in &target.profiles {
            let list = if field == "defaultQualityProfileId" {
                &current.quality_profiles
            } else {
                &current.metadata_profiles
            };
            match list.iter().find(|(_, n)| n == name) {
                Some((id, _)) => {
                    fields.insert(field.clone(), Value::from(*id));
                }
                None => unknown.push(format!(
                    "{}: {field} names profile {name:?}, which the service does not have",
                    subject(&target.path)
                )),
            }
        }
        fields
    }

    fn read_profiles(
        t: &dyn Transport,
        ep: Option<Endpoint>,
        what: &str,
    ) -> Result<Vec<(i64, String)>, Error> {
        let Some(ep) = ep else {
            return Err(Error::Request {
                method: "GET",
                path: format!("{what} profiles"),
                reason: "this API has no profiles for root folders".to_string(),
            });
        };
        let reply = t.get(ep.path)?;
        expect_status(&ep, &reply, &[200])?;
        let list: Vec<QualityProfileName> = decode(ep.path, &reply.body)?;
        if list.is_empty() {
            return Err(Error::EmptyList {
                path: ep.path.to_string(),
            });
        }
        list.into_iter()
            .enumerate()
            .map(|(index, p)| {
                p.name.map(|n| (p.id, n)).ok_or_else(|| Error::MissingName {
                    path: ep.path.to_string(),
                    index,
                })
            })
            .collect()
    }
}

impl Task for RootFolders {
    type Current = Folders;

    fn probe(&self, t: &dyn Transport) -> Result<String, Probe> {
        probe(t, self.api)
    }

    /// An empty folder list is a valid answer: a fresh service has none, and
    /// every folder the spec names then shows up as a change.
    fn read(&self, t: &dyn Transport) -> Result<Self::Current, Error> {
        let ep = self.api.root_folders;
        let reply = t.get(ep.path)?;
        expect_status(&ep, &reply, &[200])?;
        let folders: Vec<Map<String, Value>> = decode(ep.path, &reply.body)?;
        if let Some(index) = folders
            .iter()
            .position(|f| f.get("path").and_then(Value::as_str).is_none())
        {
            return Err(Error::MissingName {
                path: ep.path.to_string(),
                index,
            });
        }
        let quality_profiles = if self.wants("defaultQualityProfileId") {
            Self::read_profiles(t, self.api.quality_profiles, "quality")?
        } else {
            Vec::new()
        };
        let metadata_profiles = if self.wants("defaultMetadataProfileId") {
            Self::read_profiles(t, self.api.metadata_profiles, "metadata")?
        } else {
            Vec::new()
        };
        Ok(Folders {
            folders,
            quality_profiles,
            metadata_profiles,
        })
    }

    fn diff(&self, current: &Self::Current) -> Result<Vec<Change>, Error> {
        let (mut missing, mut unknown, mut changes) = (Vec::new(), Vec::new(), Vec::new());
        for target in &self.folders {
            let fields = Self::resolved(target, current, &mut unknown);
            match Self::find(current, &target.path) {
                Some(folder) => changes.extend(compare(
                    &subject(&target.path),
                    folder,
                    &fields,
                    &mut missing,
                )),
                None => changes.push(Change {
                    subject: subject(&target.path),
                    field: String::new(),
                    current: "(missing)".to_string(),
                    desired: "(added)".to_string(),
                }),
            }
        }
        if !unknown.is_empty() {
            return Err(Error::NotFound(unknown));
        }
        if !missing.is_empty() {
            return Err(Error::MissingField(missing));
        }
        Ok(changes)
    }

    fn notes(&self, current: &Self::Current) -> Vec<String> {
        current
            .folders
            .iter()
            .filter_map(|f| f.get("path").and_then(Value::as_str))
            .filter(|path| !self.folders.iter().any(|t| t.path == *path))
            .map(|path| format!("not in the spec: {}", subject(path)))
            .collect()
    }

    fn write(&self, t: &dyn Transport, current: &Self::Current) -> Result<(), Error> {
        for target in &self.folders {
            let mut unknown = Vec::new();
            let fields = Self::resolved(target, current, &mut unknown);
            if !unknown.is_empty() {
                return Err(Error::NotFound(unknown));
            }
            let Some(folder) = Self::find(current, &target.path) else {
                let ep = self.api.root_folder_create;
                let mut body = Map::new();
                body.insert("path".to_string(), Value::from(target.path.clone()));
                body.extend(fields);
                let body = serialize(ep.method, ep.path, &Value::Object(body))?;
                let reply = t.post_json(ep.path, &body)?;
                // Servarr answers 201 Created.
                expect_status(&ep, &reply, &[200, 201])?;
                continue;
            };
            let mut missing = Vec::new();
            if compare(&subject(&target.path), folder, &fields, &mut missing).is_empty() {
                continue;
            }
            let Some(ep) = self.api.root_folder_update else {
                return Err(Error::Request {
                    method: "PUT",
                    path: self.api.root_folders.path.to_string(),
                    reason: format!(
                        "{} differs, but this API cannot update a root folder",
                        subject(&target.path)
                    ),
                });
            };
            let path = ep.path.replace(
                "{id}",
                &id_of(folder, self.api.root_folders.path)?.to_string(),
            );
            let mut updated = folder.clone();
            updated.extend(fields);
            let body = serialize(ep.method, &path, &Value::Object(updated))?;
            let reply = t.put_json(&path, &body)?;
            expect_status_at(ep.method, &path, &reply, &[200, 202])?;
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

    const RADARR_NAMING: &str = include_str!("../../tests/fixtures/radarr-6.3.0.10514/naming.json");
    const RADARR_MM: &str =
        include_str!("../../tests/fixtures/radarr-6.3.0.10514/mediamanagement.json");
    const RADARR_FOLDERS: &str =
        include_str!("../../tests/fixtures/radarr-6.3.0.10514/rootfolder.json");
    const RADARR_STATUS: &str =
        include_str!("../../tests/fixtures/radarr-6.3.0.10514/system-status.json");
    const SONARR_NAMING: &str = include_str!("../../tests/fixtures/sonarr-4.0.19.2979/naming.json");
    const LIDARR_STATUS: &str =
        include_str!("../../tests/fixtures/lidarr-3.1.0.4875/system-status.json");
    const LIDARR_MM: &str =
        include_str!("../../tests/fixtures/lidarr-3.1.0.4875/mediamanagement.json");
    const LIDARR_FOLDERS: &str =
        include_str!("../../tests/fixtures/lidarr-3.1.0.4875/rootfolder.json");
    const LIDARR_QUALITY: &str =
        include_str!("../../tests/fixtures/lidarr-3.1.0.4875/qualityprofile.json");
    const LIDARR_METADATA: &str =
        include_str!("../../tests/fixtures/lidarr-3.1.0.4875/metadataprofile.json");

    fn fields(value: Value) -> BTreeMap<String, Value> {
        value.as_object().unwrap().clone().into_iter().collect()
    }

    fn document(api: &'static Api, kind: Kind, set: Value) -> Document {
        Document {
            api,
            kind,
            set: fields(set),
        }
    }

    #[test]
    fn the_paths_differ_by_api_version() {
        assert_eq!(V3.naming_write.path, "/api/v3/config/naming/{id}");
        assert_eq!(
            V1.media_management_read.path,
            "/api/v1/config/mediamanagement"
        );
        assert!(V3.root_folder_update.is_none() && V1.root_folder_update.is_some());
        assert_eq!(V3.task_endpoints().len(), 6);
        assert_eq!(V1.task_endpoints().len(), 9);
        assert!(std::ptr::eq(Api::of(Service::Lidarr).unwrap(), &V1));
        assert!(Api::of(Service::Jellyfin).is_none());
    }

    #[test]
    fn the_hosts_documents_match_what_the_services_hold() {
        // From the host's tofu configuration, in API names.
        let naming = document(
            &V3,
            Kind::Naming,
            json!({
                "renameMovies": true, "replaceIllegalCharacters": true,
                "colonReplacementFormat": "smart",
                "movieFolderFormat": "{Movie CleanTitle} ({Release Year}) [imdbid-{ImdbId}]"
            }),
        );
        let t = FakeTransport::default().on_get(V3.naming_read.path, vec![ok(RADARR_NAMING)]);
        assert_eq!(naming.diff(&naming.read(&t).unwrap()).unwrap(), vec![]);

        let lidarr = document(
            &V1,
            Kind::MediaManagement,
            json!({"copyUsingHardlinks": false, "allowFingerprinting": "newFiles",
                   "watchLibraryForChanges": true, "extraFileExtensions": "lrc,cue,nfo"}),
        );
        let t = FakeTransport::default().on_get(V1.media_management_read.path, vec![ok(LIDARR_MM)]);
        assert_eq!(lidarr.diff(&lidarr.read(&t).unwrap()).unwrap(), vec![]);

        // Sonarr's colon replacement is a number, Radarr's a string.
        let sonarr = document(
            &V3,
            Kind::Naming,
            json!({"colonReplacementFormat": 4, "multiEpisodeStyle": 5}),
        );
        let t = FakeTransport::default().on_get(V3.naming_read.path, vec![ok(SONARR_NAMING)]);
        assert_eq!(sonarr.diff(&sonarr.read(&t).unwrap()).unwrap(), vec![]);
    }

    #[test]
    fn a_differing_field_is_written_back_with_the_whole_document() {
        let task = document(
            &V3,
            Kind::MediaManagement,
            json!({"copyUsingHardlinks": false}),
        );
        let changed = {
            let mut v: Value = serde_json::from_str(RADARR_MM).unwrap();
            v["copyUsingHardlinks"] = false.into();
            v.to_string()
        };
        let t = FakeTransport::default()
            .on_get(V3.status.path, vec![ok(RADARR_STATUS)])
            .on_get(
                V3.media_management_read.path,
                vec![ok(RADARR_MM), ok(RADARR_MM), ok(&changed)],
            )
            .on_put(vec![Step::Answer(202, String::new())]);
        let report = run(Mode::Apply, &task, &t, &FakeClock::new(), Timing::default()).unwrap();
        match report.outcome {
            Outcome::Changed(changes) => assert_eq!(
                changes[0].to_string(),
                "MediaManagementConfigResource: copyUsingHardlinks true -> false"
            ),
            other => panic!("expected Changed, got {other:?}"),
        }
        let written = t.written.borrow();
        assert_eq!(written.len(), 1);
        assert_eq!(written[0].0, "/api/v3/config/mediamanagement/1");
        let sent: Value = serde_json::from_str(&written[0].1).unwrap();
        let original: Value = serde_json::from_str(RADARR_MM).unwrap();
        assert_eq!(sent["copyUsingHardlinks"], false);
        // Every other field goes back as it came.
        for (key, value) in original.as_object().unwrap() {
            if key != "copyUsingHardlinks" {
                assert_eq!(&sent[key], value, "{key}");
            }
        }
    }

    #[test]
    fn a_field_the_document_lacks_is_an_error_and_a_document_without_id_too() {
        let task = document(&V3, Kind::Naming, json!({"renameEpisodes": true}));
        let t = FakeTransport::default().on_get(V3.naming_read.path, vec![ok(RADARR_NAMING)]);
        let err = task
            .diff(&task.read(&t).unwrap())
            .err()
            .unwrap()
            .to_string();
        assert_eq!(
            err,
            "the answer has no such field: NamingConfigResource: renameEpisodes"
        );
        let t = FakeTransport::default()
            .on_get(V3.naming_read.path, vec![ok(r#"{"renameMovies":true}"#)]);
        assert!(task
            .read(&t)
            .err()
            .unwrap()
            .to_string()
            .contains("no integer id"));
        let t = FakeTransport::default().on_get(V3.naming_read.path, vec![ok("[]")]);
        assert!(task
            .read(&t)
            .err()
            .unwrap()
            .to_string()
            .contains("not an object"));
    }

    #[test]
    fn lidarr_waits_on_its_own_status_endpoint() {
        let task = document(&V1, Kind::Naming, json!({"renameTracks": false}));
        let t = FakeTransport::default().on_get("/api/v1/system/status", vec![ok(LIDARR_STATUS)]);
        assert_eq!(task.probe(&t).ok().unwrap(), "3.1.0.4875");
        let refused = FakeTransport::default().on_get(
            "/api/v1/system/status",
            vec![Step::Answer(401, String::new())],
        );
        assert!(matches!(task.probe(&refused), Err(Probe::Fatal(_))));
    }

    fn lidarr_folders(set: Value, quality: &str) -> RootFolders {
        RootFolders {
            api: &V1,
            folders: vec![FolderTarget {
                path: "/tank/data/media/music".to_string(),
                set: fields(set),
                profiles: fields(json!({
                    "defaultQualityProfileId": quality,
                    "defaultMetadataProfileId": "Standard"
                }))
                .into_iter()
                .map(|(k, v)| (k, v.as_str().unwrap().to_string()))
                .collect(),
            }],
        }
    }

    fn lidarr_transport(folders: Vec<Step>) -> FakeTransport {
        FakeTransport::default()
            .on_get(V1.status.path, vec![ok(LIDARR_STATUS)])
            .on_get(V1.root_folders.path, folders)
            .on_get("/api/v1/qualityprofile", vec![ok(LIDARR_QUALITY)])
            .on_get("/api/v1/metadataprofile", vec![ok(LIDARR_METADATA)])
    }

    fn host_set() -> Value {
        json!({"name": "Musik", "defaultMonitorOption": "all", "defaultNewItemMonitorOption": "all"})
    }

    #[test]
    fn the_hosts_lidarr_folder_matches_with_profiles_by_name() {
        let task = lidarr_folders(host_set(), "Standard");
        let t = lidarr_transport(vec![ok(LIDARR_FOLDERS)]);
        let current = task.read(&t).unwrap();
        assert_eq!(task.diff(&current).unwrap(), vec![]);
        assert!(task.notes(&current).is_empty());
    }

    #[test]
    fn a_profile_by_another_name_is_one_change_and_written_with_its_id() {
        let task = lidarr_folders(host_set(), "Lossless");
        let changed = {
            let mut v: Value = serde_json::from_str(LIDARR_FOLDERS).unwrap();
            assert_eq!(v[0]["defaultQualityProfileId"], 3, "fixture changed");
            v[0]["defaultQualityProfileId"] = 2.into();
            v.to_string()
        };
        let t = lidarr_transport(vec![ok(LIDARR_FOLDERS), ok(LIDARR_FOLDERS), ok(&changed)])
            .on_put(vec![Step::Answer(202, String::new())]);
        let report = run(Mode::Apply, &task, &t, &FakeClock::new(), Timing::default()).unwrap();
        match report.outcome {
            Outcome::Changed(changes) => assert_eq!(
                changes[0].to_string(),
                "root folder /tank/data/media/music: defaultQualityProfileId 3 -> 2"
            ),
            other => panic!("expected Changed, got {other:?}"),
        }
        let written = t.written.borrow();
        assert_eq!(written[0].0, "/api/v1/rootfolder/1");
        let sent: Value = serde_json::from_str(&written[0].1).unwrap();
        assert_eq!(sent["defaultQualityProfileId"], 2);
        assert_eq!(sent["path"], "/tank/data/media/music");
        assert_eq!(sent["name"], "Musik");
    }

    #[test]
    fn an_unknown_profile_name_is_an_error() {
        let task = lidarr_folders(host_set(), "Hi-Res");
        let t = lidarr_transport(vec![ok(LIDARR_FOLDERS)]);
        let err = task
            .diff(&task.read(&t).unwrap())
            .err()
            .unwrap()
            .to_string();
        assert_eq!(
            err,
            "not found on the service: root folder /tank/data/media/music: defaultQualityProfileId names profile \"Hi-Res\", which the service does not have"
        );
    }

    #[test]
    fn a_missing_folder_is_added_with_path_fields_and_profile_ids() {
        let task = lidarr_folders(host_set(), "Standard");
        let t = lidarr_transport(vec![ok("[]"), ok(LIDARR_FOLDERS)])
            .on_put(vec![Step::Answer(201, "{}".into())]);
        let report = run(Mode::Apply, &task, &t, &FakeClock::new(), Timing::default()).unwrap();
        match report.outcome {
            Outcome::Changed(changes) => assert_eq!(
                changes[0].to_string(),
                "root folder /tank/data/media/music: (missing) -> (added)"
            ),
            other => panic!("expected Changed, got {other:?}"),
        }
        let written = t.written.borrow();
        assert_eq!(written[0].0, "/api/v1/rootfolder");
        let sent: Value = serde_json::from_str(&written[0].1).unwrap();
        assert_eq!(
            sent,
            json!({"path": "/tank/data/media/music", "name": "Musik",
                   "defaultMonitorOption": "all", "defaultNewItemMonitorOption": "all",
                   "defaultQualityProfileId": 3, "defaultMetadataProfileId": 1})
        );
    }

    #[test]
    fn radarr_folders_are_paths_and_others_are_a_note() {
        let task = RootFolders {
            api: &V3,
            folders: vec![FolderTarget {
                path: "/tank/data/media/movies".to_string(),
                set: BTreeMap::new(),
                profiles: BTreeMap::new(),
            }],
        };
        // No profile list is read when the spec names no profile.
        let t = FakeTransport::default().on_get(V3.root_folders.path, vec![ok(RADARR_FOLDERS)]);
        let current = task.read(&t).unwrap();
        assert_eq!(task.diff(&current).unwrap(), vec![]);

        let other = RootFolders {
            api: &V3,
            folders: vec![FolderTarget {
                path: "/tank/data/media/filme".to_string(),
                set: BTreeMap::new(),
                profiles: BTreeMap::new(),
            }],
        };
        assert_eq!(
            other.notes(&current),
            ["not in the spec: root folder /tank/data/media/movies"]
        );
        let t = FakeTransport::default()
            .on_get(V3.root_folders.path, vec![ok(RADARR_FOLDERS)])
            .on_put(vec![Step::Answer(201, "{}".into())]);
        other.write(&t, &current).unwrap();
        assert_eq!(
            t.written.borrow()[0],
            (
                "/api/v3/rootfolder".to_string(),
                r#"{"path":"/tank/data/media/filme"}"#.to_string()
            )
        );
    }

    #[test]
    fn a_folder_without_path_and_an_empty_profile_list_are_errors() {
        let task = lidarr_folders(host_set(), "Standard");
        let t = lidarr_transport(vec![ok(r#"[{"id":1}]"#)]);
        assert!(task
            .read(&t)
            .err()
            .unwrap()
            .to_string()
            .contains("has no name"));
        let t = FakeTransport::default()
            .on_get(V1.root_folders.path, vec![ok(LIDARR_FOLDERS)])
            .on_get("/api/v1/qualityprofile", vec![ok("[]")]);
        assert!(task
            .read(&t)
            .err()
            .unwrap()
            .to_string()
            .contains("empty list"));
    }

    #[test]
    fn lidarr_wire_types_carry_the_component_names() {
        let titles: Vec<String> = lidarr_wire_types()
            .iter()
            .map(|s| s.as_value()["title"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            titles,
            [
                "SystemResource",
                "QualityProfileResource",
                "MetadataProfileResource"
            ]
        );
    }
}
