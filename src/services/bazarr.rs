//! Bazarr: its language profiles (design §48). A profile says which subtitle
//! languages a series or film should have; Bazarr keeps them in its database
//! and offers no file for them.
//!
//! The one write is `POST /api/system/settings` with the form field
//! `languages-profiles`, and that field is the **whole list**: Bazarr updates
//! the profiles it names, adds the ones it does not know and deletes every
//! other one (`api/system/settings.py`, 1.6.0). So a write here always
//! carries every profile that was read -- the ones the spec does not name
//! exactly as they were.
//!
//! Bazarr hides that endpoint from its OpenAPI description
//! (`@api_ns_system_settings.hide`). The fields below are checked against
//! recorded answers and at runtime only, as for ntfy.

use std::collections::{BTreeMap, BTreeSet};

use serde::{de::Error as _, Deserialize, Deserializer};
use serde_json::{json, Map, Value};

use crate::{
    client::{Reply, Transport},
    endpoint::Endpoint,
    engine::{Change, Probe, Task},
    error::Error,
};

pub const STATUS: Endpoint = Endpoint {
    method: "GET",
    path: "/api/system/status",
    request: None,
    response: None,
};
pub const PROFILES: Endpoint = Endpoint {
    method: "GET",
    path: "/api/system/languages/profiles",
    request: None,
    response: None,
};
pub const LANGUAGES: Endpoint = Endpoint {
    method: "GET",
    path: "/api/system/languages",
    request: None,
    response: None,
};
pub const SETTINGS: Endpoint = Endpoint {
    method: "POST",
    path: "/api/system/settings",
    request: None,
    response: None,
};
pub const ENDPOINTS: [Endpoint; 4] = [STATUS, PROFILES, LANGUAGES, SETTINGS];

/// The form field of `SETTINGS` that holds the list of profiles, as JSON.
const FORM_FIELD: &str = "languages-profiles";

/// Bazarr's cutoff value for "any language of the profile". It names no
/// item, so it survives a change of the items.
const CUTOFF_ANY: i64 = 65535;

/// Fields of a profile Bazarr reads with `item['…']` when it takes the list
/// back. One profile without one of them is a `KeyError` in the middle of
/// the loop -- after the profiles before it were written.
const REQUIRED: [&str; 7] = [
    "profileId",
    "name",
    "cutoff",
    "items",
    "mustContain",
    "mustNotContain",
    "originalFormat",
];

/// One language of a profile, as a spec names it: Bazarr's two-letter code
/// and the four switches of an item. The order of a profile's languages is
/// part of the profile.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Language {
    pub language: String,
    #[serde(default)]
    pub hi: bool,
    #[serde(default)]
    pub forced: bool,
    #[serde(default)]
    pub audio_exclude: bool,
    #[serde(default)]
    pub audio_only_include: bool,
}

impl Language {
    /// `de`, or `en (forced, hi)`.
    pub fn label(&self) -> String {
        let flags: Vec<&str> = [
            (self.hi, "hi"),
            (self.forced, "forced"),
            (self.audio_exclude, "audio_exclude"),
            (self.audio_only_include, "audio_only_include"),
        ]
        .into_iter()
        .filter(|(set, _)| *set)
        .map(|(_, name)| name)
        .collect();
        if flags.is_empty() {
            self.language.clone()
        } else {
            format!("{} ({})", self.language, flags.join(", "))
        }
    }
}

fn labels(languages: &[Language]) -> String {
    let all: Vec<String> = languages.iter().map(Language::label).collect();
    format!("[{}]", all.join(", "))
}

/// Bazarr keeps an item's switches as the strings `"True"` and `"False"`
/// (the web page sends them that way). The error names no value.
fn flag<'de, D: Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Bool(bool),
        Text(String),
    }
    match Raw::deserialize(deserializer)? {
        Raw::Bool(value) => Ok(value),
        Raw::Text(text) => match text.as_str() {
            "True" => Ok(true),
            "False" => Ok(false),
            _ => Err(D::Error::custom("a switch that is neither True nor False")),
        },
    }
}

#[derive(Deserialize)]
struct Status {
    data: StatusData,
}

#[derive(Deserialize)]
struct StatusData {
    #[serde(default)]
    bazarr_version: Option<String>,
}

