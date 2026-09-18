use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Deserializer};

use crate::{error::Error, secret::Secret};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Service {
    Radarr,
    Sonarr,
    Jellyfin,
    Trailarr,
    Ntfy,
    Lidarr,
    Prowlarr,
    Bindery,
    Seerr,
    Koel,
    #[serde(rename = "suggestarr")]
    SuggestArr,
    Kavita,
}

impl Service {
    pub fn name(self) -> &'static str {
        match self {
            Service::Radarr => "radarr",
            Service::Sonarr => "sonarr",
            Service::Jellyfin => "jellyfin",
            Service::Trailarr => "trailarr",
            Service::Ntfy => "ntfy",
            Service::Lidarr => "lidarr",
            Service::Prowlarr => "prowlarr",
            Service::Bindery => "bindery",
            Service::Seerr => "seerr",
            Service::Koel => "koel",
            Service::SuggestArr => "suggestarr",
            Service::Kavita => "kavita",
        }
    }

    /// The request header the API key travels in.
    pub fn key_header(self) -> &'static str {
        match self {
            Service::Radarr
            | Service::Sonarr
            | Service::Lidarr
            | Service::Prowlarr
            | Service::Bindery
            | Service::Seerr
            | Service::Kavita => "X-Api-Key",
            Service::Jellyfin => "X-Emby-Token",
            Service::Trailarr => "X-API-KEY",
            Service::Ntfy | Service::Koel | Service::SuggestArr => "Authorization",
        }
    }

    /// The header's value. The credential holds the bare key; ntfy and Koel
    /// want it as a bearer token, and for SuggestArr the value is the JWT a
    /// login returned (the credential there holds a password, never a key).
    pub fn key_value(self, key: Secret) -> Secret {
        match self {
            Service::Ntfy | Service::Koel | Service::SuggestArr => {
                Secret::new(format!("Bearer {}", key.expose()))
            }
            _ => key,
        }
    }

    /// Whether every request must say `Accept: application/json`. Koel
    /// answers a refused request without it with a redirect to its web page
    /// (design §18).
    pub fn accepts_json_only(self) -> bool {
        self == Service::Koel
    }

    fn is_arr(self) -> bool {
        matches!(self, Service::Radarr | Service::Sonarr)
    }

    /// Radarr, Sonarr and Lidarr: the same application underneath (Servarr),
    /// whose configuration documents and root folders share their shape.
    pub fn is_servarr(self) -> bool {
        matches!(self, Service::Radarr | Service::Sonarr | Service::Lidarr)
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
    NamedConfiguration,
    Connections,
    TrailerProfiles,
    AccountSubscriptions,
    Naming,
    MediaManagement,
    DownloadClientConfig,
    IndexerConfig,
    DelayProfiles,
    RootFolders,
    DownloadClients,
    Notifications,
    Applications,
    Indexers,
    IndexerProxies,
    ProwlarrInstances,
    OidcProviders,
    Settings,
    Main,
    Jellyfin,
    RadarrServers,
    SonarrServers,
    Webhook,
    RadioStations,
    Configuration,
    ServerSettings,
    Libraries,
}

impl TaskName {
    fn belongs_to(self, service: Service) -> bool {
        match self {
            TaskName::QualityDefinitions | TaskName::QualityProfiles => service.is_arr(),
            TaskName::ServerConfiguration
            | TaskName::LibraryOptions
            | TaskName::ScheduledTaskTriggers
            | TaskName::PluginConfigurations
            | TaskName::NamedConfiguration => service == Service::Jellyfin,
            TaskName::Connections | TaskName::TrailerProfiles => service == Service::Trailarr,
            TaskName::AccountSubscriptions => service == Service::Ntfy,
            TaskName::Naming
            | TaskName::MediaManagement
            | TaskName::DownloadClientConfig
            | TaskName::IndexerConfig
            | TaskName::DelayProfiles => service.is_servarr(),
            TaskName::RootFolders => service.is_servarr() || service == Service::Bindery,
            TaskName::DownloadClients => {
                service.is_servarr() || service == Service::Prowlarr || service == Service::Bindery
            }
            TaskName::Notifications => service.is_servarr() || service == Service::Prowlarr,
            TaskName::ProwlarrInstances | TaskName::Settings | TaskName::OidcProviders => {
                service == Service::Bindery
            }
            // `indexers` means two different things: Prowlarr's own indexer
            // providers (§13), and the switch over the ones bindery has synced
            // from Prowlarr (§23). The arm below tells them apart by service.
            TaskName::Indexers => service == Service::Prowlarr || service == Service::Bindery,
            TaskName::Applications | TaskName::IndexerProxies => service == Service::Prowlarr,
            TaskName::Main
            | TaskName::Jellyfin
            | TaskName::RadarrServers
            | TaskName::SonarrServers
            | TaskName::Webhook => service == Service::Seerr,
            TaskName::RadioStations => service == Service::Koel,
            TaskName::Configuration => service == Service::SuggestArr,
            TaskName::ServerSettings | TaskName::Libraries => service == Service::Kavita,
        }
    }
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

/// A Jellyfin named configuration (`/System/Configuration/{key}`) and fields
/// by path in it. Only keys whose document the OpenAPI description names are
/// accepted: the generic endpoint declares its answer as binary, so the paths
/// are checked against the component the key maps to (design §15).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NamedConfigurationSettings {
    pub key: NamedKey,
    pub set: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NamedKey {
    Network,
    Branding,
}

impl NamedKey {
    /// The key as it goes into the path.
    pub fn path_segment(self) -> &'static str {
        match self {
            NamedKey::Network => "network",
            NamedKey::Branding => "branding",
        }
    }

    /// The OpenAPI component the document's paths are checked against. For
    /// branding that is the DTO the write endpoint accepts: the stored
    /// `BrandingOptions` has one more field (`SplashscreenLocation`) the API
    /// does not let a client set.
    pub fn component(self) -> &'static str {
        match self {
            NamedKey::Network => "NetworkConfiguration",
            NamedKey::Branding => "BrandingOptionsDto",
        }
    }
}

/// The complete trigger list of every scheduled task whose key starts with
/// `key_prefix`, compared on the keys each trigger object names.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskTriggers {
    pub key_prefix: String,
    pub triggers: Vec<serde_json::Map<String, serde_json::Value>>,
}

/// One plugin's configuration: plain fields by path, fields whose value
/// comes from a systemd credential (path -> credential name), and entries of
/// lists the plugin shares with others (path -> keyed items). The name is the
/// plugin's name as `/Plugins` reports it, so a wrong id fails loudly.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginSettings {
    pub name: String,
    #[serde(default)]
    pub set: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub secrets: BTreeMap<String, String>,
    #[serde(default)]
    pub lists: BTreeMap<String, ListItems>,
}

/// Entries of a list that is not converge's alone. Each item is found by the
/// value of its `key` field; the fields it names are set, a missing item is
/// appended, and every other entry of the list stays as it is -- in its place.
/// converge never removes an entry.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListItems {
    pub key: String,
    pub items: Vec<serde_json::Map<String, serde_json::Value>>,
}

/// Trailarr's links to Radarr and Sonarr, by connection name.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrailarrConnections {
    pub connections: BTreeMap<String, ConnectionSettings>,
}

/// One connection: top-level fields of Trailarr's connection object, and the
/// credential holding the API key of the service it connects to.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionSettings {
    pub set: BTreeMap<String, serde_json::Value>,
    pub api_key_credential: String,
}

/// Fields every trailer profile gets.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrailerProfileSettings {
    pub set: BTreeMap<String, serde_json::Value>,
}

/// Which of Seerr's server lists a `*-servers` task is about (design §17).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeerrKind {
    Radarr,
    Sonarr,
}

impl SeerrKind {
    /// The path segment under `/api/v1/settings/`.
    pub fn path(self) -> &'static str {
        match self {
            SeerrKind::Radarr => "radarr",
            SeerrKind::Sonarr => "sonarr",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SeerrKind::Radarr => "Radarr",
            SeerrKind::Sonarr => "Sonarr",
        }
    }
}

/// Seerr's link to Jellyfin: the connection fields, the credential holding
/// Jellyfin's key, and the libraries Seerr scans -- exactly these, by name.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeerrJellyfin {
    pub set: BTreeMap<String, serde_json::Value>,
    pub api_key_credential: String,
    pub libraries: Vec<String>,
}

/// One of Seerr's Radarr or Sonarr entries: top-level fields, the credential
/// holding the service's key, and the quality profile and root folder by
/// name -- Seerr's connection test turns them into id and path.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeerrServer {
    pub set: BTreeMap<String, serde_json::Value>,
    pub api_key_credential: String,
    pub profile: String,
    pub root_folder: String,
}

/// Seerr's webhook agent: fields by path, the payload template as an
/// object, and custom headers whose values come from credentials.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeerrWebhook {
    #[serde(default)]
    pub set: BTreeMap<String, serde_json::Value>,
    pub payload: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

/// One of Koel's radio stations, found by its name (design §18). Every
/// field is sent on every write: Koel's update sets `is_public` to false,
/// `description` to empty and `homepage_url` to null when they are absent.
/// `logo_file` is an image file read on every run; it is sent only when the
/// station has no logo, because Koel stores an image under a random name and
/// nothing can be compared.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KoelStation {
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub description: String,
    pub is_public: bool,
    #[serde(default)]
    pub homepage_url: Option<String>,
    #[serde(default)]
    pub logo_file: Option<PathBuf>,
}

/// SuggestArr's whole configuration, as a spec describes it: plain fields,
/// fields whose value comes from a systemd credential, and the rule that
/// derives the Jellyfin libraries from what the service itself reports.
///
/// `username` is the service account the task logs in as; its password is
/// `api_key_credential`. The account is not a secret and belongs in the
/// spec, so a wrong one is visible in a plan.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SuggestArrConfiguration {
    pub username: String,
    #[serde(default)]
    pub set: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub secrets: BTreeMap<String, String>,
    #[serde(default)]
    pub jellyfin_libraries: Option<SuggestArrLibraries>,
}

/// Which Jellyfin collection types the libraries must not include. SuggestArr
/// reads an empty library list as "all of them", so leaving the home videos
/// out is something to say, not something to omit.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SuggestArrLibraries {
    pub exclude_collection_types: Vec<String>,
}

/// Koel's longest station name (`max:191` in its store and update requests).
const KOEL_NAME_MAX: usize = 191;

/// Subscriptions an ntfy account must have. The topics are secrets -- a topic
/// name is all it takes to read a topic -- so the spec only names the
/// credential that lists them, one per line.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountSubscriptions {
    pub base_url: String,
    pub topics_credential: String,
}

