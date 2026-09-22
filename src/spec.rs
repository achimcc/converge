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
    Audiobookshelf,
    Dispatcharr,
    Authentik,
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
            Service::Audiobookshelf => "audiobookshelf",
            Service::Dispatcharr => "dispatcharr",
            Service::Authentik => "authentik",
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
            Service::Ntfy
            | Service::Koel
            | Service::SuggestArr
            | Service::Audiobookshelf
            | Service::Dispatcharr
            | Service::Authentik => "Authorization",
        }
    }

    /// The header's value. The credential holds the bare key; ntfy, Koel and
    /// authentik want it as a bearer token, and for SuggestArr and Dispatcharr
    /// the value is the JWT a login returned (the credential there holds a
    /// password, never a key).
    pub fn key_value(self, key: Secret) -> Secret {
        match self {
            Service::Ntfy
            | Service::Koel
            | Service::SuggestArr
            | Service::Audiobookshelf
            | Service::Dispatcharr
            | Service::Authentik => Secret::new(format!("Bearer {}", key.expose())),
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
    CustomFormats,
    AppProfiles,
    ServerConfiguration,
    LibraryOptions,
    ScheduledTaskTriggers,
    PluginConfigurations,
    NamedConfiguration,
    UserPolicies,
    DisplayPreferences,
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
    AuthSettings,
    AdminPermissions,
    StreamSettings,
    M3uAccounts,
    M3uGroups,
    EpgSources,
    ChannelEpg,
}

impl TaskName {
    fn belongs_to(self, service: Service) -> bool {
        match self {
            TaskName::QualityDefinitions | TaskName::QualityProfiles | TaskName::CustomFormats => {
                service.is_arr()
            }
            TaskName::AppProfiles => service == Service::Prowlarr,
            TaskName::ServerConfiguration
            | TaskName::LibraryOptions
            | TaskName::ScheduledTaskTriggers
            | TaskName::PluginConfigurations
            | TaskName::NamedConfiguration
            | TaskName::UserPolicies
            | TaskName::DisplayPreferences => service == Service::Jellyfin,
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
            TaskName::ProwlarrInstances | TaskName::OidcProviders => service == Service::Bindery,
            // `settings` means two different things, as `indexers` does
            // below: bindery's settings by key (§14), and authentik's tenant
            // settings by field (§37). The parse arm tells them apart.
            TaskName::Settings => service == Service::Bindery || service == Service::Authentik,
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
            TaskName::ServerSettings => service == Service::Kavita,
            // `libraries` means the same thing for both, and each service
            // parses its own fields below: libraries found by a folder they
            // hold (§26, §38).
            TaskName::Libraries => service == Service::Kavita || service == Service::Audiobookshelf,
            TaskName::AuthSettings | TaskName::AdminPermissions => {
                service == Service::Audiobookshelf
            }
            TaskName::StreamSettings
            | TaskName::M3uAccounts
            | TaskName::M3uGroups
            | TaskName::EpgSources
            | TaskName::ChannelEpg => service == Service::Dispatcharr,
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
    /// "This spec names the whole collection": the task removes every entry
    /// it does not name. Opt-in per spec, never the default, and only three
    /// tasks take it (design §39).
    #[serde(default)]
    exactly: bool,
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
    /// The profiles that survive, with `exactly` -- every other one is
    /// removed. `None` without `exactly`, and required with it: there is no
    /// sensible default for a list whose absence would empty the service
    /// (design §39).
    #[serde(default)]
    pub keep: Option<Vec<String>>,
    /// The score a named custom format must have in every profile -- in
    /// every **kept** profile where `keep` is set (design §41). Absent where
    /// the spec says nothing about scores; every `formatItems` entry it does
    /// not name stays as it is, because Recyclarr writes those.
    #[serde(default)]
    pub format_scores: Option<BTreeMap<String, i64>>,
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
    LiveTv,
}

impl NamedKey {
    /// The key as it goes into the path.
    pub fn path_segment(self) -> &'static str {
        match self {
            NamedKey::Network => "network",
            NamedKey::Branding => "branding",
            NamedKey::LiveTv => "livetv",
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
            NamedKey::LiveTv => "LiveTvOptions",
        }
    }
}

/// Jellyfin account policies (design §40). `all` holds what every account
/// must carry, `accounts` what single accounts carry on top of it, by the
/// `Name` of the account. Each key is a top-level field of `UserPolicy`; a
/// policy is written as a whole, so a field converge does not name travels
/// back as it was read.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserPolicySettings {
    #[serde(default)]
    pub all: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub accounts: BTreeMap<String, BTreeMap<String, serde_json::Value>>,
}

/// The `CustomPrefs` of one scope. Jellyfin stores them as string -> string,
/// so `"false"` is a value and `false` is a spec error.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomPrefs {
    #[serde(default)]
    pub custom_prefs: BTreeMap<String, String>,
}

/// One client's display preferences (design §40), for every account and for
/// single ones. `client` is the name the client stores them under (`emby`
/// for the web interface), and it goes into the query of every request.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayPreferencesSettings {
    pub client: String,
    #[serde(default)]
    pub all: CustomPrefs,
    #[serde(default)]
    pub accounts: BTreeMap<String, CustomPrefs>,
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
    /// Prowlarr's app profile by name, instead of `set.appProfileId`
    /// (design §41). Only an `indexers` spec may carry it, and never
    /// together with the id.
    #[serde(default)]
    pub app_profile: Option<String>,
    #[serde(default)]
    pub set: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub fields: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub secret_fields: BTreeMap<String, String>,
}

/// The custom formats a spec names, by name (design §41). Formats it does
/// not name are left alone: on the host Recyclarr writes some seventy of
/// them into the same collection.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomFormatSettings {
    pub formats: BTreeMap<String, CustomFormatEntry>,
}

/// One custom format: whether it goes into a file name, and the rules it
/// matches by. The specification list is complete -- one the spec does not
/// name is removed from that format.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomFormatEntry {
    pub include_custom_format_when_renaming: bool,
    pub specifications: Vec<CustomFormatSpecification>,
}

/// One rule of a format. `fields` are the implementation's own entries by
/// name (a `ReleaseTitleSpecification` has one, `value`); they have no
/// schema and are checked against the answer at runtime, as for providers.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomFormatSpecification {
    pub name: String,
    pub implementation: String,
    pub negate: bool,
    pub required: bool,
    #[serde(default)]
    pub fields: BTreeMap<String, serde_json::Value>,
}

/// Prowlarr's app profiles by name, their fields by name (design §41).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppProfileSettings {
    pub profiles: BTreeMap<String, BTreeMap<String, serde_json::Value>>,
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
    CustomFormats(CustomFormatSettings),
    ProwlarrAppProfiles(AppProfileSettings),
    ServerConfiguration(BTreeMap<String, serde_json::Value>),
    LibraryOptions(LibrarySettings),
    ScheduledTaskTriggers(TaskTriggers),
    PluginConfigurations(BTreeMap<String, PluginSettings>),
    NamedConfiguration(NamedConfigurationSettings),
    UserPolicies(UserPolicySettings),
    DisplayPreferences(DisplayPreferencesSettings),
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
    AudiobookshelfLibraries(BTreeMap<String, BTreeMap<String, serde_json::Value>>),
    AudiobookshelfAuthSettings(AudiobookshelfAuth),
    AudiobookshelfAdminPermissions(AbsPermissions),
    Dispatcharr(DispatcharrDesired),
    AuthentikSettings(BTreeMap<String, serde_json::Value>),
}