#[derive(Deserialize)]
struct KnownLanguage {
    code2: String,
}

/// An item of a profile as Bazarr answers it. Its `id` is its position and
/// is not read; a switch an older profile lacks is off.
#[derive(Deserialize)]
struct Item {
    language: String,
    #[serde(default, deserialize_with = "flag")]
    hi: bool,
    #[serde(default, deserialize_with = "flag")]
    forced: bool,
    #[serde(default, deserialize_with = "flag")]
    audio_exclude: bool,
    #[serde(default, deserialize_with = "flag")]
    audio_only_include: bool,
}

/// What converge reads of a profile. Everything else it carries stays in
/// `Profile::raw` and goes back untouched.
#[derive(Deserialize)]
struct View {
    #[serde(rename = "profileId")]
    id: i64,
    name: String,
    cutoff: Option<i64>,
    items: Vec<Item>,
}

pub struct Profile {
    id: i64,
    name: String,
    cutoff: Option<i64>,
    languages: Vec<Language>,
    raw: Map<String, Value>,
}

pub struct Current {
    profiles: Vec<Profile>,
    /// The two-letter codes Bazarr knows.
    known: BTreeSet<String>,
}

/// Readiness: `/api/system/status` answers with a version. It needs the key,
/// so a refused key ends the wait at once.
pub fn probe(t: &dyn Transport) -> Result<String, Probe> {
    let reply = t
        .get(STATUS.path)
        .map_err(|e| Probe::NotYet(e.to_string()))?;
    match reply.status {
        200 => {}
        401 | 403 => return Err(Probe::Fatal(refused(&STATUS, &reply))),
        other => return Err(Probe::NotYet(format!("HTTP {other}"))),
    }
    let status: Status = serde_json::from_str(&reply.body)
        .map_err(|e| Probe::NotYet(format!("unexpected answer: {}", crate::error::shape(&e))))?;
    status
        .data
        .bazarr_version
        .filter(|v| !v.is_empty())
        .ok_or_else(|| Probe::NotYet("the answer has no version".to_string()))
}

/// A status error that carries nothing from the body: Bazarr answers a
/// refused setting with the validator's message, and that quotes the value.
fn refused(ep: &Endpoint, reply: &Reply) -> Error {
    Error::Status {
        method: ep.method,
        path: ep.path.to_string(),
        status: reply.status,
        validation: match reply.status {
            401 | 403 => vec!["the API key was refused".to_string()],
            _ => Vec::new(),
        },
    }
}

fn expect(ep: &Endpoint, reply: &Reply, accepted: u16) -> Result<(), Error> {
    if reply.status == accepted {
        Ok(())
    } else {
        Err(refused(ep, reply))
    }
}

fn decode<T: for<'de> Deserialize<'de>>(ep: &Endpoint, body: &str) -> Result<T, Error> {
    serde_json::from_str(body).map_err(|e| Error::Decode {
        path: ep.path.to_string(),
        reason: crate::error::shape(&e),
    })
}

/// The items of a profile as Bazarr stores them: numbered from 1 in the
/// spec's order, every switch as `"True"` or `"False"`.
fn items(languages: &[Language]) -> Value {
    let text = |set: bool| if set { "True" } else { "False" };
    Value::Array(
        languages
            .iter()
            .enumerate()
            .map(|(i, l)| {
                json!({
                    "id": i + 1,
                    "language": l.language,
                    "audio_exclude": text(l.audio_exclude),
                    "hi": text(l.hi),
                    "forced": text(l.forced),
                    "audio_only_include": text(l.audio_only_include),
                })
            })
            .collect(),
    )
}

/// Language profiles by name, each with its languages in order. A profile
/// the spec does not name is left as it is; nothing is ever removed.
pub struct LanguageProfiles {
    pub profiles: BTreeMap<String, Vec<Language>>,
}

impl LanguageProfiles {
    /// The one profile of that name. Bazarr does not keep names unique; two
    /// of a name leave no way to say which one is meant.
    fn find<'a>(current: &'a Current, name: &str) -> Result<Option<&'a Profile>, Error> {
        let mut found = current.profiles.iter().filter(|p| p.name == name);
        let first = found.next();
        match found.count() {
            0 => Ok(first),
            more => Err(Error::Mismatch(vec![format!(
                "{} profiles are named {name}",
                more + 1
            )])),
        }
    }
}

