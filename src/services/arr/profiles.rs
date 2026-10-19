//! `quality-profiles`: every quality profile allows the named qualities.
//!
//! Only allowing is possible. A profile's ladder (which qualities it has, in
//! which order, grouped how) belongs to whoever defined the profile -- on the
//! host this was written for, Recyclarr and TRaSH's templates. converge only
//! flips `allowed` on a top-level item that is already there.
//!
//! Besides the ladder: the scores of named custom formats (design §41) and,
//! for named profiles, the profile's language (design §56).

use std::collections::BTreeMap;

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

/// Every language the service knows, two pseudo-languages among them: `Any`
/// (id -1) and `Original` (id -2). Read only when the spec names a
/// profile's language (design §56).
pub const LANGUAGE_LIST: Endpoint = Endpoint {
    method: "GET",
    path: "/api/v3/language",
    request: None,
    response: Some(Shape::List("LanguageResource")),
};

/// One entry of `GET /api/v3/language`. `nameLower` stays in `rest`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LanguageResource {
    pub id: i32,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(flatten)]
    #[schemars(skip)]
    pub rest: Map<String, Value>,
}

/// A profile's language, as Radarr's `QualityProfileResource` carries it --
/// its own component, `Language`, not `LanguageResource`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Language {
    pub id: i32,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(flatten)]
    #[schemars(skip)]
    pub rest: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct QualityProfileResource {
    pub id: i32,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub items: Option<Vec<QualityProfileQualityItemResource>>,
    /// The score every custom format has in this profile (design §41).
    /// Absent where a service answers none; it must not grow a `null` on the
    /// way back, hence `skip_serializing_if`.
    #[serde(
        default,
        rename = "formatItems",
        skip_serializing_if = "Option::is_none"
    )]
    pub format_items: Option<Vec<ProfileFormatItemResource>>,
    /// The language a release must have before any score counts (design
    /// §56). Radarr's profiles carry it; Sonarr v4's have no such key, and
    /// none may appear on the way back -- hence `skip_serializing_if`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<Language>,
    #[serde(flatten)]
    #[schemars(skip)]
    pub rest: Map<String, Value>,
}

/// One custom format's score in one profile. `format` is the format's id and
/// `name` its name; the spec names the format, so both are read and only
/// `score` is ever written.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProfileFormatItemResource {
    pub format: i32,
    #[serde(default)]
    pub name: Option<String>,
    pub score: i64,
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
    /// The score a named custom format must have in every profile (in every
    /// **kept** profile where `keep` is set), by the format's name. Empty
    /// where the spec names none; every other `formatItems` entry stays as
    /// it is, because Recyclarr writes those (design §41).
    pub format_scores: BTreeMap<String, i64>,
    /// The language a named profile must have, by profile name and language
    /// name (design §56). Empty where the spec names none -- and then the
    /// language list is not even read.
    pub languages: BTreeMap<String, String>,
}

/// What `read` hands on: the profiles, and the languages the service knows
/// (empty unless the spec names a profile's language).
#[derive(Debug, Clone, PartialEq)]
pub struct Profiles {
    pub profiles: Vec<QualityProfileResource>,
    pub languages: Vec<LanguageResource>,
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

    /// The language each named profile must have, by profile id, as the
    /// service's language list spells it (design §56). Every name is checked
    /// before anything is returned, so an error stops the run before a
    /// single write.
    fn language_targets(&self, current: &Profiles) -> Result<BTreeMap<i32, Language>, Error> {
        let mut targets = BTreeMap::new();
        let mut not_found = Vec::new();
        let mut no_language = Vec::new();
        for (wanted, language) in &self.languages {
            let profile = current
                .profiles
                .iter()
                .find(|p| profile_name(p) == wanted.as_str());
            let known = current
                .languages
                .iter()
                .find(|l| l.name.as_deref() == Some(language.as_str()));
            let Some(profile) = profile else {
                not_found.push(format!("profile {wanted}"));
                continue;
            };
            let Some(known) = known else {
                let unknown = format!("language {language}");
                if !not_found.contains(&unknown) {
                    not_found.push(unknown);
                }
                continue;
            };
            // Sonarr v4: the quality profile has no key for it. Adding one
            // would be a field the service does not have.
            if profile.language.is_none() {
                no_language.push(format!(
                    "{}: the service has no profile language",
                    subject(profile)
                ));
                continue;
            }
            // A profile on its way out is not written (the spec parser lets
            // `languages` name only kept profiles).
            if self.is_surplus(profile) {
                continue;
            }
            targets.insert(
                profile.id,
                Language {
                    id: known.id,
                    name: known.name.clone(),
                    rest: Map::new(),
                },
            );
        }
        if !not_found.is_empty() {
            return Err(Error::NotFound(not_found));
        }
        if !no_language.is_empty() {
            return Err(Error::Mismatch(no_language));
        }
        Ok(targets)
    }

