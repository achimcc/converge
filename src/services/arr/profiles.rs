//! `quality-profiles`: every quality profile allows the named qualities.
//!
//! Only allowing is possible. A profile's ladder (which qualities it has, in
//! which order, grouped how) belongs to whoever defined the profile -- on the
//! host this was written for, Recyclarr and TRaSH's templates. converge only
//! flips `allowed` on a top-level item that is already there.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{probe, Quality};
use crate::{
    client::{expect_status, expect_status_at, Transport},
    endpoint::{Endpoint, Shape},
    engine::{Change, Probe, Task},
    error::Error,
};

pub const PROFILE_LIST: Endpoint = Endpoint {
    method: "GET",
    path: "/api/v3/qualityprofile",
    request: None,
    response: Some(Shape::List("QualityProfileResource")),
};
pub const PROFILE_UPDATE: Endpoint = Endpoint {
    method: "PUT",
    path: "/api/v3/qualityprofile/{id}",
    request: Some(Shape::One("QualityProfileResource")),
    response: None,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct QualityProfileResource {
    pub id: i32,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub items: Option<Vec<QualityProfileQualityItemResource>>,
    #[serde(flatten)]
    #[schemars(skip)]
    pub rest: Map<String, Value>,
}

/// One rung of a profile's ladder. A group has no `quality` key at all, and
/// it must not get one on the way back -- hence `skip_serializing_if`.
/// Nested `items` of a group stay in `rest`, untouched.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct QualityProfileQualityItemResource {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality: Option<Quality>,
    pub allowed: bool,
    #[serde(flatten)]
    #[schemars(skip)]
    pub rest: Map<String, Value>,
}

pub struct QualityProfiles {
    pub allow_in_every_profile: Vec<String>,
}

fn profile_name(profile: &QualityProfileResource) -> &str {
    profile.name.as_deref().unwrap_or("?")
}

/// The top-level item of `quality` in `profile`, if there is one.
fn top_level<'a>(
    profile: &'a QualityProfileResource,
    quality: &str,
) -> Option<&'a QualityProfileQualityItemResource> {
    profile
        .items
        .as_deref()
        .unwrap_or_default()
        .iter()
        .find(|item| item.quality.as_ref().and_then(|q| q.name.as_deref()) == Some(quality))
}

impl QualityProfiles {
    /// The profile as it should be, or `None` if it already is.
    fn corrected(&self, profile: &QualityProfileResource) -> Option<QualityProfileResource> {
        let mut fixed = profile.clone();
        let mut changed = false;
        for item in fixed.items.iter_mut().flatten() {
            let name = item.quality.as_ref().and_then(|q| q.name.as_deref());
            if let Some(name) = name {
                if !item.allowed && self.allow_in_every_profile.iter().any(|a| a == name) {
                    item.allowed = true;
                    changed = true;
                }
            }
        }
        changed.then_some(fixed)
    }
}

impl Task for QualityProfiles {
    type Current = Vec<QualityProfileResource>;

    fn probe(&self, t: &dyn Transport) -> Result<String, Probe> {
        probe(t)
    }

    fn read(&self, t: &dyn Transport) -> Result<Self::Current, Error> {
        let path = PROFILE_LIST.path.to_string();
        let reply = t.get(PROFILE_LIST.path)?;
        expect_status(&PROFILE_LIST, &reply, &[200])?;
        let list: Vec<QualityProfileResource> =
            serde_json::from_str(&reply.body).map_err(|e| Error::Decode {
                path: path.clone(),
                reason: e.to_string(),
            })?;
        if list.is_empty() {
            return Err(Error::EmptyList { path });
        }
        if let Some(index) = list.iter().position(|p| p.name.is_none()) {
            return Err(Error::MissingName { path, index });
        }
        Ok(list)
    }

    fn diff(&self, current: &Self::Current) -> Result<Vec<Change>, Error> {
        let mut missing = Vec::new();
        let mut changes = Vec::new();
        for profile in current {
            for quality in &self.allow_in_every_profile {
                match top_level(profile, quality) {
                    None => missing.push(format!("{}: {quality}", profile_name(profile))),
                    Some(item) if !item.allowed => changes.push(Change {
                        subject: format!("{}: {quality}", profile_name(profile)),
                        field: "allowed".to_string(),
                        current: "false".to_string(),
                        desired: "true".to_string(),
                    }),
                    Some(_) => {}
                }
            }
        }
        // A quality that is not a top-level rung cannot be switched on without
        // rebuilding the ladder -- which is not converge's to do. Say so.
        if !missing.is_empty() {
            return Err(Error::MissingItem(missing));
        }
        Ok(changes)
    }

    fn notes(&self, _current: &Self::Current) -> Vec<String> {
        Vec::new()
    }