impl Task for LanguageProfiles {
    type Current = Current;

    fn probe(&self, t: &dyn Transport) -> Result<String, Probe> {
        probe(t)
    }

    fn read(&self, t: &dyn Transport) -> Result<Self::Current, Error> {
        let reply = t.get(PROFILES.path)?;
        expect(&PROFILES, &reply, 200)?;
        // An empty list is a fresh Bazarr, not a failed read.
        let list: Vec<Map<String, Value>> = decode(&PROFILES, &reply.body)?;
        let mut profiles = Vec::new();
        for (i, raw) in list.into_iter().enumerate() {
            let lacking = |reason: String| Error::Decode {
                path: PROFILES.path.to_string(),
                reason: format!("profile {}: {reason}", i + 1),
            };
            if let Some(key) = REQUIRED.iter().find(|key| !raw.contains_key(**key)) {
                return Err(lacking(format!("missing field `{key}`")));
            }
            let view: View = serde_json::from_value(Value::Object(raw.clone()))
                .map_err(|e| lacking(crate::error::shape(&e)))?;
            profiles.push(Profile {
                id: view.id,
                name: view.name,
                cutoff: view.cutoff,
                languages: view
                    .items
                    .into_iter()
                    .map(|item| Language {
                        language: item.language,
                        hi: item.hi,
                        forced: item.forced,
                        audio_exclude: item.audio_exclude,
                        audio_only_include: item.audio_only_include,
                    })
                    .collect(),
                raw,
            });
        }

        let reply = t.get(LANGUAGES.path)?;
        expect(&LANGUAGES, &reply, 200)?;
        let known: Vec<KnownLanguage> = decode(&LANGUAGES, &reply.body)?;
        if known.is_empty() {
            return Err(Error::EmptyList {
                path: LANGUAGES.path.to_string(),
            });
        }
        Ok(Current {
            profiles,
            known: known.into_iter().map(|l| l.code2).collect(),
        })
    }

    fn diff(&self, current: &Self::Current) -> Result<Vec<Change>, Error> {
        let unknown: Vec<String> = self
            .profiles
            .iter()
            .flat_map(|(name, languages)| {
                languages
                    .iter()
                    .filter(|l| !current.known.contains(&l.language))
                    .map(move |l| {
                        format!("profile {name}: Bazarr knows no language {}", l.language)
                    })
            })
            .collect();
        if !unknown.is_empty() {
            return Err(Error::Mismatch(unknown));
        }
        let mut changes = Vec::new();
        for (name, languages) in &self.profiles {
            let subject = format!("profile {name}");
            match Self::find(current, name)? {
                None => changes.push(Change {
                    subject,
                    field: String::new(),
                    current: "(missing)".to_string(),
                    desired: "(added)".to_string(),
                }),
                Some(profile) if profile.languages == *languages => {}
                Some(profile) => {
                    // The cutoff names an item by its number, and the items
                    // are numbered anew.
                    if profile.cutoff.is_some_and(|cutoff| cutoff != CUTOFF_ANY) {
                        return Err(Error::Refused(format!(
                            "profile {name} has a cutoff on one of its languages, and a change of the languages would leave it pointing elsewhere; clear the cutoff in Bazarr first"
                        )));
                    }
                    changes.push(Change {
                        subject,
                        field: "languages".to_string(),
                        current: labels(&profile.languages),
                        desired: labels(languages),
                    });
                }
            }
        }
        Ok(changes)
    }

    fn notes(&self, current: &Self::Current) -> Vec<String> {
        current
            .profiles
            .iter()
            .filter(|p| !self.profiles.contains_key(&p.name))
            .map(|p| format!("profile {} is not in the spec and stays as it is", p.name))
            .collect()
    }