    /// The profile as it should be, or `None` if it already is.
    fn corrected(
        &self,
        profile: &QualityProfileResource,
        language: Option<&Language>,
    ) -> Option<QualityProfileResource> {
        let mut fixed = profile.clone();
        let mut changed = false;
        if let (Some(target), Some(have)) = (language, fixed.language.as_mut()) {
            if have.id != target.id {
                // Only id and name; anything else the service put there
                // travels back as it was.
                have.id = target.id;
                have.name.clone_from(&target.name);
                changed = true;
            }
        }
        for item in fixed.items.iter_mut().flatten() {
            let name = item.quality.as_ref().and_then(|q| q.name.as_deref());
            if let Some(name) = name {
                if !item.allowed && self.allow_in_every_profile.iter().any(|a| a == name) {
                    item.allowed = true;
                    changed = true;
                }
            }
        }
        for item in fixed.format_items.iter_mut().flatten() {
            let Some(name) = item.name.as_deref() else {
                continue;
            };
            if let Some(score) = self.format_scores.get(name) {
                if item.score != *score {
                    item.score = *score;
                    changed = true;
                }
            }
        }
        changed.then_some(fixed)
    }

    /// Score differences of one profile, and the format names it does not
    /// know. The lookup is by name, the way a spec names a format; `format`
    /// carries the id and stays the service's.
    fn score_changes(
        &self,
        profile: &QualityProfileResource,
        unknown: &mut Vec<String>,
        changes: &mut Vec<Change>,
    ) {
        let items = profile.format_items.as_deref().unwrap_or_default();
        for (format, score) in &self.format_scores {
            match items
                .iter()
                .find(|item| item.name.as_deref() == Some(format.as_str()))
            {
                None => unknown.push(format!("{}: custom format {format}", profile_name(profile))),
                Some(item) if item.score != *score => changes.push(Change {
                    subject: format!("{}: {format}", profile_name(profile)),
                    field: "score".to_string(),
                    current: item.score.to_string(),
                    desired: score.to_string(),
                }),
                Some(_) => {}
            }
        }
    }
}

