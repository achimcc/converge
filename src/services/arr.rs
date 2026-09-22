use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{
    client::{expect_status, Transport},
    endpoint::{Endpoint, Shape},
    engine::{Change, Probe, Task},
    error::Error,
    spec::{show, SizeLimits},
};

pub const SYSTEM_STATUS: Endpoint = Endpoint {
    method: "GET",
    path: "/api/v3/system/status",
    request: None,
    response: Some(Shape::One("SystemResource")),
};
pub const QUALITY_LIST: Endpoint = Endpoint {
    method: "GET",
    path: "/api/v3/qualitydefinition",
    request: None,
    response: Some(Shape::List("QualityDefinitionResource")),
};
pub const QUALITY_UPDATE: Endpoint = Endpoint {
    method: "PUT",
    path: "/api/v3/qualitydefinition/update",
    request: Some(Shape::List("QualityDefinitionResource")),
    response: None,
};
pub mod formats;
pub mod profiles;

pub use formats::{CustomFormats, CUSTOM_FORMAT_CREATE, CUSTOM_FORMAT_LIST, CUSTOM_FORMAT_UPDATE};
pub use profiles::{QualityProfiles, PROFILE_DELETE, PROFILE_LIST, PROFILE_UPDATE};

pub const ENDPOINTS: [Endpoint; 9] = [
    SYSTEM_STATUS,
    QUALITY_LIST,
    QUALITY_UPDATE,
    PROFILE_LIST,
    PROFILE_UPDATE,
    PROFILE_DELETE,
    CUSTOM_FORMAT_LIST,
    CUSTOM_FORMAT_CREATE,
    CUSTOM_FORMAT_UPDATE,
];

/// The wire types, each named exactly like its OpenAPI component. Their
/// fields are what `schema-check` compares -- derived, not listed by hand.
pub fn wire_types() -> Vec<schemars::Schema> {
    vec![
        schemars::schema_for!(SystemResource),
        schemars::schema_for!(QualityDefinitionResource),
        schemars::schema_for!(profiles::QualityProfileResource),
    ]
}