    /// One request with every profile: the read ones as they were, a changed
    /// one with new items, a missing one at the end under the next free id.
    fn write(&self, t: &dyn Transport, current: &Self::Current) -> Result<(), Error> {
        let mut list = Vec::new();
        for profile in &current.profiles {
            let mut raw = profile.raw.clone();
            if let Some(languages) = self.profiles.get(&profile.name) {
                if profile.languages != *languages {
                    raw.insert("items".to_string(), items(languages));
                }
            }
            list.push(Value::Object(raw));
        }
        let mut next = current.profiles.iter().map(|p| p.id).max().unwrap_or(0) + 1;
        for (name, languages) in &self.profiles {
            if current.profiles.iter().any(|p| p.name == *name) {
                continue;
            }
            list.push(json!({
                "profileId": next,
                "name": name,
                "cutoff": null,
                "items": items(languages),
                "mustContain": [],
                "mustNotContain": [],
                "originalFormat": 0,
                "tag": null,
            }));
            next += 1;
        }
        let body = Value::Array(list).to_string();
        let reply = t.post_form(SETTINGS.path, &[(FORM_FIELD, &body)])?;
        expect(&SETTINGS, &reply, 204)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        engine::{run, Mode, Outcome, Timing},
        testing::{ok, FakeClock, FakeTransport, Step},
    };

    const STATUS_JSON: &str =
        include_str!("../../tests/fixtures/bazarr-1.6.0/constructed-status-version-only.json");
    const PROFILES_JSON: &str =
        include_str!("../../tests/fixtures/bazarr-1.6.0/language-profiles.json");
    const LANGUAGES_JSON: &str = include_str!("../../tests/fixtures/bazarr-1.6.0/languages.json");

    fn language(code: &str) -> Language {
        Language {
            language: code.to_string(),
            hi: false,
            forced: false,
            audio_exclude: false,
            audio_only_include: false,
        }
    }

    fn task(profiles: &[(&str, Vec<Language>)]) -> LanguageProfiles {
        LanguageProfiles {
            profiles: profiles
                .iter()
                .map(|(name, languages)| (name.to_string(), languages.clone()))
                .collect(),
        }
    }

    /// The host's profile: German, then English.
    fn de_en() -> LanguageProfiles {
        task(&[("DE+EN", vec![language("de"), language("en")])])
    }

    fn bazarr(profiles: &str) -> FakeTransport {
        FakeTransport::default()
            .on_get(STATUS.path, vec![ok(STATUS_JSON)])
            .on_get(PROFILES.path, vec![ok(profiles)])
            .on_get(LANGUAGES.path, vec![ok(LANGUAGES_JSON)])
            .on_put(vec![Step::Answer(204, String::new())])
    }

    /// The profiles of the one form field a write sent.
    fn sent(t: &FakeTransport) -> Vec<Value> {
        let written = t.written.borrow();
        assert_eq!(written.len(), 1, "one request carries the whole list");
        assert_eq!(written[0].0, "/api/system/settings");
        let json = written[0]
            .1
            .strip_prefix("languages-profiles=")
            .expect("the one form field");
        serde_json::from_str(json).unwrap()
    }

    fn recorded() -> Vec<Value> {
        serde_json::from_str(PROFILES_JSON).unwrap()
    }