impl Task for QualityProfiles {
    type Current = Profiles;

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
        // The language list only where the spec names a profile's language:
        // every other spec reads what it read before (design §56).
        let languages = if self.languages.is_empty() {
            Vec::new()
        } else {
            let path = LANGUAGE_LIST.path.to_string();
            let reply = t.get(LANGUAGE_LIST.path)?;
            expect_status(&LANGUAGE_LIST, &reply, &[200])?;
            let languages: Vec<LanguageResource> =
                serde_json::from_str(&reply.body).map_err(|e| Error::Decode {
                    path: path.clone(),
                    reason: crate::error::shape(&e),
                })?;
            if languages.is_empty() {
                return Err(Error::EmptyList { path });
            }
            languages
        };
        Ok(Profiles {
            profiles: list,
            languages,
        })
    }

    fn diff(&self, current: &Self::Current) -> Result<Vec<Change>, Error> {
        let languages = self.language_targets(current)?;
        let mut missing = Vec::new();
        let mut unknown = Vec::new();
        let mut changes = Vec::new();
        for profile in &current.profiles {
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
            self.score_changes(profile, &mut unknown, &mut changes);
            if let (Some(target), Some(have)) = (languages.get(&profile.id), &profile.language) {
                if have.id != target.id {
                    changes.push(Change {
                        subject: profile_name(profile).to_string(),
                        field: "language".to_string(),
                        current: have.name.clone().unwrap_or_else(|| have.id.to_string()),
                        desired: target.name.clone().unwrap_or_else(|| target.id.to_string()),
                    });
                }
            }
        }
        // A quality that is not a top-level rung cannot be switched on without
        // rebuilding the ladder -- which is not converge's to do. Say so.
        if !missing.is_empty() {
            return Err(Error::MissingItem(missing));
        }
        // A format the service does not have: `custom-formats` creates it,
        // and the order of the two specs is the host's to arrange. Nothing
        // is written until it is there.
        if !unknown.is_empty() {
            return Err(Error::NotFound(unknown));
        }
        Ok(changes)
    }

    fn notes(&self, _current: &Self::Current) -> Vec<String> {
        Vec::new()
    }

    /// The profiles `keep` does not name -- but only once every kept profile
    /// is there, and never half of them or more.
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
            .filter(|k| {
                current
                    .profiles
                    .iter()
                    .any(|p| profile_name(p) == k.as_str())
            })
            .count();
        if present < keep.len() {
            return Err(Error::Refused(format!(
                "{present} of {} kept profiles present -- has the profile writer run yet? \
                 nothing removed",
                keep.len()
            )));
        }
        // NEVER HALF THE PROFILES OR MORE (audit 4, B211), the guard Koel's
        // stations and Trailarr's connections have. All kept names present
        // is not enough: a `keep` list cut short in the spec, or a writer
        // that added hundreds of strays, would otherwise empty the service
        // down to the kept few in one run.
        //
        // UP TO `FRESH_DEFAULTS` REMOVALS PASS ANYWAY. A freshly installed
        // Radarr or Sonarr carries six profiles of its own, and next to two
        // written ones removing them IS half or more -- the very job `keep`
        // was made for, on the day the service is rebuilt. Without the floor
        // that day would need a hand in the web UI.
        let current = &current.profiles;
        let surplus: Vec<String> = current
            .iter()
            .filter(|p| self.is_surplus(p))
            .map(subject)
            .collect();
        if surplus.len() > FRESH_DEFAULTS && 2 * surplus.len() >= current.len() {
            return Err(Error::Refused(format!(
                "would remove {} of {} profiles (half or more) -- nothing removed",
                surplus.len(),
                current.len()
            )));
        }
        Ok(surplus)
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
        for profile in current.profiles.iter().filter(|p| self.is_surplus(p)) {
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
        // Nothing is written while a named format is missing from a profile:
        // `corrected` would silently leave that score alone.
        let mut unknown = Vec::new();
        for profile in current.profiles.iter().filter(|p| !self.is_surplus(p)) {
            self.score_changes(profile, &mut unknown, &mut Vec::new());
        }
        if !unknown.is_empty() {
            return Err(Error::NotFound(unknown));
        }
        // Nor while a named profile or language is missing.
        let languages = self.language_targets(current)?;
        for profile in &current.profiles {
            if self.is_surplus(profile) {
                continue;
            }
            let Some(fixed) = self.corrected(profile, languages.get(&profile.id)) else {
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

/// The quality profiles a fresh Radarr or Sonarr ships with (Any, SD,
/// HD-720p, HD-1080p, Ultra-HD, HD - 720p/1080p): removals up to this many
/// are never refused by the half rule.
const FRESH_DEFAULTS: usize = 6;

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
            format_scores: BTreeMap::new(),
            languages: BTreeMap::new(),
        }
    }

    /// The same task, plus the score one named custom format must have.
    fn scoring(format: &str, score: i64) -> QualityProfiles {
        QualityProfiles {
            format_scores: [(format.to_string(), score)].into_iter().collect(),
            ..task()
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
            assert_eq!(current.profiles.len(), 5);
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

    // --- format_scores (design §41) ---------------------------------------

    /// The recording's `AMZN` carries 0 in all five of Radarr's profiles --
    /// one of fifteen formats that do. (Sonarr's recording has none: every
    /// one of its sixty formats is scored differently somewhere, which is
    /// what two profile families for the same library look like.)
    #[test]
    fn a_score_that_already_matches_everywhere_is_no_change() {
        let task = scoring("AMZN", 0);
        let current = task.read(&listing(RADARR)).unwrap();
        assert_eq!(task.diff(&current).unwrap(), vec![]);
    }

    #[test]
    fn a_differing_score_is_one_change_per_profile() {
        let task = scoring("German DL", 12000);
        let current = task.read(&listing(RADARR)).unwrap();
        let changes = task.diff(&current).unwrap();
        assert_eq!(changes.len(), 5, "{changes:?}");
        assert_eq!(
            changes[0].to_string(),
            "Dual Language, sonst Deutsch (1080p): German DL: score 11000 -> 12000"
        );
    }

    /// Only the named entry changes; the seventy-seven Recyclarr writes
    /// travel back as they were.
    #[test]
    fn the_write_changes_one_format_item_and_leaves_the_others() {
        let task = scoring("German DL", 12000);
        let t = listing(RADARR).on_put(vec![Step::Answer(202, String::new())]);
        let current = task.read(&t).unwrap();
        task.write(&t, &current).unwrap();
        let written = t.written.borrow();
        assert_eq!(written.len(), 5, "every profile carries the format");
        let sent: Value = serde_json::from_str(&written[0].1).unwrap();
        let recorded: Value = serde_json::from_str(RADARR).unwrap();
        let mut expected = recorded.as_array().unwrap()[0].clone();
        for item in expected["formatItems"].as_array_mut().unwrap() {
            if item["name"] == "German DL" {
                item["score"] = 12000.into();
            }
        }
        assert_eq!(sent, expected);
    }

    /// `custom-formats` creates the format; until it has, nothing is written.
    #[test]
    fn a_format_the_service_does_not_have_is_an_error_before_any_write() {
        let task = scoring("Dolby Vision HDR10+", 500);
        let t = listing(RADARR).on_put(vec![Step::Answer(202, String::new())]);
        let current = task.read(&t).unwrap();
        let err = task.diff(&current).err().unwrap().to_string();
        assert!(
            err.starts_with(
                "not found on the service: Dual Language, sonst Deutsch (1080p): \
                 custom format Dolby Vision HDR10+"
            ),
            "{err}"
        );
        assert!(task.write(&t, &current).is_err());
        assert!(t.written.borrow().is_empty(), "nothing was written");
    }

    /// With `keep`, only the profiles that survive are measured -- a profile
    /// on its way out need not carry the score.
    #[test]
    fn with_keep_only_the_kept_profiles_are_scored() {
        let task = QualityProfiles {
            format_scores: [("German DL".to_string(), 12000)].into_iter().collect(),
            ..keeping(&KEPT)
        };
        let t = listing(RADARR).on_put(vec![Step::Answer(202, String::new())]);
        let current = task.read(&t).unwrap();
        let changes = task.diff(&current).unwrap();
        // Three of the five profiles are on their way out; the two kept ones
        // are the only ones the score is measured in.
        assert_eq!(changes.len(), 2, "{changes:?}");
        assert!(changes.iter().all(|c| c.field == "score"), "{changes:?}");
        assert_eq!(task.surplus(&current).unwrap().len(), 3);
    }

    // --- exactly (design §39) ---------------------------------------------

    const KEPT: [&str; 2] = [
        "Dual Language, sonst Deutsch (1080p)",
        "Rarität, Originalsprache (auch SD)",
    ];

    /// Audit 4, B211 (probe `kb-converge-profiles.rs`): 500 profiles on the
    /// service, 5 of them kept. Before 0.53.2 the only guard asked whether
    /// the kept names were there, and 495 `DELETE`s went out.
    #[test]
    fn kb_500_profiles_against_5_kept_must_not_delete_495() {
        let list: Value = serde_json::from_str(RADARR).unwrap();
        let template = list.as_array().unwrap()[0].clone();
        let many: Vec<Value> = (0..500)
            .map(|i| {
                let mut p = template.clone();
                p["id"] = Value::from(1000 + i);
                p["name"] = Value::from(format!("Praeparat {i}"));
                p
            })
            .collect();
        let body = serde_json::to_string(&many).unwrap();
        let kept: Vec<String> = (0..5).map(|i| format!("Praeparat {i}")).collect();
        let kept_refs: Vec<&str> = kept.iter().map(String::as_str).collect();
        let task = keeping(&kept_refs);
        let t = listing(&body).on_delete(vec![Step::Answer(200, String::new())]);
        let current = task.read(&t).expect("read");
        assert_eq!(
            task.surplus(&current).unwrap_err().to_string(),
            "refused: would remove 495 of 500 profiles (half or more) -- nothing removed"
        );
        assert!(task.remove(&t, &current).is_err());
        let deleted = t.deleted.borrow().len();
        assert!(
            deleted < 250,
            "{deleted} of 500 profiles deleted on one run -- no upper bound on removals"
        );
        assert_eq!(deleted, 0, "the refusal comes before the first DELETE");
    }

    /// The floor: a fresh service's six defaults next to two written
    /// profiles go (6 of 8), seven strays next to two do not (7 of 9).
    #[test]
    fn a_fresh_services_six_defaults_pass_the_half_rule_seven_strays_do_not() {
        let list: Value = serde_json::from_str(RADARR).unwrap();
        let template = list.as_array().unwrap()[0].clone();
        let named = |n: usize| -> String {
            let all: Vec<Value> = (0..n)
                .map(|i| {
                    let mut p = template.clone();
                    p["id"] = Value::from(2000 + i);
                    p["name"] = Value::from(format!("P{i}"));
                    p
                })
                .collect();
            serde_json::to_string(&all).unwrap()
        };
        let task = keeping(&["P0", "P1"]);
        let current = task.read(&listing(&named(8))).unwrap();
        assert_eq!(task.surplus(&current).unwrap().len(), 6);
        let current = task.read(&listing(&named(9))).unwrap();
        assert_eq!(
            task.surplus(&current).unwrap_err().to_string(),
            "refused: would remove 7 of 9 profiles (half or more) -- nothing removed"
        );
    }

    fn keeping(names: &[&str]) -> QualityProfiles {
        QualityProfiles {
            allow_in_every_profile: vec!["Unknown".to_string()],
            keep: Some(names.iter().map(ToString::to_string).collect()),
            format_scores: BTreeMap::new(),
            languages: BTreeMap::new(),
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

    // --- languages (design §56) -------------------------------------------

    /// Radarr 6.4.4: six profiles; 7 and 8 carry `Any`, 9 to 12 `Original`.
    const RADARR_644: &str =
        include_str!("../../../tests/fixtures/radarr-6.4.4.10685/qualityprofile-language.json");
    const LANGUAGES_644: &str =
        include_str!("../../../tests/fixtures/radarr-6.4.4.10685/language.json");
    const VR: &str = "3D für die VR-Brille (4K, sonst 1080p)";

    fn speaking(pairs: &[(&str, &str)]) -> QualityProfiles {
        QualityProfiles {
            languages: pairs
                .iter()
                .map(|(p, l)| ((*p).to_string(), (*l).to_string()))
                .collect(),
            ..task()
        }
    }

    fn with_languages(profiles: &str) -> FakeTransport {
        listing(profiles).on_get(LANGUAGE_LIST.path, vec![ok(LANGUAGES_644)])
    }

    #[test]
    fn a_profile_on_original_is_one_change_to_any() {
        let task = speaking(&[(VR, "Any")]);
        let current = task.read(&with_languages(RADARR_644)).unwrap();
        assert_eq!(current.languages.len(), 60);
        assert_eq!(
            task.diff(&current)
                .unwrap()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            [format!("{VR}: language Original -> Any")]
        );
    }

    #[test]
    fn a_profile_that_already_has_the_language_is_no_change() {
        let task = speaking(&[(
            "Dual Language, sonst Deutsch, sonst Original (1080p)",
            "Any",
        )]);
        let current = task.read(&with_languages(RADARR_644)).unwrap();
        assert_eq!(task.diff(&current).unwrap(), vec![]);
    }

    /// Only profile 12 is sent, and it is the recorded profile with nothing
    /// changed but `language` -- id and name of `Any` as the list has them.
    #[test]
    fn the_write_changes_the_language_and_nothing_else() {
        let task = speaking(&[(VR, "Any")]);
        let t = with_languages(RADARR_644).on_put(vec![Step::Answer(202, String::new())]);
        let current = task.read(&t).unwrap();
        task.write(&t, &current).unwrap();
        let written = t.written.borrow();
        assert_eq!(written.len(), 1, "{written:?}");
        assert_eq!(written[0].0, "/api/v3/qualityprofile/12");
        let sent: Value = serde_json::from_str(&written[0].1).unwrap();
        let recorded: Value = serde_json::from_str(RADARR_644).unwrap();
        let mut expected = recorded
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == 12)
            .unwrap()
            .clone();
        expected["language"] = serde_json::json!({"id": -1, "name": "Any"});
        assert_eq!(sent, expected);
    }

    /// Without `languages` the language list is never asked for: the fake
    /// transport panics on a request it was not scripted for.
    #[test]
    fn without_languages_the_language_list_is_not_read() {
        let current = task().read(&listing(RADARR_644)).unwrap();
        assert!(current.languages.is_empty());
        assert_eq!(task().diff(&current).unwrap(), vec![]);
    }

    #[test]
    fn an_unknown_language_is_an_error_naming_it_before_any_write() {
        let task = speaking(&[(VR, "Klingonisch")]);
        let t = with_languages(RADARR_644).on_put(vec![Step::Answer(202, String::new())]);
        let current = task.read(&t).unwrap();
        assert_eq!(
            task.diff(&current).unwrap_err().to_string(),
            "not found on the service: language Klingonisch"
        );
        assert!(task.write(&t, &current).is_err());
        assert!(t.written.borrow().is_empty(), "nothing was written");
    }

    #[test]
    fn a_profile_the_service_lacks_is_an_error_before_any_write() {
        let task = speaking(&[(VR, "Any"), ("Anime", "Japanese")]);
        let t = with_languages(RADARR_644).on_put(vec![Step::Answer(202, String::new())]);
        let current = task.read(&t).unwrap();
        assert_eq!(
            task.diff(&current).unwrap_err().to_string(),
            "not found on the service: profile Anime"
        );
        assert!(task.write(&t, &current).is_err());
        assert!(t.written.borrow().is_empty(), "nothing was written");
    }

    /// Sonarr v4's profiles have no `language` key (description and
    /// recording alike). Naming one is an error, not a key added.
    #[test]
    fn a_profile_without_a_language_key_is_an_error_before_any_write() {
        let list: Value = serde_json::from_str(SONARR).unwrap();
        let name = list[0]["name"].as_str().unwrap().to_string();
        let task = speaking(&[(&name, "Any")]);
        let t = with_languages(SONARR).on_put(vec![Step::Answer(202, String::new())]);
        let current = task.read(&t).unwrap();
        assert_eq!(
            task.diff(&current).unwrap_err().to_string(),
            format!(
                "the spec does not fit the service: profile {name}: \
                 the service has no profile language"
            )
        );
        assert!(task.write(&t, &current).is_err());
        assert!(t.written.borrow().is_empty(), "nothing was written");
    }

    /// And a Sonarr profile written for another reason does not grow one.
    #[test]
    fn a_sonarr_profile_travels_back_without_a_language_key() {
        let body = with_unknown_off(SONARR, 7);
        let t = listing(&body).on_put(vec![Step::Answer(202, String::new())]);
        let current = task().read(&t).unwrap();
        task().write(&t, &current).unwrap();
        let written = t.written.borrow();
        let sent: Value = serde_json::from_str(&written[0].1).unwrap();
        assert!(sent.get("language").is_none(), "{sent}");
        let recorded: Value = serde_json::from_str(SONARR).unwrap();
        let expected = recorded
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == 7)
            .unwrap()
            .clone();
        assert_eq!(sent, expected);
    }

    /// With `keep`, a kept profile's language is set and the surplus ones
    /// are neither measured nor written.
    #[test]
    fn with_keep_the_language_of_a_kept_profile_is_set() {
        let task = QualityProfiles {
            languages: [(VR.to_string(), "Any".to_string())].into_iter().collect(),
            ..keeping(&[VR, "Dual Language, sonst Deutsch, sonst Original (1080p)"])
        };
        let t = with_languages(RADARR_644)
            .on_put(vec![Step::Answer(202, String::new())])
            .on_delete(vec![Step::Answer(200, String::new())]);
        let current = task.read(&t).unwrap();
        assert_eq!(task.diff(&current).unwrap().len(), 1);
        assert_eq!(task.surplus(&current).unwrap().len(), 4);
        task.write(&t, &current).unwrap();
        let written = t.written.borrow();
        assert_eq!(written.len(), 1);
        assert_eq!(written[0].0, "/api/v3/qualityprofile/12");
    }
}
