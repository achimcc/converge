use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Deserializer};

use crate::error::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Service {
    Radarr,
    Sonarr,
    Jellyfin,
}

impl Service {
    pub fn name(self) -> &'static str {
        match self {
            Service::Radarr => "radarr",
            Service::Sonarr => "sonarr",
            Service::Jellyfin => "jellyfin",
        }
    }

    /// The request header the API key travels in.
    pub fn key_header(self) -> &'static str {
        match self {
            Service::Radarr | Service::Sonarr => "X-Api-Key",
            Service::Jellyfin => "X-Emby-Token",
        }
    }

    fn is_arr(self) -> bool {
        matches!(self, Service::Radarr | Service::Sonarr)
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum TaskName {
    QualityDefinitions,
    QualityProfiles,
    ServerConfiguration,
    LibraryOptions,
    ScheduledTaskTriggers,
    PluginConfigurations,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSpec {
    service: Service,
    base_url: String,
    api_key_credential: String,
    task: TaskName,
    desired: serde_json::Value,
}

/// Sizes in MB per minute, as the service stores them. `None` is unlimited.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SizeLimits {
    #[serde(deserialize_with = "present")]
    pub min: Option<f64>,
    #[serde(deserialize_with = "present")]
    pub preferred: Option<f64>,
    #[serde(deserialize_with = "present")]
    pub max: Option<f64>,
}

/// With `deserialize_with`, serde requires the key. A plain `Option` field
/// would turn a forgotten key into `None` -- unlimited -- without a word.
fn present<'de, D: Deserializer<'de>>(d: D) -> Result<Option<f64>, D::Error> {
    Option::<f64>::deserialize(d)
}

impl SizeLimits {
    fn check_order(&self) -> Result<(), String> {
        let min = self.min.unwrap_or(0.0);
        let preferred = self.preferred.unwrap_or(f64::INFINITY);
        let max = self.max.unwrap_or(f64::INFINITY);
        if min > preferred {
            return Err(format!(
                "min {min} is above preferred {}",
                show(self.preferred)
            ));
        }
        if preferred > max {
            return Err(format!(
                "preferred {} is above max {}",
                show(self.preferred),
                show(self.max)
            ));
        }
        Ok(())
    }
}

pub fn show(value: Option<f64>) -> String {
    value.map_or_else(|| "unlimited".to_string(), |v| v.to_string())
}

/// Which qualities every quality profile must allow. Only allowing is
/// possible: converge never takes a quality away from a profile.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfilePolicy {
    pub allow_in_every_profile: Vec<String>,
}

/// Fields by path in each named library's `LibraryOptions`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibrarySettings {
    pub libraries: Vec<String>,
    pub set: BTreeMap<String, serde_json::Value>,
}

/// The complete trigger list of every scheduled task whose key starts with
/// `key_prefix`, compared on the keys each trigger object names.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskTriggers {
    pub key_prefix: String,
    pub triggers: Vec<serde_json::Map<String, serde_json::Value>>,
}

/// One plugin's configuration: plain fields by path, and fields whose value
/// comes from a systemd credential (path -> credential name). The name is the
/// plugin's name as `/Plugins` reports it, so a wrong id fails loudly.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginSettings {
    pub name: String,
    #[serde(default)]
    pub set: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub secrets: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Desired {
    QualityDefinitions(BTreeMap<String, SizeLimits>),
    QualityProfiles(ProfilePolicy),
    ServerConfiguration(BTreeMap<String, serde_json::Value>),
    LibraryOptions(LibrarySettings),
    ScheduledTaskTriggers(TaskTriggers),
    PluginConfigurations(BTreeMap<String, PluginSettings>),
}