/// A Dispatcharr task (design §29). Every task logs in as `username`, a
/// service account whose password is `api_key_credential`; the account is
/// not a secret and belongs in the spec, so a wrong one shows in a plan.
#[derive(Debug, Clone, PartialEq)]
pub struct DispatcharrDesired {
    pub username: String,
    pub task: DispatcharrTask,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DispatcharrTask {
    /// The default stream profile, by name.
    StreamSettings { default_stream_profile: String },
    /// M3U accounts or EPG sources by name, their fields by name, and an
    /// account's secret fields by the credential that holds each (§30).
    Entries(
        crate::services::dispatcharr::EntryKind,
        BTreeMap<String, BTreeMap<String, serde_json::Value>>,
        BTreeMap<String, BTreeMap<String, String>>,
    ),
    /// account name -> group name -> fields.
    Groups(BTreeMap<String, BTreeMap<String, BTreeMap<String, serde_json::Value>>>),
    /// channel name -> what it shows: guide entry, name, logo (§34, §35).
    ChannelEpg(BTreeMap<String, crate::services::dispatcharr::ChannelLook>),
}

/// Fields a Dispatcharr spec may not name: the entry's identity, and the
/// credentials an M3U account or EPG source can carry -- a spec has no way to
/// keep them out of a plan's output.
const DISPATCHARR_FORBIDDEN: [&str; 4] = ["id", "name", "username", "password"];

/// Permissions every account of the named types must hold (design §28).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AbsPermissions {
    pub types: Vec<String>,
    pub permissions: BTreeMap<String, bool>,
}

/// Audiobookshelf's account types (`User.accountTypes`).
const ABS_ACCOUNT_TYPES: [&str; 4] = ["root", "admin", "user", "guest"];

/// Audiobookshelf's authentication settings (design §27): fields by name, and
/// the OIDC client secret from a credential, compared without being shown.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AudiobookshelfAuth {
    #[serde(default)]
    pub set: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub secret_fields: BTreeMap<String, String>,
}

/// The fields of Audiobookshelf's auth settings that hold a secret.
const ABS_SECRET_FIELDS: [&str; 1] = ["authOpenIDClientSecret"];

/// The fields `LibraryController.update` reads, minus `folders` -- a folder
/// is what names the library (design §38).
const ABS_LIBRARY_FIELDS: [&str; 6] = [
    "name",
    "mediaType",
    "icon",
    "provider",
    "displayOrder",
    "settings",
];

/// The media types Audiobookshelf has (`Library.mediaTypes`).
const ABS_MEDIA_TYPES: [&str; 2] = ["book", "podcast"];

