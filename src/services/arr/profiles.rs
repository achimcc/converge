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
/// Only with `"exactly": true` and `desired.keep` (design §39). The
/// description lists 200; a profile a film or a series still uses is
/// refused with a 4xx, which is reported and never forced.
pub const PROFILE_DELETE: Endpoint = Endpoint {
    method: "DELETE",
    path: "/api/v3/qualityprofile/{id}",
    request: None,
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
    /// With `"exactly": true`: the profiles that survive, every other one is
    /// removed (design §39). `None` is the ordinary spec, which removes
    /// nothing -- the spec parser lets the two appear only together.
    pub keep: Option<Vec<String>>,
}

fn profile_name(profile: &QualityProfileResource) -> &str {
    profile.name.as_deref().unwrap_or("?")
}

fn subject(profile: &QualityProfileResource) -> String {
    format!("profile {}", profile_name(profile))
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
    /// Is this profile on its way out? Such a profile is left out of `diff`
    /// and of `write`: it need not allow the spec's qualities to be deleted,
    /// and a `PUT` to it would race its own `DELETE`.
    fn is_surplus(&self, profile: &QualityProfileResource) -> bool {
        match &self.keep {
            None => false,
            Some(keep) => !keep.iter().any(|k| k == profile_name(profile)),
        }
    }

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
                reason: crate::error::shape(&e),
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
            if self.is_surplus(profile) {
                continue;
            }
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

    /// The profiles `keep` does not name -- but only once every kept profile
    /// is there.
    ///
    /// On the host this was written for, Recyclarr writes the profiles from
    /// TRaSH's templates and converge takes the rest away. A kept profile
    /// that is missing means Recyclarr has not run (or was renamed), and
    /// removing "the rest" would then empty the service. So a missing one
    /// is an error, in `plan` too, and before a single `DELETE`.
    fn surplus(&self, current: &Self::Current) -> Result<Vec<String>, Error> {
        let Some(keep) = &self.keep else {
            return Ok(Vec::new());
        };
        let present = keep
            .iter()
            .filter(|k| current.iter().any(|p| profile_name(p) == k.as_str()))
            .count();
        if present < keep.len() {
            return Err(Error::Refused(format!(
                "{present} of {} kept profiles present -- has the profile writer run yet? \
                 nothing removed",
                keep.len()
            )));
        }
        Ok(current
            .iter()
            .filter(|p| self.is_surplus(p))
            .map(subject)
            .collect())
    }

    /// One `DELETE` per surplus profile. A profile films or series still
    /// hang on is refused by the service with a 4xx; converge reports that
    /// and does not force it -- so the removals after it are still tried,
    /// and the run ends with every name the service kept.
    fn remove(&self, t: &dyn Transport, current: &Self::Current) -> Result<(), Error> {
        if self.keep.is_none() {
            return Ok(());
        }
        // Nothing is removed while a kept profile is missing.
        self.surplus(current)?;
        let mut refused = Vec::new();
        for profile in current.iter().filter(|p| self.is_surplus(p)) {
            let path = PROFILE_DELETE.path.replace("{id}", &profile.id.to_string());
            let reply = t.delete(&path)?;
            if ![200, 202].contains(&reply.status) {
                refused.push(format!(
                    "{} (HTTP {}, still in use?)",
                    subject(profile),
                    reply.status
                ));
            }
        }
        if refused.is_empty() {
            Ok(())
        } else {
            Err(Error::NotRemoved(refused))
        }
    }

    fn write(&self, t: &dyn Transport, current: &Self::Current) -> Result<(), Error> {
        for profile in current {
            if self.is_surplus(profile) {
                continue;
            }
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
            keep: None,
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

    // --- exactly (design §39) ---------------------------------------------

    const KEPT: [&str; 2] = [
        "Dual Language, sonst Deutsch (1080p)",
        "Rarität, Originalsprache (auch SD)",
    ];

    fn keeping(names: &[&str]) -> QualityProfiles {
        QualityProfiles {
            allow_in_every_profile: vec!["Unknown".to_string()],
            keep: Some(names.iter().map(ToString::to_string).collect()),
        }
    }

    #[test]
    fn without_keep_nothing_is_a_surplus_and_nothing_is_removed() {
        let t = listing(RADARR).on_delete(vec![Step::Answer(200, String::new())]);
        let current = task().read(&t).unwrap();
        assert_eq!(task().surplus(&current).unwrap(), Vec::<String>::new());
        task().remove(&t, &current).unwrap();
        assert!(t.deleted.borrow().is_empty());
    }

    #[test]
    fn with_keep_every_other_profile_is_a_removal() {
        let t = listing(RADARR);
        let task = keeping(&KEPT);
        let current = task.read(&t).unwrap();
        assert_eq!(
            task.surplus(&current).unwrap(),
            [
                "profile Dual Language, sonst Deutsch (4K, sonst 1080p)",
                "profile Dual Language, sonst Originalsprache (1080p)",
                "profile Dual Language, sonst Originalsprache (4K, sonst 1080p)",
            ]
        );
        // Ids 8, 9 and 10 of the recording.
        let t = listing(RADARR).on_delete(vec![Step::Answer(200, String::new())]);
        let current = task.read(&t).unwrap();
        task.remove(&t, &current).unwrap();
        assert_eq!(
            t.deleted.borrow().as_slice(),
            [
                "/api/v3/qualityprofile/8",
                "/api/v3/qualityprofile/9",
                "/api/v3/qualityprofile/10"
            ]
        );
    }

    /// On the host Recyclarr writes the profiles and converge removes the
    /// rest. If Recyclarr has not run, everything converge keeps is missing
    /// -- and removing the rest would empty the service.
    #[test]
    fn a_kept_profile_the_service_lacks_refuses_before_anything_is_removed() {
        let task = keeping(&["Dual Language, sonst Deutsch (1080p)", "Anime"]);
        let t = listing(RADARR).on_delete(vec![Step::Answer(200, String::new())]);
        let current = task.read(&t).unwrap();
        let err = task.surplus(&current).err().unwrap().to_string();
        assert_eq!(
            err,
            "refused: 1 of 2 kept profiles present -- has the profile writer run yet? nothing removed"
        );
        assert!(task.remove(&t, &current).is_err());
        assert!(t.deleted.borrow().is_empty());
    }

    /// A profile a film or a series still hangs on cannot be deleted, and
    /// the *arr says so with a 4xx. That is reported, not forced -- and the
    /// other removals are still tried.
    #[test]
    fn a_profile_still_in_use_is_reported_by_name_and_the_others_are_still_tried() {
        let task = keeping(&KEPT);
        let t = listing(RADARR).on_delete(vec![
            Step::Answer(409, "boom".into()),
            Step::Answer(200, String::new()),
            Step::Answer(400, "boom".into()),
        ]);
        let current = task.read(&t).unwrap();
        let err = task.remove(&t, &current).err().unwrap().to_string();
        assert_eq!(
            err,
            "not removed: profile Dual Language, sonst Deutsch (4K, sonst 1080p) \
             (HTTP 409, still in use?); \
             profile Dual Language, sonst Originalsprache (4K, sonst 1080p) \
             (HTTP 400, still in use?)"
        );
        assert_eq!(t.deleted.borrow().len(), 3, "all three were tried");
    }

    /// A profile on its way out is not measured against the spec, and not
    /// written: it need not allow `Unknown` to be deleted.
    #[test]
    fn a_profile_being_removed_is_neither_checked_nor_written() {
        let body = with_unknown_off(RADARR, 9);
        let mut list: Value = serde_json::from_str(&body).unwrap();
        for profile in list.as_array_mut().unwrap() {
            if profile["id"] == 9 {
                profile["items"]
                    .as_array_mut()
                    .unwrap()
                    .retain(|item| item["quality"]["name"] != "Unknown");
            }
        }
        let body = list.to_string();
        // Without `keep` the missing rung of profile 9 is an error.
        let t = listing(&body);
        let current = task().read(&t).unwrap();
        assert!(task().diff(&current).is_err());

        let task = keeping(&KEPT);
        let t = listing(&body)
            .on_put(vec![Step::Answer(202, String::new())])
            .on_delete(vec![Step::Answer(200, String::new())]);
        let current = task.read(&t).unwrap();
        assert_eq!(task.diff(&current).unwrap(), vec![]);
        task.remove(&t, &current).unwrap();
        task.write(&t, &current).unwrap();
        assert!(t.written.borrow().is_empty(), "{:?}", t.written.borrow());
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