/// Root folders by path. A missing folder is added; converge never removes
/// one.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootFolderSettings {
    pub folders: BTreeMap<String, FolderSettings>,
}

/// One root folder: top-level fields (Lidarr's carry defaults for new
/// artists; Radarr's and Sonarr's have none to set) and profile fields given
/// by the profile's name, which converge looks up -- an id would silently be
/// the wrong one after the database is rebuilt.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FolderSettings {
    #[serde(default)]
    pub set: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub profiles: BTreeMap<String, String>,
}

/// The profile fields a root folder may name by profile name.
pub const PROFILE_FIELDS: [&str; 2] = ["defaultQualityProfileId", "defaultMetadataProfileId"];

/// Fields of a root folder the service owns.
const ROOT_FOLDER_OWN: [&str; 6] = [
    "id",
    "path",
    "accessible",
    "freeSpace",
    "totalSpace",
    "unmappedFolders",
];

/// Servarr providers of one kind (download clients, notifications,
/// Prowlarr's applications) by name.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderSettings {
    pub providers: BTreeMap<String, ProviderEntry>,
}

/// One provider: its implementation (`QBittorrent`, `Webhook`, …), top-level
/// fields of the resource, entries of its `fields` list by name, and entries
/// whose value comes from a credential. A credential's value is never
/// printed: where the service answers `********` it is handed over on every
/// `apply`, where it shows the value it is compared (design §13). `template`
/// names the template to add a missing provider from, where one
/// implementation has many (Prowlarr's `Cardigann` indexers); `tags` are
/// labels, and without them the provider's tags are left alone.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderEntry {
    pub implementation: String,
    #[serde(default)]
    pub template: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub set: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub fields: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub secret_fields: BTreeMap<String, String>,
}

/// Top-level fields of a provider the service owns or converge sets itself.
const PROVIDER_OWN: [&str; 12] = [
    "id",
    "name",
    "implementation",
    "implementationName",
    "configContract",
    "fields",
    "tags",
    "infoLink",
    "link",
    "message",
    "presets",
    "testCommand",
];

/// One bindery entry (design §14): top-level fields, and secret fields whose
/// value comes from a credential. bindery answers those empty.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BinderyEntry {
    #[serde(default)]
    pub set: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub secret_fields: BTreeMap<String, String>,
}

/// Which kind of bindery entry a spec names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinderyKind {
    DownloadClients,
    ProwlarrInstances,
    RootFolders,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Desired {
    QualityDefinitions(BTreeMap<String, SizeLimits>),
    QualityProfiles(ProfilePolicy),
    ServerConfiguration(BTreeMap<String, serde_json::Value>),
    LibraryOptions(LibrarySettings),
    ScheduledTaskTriggers(TaskTriggers),
    PluginConfigurations(BTreeMap<String, PluginSettings>),
    NamedConfiguration(NamedConfigurationSettings),
    Connections(TrailarrConnections),
    TrailerProfiles(TrailerProfileSettings),
    AccountSubscriptions(AccountSubscriptions),
    Naming(BTreeMap<String, serde_json::Value>),
    MediaManagement(BTreeMap<String, serde_json::Value>),
    DownloadClientConfig(BTreeMap<String, serde_json::Value>),
    IndexerConfig(BTreeMap<String, serde_json::Value>),
    DelayProfiles(BTreeMap<String, serde_json::Value>),
    RootFolders(RootFolderSettings),
    DownloadClients(ProviderSettings),
    Notifications(ProviderSettings),
    Applications(ProviderSettings),
    Indexers(ProviderSettings),
    IndexerProxies(ProviderSettings),
    BinderyEntries(BinderyKind, BTreeMap<String, BinderyEntry>),
    BinderySettings(BTreeMap<String, serde_json::Value>),
    BinderyOidcProviders(BTreeMap<String, BinderyEntry>),
    BinderyIndexers(Vec<String>),
    SeerrMain(BTreeMap<String, serde_json::Value>),
    SeerrJellyfin(SeerrJellyfin),
    SeerrServers(SeerrKind, BTreeMap<String, SeerrServer>),
    SeerrWebhook(SeerrWebhook),
    KoelRadioStations(Vec<KoelStation>),
    SuggestArrConfiguration(SuggestArrConfiguration),
    KavitaServerSettings(BTreeMap<String, serde_json::Value>),
    KavitaLibraries(BTreeMap<String, BTreeMap<String, serde_json::Value>>),
}

fn web_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// A non-empty station list: names neither empty, too long nor given twice,
/// URLs that Koel's `url` rule can take and that differ (Koel keeps a URL
/// unique per account), and a logo file by absolute path.
fn koel_stations(stations: &[KoelStation]) -> Result<(), String> {
    if stations.is_empty() {
        return Err("desired.stations names no station".to_string());
    }
    for (index, station) in stations.iter().enumerate() {
        let name = &station.name;
        if name.is_empty() {
            return Err(format!(
                "desired.stations[{index}]: a station name is empty"
            ));
        }
        if name.chars().count() > KOEL_NAME_MAX {
            return Err(format!(
                "desired.stations[{index}]: the name is longer than {KOEL_NAME_MAX} characters"
            ));
        }
        let at = format!("desired.stations {name}");
        if !web_url(&station.url) {
            return Err(format!("{at}: url must start with http:// or https://"));
        }
        if let Some(homepage) = &station.homepage_url {
            if !web_url(homepage) {
                return Err(format!(
                    "{at}: homepage_url must start with http:// or https:// (leave it out for none)"
                ));
            }
        }
        if let Some(file) = &station.logo_file {
            if !file.is_absolute() {
                return Err(format!("{at}: logo_file is an absolute path"));
            }
        }
        if let Some(earlier) = stations[..index].iter().find(|s| &s.name == name) {
            return Err(format!(
                "desired.stations names station {} twice",
                earlier.name
            ));
        }
        if let Some(earlier) = stations[..index].iter().find(|s| s.url == station.url) {
            return Err(format!(
                "desired.stations: the url of {} and {name} is the same",
                earlier.name
            ));
        }
    }
    Ok(())
}

/// A non-empty map of plain (undotted) field names, none of them `forbidden`.
fn plain_fields(
    map: &BTreeMap<String, serde_json::Value>,
    what: &str,
    forbidden: &[&str],
) -> Result<(), String> {
    if map.is_empty() {
        return Err(format!("{what} names no field"));
    }
    for name in map.keys() {
        if name.is_empty() || name.contains('.') {
            return Err(format!("{what}: {name:?} is not a plain field name"));
        }
        if forbidden.contains(&name.as_str()) {
            return Err(format!(
                "{what}: {name} is not set this way (these are not: {})",
                forbidden.join(", ")
            ));
        }
    }
    Ok(())
}

/// Paths of Kavita's server settings a spec may not name (design §25), with
/// the reason. A path that contains one of them, or is contained in one
/// (`oidcConfig` as a whole), is refused as well.
const KAVITA_NOT_SET_HERE: [(&str, &str); 15] = [
    ("oidcConfig.authority", "the host writes it into appsettings.json, Kavita copies it into the database at every start, and a change clears every account's OIDC link"),
    ("oidcConfig.clientId", "the host writes it into appsettings.json, and Kavita copies it into the database at every start"),
    ("oidcConfig.secret", "a secret; the host writes it into appsettings.json, and Kavita answers it masked"),
    ("oidcConfig.customScopes", "the host writes it into appsettings.json, and Kavita copies it into the database at every start"),
    ("oidcConfig.enabled", "Kavita derives it"),
    ("smtpConfig.password", "a secret, and a spec is no place for one"),
    ("port", "the host writes it into appsettings.json"),
    ("ipAddresses", "the host writes it into appsettings.json"),
    ("baseUrl", "the host writes it into appsettings.json"),
    ("cacheSize", "the host writes it into appsettings.json"),
    ("cacheDirectory", "Kavita ignores a change"),
    ("installId", "Kavita's own"),
    ("installVersion", "Kavita's own"),
    ("firstInstallDate", "Kavita's own"),
    ("firstInstallVersion", "Kavita's own"),
];

fn kavita_paths(map: &BTreeMap<String, serde_json::Value>) -> Result<(), String> {
    let within =
        |outer: &str, inner: &str| inner == outer || inner.starts_with(&format!("{outer}."));
    for path in map.keys() {
        for (refused, why) in KAVITA_NOT_SET_HERE {
            if within(refused, path) || within(path, refused) {
                return Err(format!(
                    "desired: {path} is not set this way ({refused}: {why})"
                ));
            }
        }
    }
    Ok(())
}

/// A name that stays inside `$CREDENTIALS_DIRECTORY`.
fn credential_name(name: &str, what: &str) -> Result<(), String> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') {
        Err(format!("{what}: {name:?} is not a credential name"))
    } else {
        Ok(())
    }
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

/// In a plugin's `set`, `{"$library_ids": ["Filme", "Serien"]}` anywhere in a
/// value stands for the ids of those libraries, looked up when the task
/// reads (design §16). Library ids exist only once the libraries do, so a
/// spec cannot carry them.
pub const LIBRARY_IDS: &str = "$library_ids";

/// Whether `value` holds an object key starting with `$` anywhere.
fn has_marker(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(map) => {
            map.keys().any(|k| k.starts_with('$')) || map.values().any(has_marker)
        }
        serde_json::Value::Array(items) => items.iter().any(has_marker),
        _ => false,
    }
}

/// Every marker in `value` well-formed: `$library_ids` alone in its object,
/// with a non-empty list of distinct, non-empty names. Any other key
/// starting with `$` is a misspelt marker, not a plugin field.
fn library_markers(value: &serde_json::Value) -> Result<(), String> {
    use serde_json::Value;
    match value {
        Value::Array(items) => items.iter().try_for_each(library_markers),
        Value::Object(map) => {
            let Some(names) = map.get(LIBRARY_IDS) else {
                if let Some(key) = map.keys().find(|k| k.starts_with('$')) {
                    return Err(format!("unknown marker {key}"));
                }
                return map.values().try_for_each(library_markers);
            };
            if map.len() != 1 {
                return Err(format!("{LIBRARY_IDS} stands alone in its object"));
            }
            let names: Option<Vec<&str>> = names.as_array().and_then(|list| {
                list.iter()
                    .map(|n| n.as_str().filter(|s| !s.is_empty()))
                    .collect()
            });
            let Some(names) = names else {
                return Err(format!("{LIBRARY_IDS} takes a list of library names"));
            };
            if names.is_empty() {
                return Err(format!("{LIBRARY_IDS} names no library"));
            }
            let mut seen = std::collections::BTreeSet::new();
            match names.into_iter().find(|n| !seen.insert(*n)) {
                Some(twice) => Err(format!("{LIBRARY_IDS} names {twice} twice")),
                None => Ok(()),
            }
        }
        _ => Ok(()),
    }
}