/// One library of an Audiobookshelf `libraries` spec. `name` is required
/// only where the library has to be created, which the task decides against
/// the answer; here every field that is given must have the type
/// Audiobookshelf reads it as. A string that is not truthy it ignores, so
/// `""` would differ forever while it reported nothing to change.
fn abs_library(fields: &BTreeMap<String, serde_json::Value>, at: &str) -> Result<(), String> {
    if fields.is_empty() {
        return Err(format!("{at} names no field"));
    }
    for (field, value) in fields {
        if !ABS_LIBRARY_FIELDS.contains(&field.as_str()) {
            return Err(format!(
                "{at}: {field:?} is no field of a library ({})",
                ABS_LIBRARY_FIELDS.join(", ")
            ));
        }
        match field.as_str() {
            "displayOrder" => {
                if !value.is_number() {
                    return Err(format!("{at}.{field}: {value} is not a number"));
                }
            }
            "settings" => match value.as_object() {
                None => return Err(format!("{at}.{field}: this is not an object")),
                Some(map) if map.is_empty() => return Err(format!("{at}.{field} names no key")),
                Some(_) => {}
            },
            _ => {
                let Some(text) = value.as_str() else {
                    return Err(format!("{at}.{field}: {value} is not a string"));
                };
                if text.is_empty() {
                    return Err(format!(
                        "{at}.{field}: the value is empty, and Audiobookshelf ignores a string that is not truthy"
                    ));
                }
                if field == "mediaType" && !ABS_MEDIA_TYPES.contains(&text) {
                    return Err(format!(
                        "{at}.{field}: {text:?} is neither {}",
                        ABS_MEDIA_TYPES.join(" nor ")
                    ));
                }
            }
        }
    }
    Ok(())
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

/// The name of an account, as the service's answer spells it. Only its
/// emptiness can be judged here -- whether the service holds it is a question
/// for the answer, and the task asks it before it writes anything.
fn account_name(name: &str, what: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        Err(format!("{what}: an account has no name"))
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

/// The tasks `"exactly": true` is allowed on: the three whose collection a
/// spec can name in full, and whose shell predecessors on the host this was
/// written for did the deleting (design §39). Every other task refuses the
/// switch rather than accepting one that does nothing.
fn supports_exactly(desired: &Desired) -> bool {
    matches!(
        desired,
        Desired::Connections(_) | Desired::KoelRadioStations(_) | Desired::QualityProfiles(_)
    )
}

#[derive(Debug, Clone, PartialEq)]
pub struct Spec {
    pub path: PathBuf,
    pub service: Service,
    pub base_url: String,
    pub api_key_credential: String,
    /// The spec names the whole collection: what it leaves out is removed
    /// (design §39). False unless the spec says so.
    pub exactly: bool,
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
            // Authentik's tenant settings (design §37): fields of `Settings`
            // by name, never a path into `flags` or `footer_links`.
            TaskName::Settings if raw.service == Service::Authentik => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Raw {
                    set: BTreeMap<String, serde_json::Value>,
                }
                let desired: Raw = serde_json::from_value(raw.desired)
                    .map_err(|e| invalid(format!("desired: {e}")))?;
                plain_fields(&desired.set, "desired.set", &[]).map_err(invalid)?;
                Desired::AuthentikSettings(desired.set)
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
                // `keep` and `exactly` only make sense together: alone,
                // `keep` would name profiles nothing ever removes, and
                // `exactly` would have to guess which profiles survive.
                match (&policy.keep, raw.exactly) {
                    (Some(_), false) => {
                        return Err(invalid(
                            "desired.keep says which profiles survive and needs \"exactly\": true"
                                .to_string(),
                        ))
                    }
                    (None, true) => {
                        return Err(invalid(
                            "\"exactly\": true needs desired.keep -- the profiles that survive"
                                .to_string(),
                        ))
                    }
                    _ => {}
                }
                if let Some(keep) = &policy.keep {
                    if keep.is_empty() {
                        return Err(invalid(
                            "desired.keep names no profile, which would remove every one"
                                .to_string(),
                        ));
                    }
                    let mut seen = std::collections::BTreeSet::new();
                    if let Some(twice) = keep.iter().find(|n| !seen.insert(n.as_str())) {
                        return Err(invalid(format!("desired.keep names {twice:?} twice")));
                    }
                }
                if let Some(scores) = &policy.format_scores {
                    if scores.is_empty() {
                        return Err(invalid(
                            "desired.format_scores names no custom format".to_string(),
                        ));
                    }
                    if scores.keys().any(String::is_empty) {
                        return Err(invalid(
                            "desired.format_scores: a format name is empty".to_string(),
                        ));
                    }
                }
                Desired::QualityProfiles(policy)
            }
            TaskName::CustomFormats => {
                let desired: CustomFormatSettings = serde_json::from_value(raw.desired)
                    .map_err(|e| invalid(format!("desired: {e}")))?;
                if desired.formats.is_empty() {
                    return Err(invalid("desired.formats names no format".to_string()));
                }
                for (name, format) in &desired.formats {
                    if name.is_empty() {
                        return Err(invalid(
                            "desired.formats: a format name is empty".to_string(),
                        ));
                    }
                    let at = format!("desired.formats.{name}");
                    if format.specifications.is_empty() {
                        return Err(invalid(format!(
                            "{at}.specifications names no specification -- a format without one \
                             matches nothing"
                        )));
                    }
                    let mut seen = std::collections::BTreeSet::new();
                    for specification in &format.specifications {
                        if specification.name.is_empty() {
                            return Err(invalid(format!(
                                "{at}.specifications: a specification name is empty"
                            )));
                        }
                        // Found by name on both sides: two of a name would
                        // make "the second one" undecidable.
                        if !seen.insert(specification.name.as_str()) {
                            return Err(invalid(format!(
                                "{at}.specifications: {:?} is named twice",
                                specification.name
                            )));
                        }
                        if specification.implementation.is_empty() {
                            return Err(invalid(format!(
                                "{at}.specifications.{}: implementation is empty",
                                specification.name
                            )));
                        }
                        if specification.fields.keys().any(String::is_empty) {
                            return Err(invalid(format!(
                                "{at}.specifications.{}.fields: a field name is empty",
                                specification.name
                            )));
                        }
                    }
                }
                Desired::CustomFormats(desired)
            }
            TaskName::AppProfiles => {
                let desired: AppProfileSettings = serde_json::from_value(raw.desired)
                    .map_err(|e| invalid(format!("desired: {e}")))?;
                if desired.profiles.is_empty() {
                    return Err(invalid("desired.profiles names no profile".to_string()));
                }
                for (name, set) in &desired.profiles {
                    if name.is_empty() {
                        return Err(invalid(
                            "desired.profiles: a profile name is empty".to_string(),
                        ));
                    }
                    let at = format!("desired.profiles.{name}");
                    if set.is_empty() {
                        return Err(invalid(format!("{at} names no field")));
                    }
                    // `id` and `name` are not among them: the name is how a
                    // profile is found, the id is the service's.
                    for field in set.keys() {
                        if !crate::services::prowlarr::SETTABLE.contains(&field.as_str()) {
                            return Err(invalid(format!(
                                "{at}: {field} is not a field of an app profile (these are: {})",
                                crate::services::prowlarr::SETTABLE.join(", ")
                            )));
                        }
                    }
                }
                Desired::ProwlarrAppProfiles(desired)
            }
            TaskName::ServerSettings => {
                let map: BTreeMap<String, serde_json::Value> = serde_json::from_value(raw.desired)
                    .map_err(|e| invalid(format!("desired: {e}")))?;
                field_paths(&map, "desired").map_err(invalid)?;
                kavita_paths(&map).map_err(invalid)?;
                Desired::KavitaServerSettings(map)
            }
            TaskName::StreamSettings
            | TaskName::M3uAccounts
            | TaskName::M3uGroups
            | TaskName::EpgSources
            | TaskName::ChannelEpg => {
                Desired::Dispatcharr(dispatcharr_desired(raw.task, raw.desired).map_err(invalid)?)
            }
            TaskName::AdminPermissions => {
                let desired: AbsPermissions = serde_json::from_value(raw.desired)
                    .map_err(|e| invalid(format!("desired: {e}")))?;
                if desired.types.is_empty() {
                    return Err(invalid("desired.types names no account type".to_string()));
                }
                if let Some(bad) = desired
                    .types
                    .iter()
                    .find(|t| !ABS_ACCOUNT_TYPES.contains(&t.as_str()))
                {
                    return Err(invalid(format!(
                        "desired.types: {bad:?} is not an account type ({})",
                        ABS_ACCOUNT_TYPES.join(", ")
                    )));
                }
                if desired.permissions.is_empty() {
                    return Err(invalid(
                        "desired.permissions names no permission".to_string(),
                    ));
                }
                if let Some(bad) = desired
                    .permissions
                    .keys()
                    .find(|k| k.is_empty() || k.contains('.'))
                {
                    return Err(invalid(format!(
                        "desired.permissions: {bad:?} is not a permission name"
                    )));
                }
                Desired::AudiobookshelfAdminPermissions(desired)
            }
            TaskName::AuthSettings => {
                let desired: AudiobookshelfAuth = serde_json::from_value(raw.desired)
                    .map_err(|e| invalid(format!("desired: {e}")))?;
                if desired.set.is_empty() && desired.secret_fields.is_empty() {
                    return Err(invalid("desired names no field".to_string()));
                }
                if !desired.set.is_empty() {
                    // The sample is Audiobookshelf's own text; a secret has
                    // no place in a spec.
                    let mut forbidden = vec!["authOpenIDSamplePermissions"];
                    forbidden.extend(ABS_SECRET_FIELDS);
                    plain_fields(&desired.set, "desired.set", &forbidden).map_err(invalid)?;
                }
                for (field, credential) in &desired.secret_fields {
                    if !ABS_SECRET_FIELDS.contains(&field.as_str()) {
                        return Err(invalid(format!(
                            "desired.secret_fields: {field} is not one of Audiobookshelf's secret fields ({})",
                            ABS_SECRET_FIELDS.join(", ")
                        )));
                    }
                    credential_name(credential, &format!("desired.secret_fields.{field}"))
                        .map_err(invalid)?;
                }
                Desired::AudiobookshelfAuthSettings(desired)
            }
            // Libraries by a folder they hold: Kavita's (design §26) and
            // Audiobookshelf's (§38). Same shape, different fields.
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
                    if raw.service == Service::Audiobookshelf {
                        abs_library(fields, &at).map_err(invalid)?;
                    } else {
                        // The id and the folders say WHICH library is meant;
                        // the file types travel under two names and are left
                        // alone.
                        plain_fields(
                            fields,
                            &at,
                            &["id", "folders", "fileGroupTypes", "libraryFileTypes"],
                        )
                        .map_err(invalid)?;
                    }
                }
                if raw.service == Service::Audiobookshelf {
                    Desired::AudiobookshelfLibraries(desired.libraries)
                } else {
                    Desired::KavitaLibraries(desired.libraries)
                }
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
            TaskName::UserPolicies => {
                let settings: UserPolicySettings = serde_json::from_value(raw.desired)
                    .map_err(|e| invalid(format!("desired: {e}")))?;
                if settings.all.is_empty() && settings.accounts.is_empty() {
                    return Err(invalid("desired names no field".to_string()));
                }
                // Every key is a top-level field of `UserPolicy`: a policy
                // has no nested document a path could reach into.
                if !settings.all.is_empty() {
                    plain_fields(&settings.all, "desired.all", &[]).map_err(invalid)?;
                }
                for (name, set) in &settings.accounts {
                    account_name(name, "desired.accounts").map_err(invalid)?;
                    plain_fields(set, &format!("desired.accounts.{name}"), &[]).map_err(invalid)?;
                }
                Desired::UserPolicies(settings)
            }
            TaskName::DisplayPreferences => {
                let settings: DisplayPreferencesSettings = serde_json::from_value(raw.desired)
                    .map_err(|e| invalid(format!("desired: {e}")))?;
                // The client travels in the query of every request, so it
                // stays within what needs no escaping there.
                if settings.client.is_empty()
                    || !settings
                        .client
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
                {
                    return Err(invalid(format!(
                        "desired.client: {:?} is not a client name",
                        settings.client
                    )));
                }
                if settings.all.custom_prefs.is_empty() && settings.accounts.is_empty() {
                    return Err(invalid("desired names no custom pref".to_string()));
                }
                let mut scopes = vec![("desired.all".to_string(), &settings.all)];
                for (name, prefs) in &settings.accounts {
                    account_name(name, "desired.accounts").map_err(invalid)?;
                    scopes.push((format!("desired.accounts.{name}"), prefs));
                }
                for (at, prefs) in scopes {
                    if at != "desired.all" && prefs.custom_prefs.is_empty() {
                        return Err(invalid(format!("{at}.custom_prefs names no key")));
                    }
                    if prefs.custom_prefs.keys().any(String::is_empty) {
                        return Err(invalid(format!("{at}.custom_prefs: a key is empty")));
                    }
                }
                Desired::DisplayPreferences(settings)
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
                    if let Some(profile) = &provider.app_profile {
                        // Prowlarr's indexers are the only providers that
                        // carry an `appProfileId` at all (design §41).
                        if raw.service != Service::Prowlarr
                            || !matches!(raw.task, TaskName::Indexers)
                        {
                            return Err(invalid(format!(
                                "{at}.app_profile belongs to a prowlarr indexers spec"
                            )));
                        }
                        if profile.is_empty() {
                            return Err(invalid(format!("{at}.app_profile is empty")));
                        }
                        // Two ways of saying the same thing, and nothing to
                        // decide which one wins.
                        if provider.set.contains_key("appProfileId") {
                            return Err(invalid(format!(
                                "{at}: app_profile names the profile and appProfileId its id -- \
                                 name one of them"
                            )));
                        }
                    }
                    if provider.set.is_empty()
                        && provider.fields.is_empty()
                        && provider.secret_fields.is_empty()
                        && provider.tags.is_none()
                        && provider.app_profile.is_none()
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
        let spec = Spec {
            path: path.to_path_buf(),
            service: raw.service,
            base_url: raw.base_url,
            api_key_credential: raw.api_key_credential,
            exactly: raw.exactly,
            desired,
        };
        // Last, so the message can name the task the way the spec spells it.
        // A task that cannot remove says so instead of taking a switch that
        // would quietly do nothing.
        if spec.exactly && !supports_exactly(&spec.desired) {
            return Err(invalid(format!(
                "task {} does not support exactly",
                spec.task_name()
            )));
        }
        Ok(spec)
    }

    pub fn task_name(&self) -> &'static str {
        match self.desired {
            Desired::QualityDefinitions(_) => "quality-definitions",
            Desired::QualityProfiles(_) => "quality-profiles",
            Desired::CustomFormats(_) => "custom-formats",
            Desired::ProwlarrAppProfiles(_) => "app-profiles",
            Desired::ServerConfiguration(_) => "server-configuration",
            Desired::LibraryOptions(_) => "library-options",
            Desired::ScheduledTaskTriggers(_) => "scheduled-task-triggers",
            Desired::PluginConfigurations(_) => "plugin-configurations",
            Desired::NamedConfiguration(_) => "named-configuration",
            Desired::UserPolicies(_) => "user-policies",
            Desired::DisplayPreferences(_) => "display-preferences",
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
            Desired::KavitaLibraries(_) | Desired::AudiobookshelfLibraries(_) => "libraries",
            Desired::AudiobookshelfAuthSettings(_) => "auth-settings",
            Desired::AudiobookshelfAdminPermissions(_) => "admin-permissions",
            Desired::Dispatcharr(ref d) => match &d.task {
                DispatcharrTask::StreamSettings { .. } => "stream-settings",
                DispatcharrTask::Entries(
                    crate::services::dispatcharr::EntryKind::M3uAccount,
                    _,
                    _,
                ) => "m3u-accounts",
                DispatcharrTask::Entries(
                    crate::services::dispatcharr::EntryKind::EpgSource,
                    _,
                    _,
                ) => "epg-sources",
                DispatcharrTask::Groups(_) => "m3u-groups",
                DispatcharrTask::ChannelEpg(_) => "channel-epg",
            },
            Desired::AuthentikSettings(_) => "settings",
        }
    }
}