    fn write(&self, t: &dyn Transport, current: &Self::Current) -> Result<(), Error> {
        for profile in current {
            let Some(fixed) = self.corrected(profile) else {
                continue;
            };
            let path = PROFILE_UPDATE.path.replace("{id}", &profile.id.to_string());
            let body = serde_json::to_string(&fixed).map_err(|e| Error::Request {
                method: PROFILE_UPDATE.method,
                path: path.clone(),
                reason: format!("cannot serialize: {e}"),
            })?;
            let reply = t.put_json(&path, &body)?;
            // 202 as with the quality definitions: accepted, not yet saved.
            expect_status_at(PROFILE_UPDATE.method, &path, &reply, &[200, 202])?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;
    use crate::testing::{ok, FakeTransport, Step};

    const RADARR: &str =
        include_str!("../../../tests/fixtures/radarr-6.3.0.10514/qualityprofile.json");
    const SONARR: &str =
        include_str!("../../../tests/fixtures/sonarr-4.0.19.2979/qualityprofile.json");

    fn task() -> QualityProfiles {
        QualityProfiles {
            allow_in_every_profile: vec!["Unknown".to_string()],
        }
    }

    fn listing(body: &str) -> FakeTransport {
        FakeTransport::default().on_get(PROFILE_LIST.path, vec![ok(body)])
    }

    /// The recording with `Unknown` switched off in profile `id`.
    fn with_unknown_off(body: &str, id: i64) -> String {
        let mut list: Value = serde_json::from_str(body).unwrap();
        for profile in list.as_array_mut().unwrap() {
            if profile["id"] == id {
                for item in profile["items"].as_array_mut().unwrap() {
                    if item["quality"]["name"] == "Unknown" {
                        item["allowed"] = false.into();
                    }
                }
            }
        }
        list.to_string()
    }

    #[test]
    fn the_recorded_state_already_allows_unknown_everywhere() {
        for body in [RADARR, SONARR] {
            let current = task().read(&listing(body)).unwrap();
            assert_eq!(current.len(), 5);
            assert_eq!(task().diff(&current).unwrap(), vec![]);
        }
    }

    #[test]
    fn a_profile_that_disallows_unknown_is_one_change() {
        let body = with_unknown_off(RADARR, 11);
        let current = task().read(&listing(&body)).unwrap();
        let changes = task().diff(&current).unwrap();
        assert_eq!(changes.len(), 1, "{changes:?}");
        assert_eq!(
            changes[0].to_string(),
            "Rarität, Originalsprache (auch SD): Unknown: allowed false -> true"
        );
    }

    #[test]
    fn a_profile_without_the_quality_on_top_is_an_error_naming_it() {
        let mut list: Value = serde_json::from_str(SONARR).unwrap();
        let profile = &mut list.as_array_mut().unwrap()[0];
        let name = profile["name"].as_str().unwrap().to_string();
        profile["items"]
            .as_array_mut()
            .unwrap()
            .retain(|item| item["quality"]["name"] != "Unknown");
        let current = task().read(&listing(&list.to_string())).unwrap();
        let err = task().diff(&current).err().unwrap().to_string();
        assert!(err.contains(&format!("{name}: Unknown")), "{err}");
    }

    #[test]
    fn empty_list_and_nameless_profile_are_errors() {
        let err = task().read(&listing("[]")).err().unwrap().to_string();
        assert!(err.contains("empty list"), "{err}");
        let err = task()
            .read(&listing(r#"[{"id":1,"items":[]}]"#))
            .err()
            .unwrap()
            .to_string();
        assert!(err.contains("has no name"), "{err}");
    }

    #[test]
    fn write_puts_only_the_differing_profile_and_keeps_everything_else() {
        let body = with_unknown_off(RADARR, 11);
        let transport = listing(&body).on_put(vec![Step::Answer(202, String::new())]);
        let current = task().read(&transport).unwrap();
        task().write(&transport, &current).unwrap();

        let written = transport.written.borrow();
        assert_eq!(written.len(), 1, "only profile 11 differs");
        assert_eq!(written[0].0, "/api/v3/qualityprofile/11");

        let sent: Value = serde_json::from_str(&written[0].1).unwrap();
        let mut expected: Value = serde_json::from_str(RADARR).unwrap();
        let expected = expected
            .as_array_mut()
            .unwrap()
            .iter()
            .find(|p| p["id"] == 11)
            .unwrap()
            .clone();
        // Byte-for-byte the recorded profile: Unknown back on, groups still
        // without a `quality` key, `language` and scores untouched.
        assert_eq!(sent, expected);
    }

    #[test]
    fn write_names_the_profile_path_on_failure() {
        let body = with_unknown_off(SONARR, 7);
        let transport = listing(&body).on_put(vec![Step::Answer(500, "boom".into())]);
        let current = task().read(&transport).unwrap();
        assert_eq!(
            task()
                .write(&transport, &current)
                .err()
                .unwrap()
                .to_string(),
            "PUT /api/v3/qualityprofile/7 answered HTTP 500"
        );
    }
}