/// A well-formed keyed list: a dotted path, a key every item carries as a
/// string, no key twice, and at least one field besides the key.
fn list_items(path: &str, list: &ListItems) -> Result<(), String> {
    if crate::paths::segments(path).is_none() {
        return Err(format!("{path:?} is not a dotted path"));
    }
    if list.key.is_empty() {
        return Err(format!("{path}: the key is empty"));
    }
    if list.items.is_empty() {
        return Err(format!("{path} names no item"));
    }
    let mut seen = std::collections::BTreeSet::new();
    for (index, item) in list.items.iter().enumerate() {
        let Some(serde_json::Value::String(key)) = item.get(&list.key) else {
            return Err(format!(
                "{path}: item {index} has no key {} as a string",
                list.key
            ));
        };
        if !seen.insert(key.as_str()) {
            return Err(format!("{path}: {}={key} is named twice", list.key));
        }
        if item.len() < 2 {
            return Err(format!(
                "{path}: {}={key} names no field besides its key",
                list.key
            ));
        }
    }
    Ok(())
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
        if !raw.task.belongs_to(raw.service) {
            return Err(invalid(format!(
                "the task does not belong to service {:?}",
                raw.service.name()
            )));
        }
        let desired = match raw.task {
            TaskName::DownloadClients | TaskName::RootFolders | TaskName::ProwlarrInstances
                if raw.service == Service::Bindery =>
            {
                let (kind, wrapper) = match raw.task {
                    TaskName::DownloadClients => (BinderyKind::DownloadClients, "clients"),
                    TaskName::ProwlarrInstances => (BinderyKind::ProwlarrInstances, "instances"),
                    _ => (BinderyKind::RootFolders, "folders"),
                };
                let mut outer: BTreeMap<String, BTreeMap<String, BinderyEntry>> =
                    serde_json::from_value(raw.desired)
                        .map_err(|e| invalid(format!("desired: {e}")))?;
                if outer.len() != 1 || !outer.contains_key(wrapper) {
                    return Err(invalid(format!(
                        "desired must be an object with exactly the key {wrapper}"
                    )));
                }
                let entries = outer.remove(wrapper).unwrap_or_default();
                if entries.is_empty() {
                    return Err(invalid(format!("desired.{wrapper} names nothing")));
                }
                for (key, entry) in &entries {
                    let at = format!("desired.{wrapper}.{key}");
                    if key.is_empty() {
                        return Err(invalid(format!("desired.{wrapper}: a key is empty")));
                    }
                    if kind == BinderyKind::RootFolders {
                        if !key.starts_with('/') {
                            return Err(invalid(format!(
                                "{at}: a root folder is an absolute path"
                            )));
                        }
                        if !entry.set.is_empty() || !entry.secret_fields.is_empty() {
                            return Err(invalid(format!(
                                "{at}: bindery cannot update a root folder, so it takes no fields"
                            )));
                        }
                    }
                    for (field, credential) in &entry.secret_fields {
                        if !["apiKey", "password"].contains(&field.as_str()) {
                            return Err(invalid(format!(
                                "{at}.secret_fields: {field} is not one of bindery's write-only fields (apiKey, password)"
                            )));
                        }
                        credential_name(credential, &format!("{at}.secret_fields.{field}"))
                            .map_err(invalid)?;
                        if entry.set.contains_key(field) {
                            return Err(invalid(format!(
                                "{at}: {field} is both in set and in secret_fields"
                            )));
                        }
                    }
                    for own in ["id", "name", "path", "createdAt", "updatedAt", "health"] {
                        if entry.set.contains_key(own) {
                            return Err(invalid(format!("{at}.set: {own} is not set this way")));
                        }
                    }
                }
                Desired::BinderyEntries(kind, entries)
            }
            // bindery's switch over the indexers Prowlarr synced into it
            // (§23). A list of names, not a map of entries: the task sets one
            // field, and what it says about the rows it does **not** name is
            // fixed in the code, not in the spec.
            TaskName::Indexers if raw.service == Service::Bindery => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Raw {
                    enabled: Vec<String>,
                }
                let desired: Raw = serde_json::from_value(raw.desired)
                    .map_err(|e| invalid(format!("desired: {e}")))?;
                if desired.enabled.is_empty() {
                    return Err(invalid(
                        "desired.enabled names no indexer -- a task that only switches things off is not this one".to_string(),
                    ));
                }
                let mut seen = std::collections::BTreeSet::new();
                for name in &desired.enabled {
                    if name.is_empty() {
                        return Err(invalid("desired.enabled: a name is empty".to_string()));
                    }
                    if !seen.insert(name.as_str()) {
                        return Err(invalid(format!("desired.enabled names {name} twice")));
                    }
                }
                Desired::BinderyIndexers(desired.enabled)
            }
            // bindery's own login. The entries are keyed by the provider's
            // `id`, and the only write-only field here is `client_secret` —
            // NOT bindery's `apiKey`/`password`, which belong to the entries
            // above.
            TaskName::OidcProviders => {
                let mut outer: BTreeMap<String, BTreeMap<String, BinderyEntry>> =
                    serde_json::from_value(raw.desired)
                        .map_err(|e| invalid(format!("desired: {e}")))?;
                if outer.len() != 1 || !outer.contains_key("providers") {
                    return Err(invalid(
                        "desired must be an object with exactly the key providers".to_string(),
                    ));
                }
                let entries = outer.remove("providers").unwrap_or_default();
                if entries.is_empty() {
                    return Err(invalid("desired.providers names nothing".to_string()));
                }
                for (id, entry) in &entries {
                    let at = format!("desired.providers.{id}");
                    if id.is_empty() {
                        return Err(invalid("desired.providers: an id is empty".to_string()));
                    }
                    for (field, credential) in &entry.secret_fields {
                        if field != "client_secret" {
                            return Err(invalid(format!(
                                "{at}.secret_fields: {field} is not bindery's write-only OIDC field (client_secret)"
                            )));
                        }
                        credential_name(credential, &format!("{at}.secret_fields.{field}"))
                            .map_err(invalid)?;
                        if entry.set.contains_key(field) {
                            return Err(invalid(format!(
                                "{at}: {field} is both in set and in secret_fields"
                            )));
                        }
                    }
                    // The id addresses the entry, `status` is bindery's own.
                    for own in ["id", "status"] {
                        if entry.set.contains_key(own) {
                            return Err(invalid(format!("{at}.set: {own} is not set this way")));
                        }
                    }
                    if entry.set.is_empty() && entry.secret_fields.is_empty() {
                        return Err(invalid(format!("{at} names no field")));
                    }
                }
                Desired::BinderyOidcProviders(entries)
            }
            // Not reachable: `belongs_to` lets it through for bindery only,
            // and the guarded arm above takes it there.
            TaskName::ProwlarrInstances => {
                return Err(invalid("prowlarr-instances is a bindery task".to_string()))
            }
            TaskName::Settings => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Raw {
                    settings: BTreeMap<String, serde_json::Value>,
                }
                let desired: Raw = serde_json::from_value(raw.desired)
                    .map_err(|e| invalid(format!("desired: {e}")))?;
                if desired.settings.is_empty() {
                    return Err(invalid("desired.settings names no setting".to_string()));
                }
                for key in desired.settings.keys() {
                    if key.is_empty() || key.contains('/') || key.contains('?') {
                        return Err(invalid(format!(
                            "desired.settings: {key:?} is not a setting key"
                        )));
                    }
                }
                Desired::BinderySettings(desired.settings)
            }
            TaskName::Main => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Raw {
                    set: BTreeMap<String, serde_json::Value>,
                }
                let desired: Raw = serde_json::from_value(raw.desired)
                    .map_err(|e| invalid(format!("desired: {e}")))?;
                // The key is Seerr's own; the host sets it before the first start.
                plain_fields(&desired.set, "desired.set", &["apiKey"]).map_err(invalid)?;
                Desired::SeerrMain(desired.set)
            }
            TaskName::Jellyfin => {
                let desired: SeerrJellyfin = serde_json::from_value(raw.desired)
                    .map_err(|e| invalid(format!("desired: {e}")))?;
                // serverId and name are what the connection test fills in.
                plain_fields(
                    &desired.set,
                    "desired.set",
                    &["apiKey", "libraries", "serverId", "name"],
                )
                .map_err(invalid)?;
                credential_name(&desired.api_key_credential, "desired.api_key_credential")
                    .map_err(invalid)?;
                if desired.libraries.is_empty() {
                    return Err(invalid(
                        "desired.libraries names no library -- that would disable every one"
                            .to_string(),
                    ));
                }
                let mut seen = std::collections::BTreeSet::new();
                for name in &desired.libraries {
                    if name.is_empty() {
                        return Err(invalid("desired.libraries: a name is empty".to_string()));
                    }
                    if !seen.insert(name.as_str()) {
                        return Err(invalid(format!("desired.libraries names {name} twice")));
                    }
                }
                Desired::SeerrJellyfin(desired)
            }
            TaskName::RadarrServers | TaskName::SonarrServers => {
                let kind = match raw.task {
                    TaskName::RadarrServers => SeerrKind::Radarr,
                    _ => SeerrKind::Sonarr,
                };
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Raw {
                    servers: BTreeMap<String, SeerrServer>,
                }
                let desired: Raw = serde_json::from_value(raw.desired)
                    .map_err(|e| invalid(format!("desired: {e}")))?;
                if desired.servers.is_empty() {
                    return Err(invalid("desired.servers names no server".to_string()));
                }
                for (name, server) in &desired.servers {
                    if name.is_empty() {
                        return Err(invalid(
                            "desired.servers: a server name is empty".to_string(),
                        ));
                    }
                    let at = format!("desired.servers.{name}");
                    // The name is the key, the key comes from the credential, the
                    // id is Seerr's, and the three active* fields are resolved.
                    plain_fields(
                        &server.set,
                        &format!("{at}.set"),
                        &[
                            "id",
                            "name",
                            "apiKey",
                            "activeProfileId",
                            "activeProfileName",
                            "activeDirectory",
                        ],
                    )
                    .map_err(invalid)?;
                    for needed in ["hostname", "port"] {
                        if !server.set.contains_key(needed) {
                            return Err(invalid(format!(
                                "{at}.set needs {needed}: the connection test uses it"
                            )));
                        }
                    }
                    credential_name(
                        &server.api_key_credential,
                        &format!("{at}.api_key_credential"),
                    )
                    .map_err(invalid)?;
                    if server.profile.is_empty() {
                        return Err(invalid(format!("{at}.profile is empty")));
                    }
                    if !server.root_folder.starts_with('/') {
                        return Err(invalid(format!("{at}.root_folder is an absolute path")));
                    }
                }
                Desired::SeerrServers(kind, desired.servers)
            }
            TaskName::Webhook => {
                let desired: SeerrWebhook = serde_json::from_value(raw.desired)
                    .map_err(|e| invalid(format!("desired: {e}")))?;
                if !desired.set.is_empty() {
                    field_paths(&desired.set, "desired.set").map_err(invalid)?;
                }
                for own in ["options.jsonPayload", "options.customHeaders"] {
                    if desired.set.contains_key(own) {
                        return Err(invalid(format!(
                            "desired.set: {own} is not set this way (payload and headers are)"
                        )));
                    }
                }
                if desired.payload.is_empty() {
                    return Err(invalid("desired.payload is empty".to_string()));
                }
                for (key, credential) in &desired.headers {
                    if key.is_empty() {
                        return Err(invalid(
                            "desired.headers: a header name is empty".to_string(),
                        ));
                    }
                    credential_name(credential, &format!("desired.headers.{key}"))
                        .map_err(invalid)?;
                }
                Desired::SeerrWebhook(desired)
            }
            TaskName::Configuration => {
                let desired: SuggestArrConfiguration = serde_json::from_value(raw.desired)
                    .map_err(|e| invalid(format!("desired: {e}")))?;
                if desired.username.is_empty() {
                    return Err(invalid("desired.username is empty".to_string()));
                }
                if desired.set.is_empty() && desired.secrets.is_empty() {
                    return Err(invalid(
                        "desired names neither a field nor a secret".to_string(),
                    ));
                }
                let derived = desired.jellyfin_libraries.is_some();
                for (name, value) in &desired.set {
                    let at = "desired.set";
                    if name.is_empty() || name.contains('.') {
                        return Err(invalid(format!("{at}: {name:?} is not a plain field name")));
                    }
                    if name == "integrations" {
                        return Err(invalid(format!(
                            "{at}: integrations is the fetch endpoint's own key, not a setting"
                        )));
                    }
                    if crate::services::suggestarr::is_secret_field(name) {
                        return Err(invalid(format!(
                            "{at}: {name} holds a secret and belongs in desired.secrets"
                        )));
                    }
                    if derived && name == "JELLYFIN_LIBRARIES" {
                        return Err(invalid(format!(
                            "{at}: JELLYFIN_LIBRARIES is derived by jellyfin_libraries; two writers on one field is how a value ends up depending on who ran last"
                        )));
                    }
                    // SuggestArr drops every empty value when it writes the
                    // file (`save_env_vars`), so the next read answers with
                    // the default instead: converge would write, read back
                    // something else and never come to rest.
                    if value.is_null() || value.as_str() == Some("") {
                        return Err(invalid(format!(
                            "{at}.{name} is empty; SuggestArr stores no empty value, so it can never be read back"
                        )));
                    }
                }
                for (name, credential) in &desired.secrets {
                    if name.is_empty() || name.contains('.') {
                        return Err(invalid(format!(
                            "desired.secrets: {name:?} is not a plain field name"
                        )));
                    }
                    credential_name(credential, &format!("desired.secrets.{name}"))
                        .map_err(invalid)?;
                }
                if let Some(rule) = &desired.jellyfin_libraries {
                    if rule.exclude_collection_types.is_empty() {
                        return Err(invalid(
                            "desired.jellyfin_libraries.exclude_collection_types names no type; leave the key out to set the libraries by hand".to_string(),
                        ));
                    }
                    if rule.exclude_collection_types.iter().any(String::is_empty) {
                        return Err(invalid(
                            "desired.jellyfin_libraries.exclude_collection_types: a type is empty"
                                .to_string(),
                        ));
                    }
                }
                Desired::SuggestArrConfiguration(desired)
            }
            TaskName::RadioStations => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Raw {
                    stations: Vec<KoelStation>,
                }
                let desired: Raw = serde_json::from_value(raw.desired)
                    .map_err(|e| invalid(format!("desired: {e}")))?;
                koel_stations(&desired.stations).map_err(invalid)?;
                Desired::KoelRadioStations(desired.stations)
            }
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
            TaskName::ServerSettings => {
                let map: BTreeMap<String, serde_json::Value> = serde_json::from_value(raw.desired)
                    .map_err(|e| invalid(format!("desired: {e}")))?;
                field_paths(&map, "desired").map_err(invalid)?;
                kavita_paths(&map).map_err(invalid)?;
                Desired::KavitaServerSettings(map)
            }
            // Kavita's libraries by a folder they hold (design §26).
            TaskName::Libraries => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Raw {
                    libraries: BTreeMap<String, BTreeMap<String, serde_json::Value>>,
                }
                let desired: Raw = serde_json::from_value(raw.desired)
                    .map_err(|e| invalid(format!("desired: {e}")))?;
                if desired.libraries.is_empty() {
                    return Err(invalid("desired.libraries names no library".to_string()));
                }
                for (folder, fields) in &desired.libraries {
                    let at = format!("desired.libraries.{folder}");
                    if !folder.starts_with('/') || folder.len() < 2 {
                        return Err(invalid(format!(
                            "{at}: a library is named by an absolute folder"
                        )));
                    }
                    // The id and the folders say WHICH library is meant; the
                    // file types travel under two names and are left alone.
                    plain_fields(
                        fields,
                        &at,
                        &["id", "folders", "fileGroupTypes", "libraryFileTypes"],
                    )
                    .map_err(invalid)?;
                }
                Desired::KavitaLibraries(desired.libraries)
            }
            TaskName::ServerConfiguration => {
                let map: BTreeMap<String, serde_json::Value> = serde_json::from_value(raw.desired)
                    .map_err(|e| invalid(format!("desired: {e}")))?;
                field_paths(&map, "desired").map_err(invalid)?;
                Desired::ServerConfiguration(map)
            }
            TaskName::NamedConfiguration => {
                let settings: NamedConfigurationSettings = serde_json::from_value(raw.desired)
                    .map_err(|e| invalid(format!("desired: {e}")))?;
                field_paths(&settings.set, "desired.set").map_err(invalid)?;
                Desired::NamedConfiguration(settings)
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
                    if plugin.set.is_empty() && plugin.secrets.is_empty() && plugin.lists.is_empty()
                    {
                        return Err(invalid(format!("{at} names no field")));
                    }
                    if !plugin.set.is_empty() {
                        field_paths(&plugin.set, &format!("{at}.set")).map_err(invalid)?;
                    }
                    for (path, value) in &plugin.set {
                        library_markers(value)
                            .map_err(|e| invalid(format!("{at}.set.{path}: {e}")))?;
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
                    for (path, list) in &plugin.lists {
                        list_items(path, list).map_err(|e| invalid(format!("{at}.lists: {e}")))?;
                        if list.items.iter().any(|item| item.values().any(has_marker)) {
                            return Err(invalid(format!(
                                "{at}.lists.{path}: a marker works only in set"
                            )));
                        }
                        if plugin.set.contains_key(path) || plugin.secrets.contains_key(path) {
                            return Err(invalid(format!(
                                "{at}: {path} is both in set and in lists"
                            )));
                        }
                    }
                }
                Desired::PluginConfigurations(plugins)
            }
            TaskName::Connections => {
                let desired: TrailarrConnections = serde_json::from_value(raw.desired)
                    .map_err(|e| invalid(format!("desired: {e}")))?;
                if desired.connections.is_empty() {
                    return Err(invalid(
                        "desired.connections names no connection".to_string(),
                    ));
                }
                for (name, connection) in &desired.connections {
                    if name.is_empty() {
                        return Err(invalid(
                            "desired.connections: a connection name is empty".to_string(),
                        ));
                    }
                    let at = format!("desired.connections.{name}");
                    // The name identifies the connection and the key comes
                    // from its credential; id and added_at are the service's.
                    plain_fields(
                        &connection.set,
                        &format!("{at}.set"),
                        &["name", "api_key", "id", "added_at"],
                    )
                    .map_err(invalid)?;
                    credential_name(
                        &connection.api_key_credential,
                        &format!("{at}.api_key_credential"),
                    )
                    .map_err(invalid)?;
                }
                Desired::Connections(desired)
            }
            TaskName::TrailerProfiles => {
                let desired: TrailerProfileSettings = serde_json::from_value(raw.desired)
                    .map_err(|e| invalid(format!("desired: {e}")))?;
                plain_fields(
                    &desired.set,
                    "desired.set",
                    &["id", "customfilter", "customfilter_id"],
                )
                .map_err(invalid)?;
                // One setting travels as UpdateSetting, whose value is an
                // integer, a string or a boolean -- nothing else.
                for (name, value) in &desired.set {
                    let fits = value.is_boolean() || value.is_string() || value.is_i64();
                    if !fits {
                        return Err(invalid(format!(
                            "desired.set.{name}: a trailer profile setting is a string, a boolean or an integer"
                        )));
                    }
                }
                Desired::TrailerProfiles(desired)
            }
            TaskName::AccountSubscriptions => {
                let desired: AccountSubscriptions = serde_json::from_value(raw.desired)
                    .map_err(|e| invalid(format!("desired: {e}")))?;
                if !(desired.base_url.starts_with("http://")
                    || desired.base_url.starts_with("https://"))
                {
                    return Err(invalid(format!(
                        "desired.base_url must start with http:// or https://, got {:?}",
                        desired.base_url
                    )));
                }
                credential_name(&desired.topics_credential, "desired.topics_credential")
                    .map_err(invalid)?;
                Desired::AccountSubscriptions(desired)
            }
            TaskName::Naming
            | TaskName::MediaManagement
            | TaskName::DownloadClientConfig
            | TaskName::IndexerConfig
            | TaskName::DelayProfiles => {
                let map: BTreeMap<String, serde_json::Value> = serde_json::from_value(raw.desired)
                    .map_err(|e| invalid(format!("desired: {e}")))?;
                // The id addresses the document; it is the service's. `tags`
                // and `order` say WHICH delay profile is meant, and the task
                // answers that itself -- the one without tags.
                let own: &[&str] = match raw.task {
                    TaskName::DelayProfiles => &["id", "tags", "order"],
                    _ => &["id"],
                };
                plain_fields(&map, "desired", own).map_err(invalid)?;
                match raw.task {
                    TaskName::Naming => Desired::Naming(map),
                    TaskName::MediaManagement => Desired::MediaManagement(map),
                    TaskName::IndexerConfig => Desired::IndexerConfig(map),
                    TaskName::DelayProfiles => Desired::DelayProfiles(map),
                    _ => Desired::DownloadClientConfig(map),
                }
            }
            TaskName::RootFolders => {
                let desired: RootFolderSettings = serde_json::from_value(raw.desired)
                    .map_err(|e| invalid(format!("desired: {e}")))?;
                if desired.folders.is_empty() {
                    return Err(invalid("desired.folders names no folder".to_string()));
                }
                for (path, folder) in &desired.folders {
                    let at = format!("desired.folders.{path}");
                    if !path.starts_with('/') || path.len() < 2 || path.ends_with('/') {
                        return Err(invalid(format!(
                            "{at}: a root folder is an absolute path without a trailing slash"
                        )));
                    }
                    // Radarr's and Sonarr's root folders are a path and
                    // nothing the caller could set.
                    if raw.service != Service::Lidarr
                        && !(folder.set.is_empty() && folder.profiles.is_empty())
                    {
                        return Err(invalid(format!(
                            "{at}: a {} root folder has nothing to set but its path",
                            raw.service.name()
                        )));
                    }
                    if !folder.set.is_empty() {
                        plain_fields(&folder.set, &format!("{at}.set"), &ROOT_FOLDER_OWN)
                            .map_err(invalid)?;
                    }
                    for (field, name) in &folder.profiles {
                        if !PROFILE_FIELDS.contains(&field.as_str()) {
                            return Err(invalid(format!(
                                "{at}.profiles: {field} is not a profile field (these are: {})",
                                PROFILE_FIELDS.join(", ")
                            )));
                        }
                        if name.is_empty() {
                            return Err(invalid(format!(
                                "{at}.profiles.{field}: the name is empty"
                            )));
                        }
                        if folder.set.contains_key(field) {
                            return Err(invalid(format!(
                                "{at}: {field} is both in set and in profiles"
                            )));
                        }
                    }
                }
                Desired::RootFolders(desired)
            }
            TaskName::DownloadClients
            | TaskName::Notifications
            | TaskName::Applications
            | TaskName::Indexers
            | TaskName::IndexerProxies => {
                let desired: ProviderSettings = serde_json::from_value(raw.desired)
                    .map_err(|e| invalid(format!("desired: {e}")))?;
                if desired.providers.is_empty() {
                    return Err(invalid("desired.providers names no provider".to_string()));
                }
                for (name, provider) in &desired.providers {
                    if name.is_empty() {
                        return Err(invalid(
                            "desired.providers: a provider name is empty".to_string(),
                        ));
                    }
                    let at = format!("desired.providers.{name}");
                    if provider.implementation.is_empty() {
                        return Err(invalid(format!("{at}.implementation is empty")));
                    }
                    if provider.template.as_deref() == Some("") {
                        return Err(invalid(format!("{at}.template is empty")));
                    }
                    if let Some(tags) = &provider.tags {
                        for (i, label) in tags.iter().enumerate() {
                            // Servarr stores labels in lower case; another
                            // spelling would never match what it answers.
                            if label.is_empty()
                                || label.trim() != label
                                || label.to_lowercase() != *label
                            {
                                return Err(invalid(format!(
                                    "{at}.tags: {label:?} is not a lower-case label without surrounding spaces"
                                )));
                            }
                            if tags[..i].contains(label) {
                                return Err(invalid(format!("{at}.tags: {label} is named twice")));
                            }
                        }
                    }
                    if provider.set.is_empty()
                        && provider.fields.is_empty()
                        && provider.secret_fields.is_empty()
                        && provider.tags.is_none()
                    {
                        return Err(invalid(format!("{at} names no field")));
                    }
                    if !provider.set.is_empty() {
                        plain_fields(&provider.set, &format!("{at}.set"), &PROVIDER_OWN)
                            .map_err(invalid)?;
                    }
                    for field in provider.fields.keys() {
                        if field.is_empty() {
                            return Err(invalid(format!("{at}.fields: a field name is empty")));
                        }
                    }
                    for (field, credential) in &provider.secret_fields {
                        if field.is_empty() {
                            return Err(invalid(format!(
                                "{at}.secret_fields: a field name is empty"
                            )));
                        }
                        credential_name(credential, &format!("{at}.secret_fields.{field}"))
                            .map_err(invalid)?;
                        if provider.fields.contains_key(field) {
                            return Err(invalid(format!(
                                "{at}: {field} is both in fields and in secret_fields"
                            )));
                        }
                    }
                }
                match raw.task {
                    TaskName::DownloadClients => Desired::DownloadClients(desired),
                    TaskName::Notifications => Desired::Notifications(desired),
                    TaskName::Indexers => Desired::Indexers(desired),
                    TaskName::IndexerProxies => Desired::IndexerProxies(desired),
                    _ => Desired::Applications(desired),
                }
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
            Desired::NamedConfiguration(_) => "named-configuration",
            Desired::Connections(_) => "connections",
            Desired::TrailerProfiles(_) => "trailer-profiles",
            Desired::AccountSubscriptions(_) => "account-subscriptions",
            Desired::Naming(_) => "naming",
            Desired::MediaManagement(_) => "media-management",
            Desired::DownloadClientConfig(_) => "download-client-config",
            Desired::IndexerConfig(_) => "indexer-config",
            Desired::DelayProfiles(_) => "delay-profiles",
            Desired::RootFolders(_) => "root-folders",
            Desired::DownloadClients(_) => "download-clients",
            Desired::Notifications(_) => "notifications",
            Desired::Applications(_) => "applications",
            Desired::Indexers(_) => "indexers",
            Desired::IndexerProxies(_) => "indexer-proxies",
            Desired::BinderyEntries(BinderyKind::DownloadClients, _) => "download-clients",
            Desired::BinderyEntries(BinderyKind::ProwlarrInstances, _) => "prowlarr-instances",
            Desired::BinderyEntries(BinderyKind::RootFolders, _) => "root-folders",
            Desired::BinderySettings(_) => "settings",
            Desired::BinderyOidcProviders(_) => "oidc-providers",
            Desired::BinderyIndexers(_) => "indexers",
            Desired::SeerrMain(_) => "main",
            Desired::SeerrJellyfin(_) => "jellyfin",
            Desired::SeerrServers(SeerrKind::Radarr, _) => "radarr-servers",
            Desired::SeerrServers(SeerrKind::Sonarr, _) => "sonarr-servers",
            Desired::SeerrWebhook(_) => "webhook",
            Desired::KoelRadioStations(_) => "radio-stations",
            Desired::SuggestArrConfiguration(_) => "configuration",
            Desired::KavitaServerSettings(_) => "server-settings",
            Desired::KavitaLibraries(_) => "libraries",
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
    fn parses_named_configurations_by_key() {
        let spec = jellyfin(
            "named-configuration",
            r#"{"key":"network","set":{"KnownProxies":["10.0.20.11"]}}"#,
        )
        .unwrap();
        assert_eq!(spec.task_name(), "named-configuration");
        match &spec.desired {
            Desired::NamedConfiguration(s) => {
                assert_eq!(s.key, NamedKey::Network);
                assert_eq!(s.key.component(), "NetworkConfiguration");
                assert_eq!(s.key.path_segment(), "network");
            }
            other => panic!("unexpected {other:?}"),
        }
        let spec = jellyfin(
            "named-configuration",
            r#"{"key":"branding","set":{"LoginDisclaimer":"x"}}"#,
        )
        .unwrap();
        match &spec.desired {
            Desired::NamedConfiguration(s) => assert_eq!(s.key.component(), "BrandingOptionsDto"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn named_configuration_specs_are_strict() {
        let unknown = jellyfin("named-configuration", r#"{"key":"encoding","set":{"A":1}}"#);
        assert!(unknown.is_err(), "a key without a checked component");
        let empty = jellyfin("named-configuration", r#"{"key":"network","set":{}}"#);
        assert!(empty.is_err(), "empty set");
        let bad = jellyfin(
            "named-configuration",
            r#"{"key":"network","set":{"A..B":1}}"#,
        );
        assert!(bad.is_err(), "bad path");
        let other_service = parse(
            r#"{"service":"radarr","base_url":"http://x","api_key_credential":"k","task":"named-configuration","desired":{"key":"network","set":{"A":1}}}"#,
        );
        assert!(other_service.is_err(), "Jellyfin only");
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
    fn library_id_markers_are_strict() {
        let spec = |set: &str| {
            jellyfin(
                "plugin-configurations",
                &format!(
                    r#"{{"958aad6637844d2ab89aa7b6fab6e25c":{{"name":"LDAP-Auth","set":{set}}}}}"#
                ),
            )
        };
        let reason = |set: &str| spec(set).err().unwrap().to_string();
        assert!(spec(r#"{"EnabledFolders":{"$library_ids":["Filme","Serien"]}}"#).is_ok());
        assert!(spec(
            r#"{"A.FolderRoleMapping":[{"Role":"Medien","Folders":{"$library_ids":["Filme"]}}]}"#
        )
        .is_ok());
        assert!(reason(r#"{"EnabledFolders":{"$library_ids":[]}}"#)
            .contains("desired.958aad6637844d2ab89aa7b6fab6e25c.set.EnabledFolders: $library_ids names no library"));
        assert!(
            reason(r#"{"EnabledFolders":{"$library_ids":["Filme","Filme"]}}"#)
                .contains("$library_ids names Filme twice")
        );
        assert!(
            reason(r#"{"EnabledFolders":{"$library_ids":["Filme",""]}}"#)
                .contains("$library_ids takes a list of library names")
        );
        assert!(reason(r#"{"EnabledFolders":{"$library_ids":"Filme"}}"#)
            .contains("$library_ids takes a list of library names"));
        assert!(
            reason(r#"{"EnabledFolders":{"$library_ids":["Filme"],"Other":1}}"#)
                .contains("$library_ids stands alone in its object")
        );
        assert!(
            reason(r#"{"A":[{"Folders":{"$library_id":["Filme"]}}]}"#).contains(
                "desired.958aad6637844d2ab89aa7b6fab6e25c.set.A: unknown marker $library_id"
            )
        );
        let in_a_list = jellyfin(
            "plugin-configurations",
            r#"{"f5a34f7b2e8a4e6aa7223a216a81b374":{"name":"X","lists":{"L":{"key":"Name","items":[{"Name":"a","F":{"$library_ids":["Filme"]}}]}}}}"#,
        );
        assert!(in_a_list
            .err()
            .unwrap()
            .to_string()
            .contains("a marker works only in set"));
    }

    fn seerr(task: &str, desired: &str) -> Result<Spec, Error> {
        parse(&format!(
            r#"{{"service":"seerr","base_url":"http://localhost:5055","api_key_credential":"k","task":"{task}","desired":{desired}}}"#
        ))
    }

    #[test]
    fn seerr_specs_are_strict() {
        let reason = |r: Result<Spec, Error>| r.err().unwrap().to_string();
        assert_eq!(
            seerr("main", r#"{"set":{"locale":"de"}}"#)
                .unwrap()
                .task_name(),
            "main"
        );
        assert!(reason(seerr("main", r#"{"set":{"apiKey":"x"}}"#))
            .contains("apiKey is not set this way"));

        let jf = r#"{"set":{"ip":"10.0.30.10","port":8096},"api_key_credential":"jf","libraries":["Filme","Serien"]}"#;
        assert_eq!(seerr("jellyfin", jf).unwrap().task_name(), "jellyfin");
        assert!(reason(seerr(
            "jellyfin",
            &jf.replace(r#"["Filme","Serien"]"#, "[]")
        ))
        .contains("names no library"));
        assert!(
            reason(seerr("jellyfin", &jf.replace(r#""Serien""#, r#""Filme""#)))
                .contains("names Filme twice")
        );
        assert!(reason(seerr(
            "jellyfin",
            &jf.replace(r#""port":8096"#, r#""port":8096,"serverId":"x""#)
        ))
        .contains("serverId is not set this way"));

        let servers = r#"{"servers":{"Radarr":{"set":{"hostname":"10.0.10.10","port":7878},"api_key_credential":"rk","profile":"HD","root_folder":"/tank/movies"}}}"#;
        assert_eq!(
            seerr("radarr-servers", servers).unwrap().task_name(),
            "radarr-servers"
        );
        assert_eq!(
            seerr("sonarr-servers", servers).unwrap().task_name(),
            "sonarr-servers"
        );
        assert!(reason(seerr(
            "radarr-servers",
            &servers.replace(r#""port":7878"#, r#""port":7878,"activeProfileId":7"#)
        ))
        .contains("activeProfileId is not set this way"));
        assert!(reason(seerr(
            "radarr-servers",
            &servers.replace(r#","port":7878"#, "")
        ))
        .contains("set needs port"));
        assert!(reason(seerr(
            "radarr-servers",
            &servers.replace("/tank/movies", "movies")
        ))
        .contains("root_folder is an absolute path"));
        assert!(reason(seerr(
            "radarr-servers",
            &servers.replace(r#""profile":"HD""#, r#""profile":"" "#)
        ))
        .contains("profile is empty"));
        assert!(reason(seerr("radarr-servers", r#"{"servers":{}}"#)).contains("names no server"));

        let webhook = r#"{"set":{"enabled":true,"options.webhookUrl":"http://x/seerr"},"payload":{"a":"{{a}}"},"headers":{"X-Webhook-Token":"marke"}}"#;
        assert_eq!(seerr("webhook", webhook).unwrap().task_name(), "webhook");
        assert!(reason(seerr(
            "webhook",
            &webhook.replace(r#""enabled":true"#, r#""options.jsonPayload":"x""#)
        ))
        .contains("options.jsonPayload is not set this way"));
        assert!(
            reason(seerr("webhook", &webhook.replace(r#"{"a":"{{a}}"}"#, "{}")))
                .contains("payload is empty")
        );
        assert!(reason(seerr(
            "webhook",
            &webhook.replace(r#""marke""#, r#""../x""#)
        ))
        .contains("not a credential name"));
        // A Seerr task for another service, and vice versa.
        assert!(reason(seerr("quality-definitions", "{}")).contains("does not belong"));
        assert!(reason(jellyfin("main", r#"{"set":{"locale":"de"}}"#)).contains("does not belong"));
    }

    #[test]
    fn plugin_lists_are_strict() {
        let spec = |lists: &str| {
            jellyfin(
                "plugin-configurations",
                &format!(
                    r#"{{"f5a34f7b2e8a4e6aa7223a216a81b374":{{"name":"JavaScript Injector","lists":{lists}}}}}"#
                ),
            )
        };
        let good = r#"{"CustomJavaScripts":{"key":"Name","items":[{"Name":"A","Enabled":true}]}}"#;
        let parsed = spec(good).unwrap();
        let Desired::PluginConfigurations(plugins) = parsed.desired else {
            panic!("wrong task")
        };
        assert_eq!(
            plugins["f5a34f7b2e8a4e6aa7223a216a81b374"].lists["CustomJavaScripts"].key,
            "Name"
        );

        let no_key_in_item = r#"{"CustomJavaScripts":{"key":"Name","items":[{"Enabled":true}]}}"#;
        assert!(spec(no_key_in_item)
            .err()
            .unwrap()
            .to_string()
            .contains("has no key Name"));
        let twice = r#"{"CustomJavaScripts":{"key":"Name","items":[{"Name":"A","Enabled":true},{"Name":"A","Enabled":false}]}}"#;
        assert!(spec(twice).err().unwrap().to_string().contains("twice"));
        let only_key = r#"{"CustomJavaScripts":{"key":"Name","items":[{"Name":"A"}]}}"#;
        assert!(spec(only_key)
            .err()
            .unwrap()
            .to_string()
            .contains("no field besides"));
        let empty = r#"{"CustomJavaScripts":{"key":"Name","items":[]}}"#;
        assert!(spec(empty).is_err());
        let misspelt = r#"{"CustomJavaScripts":{"key":"Name","item":[{"Name":"A"}]}}"#;
        assert!(spec(misspelt).err().unwrap().to_string().contains("item"));
        let both = r#"{"f5a34f7b2e8a4e6aa7223a216a81b374":{"name":"X","set":{"CustomJavaScripts":[]},"lists":{"CustomJavaScripts":{"key":"Name","items":[{"Name":"A","Enabled":true}]}}}}"#;
        assert!(jellyfin("plugin-configurations", both)
            .err()
            .unwrap()
            .to_string()
            .contains("both in set and in lists"));
    }

    fn trailarr(task: &str, desired: &str) -> Result<Spec, Error> {
        parse(&format!(
            r#"{{"service":"trailarr","base_url":"http://localhost:7889","api_key_credential":"trailarr-api-key","task":"{task}","desired":{desired}}}"#
        ))
    }

    fn reason(result: Result<Spec, Error>) -> String {
        result.expect_err("an error").to_string()
    }

    const CONNECTIONS: &str = r#"{"connections":{"Radarr":{"set":{"arr_type":"radarr","url":"http://127.0.0.1:7878","monitor_new_media":true,"external_url":"","path_mappings":[]},"api_key_credential":"radarr-api-key"}}}"#;

    #[test]
    fn trailarr_connections_are_strict() {
        let spec = trailarr("connections", CONNECTIONS).unwrap();
        assert_eq!(spec.service.key_header(), "X-API-KEY");
        assert_eq!(spec.task_name(), "connections");
        let Desired::Connections(desired) = spec.desired else {
            panic!("wrong task")
        };
        assert_eq!(
            desired.connections["Radarr"].api_key_credential,
            "radarr-api-key"
        );

        let unknown =
            CONNECTIONS.replace(r#""api_key_credential""#, r#""key":1,"api_key_credential""#);
        assert!(reason(trailarr("connections", &unknown)).contains("key"));
        let misspelt = CONNECTIONS.replace("connections", "connection");
        assert!(reason(trailarr("connections", &misspelt)).contains("connection"));
        assert!(reason(trailarr("connections", r#"{"connections":{}}"#))
            .contains("names no connection"));
        let nameless = CONNECTIONS.replace(r#""Radarr""#, r#""""#);
        assert!(reason(trailarr("connections", &nameless)).contains("name is empty"));
        let no_set = r#"{"connections":{"Radarr":{"set":{},"api_key_credential":"k"}}}"#;
        assert!(reason(trailarr("connections", no_set)).contains("names no field"));
        for forbidden in ["name", "api_key", "id", "added_at"] {
            let text = CONNECTIONS.replace(r#""arr_type""#, &format!(r#""{forbidden}""#));
            assert!(
                reason(trailarr("connections", &text)).contains("is not set this way"),
                "{forbidden}"
            );
        }
        let dotted = CONNECTIONS.replace(r#""url""#, r#""a.url""#);
        assert!(reason(trailarr("connections", &dotted)).contains("not a plain field name"));
        let path_credential = CONNECTIONS.replace("radarr-api-key", "../radarr-api-key");
        assert!(
            reason(trailarr("connections", &path_credential)).contains("is not a credential name")
        );
        let no_credential = r#"{"connections":{"Radarr":{"set":{"url":"x"}}}}"#;
        assert!(reason(trailarr("connections", no_credential)).contains("api_key_credential"));
    }

    const PROFILE_SET: &str = r#"{"set":{"search_query":"{title} {year} deutscher trailer","always_search":true,"retry_count":2}}"#;

    #[test]
    fn trailer_profile_settings_are_strict() {
        let spec = trailarr("trailer-profiles", PROFILE_SET).unwrap();
        assert_eq!(spec.task_name(), "trailer-profiles");
        assert!(reason(trailarr("trailer-profiles", r#"{"set":{}}"#)).contains("names no field"));
        let unknown = PROFILE_SET.replace(r#"{"set""#, r#"{"profiles":[1],"set""#);
        assert!(reason(trailarr("trailer-profiles", &unknown)).contains("profiles"));
        for forbidden in ["id", "customfilter", "customfilter_id"] {
            let text = PROFILE_SET.replace("retry_count", forbidden);
            assert!(
                reason(trailarr("trailer-profiles", &text)).contains("is not set this way"),
                "{forbidden}"
            );
        }
        for value in ["null", "1.5", "[]", r#"{"a":1}"#] {
            let text = format!(r#"{{"set":{{"retry_count":{value}}}}}"#);
            assert!(
                reason(trailarr("trailer-profiles", &text))
                    .contains("a string, a boolean or an integer"),
                "{value}"
            );
        }
    }

    fn ntfy(desired: &str) -> Result<Spec, Error> {
        parse(&format!(
            r#"{{"service":"ntfy","base_url":"http://localhost:2586","api_key_credential":"ntfy-token","task":"account-subscriptions","desired":{desired}}}"#
        ))
    }

    #[test]
    fn account_subscriptions_are_strict() {
        let good =
            r#"{"base_url":"https://ntfy.rusty-vault.de","topics_credential":"ntfy-abo-topics"}"#;
        let spec = ntfy(good).unwrap();
        assert_eq!(spec.task_name(), "account-subscriptions");
        assert_eq!(spec.service.key_header(), "Authorization");
        let unknown = good.replace(
            r#""topics_credential""#,
            r#""topics":["x"],"topics_credential""#,
        );
        assert!(reason(ntfy(&unknown)).contains("topics"));
        assert!(reason(ntfy(r#"{"base_url":"https://x"}"#)).contains("topics_credential"));
        let bad_url = good.replace("https://ntfy.rusty-vault.de", "ntfy.rusty-vault.de");
        assert!(reason(ntfy(&bad_url)).contains("must start with http"));
        let bad_credential = good.replace("ntfy-abo-topics", "a/b");
        assert!(reason(ntfy(&bad_credential)).contains("is not a credential name"));
        assert!(reason(ntfy(r#"{}"#)).contains("missing field"));
    }

    fn kavita(desired: &str) -> Result<Spec, Error> {
        parse(&format!(
            r#"{{"service":"kavita","base_url":"http://127.0.0.1:5000","api_key_credential":"kavita-converge-key","task":"server-settings","desired":{desired}}}"#
        ))
    }

    #[test]
    fn kavita_takes_setting_paths_but_not_what_the_host_or_kavita_own() {
        let spec = kavita(r#"{"oidcConfig.autoLogin": true, "enableOpds": true}"#).unwrap();
        assert_eq!(spec.task_name(), "server-settings");
        assert_eq!(spec.service.key_header(), "X-Api-Key");
        for path in [
            "oidcConfig.authority",
            "oidcConfig.secret",
            "oidcConfig.customScopes",
            "oidcConfig",
            "smtpConfig",
            "smtpConfig.password",
            "port",
            "installId",
        ] {
            let err = reason(kavita(&format!(r#"{{"{path}": 1}}"#)));
            assert!(err.contains("is not set this way"), "{path}: {err}");
        }
        // A field next to a refused one is fine.
        assert!(kavita(r#"{"smtpConfig.host": "mail"}"#).is_ok());
        assert!(reason(kavita("{}")).contains("names no field"));
        let other = r#"{"service":"jellyfin","base_url":"http://x","api_key_credential":"k","task":"server-settings","desired":{"a":1}}"#;
        assert!(reason(parse(other)).contains("does not belong"));
    }

    #[test]
    fn kavita_libraries_are_named_by_folder_and_not_by_id() {
        let lib = |d: &str| {
            parse(&format!(
                r#"{{"service":"kavita","base_url":"http://127.0.0.1:5000","api_key_credential":"k","task":"libraries","desired":{d}}}"#
            ))
        };
        let spec =
            lib(r#"{"libraries":{"/tank/data/media/books":{"name":"Buecher","type":2}}}"#).unwrap();
        assert_eq!(spec.task_name(), "libraries");
        for field in ["id", "folders", "fileGroupTypes", "libraryFileTypes"] {
            let d = format!(r#"{{"libraries":{{"/b":{{"{field}":1}}}}}}"#);
            assert!(reason(lib(&d)).contains("is not set this way"), "{field}");
        }
        assert!(reason(lib(r#"{"libraries":{"books":{"type":2}}}"#)).contains("absolute folder"));
        assert!(reason(lib(r#"{"libraries":{}}"#)).contains("names no library"));
        assert!(reason(lib(r#"{"libraries":{"/b":{}}}"#)).contains("names no field"));
    }

    #[test]
    fn only_ntfy_wants_a_bearer_token() {
        let value = |service: Service| service.key_value(Secret::new("t0ken".into()));
        assert_eq!(value(Service::Ntfy).expose(), "Bearer t0ken");
        for service in [
            Service::Radarr,
            Service::Sonarr,
            Service::Jellyfin,
            Service::Trailarr,
            Service::Lidarr,
            Service::Prowlarr,
        ] {
            assert_eq!(value(service).expose(), "t0ken", "{service:?}");
        }
    }

    #[test]
    fn koel_wants_a_bearer_token_and_json() {
        let key = Service::Koel.key_value(Secret::new("t0ken".into()));
        assert_eq!(Service::Koel.key_header(), "Authorization");
        assert_eq!(key.expose(), "Bearer t0ken");
        assert!(Service::Koel.accepts_json_only());
        for service in [Service::Radarr, Service::Ntfy, Service::Seerr] {
            assert!(!service.accepts_json_only(), "{service:?}");
        }
    }

    fn suggestarr(desired: &str) -> Result<Spec, Error> {
        parse(&format!(
            r#"{{"service":"suggestarr","base_url":"http://localhost:5000","api_key_credential":"suggestarr-converge-passwort","task":"configuration","desired":{desired}}}"#
        ))
    }

    #[test]
    fn a_suggestarr_configuration_carries_fields_secrets_and_a_library_rule() {
        let spec = suggestarr(
            r#"{"username":"converge",
                "set":{"FILTER_RATING_SOURCE":"both"},
                "secrets":{"OMDB_API_KEY":"omdb-api-key"},
                "jellyfin_libraries":{"exclude_collection_types":["homevideos"]}}"#,
        )
        .unwrap();
        let Desired::SuggestArrConfiguration(desired) = &spec.desired else {
            panic!("wrong variant");
        };
        assert_eq!(desired.username, "converge");
        assert_eq!(spec.task_name(), "configuration");
        assert_eq!(
            desired.secrets.get("OMDB_API_KEY").map(String::as_str),
            Some("omdb-api-key")
        );
    }

    #[test]
    fn a_suggestarr_secret_does_not_belong_in_set() {
        let e = suggestarr(r#"{"username":"converge","set":{"OMDB_API_KEY":"abc"}}"#).unwrap_err();
        assert!(e.to_string().contains("desired.secrets"), "{e}");
    }

    #[test]
    fn an_empty_suggestarr_value_is_refused_because_it_cannot_be_read_back() {
        for value in ["\"\"", "null"] {
            let e = suggestarr(&format!(
                r#"{{"username":"converge","set":{{"TMDB_LANGUAGE":{value}}}}}"#
            ))
            .unwrap_err();
            assert!(e.to_string().contains("read back"), "{e}");
        }
    }

    #[test]
    fn suggestarr_libraries_have_one_writer_only() {
        let e = suggestarr(
            r#"{"username":"converge",
                "set":{"JELLYFIN_LIBRARIES":[{"id":"1","name":"Filme"}]},
                "jellyfin_libraries":{"exclude_collection_types":["homevideos"]}}"#,
        )
        .unwrap_err();
        assert!(e.to_string().contains("two writers"), "{e}");

        // Without the rule, naming them by hand is allowed.
        suggestarr(
            r#"{"username":"converge","set":{"JELLYFIN_LIBRARIES":[{"id":"1","name":"Filme"}]}}"#,
        )
        .unwrap();
    }

    #[test]
    fn a_suggestarr_configuration_needs_a_username_a_field_and_real_credentials() {
        let empty_user = suggestarr(r#"{"username":"","set":{"TMDB_LANGUAGE":"de"}}"#).unwrap_err();
        assert!(empty_user.to_string().contains("username"), "{empty_user}");

        let nothing = suggestarr(r#"{"username":"converge"}"#).unwrap_err();
        assert!(
            nothing.to_string().contains("neither a field nor a secret"),
            "{nothing}"
        );

        let bad_credential =
            suggestarr(r#"{"username":"converge","secrets":{"OMDB_API_KEY":"../x"}}"#).unwrap_err();
        assert!(
            bad_credential.to_string().contains("credential name"),
            "{bad_credential}"
        );

        let empty_rule = suggestarr(
            r#"{"username":"converge","set":{"TMDB_LANGUAGE":"de"},
                "jellyfin_libraries":{"exclude_collection_types":[]}}"#,
        )
        .unwrap_err();
        assert!(
            empty_rule.to_string().contains("names no type"),
            "{empty_rule}"
        );
    }

    #[test]
    fn the_configuration_task_belongs_to_suggestarr_alone() {
        let e = parse(
            r#"{"service":"koel","base_url":"http://localhost:8080","api_key_credential":"koel-token","task":"configuration","desired":{"username":"converge","set":{"TMDB_LANGUAGE":"de"}}}"#,
        )
        .unwrap_err();
        assert!(
            e.to_string()
                .contains("the task does not belong to service"),
            "{e}"
        );
    }

    fn koel(desired: &str) -> Result<Spec, Error> {
        parse(&format!(
            r#"{{"service":"koel","base_url":"http://localhost:8080","api_key_credential":"koel-token","task":"radio-stations","desired":{desired}}}"#
        ))
    }

    #[test]
    fn koel_radio_stations_are_strict() {
        let good = r#"{"stations":[
            {"name":"Radio Dreyeckland","url":"https://stream.rdl.de/rdl","description":"Freiburg",
             "is_public":true,"homepage_url":"https://rdl.de/","logo_file":"/nix/store/x-rdl.png"},
            {"name":"FSK","url":"https://streaming.fueralle.org/fsk.mp3","is_public":true}]}"#;
        let spec = koel(good).unwrap();
        assert_eq!(spec.task_name(), "radio-stations");
        let Desired::KoelRadioStations(stations) = &spec.desired else {
            panic!("{:?}", spec.desired)
        };
        assert_eq!(stations.len(), 2);
        assert_eq!(
            stations[1].description, "",
            "absent is empty, as Koel stores it"
        );
        assert_eq!(stations[1].homepage_url, None);
        assert_eq!(stations[1].logo_file, None);

        for (desired, needle) in [
            (r#"{"stations":[]}"#, "names no station"),
            (
                r#"{"stations":[{"name":"A","url":"https://a/x"}]}"#,
                "is_public",
            ),
            (
                r#"{"stations":[{"name":"A","url":"https://a/x","is_public":true,"logo":"x"}]}"#,
                "unknown field `logo`",
            ),
            (
                r#"{"stations":[{"name":"","url":"https://a/x","is_public":true}]}"#,
                "a station name is empty",
            ),
            (
                r#"{"stations":[{"name":"A","url":"https://a/x","is_public":true},{"name":"A","url":"https://b/x","is_public":true}]}"#,
                "names station A twice",
            ),
            (
                r#"{"stations":[{"name":"A","url":"https://a/x","is_public":true},{"name":"B","url":"https://a/x","is_public":true}]}"#,
                "the url of A and B is the same",
            ),
            (
                r#"{"stations":[{"name":"A","url":"a/x","is_public":true}]}"#,
                "A: url must start with http:// or https://",
            ),
            (
                r#"{"stations":[{"name":"A","url":"https://a/x","is_public":true,"homepage_url":""}]}"#,
                "A: homepage_url must start with http:// or https://",
            ),
            (
                r#"{"stations":[{"name":"A","url":"https://a/x","is_public":true,"logo_file":"rdl.png"}]}"#,
                "A: logo_file is an absolute path",
            ),
        ] {
            let err = reason(koel(desired));
            assert!(err.contains(needle), "{desired}: {err}");
        }
        let long = "x".repeat(192);
        let err = reason(koel(&format!(
            r#"{{"stations":[{{"name":"{long}","url":"https://a/x","is_public":true}}]}}"#
        )));
        assert!(err.contains("longer than 191 characters"), "{err}");
        let err = reason(parse(&format!(
            r#"{{"service":"seerr","base_url":"http://localhost:5055","api_key_credential":"k","task":"radio-stations","desired":{good}}}"#
        )));
        assert!(err.contains("does not belong to service"), "{err}");
    }

    fn servarr(service: &str, task: &str, desired: &str) -> Result<Spec, Error> {
        parse(&format!(
            r#"{{"service":"{service}","base_url":"http://localhost:8686","api_key_credential":"k","task":"{task}","desired":{desired}}}"#
        ))
    }

    #[test]
    fn naming_and_media_management_are_plain_fields_without_id() {
        for (service, task) in [
            ("radarr", "naming"),
            ("sonarr", "media-management"),
            ("lidarr", "naming"),
        ] {
            let spec = servarr(service, task, r#"{"renameMovies":true}"#).unwrap();
            assert_eq!(spec.task_name(), task);
            assert_eq!(spec.service.key_header(), "X-Api-Key");
        }
        assert!(reason(servarr("radarr", "naming", "{}")).contains("names no field"));
        assert!(reason(servarr("radarr", "naming", r#"{"id":2}"#)).contains("is not set this way"));
        assert!(
            reason(servarr("radarr", "naming", r#"{"a.b":1}"#)).contains("not a plain field name")
        );
        assert!(reason(servarr("jellyfin", "naming", r#"{"x":1}"#)).contains("does not belong"));
        assert!(reason(servarr(
            "trailarr",
            "root-folders",
            r#"{"folders":{"/a":{}}}"#
        ))
        .contains("does not belong"));
    }

    const LIDARR_FOLDERS: &str = r#"{"folders":{"/tank/data/media/music":{"set":{"name":"Musik","defaultMonitorOption":"all"},"profiles":{"defaultQualityProfileId":"Standard","defaultMetadataProfileId":"Standard"}}}}"#;

    #[test]
    fn root_folders_are_strict() {
        let spec = servarr("lidarr", "root-folders", LIDARR_FOLDERS).unwrap();
        let Desired::RootFolders(desired) = spec.desired else {
            panic!("wrong task")
        };
        assert_eq!(
            desired.folders["/tank/data/media/music"].profiles["defaultQualityProfileId"],
            "Standard"
        );
        let radarr = r#"{"folders":{"/tank/data/media/movies":{}}}"#;
        assert!(servarr("radarr", "root-folders", radarr).is_ok());
        let radarr_set = r#"{"folders":{"/tank/data/media/movies":{"set":{"name":"x"}}}}"#;
        assert!(reason(servarr("radarr", "root-folders", radarr_set))
            .contains("nothing to set but its path"));
        assert!(
            reason(servarr("lidarr", "root-folders", r#"{"folders":{}}"#))
                .contains("names no folder")
        );
        for bad in ["relative", "/", "/trailing/"] {
            let text = format!(r#"{{"folders":{{"{bad}":{{}}}}}}"#);
            assert!(
                reason(servarr("lidarr", "root-folders", &text)).contains("absolute path"),
                "{bad}"
            );
        }
        for own in ["path", "id", "freeSpace"] {
            let text = LIDARR_FOLDERS.replace(r#""name""#, &format!(r#""{own}""#));
            assert!(
                reason(servarr("lidarr", "root-folders", &text)).contains("is not set this way"),
                "{own}"
            );
        }
        let unknown_profile = LIDARR_FOLDERS.replace("defaultMetadataProfileId", "defaultTagId");
        assert!(reason(servarr("lidarr", "root-folders", &unknown_profile))
            .contains("not a profile field"));
        let both = LIDARR_FOLDERS.replace(r#""name":"Musik""#, r#""defaultQualityProfileId":3"#);
        assert!(reason(servarr("lidarr", "root-folders", &both))
            .contains("both in set and in profiles"));
        let empty_name = LIDARR_FOLDERS.replacen(r#":"Standard""#, r#":"""#, 1);
        assert!(
            reason(servarr("lidarr", "root-folders", &empty_name)).contains("the name is empty")
        );
        let misspelt = LIDARR_FOLDERS.replace(r#""profiles""#, r#""profile""#);
        assert!(reason(servarr("lidarr", "root-folders", &misspelt)).contains("profile"));
    }

    const QBITTORRENT: &str = r#"{"providers":{"qBittorrent":{"implementation":"QBittorrent","set":{"enable":true,"priority":1},"fields":{"host":"10.0.20.10","port":8080,"username":"admin","movieCategory":"radarr"},"secret_fields":{"password":"qbittorrent-password"}}}}"#;

    #[test]
    fn providers_are_strict() {
        let spec = servarr("radarr", "download-clients", QBITTORRENT).unwrap();
        assert_eq!(spec.task_name(), "download-clients");
        let Desired::DownloadClients(desired) = spec.desired else {
            panic!("wrong task")
        };
        let qb = &desired.providers["qBittorrent"];
        assert_eq!(qb.secret_fields["password"], "qbittorrent-password");
        assert_eq!(qb.fields["port"], 8080);

        assert!(servarr("prowlarr", "download-clients", QBITTORRENT).is_ok());
        assert!(servarr("lidarr", "notifications", QBITTORRENT).is_ok());
        assert!(servarr("prowlarr", "applications", QBITTORRENT).is_ok());
        assert!(reason(servarr("radarr", "applications", QBITTORRENT)).contains("does not belong"));
        assert!(reason(servarr("prowlarr", "naming", r#"{"x":1}"#)).contains("does not belong"));
        assert_eq!(
            servarr("prowlarr", "applications", QBITTORRENT)
                .unwrap()
                .service
                .key_header(),
            "X-Api-Key"
        );

        assert!(
            reason(servarr("radarr", "download-clients", r#"{"providers":{}}"#))
                .contains("names no provider")
        );
        let nameless = QBITTORRENT.replace(r#""qBittorrent""#, r#""""#);
        assert!(reason(servarr("radarr", "download-clients", &nameless)).contains("name is empty"));
        let no_impl = QBITTORRENT.replace(r#""QBittorrent""#, r#""""#);
        assert!(reason(servarr("radarr", "download-clients", &no_impl))
            .contains("implementation is empty"));
        let nothing = r#"{"providers":{"x":{"implementation":"Webhook"}}}"#;
        assert!(reason(servarr("radarr", "notifications", nothing)).contains("names no field"));
        for own in ["name", "id", "fields", "implementation", "tags"] {
            let text = QBITTORRENT.replace(r#""priority""#, &format!(r#""{own}""#));
            assert!(
                reason(servarr("radarr", "download-clients", &text))
                    .contains("is not set this way"),
                "{own}"
            );
        }
        let both = QBITTORRENT.replace(r#""username":"admin""#, r#""password":"x""#);
        assert!(reason(servarr("radarr", "download-clients", &both))
            .contains("both in fields and in secret_fields"));
        let path_credential = QBITTORRENT.replace("qbittorrent-password", "../x");
        assert!(
            reason(servarr("radarr", "download-clients", &path_credential))
                .contains("is not a credential name")
        );
        let misspelt = QBITTORRENT.replace("secret_fields", "secrets");
        assert!(reason(servarr("radarr", "download-clients", &misspelt)).contains("secrets"));
    }

    const TNTRACKER: &str = r#"{"providers":{"TNTracker":{"implementation":"Torznab","template":"Torrent Network","set":{"enable":true,"appProfileId":1},"fields":{"baseUrl":"http://tntracker.org"},"secret_fields":{"apiKey":"tntracker-apikey"},"tags":["umlautadaptarr"]}}}"#;

    #[test]
    fn indexers_and_proxies_belong_to_prowlarr_and_take_a_template_and_labels() {
        let spec = servarr("prowlarr", "indexers", TNTRACKER).unwrap();
        assert_eq!(spec.task_name(), "indexers");
        let Desired::Indexers(desired) = spec.desired else {
            panic!("wrong task")
        };
        let tnt = &desired.providers["TNTracker"];
        assert_eq!(tnt.template.as_deref(), Some("Torrent Network"));
        assert_eq!(
            tnt.tags.as_deref(),
            Some(&["umlautadaptarr".to_string()][..])
        );
        assert_eq!(
            servarr("prowlarr", "indexer-proxies", TNTRACKER)
                .unwrap()
                .task_name(),
            "indexer-proxies"
        );
        assert!(reason(servarr("radarr", "indexers", TNTRACKER)).contains("does not belong"));
        assert!(reason(servarr("lidarr", "indexer-proxies", TNTRACKER)).contains("does not belong"));

        // Only tags is a field of its own: an empty label list is one.
        let only_tags = r#"{"providers":{"x":{"implementation":"Http","tags":[]}}}"#;
        assert!(servarr("prowlarr", "indexer-proxies", only_tags).is_ok());
        for bad in ["UmlautAdaptarr", " vpn", ""] {
            let text = TNTRACKER.replace("umlautadaptarr", bad);
            assert!(
                reason(servarr("prowlarr", "indexers", &text))
                    .contains("is not a lower-case label"),
                "{bad:?}"
            );
        }
        let twice = TNTRACKER.replace(r#"["umlautadaptarr"]"#, r#"["vpn","vpn"]"#);
        assert!(reason(servarr("prowlarr", "indexers", &twice)).contains("vpn is named twice"));
        let empty_template = TNTRACKER.replace(r#""Torrent Network""#, r#""""#);
        assert!(
            reason(servarr("prowlarr", "indexers", &empty_template)).contains("template is empty")
        );
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
        let err = reason(trailarr(
            "account-subscriptions",
            r#"{"base_url":"https://x","topics_credential":"t"}"#,
        ));
        assert!(err.contains("does not belong to service"), "{err}");
        let err = reason(parse(&format!(
            r#"{{"service":"ntfy","base_url":"http://localhost:2586","api_key_credential":"k","task":"connections","desired":{CONNECTIONS}}}"#
        )));
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