/// A Dispatcharr spec's `desired`, checked. The errors are the reason only;
/// the caller names the spec.
fn dispatcharr_desired(
    task: TaskName,
    desired: serde_json::Value,
) -> Result<DispatcharrDesired, String> {
    use crate::services::dispatcharr::{
        EntryKind, GROUP_CUSTOM_FIELDS, GROUP_FIELDS, GROUP_STREAM_PROFILE,
    };
    type Fields = BTreeMap<String, serde_json::Value>;

    fn fields_ok(at: &str, fields: &Fields, allowed: Option<&[&str]>) -> Result<(), String> {
        if fields.is_empty() {
            return Err(format!("{at} names no field"));
        }
        for name in fields.keys() {
            if name.is_empty() || name.contains('.') {
                return Err(format!("{at}: {name:?} is not a plain field name"));
            }
            if DISPATCHARR_FORBIDDEN.contains(&name.as_str()) {
                return Err(format!(
                    "{at}: {name} is not something a spec sets (the entry's identity, or a credential a plan would print)"
                ));
            }
            if let Some(allowed) = allowed {
                if !allowed.contains(&name.as_str()) {
                    return Err(format!(
                        "{at}: {name} is not a group setting ({})",
                        allowed.join(", ")
                    ));
                }
            }
        }
        Ok(())
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Stream {
        username: String,
        default_stream_profile: String,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Accounts {
        username: String,
        accounts: BTreeMap<String, Fields>,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Sources {
        username: String,
        sources: BTreeMap<String, Fields>,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Groups {
        username: String,
        accounts: BTreeMap<String, BTreeMap<String, Fields>>,
    }
    let parse_err = |e: serde_json::Error| format!("desired: {e}");
    let (username, task) = match task {
        TaskName::StreamSettings => {
            let d: Stream = serde_json::from_value(desired).map_err(parse_err)?;
            if d.default_stream_profile.is_empty() {
                return Err("desired.default_stream_profile is empty".to_string());
            }
            (
                d.username,
                DispatcharrTask::StreamSettings {
                    default_stream_profile: d.default_stream_profile,
                },
            )
        }
        TaskName::M3uAccounts | TaskName::EpgSources => {
            let (username, entries, kind, at) = if matches!(task, TaskName::M3uAccounts) {
                let d: Accounts = serde_json::from_value(desired).map_err(parse_err)?;
                (
                    d.username,
                    d.accounts,
                    EntryKind::M3uAccount,
                    "desired.accounts",
                )
            } else {
                let d: Sources = serde_json::from_value(desired).map_err(parse_err)?;
                (
                    d.username,
                    d.sources,
                    EntryKind::EpgSource,
                    "desired.sources",
                )
            };
            if entries.is_empty() {
                return Err(format!("{at} names no entry"));
            }
            let mut plain = BTreeMap::new();
            let mut secrets = BTreeMap::new();
            for (name, mut fields) in entries {
                if name.is_empty() {
                    return Err(format!("{at}: an entry name is empty"));
                }
                let here = format!("{at}.{name}");
                // `secret_fields` is not a Dispatcharr field: field -> the
                // credential holding its value (§30 accounts, §31 sources).
                if let Some(raw) = fields.remove("secret_fields") {
                    let allowed = kind.secret_fields();
                    let map: BTreeMap<String, String> = serde_json::from_value(raw)
                        .map_err(|e| format!("{here}.secret_fields: {e}"))?;
                    for (field, credential) in &map {
                        if !allowed.contains(&field.as_str()) {
                            return Err(format!(
                                "{here}.secret_fields: {field} is not a secret field ({})",
                                allowed.join(", ")
                            ));
                        }
                        if fields.contains_key(field) {
                            return Err(format!(
                                "{here}: {field} is both a field and in secret_fields"
                            ));
                        }
                        credential_name(credential, &format!("{here}.secret_fields.{field}"))?;
                    }
                    if !map.is_empty() {
                        secrets.insert(name.clone(), map);
                    }
                }
                fields_ok(&here, &fields, None)?;
                plain.insert(name, fields);
            }
            (username, DispatcharrTask::Entries(kind, plain, secrets))
        }
        TaskName::M3uGroups => {
            let d: Groups = serde_json::from_value(desired).map_err(parse_err)?;
            if d.accounts.is_empty() {
                return Err("desired.accounts names no account".to_string());
            }
            for (account, groups) in &d.accounts {
                if groups.is_empty() {
                    return Err(format!("desired.accounts.{account} names no group"));
                }
                let allowed: Vec<&str> = GROUP_FIELDS
                    .iter()
                    .copied()
                    .chain([GROUP_STREAM_PROFILE])
                    .chain(GROUP_CUSTOM_FIELDS)
                    .collect();
                for (group, fields) in groups {
                    let at = format!("desired.accounts.{account}.{group}");
                    fields_ok(&at, fields, Some(&allowed))?;
                    if let Some(profile) = fields.get(GROUP_STREAM_PROFILE) {
                        if profile.as_str().is_none_or(str::is_empty) {
                            return Err(format!(
                                "{at}.{GROUP_STREAM_PROFILE}: a stream profile is named, not numbered"
                            ));
                        }
                    }
                    for key in GROUP_CUSTOM_FIELDS {
                        if fields.get(key).is_some_and(|v| !v.is_string()) {
                            return Err(format!("{at}.{key}: a pattern is text"));
                        }
                    }
                }
            }
            (d.username, DispatcharrTask::Groups(d.accounts))
        }
        TaskName::ChannelEpg => {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Look {
                source: Option<String>,
                tvg_id: Option<String>,
                name: Option<String>,
                logo_url: Option<String>,
                fallback_streams: Option<Vec<String>>,
            }
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Channels {
                username: String,
                channels: BTreeMap<String, Look>,
            }
            let d: Channels = serde_json::from_value(desired).map_err(parse_err)?;
            if d.channels.is_empty() {
                return Err("desired.channels names no channel".to_string());
            }
            let mut channels = BTreeMap::new();
            for (name, l) in d.channels {
                let at = format!("desired.channels.{name}");
                if name.is_empty() {
                    return Err("desired.channels: a channel name is empty".to_string());
                }
                let epg = match (l.source, l.tvg_id) {
                    (Some(s), Some(t)) if !s.is_empty() && !t.is_empty() => {
                        Some(crate::services::dispatcharr::EpgTarget {
                            source: s,
                            tvg_id: t,
                        })
                    }
                    (None, None) => None,
                    _ => {
                        return Err(format!(
                            "{at}: source and tvg_id are named together, neither empty"
                        ))
                    }
                };
                if l.name.as_deref() == Some("") {
                    return Err(format!("{at}.name is empty"));
                }
                if let Some(url) = &l.logo_url {
                    if !(url.starts_with("http://") || url.starts_with("https://")) {
                        return Err(format!("{at}.logo_url must start with http:// or https://"));
                    }
                }
                if let Some(fallbacks) = &l.fallback_streams {
                    if fallbacks.is_empty() {
                        return Err(format!("{at}.fallback_streams names no stream"));
                    }
                    let mut seen = std::collections::BTreeSet::new();
                    for f in fallbacks {
                        if f.is_empty() {
                            return Err(format!("{at}.fallback_streams: a stream name is empty"));
                        }
                        if !seen.insert(f) {
                            return Err(format!("{at}.fallback_streams names {f} twice"));
                        }
                    }
                }
                if epg.is_none()
                    && l.name.is_none()
                    && l.logo_url.is_none()
                    && l.fallback_streams.is_none()
                {
                    return Err(format!("{at} names nothing to show"));
                }
                channels.insert(
                    name,
                    crate::services::dispatcharr::ChannelLook {
                        epg,
                        name: l.name,
                        logo_url: l.logo_url,
                        fallback_streams: l.fallback_streams.unwrap_or_default(),
                    },
                );
            }
            (d.username, DispatcharrTask::ChannelEpg(channels))
        }
        _ => unreachable!("only Dispatcharr's tasks come here"),
    };
    if username.is_empty() {
        return Err("desired.username is empty".to_string());
    }
    Ok(DispatcharrDesired { username, task })
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
                allow_in_every_profile: vec!["Unknown".to_string()],
                keep: None,
                format_scores: None,
            })
        );
    }

    // --- exactly (design §39) ---------------------------------------------

    const STATIONS: &str = r#"{"service":"koel","base_url":"http://localhost","api_key_credential":"koel-token","task":"radio-stations","desired":{"stations":[{"name":"RDL","url":"https://stream.rdl.de/rdl","description":"","is_public":true,"homepage_url":null}]}}"#;
    const CONNECTION_SPEC: &str = r#"{"service":"trailarr","base_url":"http://localhost:7889","api_key_credential":"trailarr-api-key","task":"connections","desired":{"connections":{"Radarr":{"set":{"arr_type":"radarr","url":"http://127.0.0.1:7878"},"api_key_credential":"radarr-api-key"}}}}"#;

    /// The same spec with `"exactly": <value>` next to `task`.
    fn with_exactly(spec: &str, value: &str) -> String {
        spec.replace(r#""desired""#, &format!(r#""exactly":{value},"desired""#))
    }

    #[test]
    fn exactly_defaults_to_false_and_may_be_written_out() {
        assert!(!parse(STATIONS).unwrap().exactly);
        assert!(!parse(&with_exactly(STATIONS, "false")).unwrap().exactly);
        assert!(parse(&with_exactly(STATIONS, "true")).unwrap().exactly);
        assert!(
            parse(&with_exactly(CONNECTION_SPEC, "true"))
                .unwrap()
                .exactly
        );
    }

    /// Only the three tasks that can remove take it; every other one says so
    /// instead of accepting a switch that would do nothing.
    #[test]
    fn exactly_on_a_task_that_cannot_delete_is_invalid() {
        let err = parse(&with_exactly(GOOD, "true"))
            .err()
            .unwrap()
            .to_string();
        assert!(
            err.contains("task quality-definitions does not support exactly"),
            "{err}"
        );
        let trailer_profiles = r#"{"service":"trailarr","base_url":"http://localhost:7889","api_key_credential":"k","task":"trailer-profiles","exactly":true,"desired":{"set":{"always_search":true}}}"#;
        let err = parse(trailer_profiles).err().unwrap().to_string();
        assert!(
            err.contains("task trailer-profiles does not support exactly"),
            "{err}"
        );
        // `false` is what every other task already means, and stays allowed.
        assert!(parse(&with_exactly(GOOD, "false")).is_ok());
    }

    /// `keep` says which profiles survive, so it makes sense only with
    /// `exactly` -- and with `exactly` there is no default for it.
    #[test]
    fn quality_profiles_keep_belongs_to_exactly_and_is_required_with_it() {
        let with_keep =
            PROFILES.replace(r#"["Unknown"]}"#, r#"["Unknown"],"keep":["Anime","HD"]}"#);
        let err = parse(&with_keep).err().unwrap().to_string();
        assert!(err.contains("desired.keep"), "{err}");
        assert!(err.contains("exactly"), "{err}");

        let err = parse(&with_exactly(PROFILES, "true"))
            .err()
            .unwrap()
            .to_string();
        assert!(err.contains("desired.keep"), "{err}");

        let spec = parse(&with_exactly(&with_keep, "true")).unwrap();
        assert!(spec.exactly);
        assert_eq!(
            spec.desired,
            Desired::QualityProfiles(ProfilePolicy {
                allow_in_every_profile: vec!["Unknown".to_string()],
                keep: Some(vec!["Anime".to_string(), "HD".to_string()]),
                format_scores: None,
            })
        );
    }

    #[test]
    fn an_empty_or_repeated_keep_is_invalid() {
        let empty = PROFILES.replace(r#"["Unknown"]}"#, r#"["Unknown"],"keep":[]}"#);
        assert!(parse(&with_exactly(&empty, "true")).is_err());
        let twice = PROFILES.replace(r#"["Unknown"]}"#, r#"["Unknown"],"keep":["HD","HD"]}"#);
        assert!(parse(&with_exactly(&twice, "true")).is_err());
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
    fn parses_the_live_tv_configuration() {
        let spec = jellyfin(
            "named-configuration",
            r#"{"key":"livetv","set":{"TunerHosts":[]}}"#,
        )
        .unwrap();
        match &spec.desired {
            Desired::NamedConfiguration(s) => {
                assert_eq!(s.key, NamedKey::LiveTv);
                assert_eq!(s.key.component(), "LiveTvOptions");
                assert_eq!(s.key.path_segment(), "livetv");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parses_the_account_policies_of_all_and_of_one_account() {
        let spec = jellyfin(
            "user-policies",
            r#"{"all":{"EnableAllFolders":false,"IsAdministrator":false},
                "accounts":{"konto1":{"IsAdministrator":true}}}"#,
        )
        .unwrap();
        assert_eq!(spec.task_name(), "user-policies");
        match &spec.desired {
            Desired::UserPolicies(s) => {
                assert_eq!(s.all["IsAdministrator"], serde_json::json!(false));
                assert_eq!(
                    s.accounts["konto1"]["IsAdministrator"],
                    serde_json::json!(true)
                );
            }
            other => panic!("unexpected {other:?}"),
        }
        // Either map alone is enough.
        assert!(jellyfin("user-policies", r#"{"all":{"IsHidden":true}}"#).is_ok());
        assert!(
            jellyfin("user-policies", r#"{"accounts":{"k":{"IsHidden":true}}}"#).is_ok(),
            "accounts alone"
        );
    }

    #[test]
    fn user_policy_specs_are_strict() {
        for (desired, why) in [
            (r#"{}"#, "neither map"),
            (r#"{"all":{},"accounts":{}}"#, "both maps empty"),
            (
                r#"{"accounts":{"konto1":{}}}"#,
                "an account without a field",
            ),
            (
                r#"{"all":{"Policy.IsHidden":true}}"#,
                "a path, not a top-level field",
            ),
            (r#"{"all":{"IsHidden":true},"extra":1}"#, "an unknown key"),
        ] {
            assert!(jellyfin("user-policies", desired).is_err(), "{why}");
        }
    }

    #[test]
    fn parses_the_display_preferences_of_a_client() {
        let spec = jellyfin(
            "display-preferences",
            r#"{"client":"emby",
                "all":{"custom_prefs":{"livetv-favoritechannelsattop":"false"}},
                "accounts":{"konto1":{"custom_prefs":{"useModularHome":"true"}}}}"#,
        )
        .unwrap();
        assert_eq!(spec.task_name(), "display-preferences");
        match &spec.desired {
            Desired::DisplayPreferences(s) => {
                assert_eq!(s.client, "emby");
                assert_eq!(s.all.custom_prefs["livetv-favoritechannelsattop"], "false");
                assert_eq!(s.accounts["konto1"].custom_prefs["useModularHome"], "true");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn display_preferences_specs_are_strict() {
        for (desired, why) in [
            (
                r#"{"client":"","all":{"custom_prefs":{"a":"b"}}}"#,
                "an empty client",
            ),
            (
                r#"{"client":"em by","all":{"custom_prefs":{"a":"b"}}}"#,
                "a client that would have to be escaped in the query",
            ),
            (r#"{"all":{"custom_prefs":{"a":"b"}}}"#, "no client at all"),
            (r#"{"client":"emby"}"#, "no key anywhere"),
            (
                r#"{"client":"emby","accounts":{"konto1":{"custom_prefs":{}}}}"#,
                "an account without a key",
            ),
            (
                r#"{"client":"emby","all":{"custom_prefs":{"a":false}}}"#,
                "a value that is not a string",
            ),
        ] {
            assert!(jellyfin("display-preferences", desired).is_err(), "{why}");
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
    fn audiobookshelf_libraries_are_named_by_a_folder_and_carry_only_known_fields() {
        let lib = |d: &str| {
            parse(&format!(
                r#"{{"service":"audiobookshelf","base_url":"http://10.0.90.10:8000","api_key_credential":"t","task":"libraries","desired":{d}}}"#
            ))
        };
        let spec = lib(
            r#"{"libraries":{"/tank/data/media/audiobooks":{"name":"Hoerbuecher","mediaType":"book","icon":"audiobook","provider":"google","displayOrder":1,"settings":{"markAsFinishedTimeRemaining":10}}}}"#,
        )
        .unwrap();
        assert_eq!(spec.task_name(), "libraries");
        assert_eq!(spec.service.key_header(), "Authorization");
        let Desired::AudiobookshelfLibraries(libraries) = &spec.desired else {
            panic!("not an Audiobookshelf library spec");
        };
        assert_eq!(libraries.len(), 1);

        // The folders are the key itself, and `update` takes nothing else.
        for field in ["id", "folders", "lastScan", "settings.icon"] {
            let d = format!(r#"{{"libraries":{{"/b":{{"{field}":1}}}}}}"#);
            assert!(
                reason(lib(&d)).contains("is no field of a library"),
                "{field}"
            );
        }
        // `update` ignores a string that is not truthy, so "" would differ
        // forever while Audiobookshelf reported nothing to change.
        assert!(reason(lib(r#"{"libraries":{"/b":{"name":""}}}"#)).contains("is empty"));
        assert!(reason(lib(r#"{"libraries":{"/b":{"icon":5}}}"#)).contains("a string"));
        assert!(reason(lib(r#"{"libraries":{"/b":{"mediaType":"film"}}}"#)).contains("book"));
        assert!(reason(lib(r#"{"libraries":{"/b":{"displayOrder":"1"}}}"#)).contains("a number"));
        assert!(reason(lib(r#"{"libraries":{"/b":{"settings":[]}}}"#)).contains("an object"));
        assert!(reason(lib(r#"{"libraries":{"/b":{"settings":{}}}}"#)).contains("names no key"));
        assert!(reason(lib(r#"{"libraries":{"books":{"name":"B"}}}"#)).contains("absolute folder"));
        assert!(reason(lib(r#"{"libraries":{}}"#)).contains("names no library"));
        assert!(reason(lib(r#"{"libraries":{"/b":{}}}"#)).contains("names no field"));
        // The task belongs to Kavita as well, and to nobody else.
        let koel = r#"{"service":"koel","base_url":"http://x","api_key_credential":"k","task":"libraries","desired":{"libraries":{"/b":{"name":"B"}}}}"#;
        assert!(reason(parse(koel)).contains("does not belong"));
    }

    #[test]
    fn audiobookshelf_takes_fields_and_its_client_secret_only_from_a_credential() {
        let abs = |d: &str| {
            parse(&format!(
                r#"{{"service":"audiobookshelf","base_url":"http://10.0.90.10:8000","api_key_credential":"t","task":"auth-settings","desired":{d}}}"#
            ))
        };
        let spec = abs(r#"{"set":{"authOpenIDAutoLaunch":true},"secret_fields":{"authOpenIDClientSecret":"abs-oidc"}}"#).unwrap();
        assert_eq!(spec.task_name(), "auth-settings");
        assert_eq!(spec.service.key_header(), "Authorization");
        assert_eq!(
            spec.service.key_value(Secret::new("t0ken".into())).expose(),
            "Bearer t0ken"
        );
        assert!(reason(abs(r#"{"set":{"authOpenIDClientSecret":"x"}}"#))
            .contains("is not set this way"));
        assert!(
            reason(abs(r#"{"set":{"authOpenIDSamplePermissions":"x"}}"#))
                .contains("is not set this way")
        );
        assert!(
            reason(abs(r#"{"secret_fields":{"authOpenIDClientID":"c"}}"#)).contains("not one of")
        );
        assert!(
            reason(abs(r#"{"secret_fields":{"authOpenIDClientSecret":"a/b"}}"#))
                .contains("credential name")
        );
        assert!(reason(abs("{}")).contains("names no field"));
    }

    #[test]
    fn audiobookshelf_permissions_name_account_types_and_booleans() {
        let abs = |d: &str| {
            parse(&format!(
                r#"{{"service":"audiobookshelf","base_url":"http://10.0.90.10:8000","api_key_credential":"t","task":"admin-permissions","desired":{d}}}"#
            ))
        };
        let spec = abs(r#"{"types":["admin","root"],"permissions":{"delete":true}}"#).unwrap();
        assert_eq!(spec.task_name(), "admin-permissions");
        assert!(
            reason(abs(r#"{"types":["owner"],"permissions":{"delete":true}}"#))
                .contains("not an account type")
        );
        assert!(reason(abs(r#"{"types":[],"permissions":{"delete":true}}"#))
            .contains("no account type"));
        assert!(reason(abs(r#"{"types":["admin"],"permissions":{}}"#)).contains("no permission"));
        assert!(
            reason(abs(r#"{"types":["admin"],"permissions":{"delete":"yes"}}"#))
                .contains("desired")
        );
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

    fn dispatcharr(task: &str, desired: &str) -> Result<Spec, Error> {
        parse(&format!(
            r#"{{"service":"dispatcharr","base_url":"http://localhost:9191","api_key_credential":"dispatcharr-converge-passwort","task":"{task}","desired":{desired}}}"#
        ))
    }

    #[test]
    fn dispatcharr_tasks_parse_with_their_account() {
        let spec = dispatcharr(
            "stream-settings",
            r#"{"username":"converge","default_stream_profile":"streamlink"}"#,
        )
        .unwrap();
        assert_eq!(spec.task_name(), "stream-settings");
        let spec = dispatcharr(
            "m3u-accounts",
            r#"{"username":"converge","accounts":{"A":{"file_path":"/m3u/a.m3u"}}}"#,
        )
        .unwrap();
        assert_eq!(spec.task_name(), "m3u-accounts");
        let spec = dispatcharr(
            "epg-sources",
            r#"{"username":"converge","sources":{"E":{"url":"https://x/e.xml"}}}"#,
        )
        .unwrap();
        assert_eq!(spec.task_name(), "epg-sources");
        let spec = dispatcharr(
            "m3u-groups",
            r#"{"username":"converge","accounts":{"A":{"G":{"auto_channel_sync":true}}}}"#,
        )
        .unwrap();
        assert_eq!(spec.task_name(), "m3u-groups");
        let Desired::Dispatcharr(d) = &spec.desired else {
            panic!("not a Dispatcharr spec")
        };
        assert_eq!(d.username, "converge");
    }

    #[test]
    fn dispatcharr_specs_are_strict() {
        let no_user = dispatcharr(
            "stream-settings",
            r#"{"username":"","default_stream_profile":"streamlink"}"#,
        );
        assert!(no_user.is_err(), "empty username");
        let password = dispatcharr(
            "m3u-accounts",
            r#"{"username":"c","accounts":{"A":{"password":"x"}}}"#,
        )
        .unwrap_err()
        .to_string();
        assert!(password.contains("credential"), "{password}");
        let group = dispatcharr(
            "m3u-groups",
            r#"{"username":"c","accounts":{"A":{"G":{"stream_count":1}}}}"#,
        )
        .unwrap_err()
        .to_string();
        assert!(group.contains("not a group setting"), "{group}");
        let profile = dispatcharr(
            "m3u-groups",
            r#"{"username":"c","accounts":{"A":{"G":{"stream_profile":"Proxy"}}}}"#,
        );
        assert!(profile.is_ok(), "stream_profile by name");
        let filter = dispatcharr(
            "m3u-groups",
            r#"{"username":"c","accounts":{"A":{"G":{"name_match_regex":" 4K$","name_regex_pattern":"^X: ","name_replace_pattern":"","name_match_exclude_regex":"MOBIL"}}}}"#,
        );
        assert!(filter.is_ok(), "name filters: {filter:?}");
        let not_text = dispatcharr(
            "m3u-groups",
            r#"{"username":"c","accounts":{"A":{"G":{"name_match_regex":1}}}}"#,
        )
        .unwrap_err()
        .to_string();
        assert!(not_text.contains("name_match_regex"), "{not_text}");
        let unnamed = dispatcharr(
            "m3u-groups",
            r#"{"username":"c","accounts":{"A":{"G":{"stream_profile":3}}}}"#,
        )
        .unwrap_err()
        .to_string();
        assert!(unnamed.contains("stream_profile"), "{unnamed}");
        let empty = dispatcharr("epg-sources", r#"{"username":"c","sources":{}}"#);
        assert!(empty.is_err(), "no source");
        let other = parse(
            r#"{"service":"jellyfin","base_url":"http://x","api_key_credential":"k","task":"m3u-accounts","desired":{"username":"c","accounts":{"A":{"file_path":"/a"}}}}"#,
        );
        assert!(other.is_err(), "Dispatcharr only");
    }

    #[test]
    fn an_xtream_account_takes_its_secrets_from_credentials() {
        let spec = dispatcharr(
            "m3u-accounts",
            r#"{"username":"c","accounts":{"X":{"account_type":"XC","secret_fields":{"server_url":"xt-url","username":"xt-user","password":"xt-pass"}}}}"#,
        )
        .unwrap();
        let Desired::Dispatcharr(d) = &spec.desired else {
            panic!("not a Dispatcharr spec")
        };
        let DispatcharrTask::Entries(_, entries, secrets) = &d.task else {
            panic!("not an entries task")
        };
        // The credential names are not fields Dispatcharr would be sent.
        assert_eq!(entries["X"].keys().collect::<Vec<_>>(), ["account_type"]);
        assert_eq!(secrets["X"]["password"], "xt-pass");
        assert_eq!(secrets["X"]["server_url"], "xt-url");
        assert_eq!(secrets["X"].len(), 3);
    }

    #[test]
    fn an_epg_source_takes_its_url_from_a_credential() {
        let spec = dispatcharr(
            "epg-sources",
            r#"{"username":"c","sources":{"Anbieter":{"source_type":"xmltv","secret_fields":{"url":"xt-epg"}}}}"#,
        )
        .unwrap();
        let Desired::Dispatcharr(d) = &spec.desired else {
            panic!("not a Dispatcharr spec")
        };
        let DispatcharrTask::Entries(_, entries, secrets) = &d.task else {
            panic!("not an entries task")
        };
        assert_eq!(
            entries["Anbieter"].keys().collect::<Vec<_>>(),
            ["source_type"]
        );
        assert_eq!(secrets["Anbieter"]["url"], "xt-epg");
    }

    #[test]
    fn channel_epg_names_a_source_and_a_tvg_id_per_channel() {
        let spec = dispatcharr(
            "channel-epg",
            r#"{"username":"c","channels":{"SKY SPORT NEWS":{"source":"epgshare01-de","tvg_id":"Sky.Sport.News.de"}}}"#,
        )
        .unwrap();
        assert_eq!(spec.task_name(), "channel-epg");
        let look = dispatcharr(
            "channel-epg",
            r#"{"username":"c","channels":{"SKY SPORT NEWS":{"name":"Sky Sport News","logo_url":"https://l.example/n.png"}}}"#,
        );
        assert!(look.is_ok(), "name and logo alone: {look:?}");
        let fallback = dispatcharr(
            "channel-epg",
            r#"{"username":"c","channels":{"SKY SPORT GOLF":{"fallback_streams":["SKYGO: SKY SPORT GOLF HD"]}}}"#,
        );
        assert!(fallback.is_ok(), "fallback streams alone: {fallback:?}");
        for bad in [
            r#"{"username":"c","channels":{"X":{"fallback_streams":[]}}}"#,
            r#"{"username":"c","channels":{"X":{"fallback_streams":[""]}}}"#,
            r#"{"username":"c","channels":{"X":{"fallback_streams":["a","a"]}}}"#,
            r#"{"username":"c","channels":{}}"#,
            r#"{"username":"c","channels":{"X":{"source":"","tvg_id":"a"}}}"#,
            r#"{"username":"c","channels":{"X":{"source":"s","tvg_id":""}}}"#,
            r#"{"username":"c","channels":{"X":{"source":"s","tvg_id":"a","epg_data_id":3}}}"#,
            r#"{"username":"c","channels":{"X":{"source":"s"}}}"#,
            r#"{"username":"c","channels":{"X":{}}}"#,
            r#"{"username":"c","channels":{"X":{"logo_url":"file:///x.png"}}}"#,
        ] {
            assert!(dispatcharr("channel-epg", bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn secret_fields_are_checked() {
        let refused =
            |task: &str, desired: &str| dispatcharr(task, desired).unwrap_err().to_string();
        let source = refused(
            "epg-sources",
            r#"{"username":"c","sources":{"E":{"source_type":"xmltv","secret_fields":{"username":"k"}}}}"#,
        );
        assert!(source.contains("not a secret field (url)"), "{source}");
        let account_url = refused(
            "m3u-accounts",
            r#"{"username":"c","accounts":{"X":{"account_type":"XC","secret_fields":{"url":"k"}}}}"#,
        );
        assert!(
            account_url.contains("server_url, username, password"),
            "{account_url}"
        );
        let url_twice = refused(
            "epg-sources",
            r#"{"username":"c","sources":{"E":{"url":"https://x/e.xml","secret_fields":{"url":"k"}}}}"#,
        );
        assert!(url_twice.contains("both"), "{url_twice}");
        let unknown = refused(
            "m3u-accounts",
            r#"{"username":"c","accounts":{"X":{"account_type":"XC","secret_fields":{"max_streams":"k"}}}}"#,
        );
        assert!(
            unknown.contains("server_url, username, password"),
            "{unknown}"
        );
        let twice = refused(
            "m3u-accounts",
            r#"{"username":"c","accounts":{"X":{"server_url":"http://a","secret_fields":{"server_url":"k"}}}}"#,
        );
        assert!(twice.contains("both"), "{twice}");
        let path = refused(
            "m3u-accounts",
            r#"{"username":"c","accounts":{"X":{"account_type":"XC","secret_fields":{"password":"../k"}}}}"#,
        );
        assert!(path.contains("secret_fields.password"), "{path}");
        let not_a_map = refused(
            "m3u-accounts",
            r#"{"username":"c","accounts":{"X":{"account_type":"XC","secret_fields":["password"]}}}"#,
        );
        assert!(not_a_map.contains("secret_fields"), "{not_a_map}");
    }

    fn authentik(desired: &str) -> Result<Spec, Error> {
        parse(&format!(
            r#"{{"service":"authentik","base_url":"http://10.0.10.10:9000","api_key_credential":"authentik-converge-token","task":"settings","desired":{desired}}}"#
        ))
    }

    #[test]
    fn authentik_settings_are_plain_field_names_behind_a_bearer_token() {
        let spec =
            authentik(r#"{"set":{"reputation_lower_limit":-10,"impersonation":false}}"#).unwrap();
        assert_eq!(spec.task_name(), "settings");
        assert_eq!(spec.service.name(), "authentik");
        assert_eq!(spec.service.key_header(), "Authorization");
        assert_eq!(
            spec.service.key_value(Secret::new("t0ken".into())).expose(),
            "Bearer t0ken"
        );
        assert!(reason(authentik(r#"{"set":{}}"#)).contains("names no field"));
        // Every field is a top-level one: authentik takes `flags` and
        // `footer_links` whole, and a partial object would drop the rest.
        assert!(reason(authentik(
            r#"{"set":{"flags.core_default_app_access":true}}"#
        ))
        .contains("plain field name"));
        assert!(reason(authentik(r#"{"settings":{"impersonation":false}}"#)).contains("unknown"));
        // `settings` is bindery's task name too, and it keeps its own shape.
        let bindery = r#"{"service":"bindery","base_url":"http://127.0.0.1:8080","api_key_credential":"k","task":"settings","desired":{"settings":{"BaseUrl":"http://x"}}}"#;
        assert!(parse(bindery).is_ok());
        let elsewhere = r#"{"service":"kavita","base_url":"http://127.0.0.1:5000","api_key_credential":"k","task":"settings","desired":{"set":{"a":1}}}"#;
        assert!(reason(parse(elsewhere)).contains("does not belong"));
    }
    // --- custom formats, app profiles and format scores (design §41) ------

    fn radarr_task(task: &str, desired: &str) -> Result<Spec, Error> {
        parse(&format!(
            r#"{{"service":"radarr","base_url":"http://localhost:7878",
                 "api_key_credential":"radarr-api-key","task":"{task}","desired":{desired}}}"#
        ))
    }

    fn prowlarr_task(task: &str, desired: &str) -> Result<Spec, Error> {
        parse(&format!(
            r#"{{"service":"prowlarr","base_url":"http://localhost:9696",
                 "api_key_credential":"prowlarr-api-key","task":"{task}","desired":{desired}}}"#
        ))
    }

    const THREE_D: &str = r#"{"formats":{"3D":{
        "include_custom_format_when_renaming": true,
        "specifications":[{"name":"3D","implementation":"ReleaseTitleSpecification",
                           "negate":false,"required":true,
                           "fields":{"value":"(?i)\\b(3d|hsbs|sbs)\\b"}}]}}}"#;

    #[test]
    fn parses_a_custom_formats_spec() {
        let spec = radarr_task("custom-formats", THREE_D).unwrap();
        assert_eq!(spec.task_name(), "custom-formats");
        let Desired::CustomFormats(desired) = &spec.desired else {
            panic!("{:?}", spec.desired);
        };
        let format = &desired.formats["3D"];
        assert!(format.include_custom_format_when_renaming);
        assert_eq!(format.specifications.len(), 1);
        assert_eq!(format.specifications[0].name, "3D");
        assert!(format.specifications[0].fields.contains_key("value"));
    }

    #[test]
    fn a_custom_formats_spec_is_strict_about_its_shape() {
        assert!(
            reason(radarr_task("custom-formats", r#"{"formats":{}}"#)).contains("names no format")
        );
        let no_specification = r#"{"formats":{"3D":{"include_custom_format_when_renaming":false,
                                   "specifications":[]}}}"#;
        assert!(
            reason(radarr_task("custom-formats", no_specification)).contains("matches nothing"),
            "{}",
            reason(radarr_task("custom-formats", no_specification))
        );
        // Found by name on both sides.
        let twice = THREE_D.replace(
            r#""fields":{"value":"(?i)\\b(3d|hsbs|sbs)\\b"}}]"#,
            r#""fields":{}},{"name":"3D","implementation":"ReleaseTitleSpecification",
               "negate":false,"required":true,"fields":{}}]"#,
        );
        assert!(reason(radarr_task("custom-formats", &twice)).contains("is named twice"));
        // The switch is required: leaving it out would write the service's
        // default, and which one that is nobody would have decided.
        let without = THREE_D.replace(r#""include_custom_format_when_renaming": true,"#, "");
        assert!(reason(radarr_task("custom-formats", &without)).contains("missing field"));
        // The task belongs to Radarr and Sonarr, and nowhere else.
        assert!(reason(prowlarr_task("custom-formats", THREE_D)).contains("does not belong"));
    }

    /// Recyclarr writes seventy formats into the same collection; a spec
    /// that said "take the rest away" would remove all of them.
    #[test]
    fn custom_formats_refuses_exactly() {
        let with = format!(
            r#"{{"service":"radarr","base_url":"http://localhost:7878",
                 "api_key_credential":"k","task":"custom-formats","exactly":true,
                 "desired":{THREE_D}}}"#
        );
        assert!(reason(parse(&with)).contains("task custom-formats does not support exactly"));
    }

    const APP_PROFILES: &str = r#"{"profiles":{"Standard":{"enableRss":true,"minimumSeeders":1}}}"#;

    #[test]
    fn parses_an_app_profiles_spec_and_takes_only_its_four_fields() {
        let spec = prowlarr_task("app-profiles", APP_PROFILES).unwrap();
        assert_eq!(spec.task_name(), "app-profiles");
        let Desired::ProwlarrAppProfiles(desired) = &spec.desired else {
            panic!("{:?}", spec.desired);
        };
        assert_eq!(desired.profiles["Standard"].len(), 2);

        assert!(reason(prowlarr_task("app-profiles", r#"{"profiles":{}}"#))
            .contains("names no profile"));
        assert!(reason(prowlarr_task(
            "app-profiles",
            r#"{"profiles":{"Standard":{}}}"#
        ))
        .contains("names no field"));
        // `name` is how a profile is found and `id` is the service's.
        for field in ["name", "id", "enabled"] {
            let spec = format!(r#"{{"profiles":{{"Standard":{{"{field}":1}}}}}}"#);
            assert!(
                reason(prowlarr_task("app-profiles", &spec))
                    .contains("is not a field of an app profile"),
                "{field}"
            );
        }
        // Prowlarr's alone.
        assert!(reason(radarr_task("app-profiles", APP_PROFILES)).contains("does not belong"));
    }

    fn indexer(extra: &str) -> Result<Spec, Error> {
        prowlarr_task(
            "indexers",
            &format!(
                r#"{{"providers":{{"TNTracker":{{"implementation":"Torznab",{extra}
                     "fields":{{"baseUrl":"http://tntracker.org"}}}}}}}}"#
            ),
        )
    }

    #[test]
    fn an_indexer_names_an_app_profile_or_its_id_but_never_both() {
        let by_name = indexer(r#""app_profile":"Standard","#).unwrap();
        let Desired::Indexers(desired) = &by_name.desired else {
            panic!("{:?}", by_name.desired);
        };
        assert_eq!(
            desired.providers["TNTracker"].app_profile.as_deref(),
            Some("Standard")
        );
        assert!(indexer(r#""set":{"appProfileId":1},"#).is_ok());

        let both = indexer(r#""app_profile":"Standard","set":{"appProfileId":1},"#);
        assert!(
            reason(both).contains("name one of them"),
            "both are refused"
        );
        assert!(indexer(r#""app_profile":"","#)
            .err()
            .unwrap()
            .to_string()
            .contains("app_profile is empty"));
        // Only Prowlarr's indexers carry an app profile at all.
        let elsewhere = prowlarr_task(
            "applications",
            r#"{"providers":{"Radarr":{"implementation":"Radarr","app_profile":"Standard"}}}"#,
        );
        assert!(reason(elsewhere).contains("prowlarr indexers spec"));
    }

    #[test]
    fn format_scores_are_optional_and_may_not_be_empty() {
        let spec = parse(PROFILES).unwrap();
        let Desired::QualityProfiles(policy) = &spec.desired else {
            panic!("{:?}", spec.desired);
        };
        assert_eq!(policy.format_scores, None);

        let with = PROFILES.replace(
            r#"["Unknown"]}"#,
            r#"["Unknown"],"format_scores":{"3D":-10000}}"#,
        );
        let spec = parse(&with).unwrap();
        let Desired::QualityProfiles(policy) = &spec.desired else {
            panic!("{:?}", spec.desired);
        };
        assert_eq!(policy.format_scores.as_ref().unwrap()["3D"], -10000);

        let empty = PROFILES.replace(r#"["Unknown"]}"#, r#"["Unknown"],"format_scores":{}}"#);
        assert!(reason(parse(&empty)).contains("names no custom format"));
        // A score is a whole number of points, not a fraction.
        let fraction = PROFILES.replace(
            r#"["Unknown"]}"#,
            r#"["Unknown"],"format_scores":{"3D":1.5}}"#,
        );
        assert!(parse(&fraction).is_err());
    }
}