/// Readiness for Radarr and Sonarr: the status endpoint answers with a
/// version. A refused key is fatal; anything else is worth waiting for.
pub fn probe(t: &dyn Transport) -> Result<String, Probe> {
    let reply = t
        .get(SYSTEM_STATUS.path)
        .map_err(|e| Probe::NotYet(e.to_string()))?;
    match reply.status {
        200 => {}
        401 | 403 => {
            return Err(Probe::Fatal(Error::Status {
                method: SYSTEM_STATUS.method,
                path: SYSTEM_STATUS.path.to_string(),
                status: reply.status,
                validation: vec!["the API key was refused".to_string()],
            }))
        }
        other => return Err(Probe::NotYet(format!("HTTP {other}"))),
    }
    let status: SystemResource = serde_json::from_str(&reply.body)
        .map_err(|e| Probe::NotYet(format!("unexpected answer: {}", crate::error::shape(&e))))?;
    status
        .version
        .filter(|v| !v.is_empty())
        .ok_or_else(|| Probe::NotYet("the answer has no version".to_string()))
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SystemResource {
    #[serde(default)]
    pub version: Option<String>,
}

/// Declares only what this program reads or writes. Everything else the
/// service sends is kept in `rest` and written back untouched.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Quality {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(flatten)]
    #[schemars(skip)]
    pub rest: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct QualityDefinitionResource {
    pub quality: Quality,
    // `default`: the service omits null values instead of writing `null`.
    #[serde(default)]
    pub min_size: Option<f64>,
    #[serde(default)]
    pub preferred_size: Option<f64>,
    #[serde(default)]
    pub max_size: Option<f64>,
    #[serde(flatten)]
    #[schemars(skip)]
    pub rest: Map<String, Value>,
}

/// Sets the size limits of the qualities the spec names; leaves the others.
pub struct QualityDefinitions {
    pub desired: BTreeMap<String, SizeLimits>,
}

impl Task for QualityDefinitions {
    type Current = Vec<QualityDefinitionResource>;

    fn probe(&self, t: &dyn Transport) -> Result<String, Probe> {
        probe(t)
    }

    fn read(&self, t: &dyn Transport) -> Result<Self::Current, Error> {
        let path = QUALITY_LIST.path.to_string();
        let reply = t.get(QUALITY_LIST.path)?;
        expect_status(&QUALITY_LIST, &reply, &[200])?;
        let list: Vec<QualityDefinitionResource> =
            serde_json::from_str(&reply.body).map_err(|e| Error::Decode {
                path: path.clone(),
                reason: crate::error::shape(&e),
            })?;
        if list.is_empty() {
            return Err(Error::EmptyList { path });
        }
        if let Some(index) = list.iter().position(|d| d.quality.name.is_none()) {
            return Err(Error::MissingName { path, index });
        }
        Ok(list)
    }

    fn diff(&self, current: &Self::Current) -> Result<Vec<Change>, Error> {
        let known: BTreeSet<&str> = current
            .iter()
            .filter_map(|d| d.quality.name.as_deref())
            .collect();
        let unknown: Vec<String> = self
            .desired
            .keys()
            .filter(|name| !known.contains(name.as_str()))
            .cloned()
            .collect();
        if !unknown.is_empty() {
            return Err(Error::UnknownQualities(unknown));
        }
        let mut changes = Vec::new();
        for entry in current {
            let Some(name) = entry.quality.name.as_deref() else {
                continue;
            };
            let Some(want) = self.desired.get(name) else {
                continue;
            };
            for (field, have, wanted) in [
                ("min", entry.min_size, want.min),
                ("preferred", entry.preferred_size, want.preferred),
                ("max", entry.max_size, want.max),
            ] {
                // Exact: both sides are parsed from the same decimal notation.
                if have != wanted {
                    changes.push(Change {
                        subject: name.to_string(),
                        field: field.to_string(),
                        current: show(have),
                        desired: show(wanted),
                    });
                }
            }
        }
        Ok(changes)
    }

    fn notes(&self, current: &Self::Current) -> Vec<String> {
        let untouched: Vec<&str> = current
            .iter()
            .filter_map(|d| d.quality.name.as_deref())
            .filter(|name| !self.desired.contains_key(*name))
            .collect();
        if untouched.is_empty() {
            Vec::new()
        } else {
            vec![format!(
                "not in the spec, left as they are: {}",
                untouched.join(", ")
            )]
        }
    }

    fn write(&self, t: &dyn Transport, current: &Self::Current) -> Result<(), Error> {
        let updated: Vec<QualityDefinitionResource> = current
            .iter()
            .cloned()
            .map(|mut entry| {
                let want = entry
                    .quality
                    .name
                    .as_deref()
                    .and_then(|n| self.desired.get(n))
                    .copied();
                if let Some(want) = want {
                    entry.min_size = want.min;
                    entry.preferred_size = want.preferred;
                    entry.max_size = want.max;
                }
                entry
            })
            .collect();
        let body = serde_json::to_string(&updated).map_err(|e| Error::Request {
            method: QUALITY_UPDATE.method,
            path: QUALITY_UPDATE.path.to_string(),
            reason: format!("cannot serialize: {e}"),
        })?;
        let reply = t.put_json(QUALITY_UPDATE.path, &body)?;
        // 202 is what Radarr actually answers (observed 2026-09-06), although
        // its OpenAPI file lists only 200 -- and 202 means accepted, not
        // saved, which is why the engine reads back afterwards.
        expect_status(&QUALITY_UPDATE, &reply, &[200, 202])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub const RADARR_LIST: &str =
        include_str!("../../tests/fixtures/radarr-6.3.0.10514/qualitydefinition.json");
    pub const SONARR_LIST: &str =
        include_str!("../../tests/fixtures/sonarr-4.0.19.2979/qualitydefinition.json");

    fn by_name<'a>(
        list: &'a [QualityDefinitionResource],
        name: &str,
    ) -> &'a QualityDefinitionResource {
        list.iter()
            .find(|d| d.quality.name.as_deref() == Some(name))
            .unwrap()
    }

    #[test]
    fn recorded_answers_parse() {
        let radarr: Vec<QualityDefinitionResource> = serde_json::from_str(RADARR_LIST).unwrap();
        let sonarr: Vec<QualityDefinitionResource> = serde_json::from_str(SONARR_LIST).unwrap();
        assert!(radarr.len() > 20 && sonarr.len() > 20);
    }

    #[test]
    fn an_absent_nullable_field_is_none() {
        // Radarr omits null values: this entry has no maxSize key at all.
        let raw: Value = serde_json::from_str(RADARR_LIST).unwrap();
        let entry = raw
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["quality"]["name"] == "Bluray-1080p")
            .unwrap();
        assert!(
            entry.get("maxSize").is_none(),
            "fixture changed; pick another entry"
        );
        let list: Vec<QualityDefinitionResource> = serde_json::from_str(RADARR_LIST).unwrap();
        assert_eq!(by_name(&list, "Bluray-1080p").max_size, None);
    }

    #[test]
    fn unknown_fields_survive_a_round_trip() {
        let list: Vec<QualityDefinitionResource> = serde_json::from_str(RADARR_LIST).unwrap();
        let back = serde_json::to_value(by_name(&list, "Unknown")).unwrap();
        assert_eq!(back["quality"]["modifier"], "none");
        assert_eq!(back["id"], 1);
        assert!(back.get("weight").is_some() && back.get("title").is_some());
    }

    #[test]
    fn none_is_written_as_explicit_null() {
        let list: Vec<QualityDefinitionResource> = serde_json::from_str(RADARR_LIST).unwrap();
        let back = serde_json::to_value(by_name(&list, "Bluray-1080p")).unwrap();
        assert_eq!(back.get("maxSize"), Some(&Value::Null));
    }

    #[test]
    fn a_missing_quality_object_is_an_error() {
        assert!(serde_json::from_str::<QualityDefinitionResource>(r#"{"minSize":1}"#).is_err());
    }

    use std::collections::BTreeMap;

    use crate::{
        engine::{Probe, Task},
        spec::SizeLimits,
        testing::{ok, FakeTransport, Step},
    };

    const RADARR_DESIRED: &str =
        include_str!("../../tests/fixtures/radarr-6.3.0.10514/desired.json");
    const SONARR_DESIRED: &str =
        include_str!("../../tests/fixtures/sonarr-4.0.19.2979/desired.json");
    const RADARR_STATUS: &str =
        include_str!("../../tests/fixtures/radarr-6.3.0.10514/system-status.json");

    fn task(desired: &str) -> QualityDefinitions {
        let desired: BTreeMap<String, SizeLimits> = serde_json::from_str(desired).unwrap();
        QualityDefinitions { desired }
    }

    fn radarr_desired_with_bluray_min(min: f64) -> String {
        let mut desired: Value = serde_json::from_str(RADARR_DESIRED).unwrap();
        desired["Bluray-1080p"]["min"] = min.into();
        desired.to_string()
    }

    fn listing(body: &str) -> FakeTransport {
        FakeTransport::default().on_get(QUALITY_LIST.path, vec![ok(body)])
    }

    #[test]
    fn the_hosts_table_matches_what_the_service_holds() {
        // The shell unit this replaces wrote on every run against exactly
        // this state, because it compared an absent key with `null`.
        for (desired, list) in [(RADARR_DESIRED, RADARR_LIST), (SONARR_DESIRED, SONARR_LIST)] {
            let task = task(desired);
            let current = task.read(&listing(list)).unwrap();
            assert_eq!(task.diff(&current).unwrap(), vec![]);
        }
    }

    #[test]
    fn one_changed_field_is_one_change() {
        let task = task(&radarr_desired_with_bluray_min(35.0));
        let current = task.read(&listing(RADARR_LIST)).unwrap();
        let changes = task.diff(&current).unwrap();
        assert_eq!(changes.len(), 1, "{changes:?}");
        assert_eq!(changes[0].to_string(), "Bluray-1080p: min 12.5 -> 35");
    }

    #[test]
    fn an_unknown_quality_aborts_with_all_names() {
        let task = task(
            r#"{"Bluray-9000p": {"min":0,"preferred":null,"max":null}, "VHS": {"min":0,"preferred":null,"max":null}}"#,
        );
        let current = task.read(&listing(RADARR_LIST)).unwrap();
        let err = task.diff(&current).err().unwrap().to_string();
        assert!(err.ends_with("Bluray-9000p, VHS"), "{err}");
    }

    #[test]
    fn qualities_outside_the_spec_are_a_note() {
        let task = task(r#"{"CAM": {"min":0,"preferred":95,"max":100}}"#);
        let current = task.read(&listing(RADARR_LIST)).unwrap();
        let notes = task.notes(&current);
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("Bluray-1080p"), "{}", notes[0]);
        assert!(!notes[0].contains("CAM"), "{}", notes[0]);
    }

    #[test]
    fn empty_list_and_nameless_entry_are_errors() {
        let t = task(RADARR_DESIRED);
        let err = t.read(&listing("[]")).err().unwrap().to_string();
        assert!(err.contains("empty list"), "{err}");
        let nameless = r#"[{"quality":{"id":0},"minSize":0}]"#;
        let err = t.read(&listing(nameless)).err().unwrap().to_string();
        assert!(err.contains("has no name"), "{err}");
        let err = t
            .read(&listing(r#"[{"minSize":0}]"#))
            .err()
            .unwrap()
            .to_string();
        assert!(err.contains("missing field `quality`"), "{err}");
    }

    #[test]
    fn write_sends_the_full_list_with_explicit_nulls_and_unknown_fields() {
        let task = task(&radarr_desired_with_bluray_min(35.0));
        let transport = listing(RADARR_LIST).on_put(vec![Step::Answer(202, String::new())]);
        let current = task.read(&transport).unwrap();
        task.write(&transport, &current).unwrap();
        let written = transport.written.borrow();
        assert_eq!(written.len(), 1);
        assert_eq!(written[0].0, QUALITY_UPDATE.path);
        let sent: Value = serde_json::from_str(&written[0].1).unwrap();
        let sent = sent.as_array().unwrap();
        assert_eq!(sent.len(), current.len());
        let entry = sent
            .iter()
            .find(|e| e["quality"]["name"] == "Bluray-1080p")
            .unwrap();
        assert_eq!(entry["minSize"], 35.0);
        assert_eq!(entry["maxSize"], Value::Null);
        assert_eq!(entry["quality"]["modifier"], "none");
        assert!(entry.get("id").is_some() && entry.get("weight").is_some());
    }

    #[test]
    fn write_rejects_other_statuses() {
        let task = task(RADARR_DESIRED);
        let transport = listing(RADARR_LIST).on_put(vec![Step::Answer(500, "boom".into())]);
        let current = task.read(&transport).unwrap();
        assert_eq!(
            task.write(&transport, &current).err().unwrap().to_string(),
            "PUT /api/v3/qualitydefinition/update answered HTTP 500"
        );
    }

    #[test]
    fn probe_distinguishes_not_yet_from_fatal() {
        let t = task(RADARR_DESIRED);
        let status = |steps| FakeTransport::default().on_get(SYSTEM_STATUS.path, steps);
        assert_eq!(
            t.probe(&status(vec![ok(RADARR_STATUS)])).ok().unwrap(),
            "6.3.0.10514"
        );
        assert!(matches!(
            t.probe(&status(vec![Step::Refused])),
            Err(Probe::NotYet(_))
        ));
        assert!(matches!(
            t.probe(&status(vec![Step::Answer(503, String::new())])),
            Err(Probe::NotYet(_))
        ));
        assert!(matches!(
            t.probe(&status(vec![ok("{}")])),
            Err(Probe::NotYet(_))
        ));
        assert!(matches!(
            t.probe(&status(vec![Step::Answer(401, String::new())])),
            Err(Probe::Fatal(_))
        ));
    }

    #[test]
    fn wire_types_are_named_after_their_components() {
        let titles: Vec<String> = wire_types()
            .iter()
            .map(|s| s.as_value()["title"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            titles,
            [
                "SystemResource",
                "QualityDefinitionResource",
                "QualityProfileResource"
            ]
        );
    }
}