    #[test]
    fn probe_reports_the_version_and_a_refused_key_ends_the_wait() {
        assert_eq!(probe(&bazarr("[]")).ok().unwrap(), "1.6.0");
        let t = FakeTransport::default().on_get(STATUS.path, vec![Step::Answer(503, "".into())]);
        assert!(matches!(probe(&t), Err(Probe::NotYet(_))));
        let t = FakeTransport::default().on_get(STATUS.path, vec![ok(r#"{"data":{}}"#)]);
        assert!(matches!(probe(&t), Err(Probe::NotYet(_))));
        let t = FakeTransport::default()
            .on_get(STATUS.path, vec![Step::Answer(401, "Unauthorized".into())]);
        let Err(Probe::Fatal(e)) = probe(&t) else {
            panic!("a refused key is fatal");
        };
        assert_eq!(
            e.to_string(),
            "GET /api/system/status answered HTTP 401: the API key was refused"
        );
    }

    #[test]
    fn the_recorded_profile_is_already_the_hosts() {
        let task = de_en();
        let current = task.read(&bazarr(PROFILES_JSON)).unwrap();
        assert_eq!(task.diff(&current).unwrap(), vec![]);
        assert_eq!(task.notes(&current), Vec::<String>::new());
    }

    #[test]
    fn a_fresh_bazarr_gets_the_profile_under_id_one() {
        let task = de_en();
        let t = bazarr("[]");
        let current = task.read(&t).unwrap();
        let changes = task.diff(&current).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(
            changes[0].to_string(),
            "profile DE+EN: (missing) -> (added)"
        );
        task.write(&t, &current).unwrap();
        // What Bazarr answered for the profile the shell unit had created.
        let mut expected = recorded();
        expected[0]["originalFormat"] = json!(0);
        assert_eq!(sent(&t), expected);
    }

    #[test]
    fn a_profile_the_spec_does_not_name_goes_back_exactly_as_read() {
        // Bazarr deletes every profile the list leaves out.
        let task = task(&[("Nordic", vec![language("sv"), language("da")])]);
        let t = bazarr(PROFILES_JSON);
        let current = task.read(&t).unwrap();
        assert_eq!(
            task.notes(&current),
            ["profile DE+EN is not in the spec and stays as it is"]
        );
        assert_eq!(
            task.diff(&current).unwrap()[0].to_string(),
            "profile Nordic: (missing) -> (added)"
        );
        task.write(&t, &current).unwrap();
        let sent = sent(&t);
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0], recorded()[0]);
        assert_eq!(sent[1]["profileId"], json!(2));
        assert_eq!(sent[1]["name"], json!("Nordic"));
        assert_eq!(sent[1]["items"][1]["language"], json!("da"));
        assert_eq!(sent[1]["items"][1]["id"], json!(2));
    }

    #[test]
    fn changed_languages_keep_the_id_and_every_other_field() {
        let mut forced = language("en");
        forced.forced = true;
        let task = task(&[("DE+EN", vec![language("de"), forced])]);
        let mut with_tag = recorded();
        with_tag[0]["tag"] = json!("kept");
        with_tag[0]["mustContain"] = json!(["x264"]);
        let t = bazarr(&Value::Array(with_tag.clone()).to_string());
        let current = task.read(&t).unwrap();
        assert_eq!(
            task.diff(&current).unwrap()[0].to_string(),
            "profile DE+EN: languages [de, en] -> [de, en (forced)]"
        );
        task.write(&t, &current).unwrap();
        let sent = sent(&t);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0]["items"][1]["forced"], json!("True"));
        assert_eq!(sent[0]["items"][0]["forced"], json!("False"));
        let mut rest = sent[0].clone();
        rest["items"] = with_tag[0]["items"].clone();
        assert_eq!(rest, with_tag[0]);
    }

    #[test]
    fn the_order_of_the_languages_is_part_of_the_profile() {
        let task = task(&[("DE+EN", vec![language("en"), language("de")])]);
        let current = task.read(&bazarr(PROFILES_JSON)).unwrap();
        assert_eq!(
            task.diff(&current).unwrap()[0].to_string(),
            "profile DE+EN: languages [de, en] -> [en, de]"
        );
    }

    #[test]
    fn a_cutoff_on_an_item_refuses_a_change_of_the_items_and_nothing_else() {
        let with_cutoff = |cutoff: i64| {
            let mut list = recorded();
            list[0]["cutoff"] = json!(cutoff);
            Value::Array(list).to_string()
        };
        // Unchanged languages: the cutoff is nobody's business.
        let same = de_en();
        let current = same.read(&bazarr(&with_cutoff(2))).unwrap();
        assert_eq!(same.diff(&current).unwrap(), vec![]);

        let other = task(&[("DE+EN", vec![language("en")])]);
        let current = other.read(&bazarr(&with_cutoff(2))).unwrap();
        let err = other.diff(&current).err().unwrap().to_string();
        assert!(
            err.starts_with("refused: profile DE+EN has a cutoff"),
            "{err}"
        );
        // "Any language" names no item.
        let current = other.read(&bazarr(&with_cutoff(65535))).unwrap();
        assert_eq!(other.diff(&current).unwrap().len(), 1);
    }

    #[test]
    fn a_language_bazarr_does_not_know_stops_the_run_before_a_write() {
        let task = task(&[("DE+EN", vec![language("de"), language("xx")])]);
        let current = task.read(&bazarr("[]")).unwrap();
        assert_eq!(
            task.diff(&current).err().unwrap().to_string(),
            "the spec does not fit the service: profile DE+EN: Bazarr knows no language xx"
        );
    }

    #[test]
    fn two_profiles_of_one_name_are_a_mismatch() {
        let mut twice = recorded();
        let mut second = twice[0].clone();
        second["profileId"] = json!(2);
        twice.push(second);
        let task = de_en();
        let current = task
            .read(&bazarr(&Value::Array(twice).to_string()))
            .unwrap();
        assert_eq!(
            task.diff(&current).err().unwrap().to_string(),
            "the spec does not fit the service: 2 profiles are named DE+EN"
        );
    }

    #[test]
    fn a_profile_bazarr_could_not_take_back_is_an_error_at_the_read() {
        let mut list = recorded();
        list[0].as_object_mut().unwrap().remove("mustContain");
        let err = de_en()
            .read(&bazarr(&Value::Array(list).to_string()))
            .err()
            .unwrap()
            .to_string();
        assert_eq!(
            err,
            "/api/system/languages/profiles: the answer does not have the expected shape: profile 1: missing field `mustContain`"
        );
    }

    #[test]
    fn switches_are_text_or_booleans_and_an_error_quotes_no_value() {
        let mut list = recorded();
        list[0]["items"][1]["forced"] = json!(true);
        list[0]["items"][1]
            .as_object_mut()
            .unwrap()
            .remove("audio_only_include");
        let task = de_en();
        let current = task
            .read(&bazarr(&Value::Array(list.clone()).to_string()))
            .unwrap();
        assert_eq!(
            task.diff(&current).unwrap()[0].to_string(),
            "profile DE+EN: languages [de, en (forced)] -> [de, en]"
        );

        list[0]["items"][1]["forced"] = json!("leaky-value");
        let err = task
            .read(&bazarr(&Value::Array(list).to_string()))
            .err()
            .unwrap()
            .to_string();
        assert!(err.contains("profile 1"), "{err}");
        assert!(!err.contains("leaky"), "{err}");
    }

    #[test]
    fn no_known_language_at_all_is_an_error_and_not_a_spec_full_of_typos() {
        let t = FakeTransport::default()
            .on_get(PROFILES.path, vec![ok(PROFILES_JSON)])
            .on_get(LANGUAGES.path, vec![ok("[]")]);
        assert_eq!(
            de_en().read(&t).err().unwrap().to_string(),
            "/api/system/languages returned an empty list"
        );
    }

    #[test]
    fn a_refused_read_or_write_carries_nothing_from_the_body() {
        let task = de_en();
        let t = FakeTransport::default()
            .on_get(PROFILES.path, vec![Step::Answer(401, "leaky-body".into())]);
        assert_eq!(
            task.read(&t).err().unwrap().to_string(),
            "GET /api/system/languages/profiles answered HTTP 401: the API key was refused"
        );
        let t = bazarr("[]").on_put(vec![Step::Answer(406, "leaky-body".into())]);
        let current = task.read(&t).unwrap();
        assert_eq!(
            task.write(&t, &current).err().unwrap().to_string(),
            "POST /api/system/settings answered HTTP 406"
        );
    }

    #[test]
    fn apply_writes_once_and_reads_back_until_the_profile_is_there() {
        let t = bazarr("[]").on_get(PROFILES.path, vec![ok("[]"), ok("[]"), ok(PROFILES_JSON)]);
        let report = run(
            Mode::Apply,
            &de_en(),
            &t,
            &FakeClock::new(),
            Timing::default(),
        )
        .unwrap();
        assert_eq!(report.version, "1.6.0");
        let Outcome::Changed(changes) = report.outcome else {
            panic!("the profile was missing");
        };
        assert_eq!(changes.len(), 1);
        assert_eq!(t.written.borrow().len(), 1);
    }

    #[test]
    fn a_plan_writes_nothing() {
        let t = bazarr("[]");
        let report = run(
            Mode::Plan,
            &de_en(),
            &t,
            &FakeClock::new(),
            Timing::default(),
        )
        .unwrap();
        assert!(matches!(report.outcome, Outcome::Differs(_)));
        assert!(t.written.borrow().is_empty());
    }
}