/// A non-empty map of well-formed dotted paths.
fn field_paths(map: &BTreeMap<String, serde_json::Value>, what: &str) -> Result<(), String> {
    if map.is_empty() {
        return Err(format!("{what} names no field"));
    }
    match map.keys().find(|k| crate::paths::segments(k).is_none()) {
        Some(bad) => Err(format!("{what}: {bad:?} is not a dotted path")),
        None => Ok(()),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Spec {
    pub path: PathBuf,
    pub service: Service,
    pub base_url: String,
    pub api_key_credential: String,
    pub desired: Desired,
}

impl Spec {
    pub fn load(path: &Path) -> Result<Spec, Error> {
        let text = std::fs::read_to_string(path).map_err(|source| Error::SpecRead {
            path: path.to_path_buf(),
            source,
        })?;
        Self::parse(path, &text)
    }

    pub fn parse(path: &Path, text: &str) -> Result<Spec, Error> {
        let invalid = |reason: String| Error::SpecInvalid {
            path: path.to_path_buf(),
            reason,
        };
        let raw: RawSpec = serde_json::from_str(text).map_err(|e| invalid(e.to_string()))?;
        if !raw.base_url.starts_with("http://") {
            return Err(invalid(format!(
                "base_url must start with http:// (this version links no TLS), got {:?}",
                raw.base_url
            )));
        }
        let arr_task = matches!(
            raw.task,
            TaskName::QualityDefinitions | TaskName::QualityProfiles
        );
        if arr_task != raw.service.is_arr() {
            return Err(invalid(format!(
                "the task does not belong to service {:?}",
                raw.service.name()
            )));
        }
        let desired = match raw.task {
            TaskName::QualityDefinitions => {
                let map: BTreeMap<String, SizeLimits> = serde_json::from_value(raw.desired)
                    .map_err(|e| invalid(format!("desired: {e}")))?;
                if map.is_empty() {
                    return Err(invalid("desired names no quality".to_string()));
                }
                for (name, limits) in &map {
                    limits
                        .check_order()
                        .map_err(|r| invalid(format!("{name}: {r}")))?;
                }
                Desired::QualityDefinitions(map)
            }
            TaskName::QualityProfiles => {
                let policy: ProfilePolicy = serde_json::from_value(raw.desired)
                    .map_err(|e| invalid(format!("desired: {e}")))?;
                let names = &policy.allow_in_every_profile;
                if names.is_empty() {
                    return Err(invalid(
                        "desired.allow_in_every_profile names no quality".to_string(),
                    ));
                }
                let mut seen = std::collections::BTreeSet::new();
                if let Some(twice) = names.iter().find(|n| !seen.insert(n.as_str())) {
                    return Err(invalid(format!(
                        "desired.allow_in_every_profile names {twice:?} twice"
                    )));
                }
                Desired::QualityProfiles(policy)
            }
            TaskName::ServerConfiguration => {
                let map: BTreeMap<String, serde_json::Value> = serde_json::from_value(raw.desired)
                    .map_err(|e| invalid(format!("desired: {e}")))?;
                field_paths(&map, "desired").map_err(invalid)?;
                Desired::ServerConfiguration(map)
            }
            TaskName::LibraryOptions => {
                let settings: LibrarySettings = serde_json::from_value(raw.desired)
                    .map_err(|e| invalid(format!("desired: {e}")))?;
                if settings.libraries.is_empty() {
                    return Err(invalid("desired.libraries names no library".to_string()));
                }
                field_paths(&settings.set, "desired.set").map_err(invalid)?;
                Desired::LibraryOptions(settings)
            }
            TaskName::ScheduledTaskTriggers => {
                let triggers: TaskTriggers = serde_json::from_value(raw.desired)
                    .map_err(|e| invalid(format!("desired: {e}")))?;
                if triggers.key_prefix.is_empty() {
                    return Err(invalid("desired.key_prefix is empty".to_string()));
                }
                // An empty list would delete every trigger. converge does not
                // delete; say so instead of doing it.
                if triggers.triggers.is_empty() {
                    return Err(invalid(
                        "desired.triggers is empty -- that would remove every trigger".to_string(),
                    ));
                }
                Desired::ScheduledTaskTriggers(triggers)
            }
            TaskName::PluginConfigurations => {
                let plugins: BTreeMap<String, PluginSettings> = serde_json::from_value(raw.desired)
                    .map_err(|e| invalid(format!("desired: {e}")))?;
                if plugins.is_empty() {
                    return Err(invalid("desired names no plugin".to_string()));
                }
                for (id, plugin) in &plugins {
                    let at = format!("desired.{id}");
                    if id.len() != 32
                        || !id
                            .chars()
                            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
                    {
                        return Err(invalid(format!(
                            "{at}: a plugin id is 32 lowercase hex digits without dashes"
                        )));
                    }
                    if plugin.set.is_empty() && plugin.secrets.is_empty() {
                        return Err(invalid(format!("{at} names no field")));
                    }
                    if !plugin.set.is_empty() {
                        field_paths(&plugin.set, &format!("{at}.set")).map_err(invalid)?;
                    }
                    for (path, credential) in &plugin.secrets {
                        if crate::paths::segments(path).is_none() {
                            return Err(invalid(format!(
                                "{at}.secrets: {path:?} is not a dotted path"
                            )));
                        }
                        if credential.is_empty() || credential.contains('/') {
                            return Err(invalid(format!(
                                "{at}.secrets.{path}: {credential:?} is not a credential name"
                            )));
                        }
                        if plugin.set.contains_key(path) {
                            return Err(invalid(format!(
                                "{at}: {path} is both in set and in secrets"
                            )));
                        }
                    }
                }
                Desired::PluginConfigurations(plugins)
            }
        };
        Ok(Spec {
            path: path.to_path_buf(),
            service: raw.service,
            base_url: raw.base_url,
            api_key_credential: raw.api_key_credential,
            desired,
        })
    }

    pub fn task_name(&self) -> &'static str {
        match self.desired {
            Desired::QualityDefinitions(_) => "quality-definitions",
            Desired::QualityProfiles(_) => "quality-profiles",
            Desired::ServerConfiguration(_) => "server-configuration",
            Desired::LibraryOptions(_) => "library-options",
            Desired::ScheduledTaskTriggers(_) => "scheduled-task-triggers",
            Desired::PluginConfigurations(_) => "plugin-configurations",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"{
      "service": "radarr",
      "base_url": "http://localhost:7878",
      "api_key_credential": "radarr-api-key",
      "task": "quality-definitions",
      "desired": {
        "Bluray-1080p": { "min": 12.5, "preferred": null, "max": null },
        "CAM": { "min": 0, "preferred": 95, "max": 100 }
      }
    }"#;

    fn parse(text: &str) -> Result<Spec, Error> {
        Spec::parse(Path::new("t.json"), text)
    }

    #[test]
    fn parses_the_documented_shape() {
        let spec = parse(GOOD).unwrap();
        assert_eq!(spec.service, Service::Radarr);
        assert_eq!(spec.task_name(), "quality-definitions");
        let Desired::QualityDefinitions(map) = spec.desired else {
            panic!("wrong task")
        };
        assert_eq!(
            map["Bluray-1080p"],
            SizeLimits {
                min: Some(12.5),
                preferred: None,
                max: None
            }
        );
    }

    const PROFILES: &str = r#"{"service":"sonarr","base_url":"http://localhost:8989","api_key_credential":"sonarr-api-key","task":"quality-profiles","desired":{"allow_in_every_profile":["Unknown"]}}"#;

    #[test]
    fn parses_a_quality_profiles_spec() {
        let spec = parse(PROFILES).unwrap();
        assert_eq!(spec.task_name(), "quality-profiles");
        assert_eq!(
            spec.desired,
            Desired::QualityProfiles(ProfilePolicy {
                allow_in_every_profile: vec!["Unknown".to_string()]
            })
        );
    }

    #[test]
    fn a_quality_profiles_spec_is_strict() {
        let misspelt = PROFILES.replace("allow_in_every_profile", "allow_everywhere");
        let err = parse(&misspelt).err().unwrap().to_string();
        assert!(err.contains("allow_everywhere"), "{err}");
        assert!(parse(&PROFILES.replace(r#"["Unknown"]"#, "[]")).is_err());
        let twice = PROFILES.replace(r#"["Unknown"]"#, r#"["Unknown","Unknown"]"#);
        assert!(parse(&twice).is_err());
    }

    fn jellyfin(task: &str, desired: &str) -> Result<Spec, Error> {
        parse(&format!(
            r#"{{"service":"jellyfin","base_url":"http://localhost:8096","api_key_credential":"k","task":"{task}","desired":{desired}}}"#
        ))
    }

    #[test]
    fn parses_the_three_jellyfin_tasks() {
        let spec = jellyfin(
            "server-configuration",
            r#"{"TrickplayOptions.EnableHwAcceleration": true}"#,
        )
        .unwrap();
        assert_eq!(spec.service.key_header(), "X-Emby-Token");
        assert_eq!(spec.task_name(), "server-configuration");
        let spec = jellyfin(
            "library-options",
            r#"{"libraries":["Filme"],"set":{"EnableTrickplayImageExtraction":true}}"#,
        )
        .unwrap();
        assert_eq!(spec.task_name(), "library-options");
        let spec = jellyfin(
            "scheduled-task-triggers",
            r#"{"key_prefix":"Merge","triggers":[{"Type":"DailyTrigger","TimeOfDayTicks":198000000000}]}"#,
        )
        .unwrap();
        assert_eq!(spec.task_name(), "scheduled-task-triggers");
    }

    #[test]
    fn jellyfin_specs_are_strict() {
        assert!(jellyfin("server-configuration", "{}").is_err(), "empty");
        let bad_path = jellyfin("server-configuration", r#"{"A..B": 1}"#);
        assert!(bad_path
            .err()
            .unwrap()
            .to_string()
            .contains("not a dotted path"));
        let unknown = jellyfin(
            "library-options",
            r#"{"libraries":["Filme"],"set":{"X":1},"extra":1}"#,
        );
        assert!(unknown.err().unwrap().to_string().contains("extra"));
        assert!(jellyfin("library-options", r#"{"libraries":[],"set":{"X":1}}"#).is_err());
        let wipe = jellyfin(
            "scheduled-task-triggers",
            r#"{"key_prefix":"Merge","triggers":[]}"#,
        );
        assert!(wipe
            .err()
            .unwrap()
            .to_string()
            .contains("remove every trigger"));
    }

    #[test]
    fn plugin_configurations_are_strict() {
        let good = r#"{"c531afa3de204055aca5a7cc43adf783":{"name":"Jellyfin Oscars","secrets":{"OmdbApiKey":"jellyfin-omdb-key"}}}"#;
        let spec = jellyfin("plugin-configurations", good).unwrap();
        assert_eq!(spec.task_name(), "plugin-configurations");
        let dashed = good.replace(
            "c531afa3de204055aca5a7cc43adf783",
            "c531afa3-de20-4055-aca5-a7cc43adf783",
        );
        assert!(jellyfin("plugin-configurations", &dashed).is_err());
        let nothing = r#"{"c531afa3de204055aca5a7cc43adf783":{"name":"Jellyfin Oscars"}}"#;
        assert!(jellyfin("plugin-configurations", nothing).is_err());
        let both = r#"{"c531afa3de204055aca5a7cc43adf783":{"name":"X","set":{"A":1},"secrets":{"A":"k"}}}"#;
        assert!(jellyfin("plugin-configurations", both)
            .err()
            .unwrap()
            .to_string()
            .contains("both in set and in secrets"));
        let path_credential =
            r#"{"c531afa3de204055aca5a7cc43adf783":{"name":"X","secrets":{"A":"../x"}}}"#;
        assert!(jellyfin("plugin-configurations", path_credential).is_err());
    }

    #[test]
    fn a_task_of_the_wrong_service_is_an_error() {
        let err = parse(&GOOD.replace(r#""radarr""#, r#""jellyfin""#))
            .err()
            .unwrap()
            .to_string();
        assert!(err.contains("does not belong to service"), "{err}");
        let err = jellyfin(
            "quality-profiles",
            r#"{"allow_in_every_profile":["Unknown"]}"#,
        )
        .err()
        .unwrap()
        .to_string();
        assert!(err.contains("does not belong to service"), "{err}");
    }

    #[test]
    fn a_misspelt_limit_key_is_an_error() {
        let text = GOOD.replace(r#""preferred": 95"#, r#""prefered": 95"#);
        let err = parse(&text).err().unwrap().to_string();
        assert!(err.contains("prefered"), "{err}");
    }

    #[test]
    fn an_unknown_top_level_key_is_an_error() {
        let text = GOOD.replace(r#""task""#, r#""dry_run": true, "task""#);
        let err = parse(&text).err().unwrap().to_string();
        assert!(err.contains("dry_run"), "{err}");
    }

    #[test]
    fn a_missing_limit_key_is_an_error_not_unlimited() {
        let text = GOOD.replace(r#""preferred": null, "#, "");
        let err = parse(&text).err().unwrap().to_string();
        assert!(err.contains("preferred"), "{err}");
    }

    #[test]
    fn ordering_is_checked_with_null_as_unlimited() {
        let text = GOOD.replace(
            r#""min": 0, "preferred": 95"#,
            r#""min": 96, "preferred": 95"#,
        );
        let err = parse(&text).err().unwrap().to_string();
        assert!(err.contains("CAM: min 96"), "{err}");
        let text = GOOD.replace(r#""max": 100"#, r#""max": 90"#);
        assert!(parse(&text).is_err());
        let text = GOOD.replace(
            r#""preferred": 95, "max": 100"#,
            r#""preferred": null, "max": 100"#,
        );
        assert!(parse(&text).is_err(), "unlimited preferred above a max");
    }

    #[test]
    fn unknown_service_or_task_is_an_error() {
        assert!(parse(&GOOD.replace("radarr", "lidarr")).is_err());
        assert!(parse(&GOOD.replace("quality-definitions", "quality-profiles")).is_err());
    }

    #[test]
    fn empty_desired_and_https_are_errors() {
        let empty = r#"{"service":"sonarr","base_url":"http://x","api_key_credential":"k","task":"quality-definitions","desired":{}}"#;
        assert!(parse(empty).is_err());
        assert!(parse(&GOOD.replace("http://", "https://")).is_err());
    }
}
