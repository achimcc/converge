use std::{
    path::{Path, PathBuf},
    process::ExitCode,
    time::Duration,
};

use converge::{
    client::{read_credential, HttpTransport},
    clock::SystemClock,
    engine::{run, Mode, Outcome, Timing},
    error::Error,
    schema,
    services::{
        arr, audiobookshelf, authentik, bindery, dispatcharr, jellyfin, kavita, koel, lidarr, ntfy,
        providers, prowlarr, seerr, servarr, suggestarr, trailarr,
    },
    spec::{Desired, DispatcharrTask, Service, Spec},
};

const USAGE: &str = "usage:
  converge apply [--deadline <seconds>] <spec.json>...
  converge plan [--deadline <seconds>] <spec.json>...
  converge schema-check --service <radarr|sonarr|lidarr|prowlarr|jellyfin|trailarr|kavita|dispatcharr|authentik> --openapi <file> [--spec <spec.json>]...
  converge schema-check --service ntfy|bindery|seerr|koel|suggestarr|audiobookshelf [--spec <spec.json>]...";

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// The provider kind of a provider spec; any other spec is not asked.
fn provider_kind(desired: &Desired) -> providers::Kind {
    match desired {
        Desired::DownloadClients(_) => providers::Kind::DownloadClients,
        Desired::Notifications(_) => providers::Kind::Notifications,
        Desired::Indexers(_) => providers::Kind::Indexers,
        Desired::IndexerProxies(_) => providers::Kind::IndexerProxies,
        _ => providers::Kind::Applications,
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("apply") => reconcile(Mode::Apply, &args[1..]),
        Some("plan") => reconcile(Mode::Plan, &args[1..]),
        Some("schema-check") => schema_check(&args[1..]),
        _ => usage(None),
    }
}

fn usage(problem: Option<&str>) -> ExitCode {
    if let Some(problem) = problem {
        eprintln!("{problem}");
    }
    eprintln!("{USAGE}");
    ExitCode::from(1)
}

fn reconcile(mode: Mode, args: &[String]) -> ExitCode {
    let mut timing = Timing::default();
    let mut paths = Vec::new();
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        if arg == "--deadline" {
            match rest.next().and_then(|s| s.parse::<u64>().ok()) {
                Some(secs) if secs > 0 => timing.deadline = Duration::from_secs(secs),
                _ => return usage(Some("--deadline needs a positive number of seconds")),
            }
        } else if arg.starts_with("--") {
            return usage(Some(&format!("unknown option {arg}")));
        } else {
            paths.push(PathBuf::from(arg));
        }
    }
    if paths.is_empty() {
        return usage(Some("no spec given"));
    }
    let credentials = std::env::var_os("CREDENTIALS_DIRECTORY").map(PathBuf::from);
    let (mut failed, mut differs) = (false, false);
    let mut tokens = Tokens::new();
    // A failing spec does not skip the next one; it only decides the exit code.
    for path in &paths {
        match reconcile_one(mode, path, credentials.as_deref(), timing, &mut tokens) {
            Ok(d) => differs |= d,
            Err(()) => failed = true,
        }
    }
    if failed {
        ExitCode::from(1)
    } else if differs {
        ExitCode::from(2)
    } else {
        ExitCode::SUCCESS
    }
}

/// Access tokens a login returned, by base URL and account, for the rest of
/// this run. Dispatcharr allows three logins a minute per client address
/// (`REST_FRAMEWORK` rate `login`); a unit that runs four specs against it
/// would be refused on the fourth (design §29). A token lives thirty minutes
/// there, longer than any run.
type Tokens = std::collections::HashMap<(String, String), String>;

/// Prints its own lines. `Ok(true)` means a plan found a difference.
fn reconcile_one(
    mode: Mode,
    path: &Path,
    credentials: Option<&Path>,
    timing: Timing,
    tokens: &mut Tokens,
) -> Result<bool, ()> {
    let spec = Spec::load(path).map_err(|e| eprintln!("{}: error: {e}", path.display()))?;
    let label = format!("{} {}", spec.service.name(), spec.task_name());
    let fail = |e: Error| eprintln!("{label}: error: {e}");
    let key = read_credential(credentials, &spec.api_key_credential).map_err(fail)?;
    // SuggestArr has no API key for its configuration endpoints: the
    // credential holds the password of a service account, and the value the
    // requests travel with is the JWT its login returns. The login is the
    // one request that carries no key.
    let key = match &spec.desired {
        Desired::SuggestArrConfiguration(desired) => {
            let anonymous = HttpTransport::anonymous(&spec.base_url, REQUEST_TIMEOUT);
            suggestarr::login(&anonymous, &desired.username, &key).map_err(fail)?
        }
        // The same for Dispatcharr (design §29): a service account's
        // password in, a short-lived access token out.
        Desired::Dispatcharr(desired) => {
            let at = (spec.base_url.clone(), desired.username.clone());
            match tokens.get(&at) {
                Some(token) => converge::secret::Secret::new(token.clone()),
                None => {
                    let anonymous = HttpTransport::anonymous(&spec.base_url, REQUEST_TIMEOUT);
                    let token =
                        dispatcharr::login(&anonymous, &desired.username, &key).map_err(fail)?;
                    tokens.insert(at, token.expose().to_string());
                    token
                }
            }
        }
        _ => key,
    };
    let transport = HttpTransport::new(
        &spec.base_url,
        spec.service.key_header(),
        spec.service.key_value(key),
        REQUEST_TIMEOUT,
    );
    let transport = if spec.service.accepts_json_only() {
        transport.accept_json()
    } else {
        transport
    };
    let report = match &spec.desired {
        Desired::QualityDefinitions(desired) => {
            let task = arr::QualityDefinitions {
                desired: desired.clone(),
            };
            run(mode, &task, &transport, &SystemClock, timing)
        }
        Desired::QualityProfiles(policy) => {
            let task = arr::QualityProfiles {
                allow_in_every_profile: policy.allow_in_every_profile.clone(),
                // `keep` and `"exactly": true` come as a pair or not at all;
                // the spec parser refuses either alone (design §39).
                keep: policy.keep.clone(),
                // The scores of named custom formats (design §41); every
                // other `formatItems` entry stays as Recyclarr wrote it.
                format_scores: policy.format_scores.clone().unwrap_or_default(),
            };
            run(mode, &task, &transport, &SystemClock, timing)
        }
        Desired::CustomFormats(desired) => {
            let task = arr::CustomFormats {
                formats: desired
                    .formats
                    .iter()
                    .map(|(name, format)| {
                        (
                            name.clone(),
                            arr::formats::FormatTarget {
                                include_custom_format_when_renaming: format
                                    .include_custom_format_when_renaming,
                                specifications: format
                                    .specifications
                                    .iter()
                                    .map(|s| arr::formats::SpecificationTarget {
                                        name: s.name.clone(),
                                        implementation: s.implementation.clone(),
                                        negate: s.negate,
                                        required: s.required,
                                        fields: s.fields.clone(),
                                    })
                                    .collect(),
                            },
                        )
                    })
                    .collect(),
            };
            run(mode, &task, &transport, &SystemClock, timing)
        }
        Desired::ProwlarrAppProfiles(desired) => {
            let task = prowlarr::AppProfiles {
                profiles: desired.profiles.clone(),
            };
            run(mode, &task, &transport, &SystemClock, timing)
        }
        // Lidarr's own profiles: what each named profile holds (design §42).
        Desired::LidarrQualityProfiles(profiles) => {
            let task = lidarr::QualityProfiles {
                profiles: profiles.clone(),
            };
            run(mode, &task, &transport, &SystemClock, timing)
        }
        Desired::LidarrMetadataProfiles(profiles) => {
            let task = lidarr::MetadataProfiles {
                profiles: profiles.clone(),
            };
            run(mode, &task, &transport, &SystemClock, timing)
        }
        Desired::ServerConfiguration(set) => {
            let task = jellyfin::ServerConfiguration { set: set.clone() };
            run(mode, &task, &transport, &SystemClock, timing)
        }
        Desired::KavitaServerSettings(set) => {
            let task = kavita::ServerSettings { set: set.clone() };
            run(mode, &task, &transport, &SystemClock, timing)
        }
        Desired::AuthentikSettings(set) => {
            let task = authentik::Settings { set: set.clone() };
            run(mode, &task, &transport, &SystemClock, timing)
        }
        Desired::KavitaLibraries(libraries) => {
            let task = kavita::Libraries {
                libraries: libraries.clone(),
            };
            run(mode, &task, &transport, &SystemClock, timing)
        }
        Desired::Dispatcharr(desired) => match &desired.task {
            DispatcharrTask::StreamSettings {
                default_stream_profile,
            } => {
                let task = dispatcharr::StreamSettings {
                    default_stream_profile: default_stream_profile.clone(),
                };
                run(mode, &task, &transport, &SystemClock, timing)
            }
            DispatcharrTask::Entries(kind, entries, credentials_of) => {
                // Every hidden value before the first request, as for plugins.
                let mut secrets = std::collections::BTreeMap::new();
                for (name, fields) in credentials_of {
                    let mut values = std::collections::BTreeMap::new();
                    for (field, credential) in fields {
                        values.insert(
                            field.clone(),
                            read_credential(credentials, credential).map_err(fail)?,
                        );
                    }
                    secrets.insert(name.clone(), values);
                }
                let task = dispatcharr::Entries {
                    kind: *kind,
                    entries: entries.clone(),
                    secrets,
                };
                run(mode, &task, &transport, &SystemClock, timing)
            }
            DispatcharrTask::Groups(accounts) => {
                let task = dispatcharr::Groups {
                    accounts: accounts.clone(),
                };
                run(mode, &task, &transport, &SystemClock, timing)
            }
            DispatcharrTask::ChannelEpg(channels) => {
                let task = dispatcharr::ChannelEpg {
                    channels: channels.clone(),
                };
                run(mode, &task, &transport, &SystemClock, timing)
            }
        },
        Desired::AudiobookshelfLibraries(libraries) => {
            let task = audiobookshelf::Libraries {
                libraries: libraries.clone(),
            };
            run(mode, &task, &transport, &SystemClock, timing)
        }
        Desired::AudiobookshelfAdminPermissions(desired) => {
            let task = audiobookshelf::AdminPermissions {
                types: desired.types.clone(),
                permissions: desired.permissions.clone(),
            };
            run(mode, &task, &transport, &SystemClock, timing)
        }
        Desired::AudiobookshelfAuthSettings(desired) => {
            // Every secret before the first request, as for plugin keys.
            let mut secrets = std::collections::BTreeMap::new();
            for (field, credential) in &desired.secret_fields {
                secrets.insert(
                    field.clone(),
                    read_credential(credentials, credential).map_err(fail)?,
                );
            }
            let task = audiobookshelf::AuthSettings {
                set: desired.set.clone(),
                secrets,
            };
            run(mode, &task, &transport, &SystemClock, timing)
        }
        Desired::NamedConfiguration(settings) => {
            let task = jellyfin::NamedConfiguration {
                key: settings.key,
                set: settings.set.clone(),
            };
            run(mode, &task, &transport, &SystemClock, timing)
        }
        Desired::UserPolicies(settings) => {
            let task = jellyfin::UserPolicies {
                all: settings.all.clone(),
                accounts: settings.accounts.clone(),
            };
            run(mode, &task, &transport, &SystemClock, timing)
        }
        Desired::DisplayPreferences(settings) => {
            let task = jellyfin::DisplayPreferences {
                client: settings.client.clone(),
                all: settings.all.custom_prefs.clone(),
                accounts: settings
                    .accounts
                    .iter()
                    .map(|(name, prefs)| (name.clone(), prefs.custom_prefs.clone()))
                    .collect(),
            };
            run(mode, &task, &transport, &SystemClock, timing)
        }
        Desired::LibraryOptions(settings) => {
            let task = jellyfin::LibraryOptions {
                libraries: settings.libraries.clone(),
                set: settings.set.clone(),
            };
            run(mode, &task, &transport, &SystemClock, timing)
        }
        Desired::ScheduledTaskTriggers(triggers) => {
            let task = jellyfin::ScheduledTaskTriggers {
                key_prefix: triggers.key_prefix.clone(),
                triggers: triggers.triggers.clone(),
            };
            run(mode, &task, &transport, &SystemClock, timing)
        }
        Desired::PluginConfigurations(plugins) => {
            // Every secret is read before the first request: a missing
            // credential is a configuration error, not something to find
            // out halfway through.
            let mut targets = Vec::new();
            for (id, plugin) in plugins {
                let mut secrets = std::collections::BTreeMap::new();
                for (path, credential) in &plugin.secrets {
                    secrets.insert(
                        path.clone(),
                        read_credential(credentials, credential).map_err(fail)?,
                    );
                }
                targets.push(jellyfin::PluginTarget {
                    id: id.clone(),
                    name: plugin.name.clone(),
                    set: plugin.set.clone(),
                    secrets,
                    lists: plugin.lists.clone(),
                });
            }
            let task = jellyfin::PluginConfigurations { plugins: targets };
            run(mode, &task, &transport, &SystemClock, timing)
        }
        Desired::Connections(desired) => {
            // As for plugin secrets: every key before the first request.
            let mut connections = Vec::new();
            for (name, connection) in &desired.connections {
                connections.push(trailarr::ConnectionTarget {
                    name: name.clone(),
                    set: connection.set.clone(),
                    api_key: read_credential(credentials, &connection.api_key_credential)
                        .map_err(fail)?,
                });
            }
            let task = trailarr::Connections {
                connections,
                exactly: spec.exactly,
            };
            run(mode, &task, &transport, &SystemClock, timing)
        }
        Desired::TrailerProfiles(settings) => {
            let task = trailarr::TrailerProfiles {
                set: settings.set.clone(),
            };
            run(mode, &task, &transport, &SystemClock, timing)
        }
        Desired::AccountSubscriptions(desired) => {
            let content = read_credential(credentials, &desired.topics_credential).map_err(fail)?;
            let topics = ntfy::topics(&desired.topics_credential, &content).map_err(fail)?;
            let task = ntfy::AccountSubscriptions {
                base_url: desired.base_url.clone(),
                topics,
            };
            run(mode, &task, &transport, &SystemClock, timing)
        }
        Desired::SeerrMain(set) => {
            let task = seerr::Main { set: set.clone() };
            run(mode, &task, &transport, &SystemClock, timing)
        }
        Desired::SeerrJellyfin(desired) => {
            let task = seerr::Jellyfin {
                set: desired.set.clone(),
                api_key: read_credential(credentials, &desired.api_key_credential).map_err(fail)?,
                libraries: desired.libraries.clone(),
            };
            run(mode, &task, &transport, &SystemClock, timing)
        }
        Desired::SeerrServers(kind, servers) => {
            // As for plugin secrets: every key before the first request.
            let mut targets = Vec::new();
            for (name, server) in servers {
                targets.push(seerr::ServerTarget {
                    name: name.clone(),
                    set: server.set.clone(),
                    api_key: read_credential(credentials, &server.api_key_credential)
                        .map_err(fail)?,
                    profile: server.profile.clone(),
                    root_folder: server.root_folder.clone(),
                });
            }
            let task = seerr::Servers {
                kind: *kind,
                servers: targets,
            };
            run(mode, &task, &transport, &SystemClock, timing)
        }
        Desired::SeerrWebhook(desired) => {
            let mut headers = std::collections::BTreeMap::new();
            for (key, credential) in &desired.headers {
                headers.insert(
                    key.clone(),
                    read_credential(credentials, credential).map_err(fail)?,
                );
            }
            let task = seerr::Webhook {
                set: desired.set.clone(),
                payload: desired.payload.clone(),
                headers,
            };
            run(mode, &task, &transport, &SystemClock, timing)
        }
        Desired::BinderyEntries(kind, entries) => {
            // As for providers: every secret before the first request.
            let mut targets = Vec::new();
            for (key, entry) in entries {
                let mut secret_fields = std::collections::BTreeMap::new();
                for (field, credential) in &entry.secret_fields {
                    secret_fields.insert(
                        field.clone(),
                        read_credential(credentials, credential).map_err(fail)?,
                    );
                }
                targets.push(bindery::ResourceTarget {
                    key: key.clone(),
                    set: entry.set.clone(),
                    secret_fields,
                });
            }
            let task = bindery::Resources {
                api: match kind {
                    converge::spec::BinderyKind::DownloadClients => &bindery::DOWNLOAD_CLIENTS,
                    converge::spec::BinderyKind::ProwlarrInstances => &bindery::PROWLARR_INSTANCES,
                    converge::spec::BinderyKind::RootFolders => &bindery::ROOT_FOLDERS,
                },
                entries: targets,
            };
            run(mode, &task, &transport, &SystemClock, timing)
        }
        Desired::SuggestArrConfiguration(desired) => {
            // As for plugin secrets: every credential before the first
            // request, so a missing one is a configuration error and not
            // something to discover halfway through.
            let mut secrets = std::collections::BTreeMap::new();
            for (field, credential) in &desired.secrets {
                secrets.insert(
                    field.clone(),
                    read_credential(credentials, credential).map_err(fail)?,
                );
            }
            let task = suggestarr::Configuration {
                set: desired.set.clone(),
                secrets,
                libraries: desired.jellyfin_libraries.as_ref().map(|rule| {
                    suggestarr::LibraryRule {
                        exclude_collection_types: rule.exclude_collection_types.clone(),
                    }
                }),
            };
            run(mode, &task, &transport, &SystemClock, timing)
        }
        Desired::KoelRadioStations(stations) => {
            // As for credentials: every logo file before the first request.
            let mut targets = Vec::new();
            for station in stations {
                let logo = match &station.logo_file {
                    Some(file) => Some(koel::logo(file).map_err(|reason| {
                        fail(Error::SpecInvalid {
                            path: spec.path.clone(),
                            reason: format!("desired.stations {}: {reason}", station.name),
                        })
                    })?),
                    None => None,
                };
                targets.push(koel::StationTarget {
                    name: station.name.clone(),
                    url: station.url.clone(),
                    description: station.description.clone(),
                    is_public: station.is_public,
                    homepage_url: station.homepage_url.clone(),
                    logo,
                });
            }
            let task = koel::RadioStations {
                stations: targets,
                exactly: spec.exactly,
            };
            run(mode, &task, &transport, &SystemClock, timing)
        }
        Desired::BinderySettings(set) => {
            let task = bindery::Settings { set: set.clone() };
            run(mode, &task, &transport, &SystemClock, timing)
        }
        Desired::BinderyIndexers(names) => {
            let task = bindery::Indexers {
                enabled: names.clone(),
            };
            run(mode, &task, &transport, &SystemClock, timing)
        }
        Desired::BinderyOidcProviders(entries) => {
            // As for the other bindery entries: every secret before the first
            // request, so a missing credential fails before anything is read.
            let mut targets = Vec::new();
            for (id, entry) in entries {
                let mut secret_fields = std::collections::BTreeMap::new();
                for (field, credential) in &entry.secret_fields {
                    secret_fields.insert(
                        field.clone(),
                        read_credential(credentials, credential).map_err(fail)?,
                    );
                }
                targets.push(bindery::OidcTarget {
                    id: id.clone(),
                    set: entry.set.clone(),
                    secret_fields,
                });
            }
            let task = bindery::OidcProviders { entries: targets };
            run(mode, &task, &transport, &SystemClock, timing)
        }
        Desired::DelayProfiles(set) => {
            let task = servarr::DelayProfiles {
                api: servarr_api(&spec).map_err(fail)?,
                set: set.clone(),
            };
            run(mode, &task, &transport, &SystemClock, timing)
        }
        Desired::Naming(set)
        | Desired::MediaManagement(set)
        | Desired::DownloadClientConfig(set)
        | Desired::IndexerConfig(set) => {
            let kind = match spec.desired {
                Desired::Naming(_) => servarr::Kind::Naming,
                Desired::MediaManagement(_) => servarr::Kind::MediaManagement,
                Desired::IndexerConfig(_) => servarr::Kind::IndexerConfig,
                _ => servarr::Kind::DownloadClientConfig,
            };
            let task = servarr::Document {
                api: servarr_api(&spec).map_err(fail)?,
                kind,
                set: set.clone(),
            };
            run(mode, &task, &transport, &SystemClock, timing)
        }
        Desired::DownloadClients(desired)
        | Desired::Notifications(desired)
        | Desired::Applications(desired)
        | Desired::Indexers(desired)
        | Desired::IndexerProxies(desired) => {
            let kind = provider_kind(&spec.desired);
            let api = providers::ProviderApi::of(spec.service, kind).ok_or_else(|| {
                fail(Error::SpecInvalid {
                    path: spec.path.clone(),
                    reason: format!("{} has no such providers", spec.service.name()),
                })
            })?;
            // Every hidden value before the first request, as for plugins.
            let mut targets = Vec::new();
            for (name, entry) in &desired.providers {
                let mut secret_fields = std::collections::BTreeMap::new();
                for (field, credential) in &entry.secret_fields {
                    secret_fields.insert(
                        field.clone(),
                        read_credential(credentials, credential).map_err(fail)?,
                    );
                }
                targets.push(providers::ProviderTarget {
                    name: name.clone(),
                    implementation: entry.implementation.clone(),
                    template: entry.template.clone(),
                    tags: entry.tags.clone(),
                    app_profile: entry.app_profile.clone(),
                    set: entry.set.clone(),
                    fields: entry.fields.clone(),
                    secret_fields,
                });
            }
            let task = providers::Providers {
                api,
                providers: targets,
            };
            run(mode, &task, &transport, &SystemClock, timing)
        }
        Desired::RootFolders(desired) => {
            let task = servarr::RootFolders {
                api: servarr_api(&spec).map_err(fail)?,
                folders: desired
                    .folders
                    .iter()
                    .map(|(path, folder)| servarr::FolderTarget {
                        path: path.clone(),
                        set: folder.set.clone(),
                        profiles: folder.profiles.clone(),
                    })
                    .collect(),
            };
            run(mode, &task, &transport, &SystemClock, timing)
        }
    }
    .map_err(fail)?;
    println!("{label}: service version {}", report.version);
    for note in &report.notes {
        println!("{label}: note: {note}");
    }
    for line in &report.handed_over {
        println!("{label}: {line}");
    }
    match report.outcome {
        Outcome::Unchanged => {
            println!("{label}: unchanged");
            Ok(false)
        }
        Outcome::Differs(changes) => {
            for change in &changes {
                println!("{label}: would change {change}");
            }
            println!("{label}: {} field(s) differ", changes.len());
            Ok(true)
        }
        Outcome::Changed(changes) => {
            for change in &changes {
                println!("{label}: {change}");
            }
            println!(
                "{label}: changed {} field(s), read back and confirmed",
                changes.len()
            );
            Ok(false)
        }
    }
}

/// The spec parser only lets Servarr tasks through for Radarr, Sonarr and
/// Lidarr; this says so instead of trusting it.
fn servarr_api(spec: &Spec) -> Result<&'static servarr::Api, Error> {
    servarr::Api::of(spec.service).ok_or_else(|| Error::SpecInvalid {
        path: spec.path.clone(),
        reason: format!("{} has no Servarr API", spec.service.name()),
    })
}

/// ntfy (design §10), bindery (§14) and Audiobookshelf (§27) have no OpenAPI
/// description, Seerr's
/// (§17) misnames the fields its answers carry, and Koel's (§18) describes a
/// version four majors old without radio stations: their specs are loaded
/// and validated, and that is all a build can check.
fn undescribed_specs_check(service: &str, specs: &[PathBuf]) -> ExitCode {
    let mut findings = Vec::new();
    for path in specs {
        match Spec::load(path) {
            Ok(spec) if spec.service.name() == service => {}
            Ok(spec) => findings.push(format!(
                "{}: the spec is for {}, not {service}",
                path.display(),
                spec.service.name()
            )),
            Err(e) => findings.push(format!("{}: {e}", path.display())),
        }
    }
    if !findings.is_empty() {
        for finding in &findings {
            eprintln!("{service}: {finding}");
        }
        return ExitCode::from(1);
    }
    println!(
        "{service}: no OpenAPI description exists; {} spec(s) valid",
        specs.len()
    );
    println!("{service}: field names are checked by recorded answers and at runtime only");
    ExitCode::SUCCESS
}

fn schema_check(args: &[String]) -> ExitCode {
    let (mut service, mut openapi, mut specs) = (None, None, Vec::new());
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--service" => service = rest.next().cloned(),
            "--openapi" => openapi = rest.next().map(PathBuf::from),
            "--spec" => match rest.next() {
                Some(path) => specs.push(PathBuf::from(path)),
                None => return usage(Some("--spec needs a file")),
            },
            other => return usage(Some(&format!("unexpected argument {other}"))),
        }
    }
    if let Some(name @ ("ntfy" | "bindery" | "seerr" | "koel" | "suggestarr" | "audiobookshelf")) =
        service.as_deref()
    {
        return match openapi {
            Some(_) => usage(Some(&format!(
                "{name} publishes no OpenAPI description; call schema-check --service {name} without --openapi"
            ))),
            None => undescribed_specs_check(name, &specs),
        };
    }
    let (Some(service), Some(openapi)) = (service, openapi) else {
        return usage(Some("schema-check needs --service and --openapi"));
    };
    let (endpoints, wire): (Vec<converge::endpoint::Endpoint>, _) = match service.as_str() {
        "radarr" | "sonarr" => {
            let mut endpoints = arr::ENDPOINTS.to_vec();
            endpoints.extend(servarr::V3.task_endpoints());
            endpoints.extend(providers::ProviderApi::endpoints_of(Service::Radarr));
            (endpoints, arr::wire_types())
        }
        "lidarr" => {
            let mut endpoints = vec![servarr::V1.status];
            endpoints.extend(servarr::V1.task_endpoints());
            endpoints.extend(providers::ProviderApi::endpoints_of(Service::Lidarr));
            endpoints.extend(lidarr::ENDPOINTS);
            (endpoints, servarr::lidarr_wire_types())
        }
        "prowlarr" => {
            let mut endpoints = vec![servarr::V1.status];
            endpoints.extend(providers::ProviderApi::endpoints_of(Service::Prowlarr));
            endpoints.extend(prowlarr::ENDPOINTS);
            let mut wire = servarr::prowlarr_wire_types();
            wire.extend(prowlarr::wire_types());
            (endpoints, wire)
        }
        "jellyfin" => (jellyfin::ENDPOINTS.to_vec(), jellyfin::wire_types()),
        "trailarr" => (trailarr::ENDPOINTS.to_vec(), trailarr::wire_types()),
        "kavita" => (kavita::ENDPOINTS.to_vec(), kavita::wire_types()),
        "dispatcharr" => (dispatcharr::ENDPOINTS.to_vec(), dispatcharr::wire_types()),
        "authentik" => (authentik::ENDPOINTS.to_vec(), authentik::wire_types()),
        _ => return usage(Some(&format!("unknown service {service:?}"))),
    };
    let document = std::fs::read_to_string(&openapi)
        .map_err(|e| e.to_string())
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).map_err(|e| e.to_string()));
    let document = match document {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{service}: cannot read {}: {e}", openapi.display());
            return ExitCode::from(1);
        }
    };
    let mut findings = schema::check(&document, &endpoints, &wire);
    let mut checked_fields = 0;
    for path in &specs {
        let spec = match Spec::load(path) {
            Ok(spec) => spec,
            Err(e) => {
                findings.push(format!("{}: {e}", path.display()));
                continue;
            }
        };
        if spec.service.name() != service {
            findings.push(format!(
                "{}: the spec is for {}, not {service}",
                path.display(),
                spec.service.name()
            ));
            continue;
        }
        let (spec_findings, count) = match &spec.desired {
            // Typed tasks: their fields are the wire types checked above.
            // `format_scores` names custom formats, not fields -- what the
            // service has is a runtime question (design §41).
            Desired::QualityDefinitions(_)
            | Desired::QualityProfiles(_)
            | Desired::LidarrQualityProfiles(_)
            | Desired::LidarrMetadataProfiles(_) => (Vec::new(), 0),
            // An app profile's fields are plain properties of the component.
            Desired::ProwlarrAppProfiles(desired) => {
                let mut found = Vec::new();
                let mut count = 0;
                for (name, set) in &desired.profiles {
                    count += set.len();
                    found.extend(
                        schema::check_paths(&document, "AppProfileResource", set)
                            .into_iter()
                            .map(|f| format!("app profile {name}: {f}")),
                    );
                }
                (found, count)
            }
            // The format's switch is a property of `CustomFormatResource`,
            // and each specification is a `CustomFormatSpecificationSchema`.
            // Its `fields` entries depend on the implementation and have no
            // schema, as for providers (§12): runtime only.
            Desired::CustomFormats(desired) => {
                let mut found = Vec::new();
                let mut count = 0;
                for (name, format) in &desired.formats {
                    let switch: std::collections::BTreeMap<String, serde_json::Value> = [(
                        "includeCustomFormatWhenRenaming".to_string(),
                        serde_json::Value::Bool(format.include_custom_format_when_renaming),
                    )]
                    .into_iter()
                    .collect();
                    count += 1;
                    found.extend(
                        schema::check_paths(&document, arr::formats::FORMAT_COMPONENT, &switch)
                            .into_iter()
                            .map(|f| format!("custom format {name}: {f}")),
                    );
                    let specifications: Vec<serde_json::Value> = format
                        .specifications
                        .iter()
                        .map(|s| {
                            serde_json::json!({
                                "name": s.name,
                                "implementation": s.implementation,
                                "negate": s.negate,
                                "required": s.required,
                            })
                        })
                        .collect();
                    count += 4 * specifications.len();
                    found.extend(
                        schema::check_objects(
                            &document,
                            arr::formats::SPECIFICATION_COMPONENT,
                            &serde_json::Value::Array(specifications),
                        )
                        .into_iter()
                        .map(|f| format!("custom format {name}: {f}")),
                    );
                }
                (found, count)
            }
            // No schema to check against (design §7): only the endpoints and
            // PluginInfo are, above; the fields are checked at runtime.
            Desired::PluginConfigurations(_) => (Vec::new(), 0),
            Desired::ServerConfiguration(set) => (
                schema::check_paths(&document, jellyfin::SERVER_CONFIGURATION, set),
                set.len(),
            ),
            Desired::KavitaServerSettings(set) => (
                schema::check_paths(&document, kavita::SERVER_SETTINGS, set),
                set.len(),
            ),
            // Compared with the answer, written with the body of the PATCH:
            // each field must be a property of both, as for a Kavita library.
            Desired::AuthentikSettings(set) => {
                let mut found = Vec::new();
                for component in [authentik::SETTINGS, authentik::SETTINGS_PATCH] {
                    found.extend(schema::check_paths(&document, component, set));
                }
                (found, set.len())
            }
            // Written with the update's body, compared with the answer: each
            // field must be a property of both.
            Desired::KavitaLibraries(libraries) => {
                let mut found = Vec::new();
                for (folder, fields) in libraries {
                    for component in [
                        kavita::LIBRARY_UPDATE_COMPONENT,
                        kavita::LIBRARY_READ_COMPONENT,
                    ] {
                        found.extend(
                            schema::check_paths(&document, component, fields)
                                .into_iter()
                                .map(|f| format!("library {folder}: {f}")),
                        );
                    }
                }
                (found, libraries.values().map(|f| f.len()).sum())
            }
            Desired::NamedConfiguration(settings) => (
                schema::check_paths(&document, settings.key.component(), &settings.set),
                settings.set.len(),
            ),
            Desired::LibraryOptions(settings) => (
                schema::check_paths(&document, jellyfin::LIBRARY_OPTIONS, &settings.set),
                settings.set.len(),
            ),
            // Every field of both maps is a top-level property of
            // `UserPolicy`, checked for existence, type and enum (§40).
            Desired::UserPolicies(settings) => {
                let mut found =
                    schema::check_paths(&document, jellyfin::USER_POLICY, &settings.all);
                let mut count = settings.all.len();
                for (name, set) in &settings.accounts {
                    count += set.len();
                    found.extend(
                        schema::check_paths(&document, jellyfin::USER_POLICY, set)
                            .into_iter()
                            .map(|f| format!("account {name}: {f}")),
                    );
                }
                (found, count)
            }
            // `CustomPrefs` is a free string map (`additionalProperties` in
            // `DisplayPreferencesDto`): its keys are not properties a schema
            // could know. Only the endpoints are checked, above.
            Desired::DisplayPreferences(_) => (Vec::new(), 0),
            Desired::ScheduledTaskTriggers(triggers) => {
                let list = serde_json::Value::Array(
                    triggers
                        .triggers
                        .iter()
                        .cloned()
                        .map(serde_json::Value::Object)
                        .collect(),
                );
                (
                    schema::check_objects(&document, jellyfin::TASK_TRIGGER_INFO, &list),
                    triggers.triggers.iter().map(|t| t.len()).sum(),
                )
            }
            // A connection's fields are sent when it is added and when it is
            // updated, so each must be a property of both bodies.
            Desired::Connections(desired) => {
                let mut found = Vec::new();
                for (name, connection) in &desired.connections {
                    for component in [
                        trailarr::CONNECTION_CREATE_COMPONENT,
                        trailarr::CONNECTION_UPDATE_COMPONENT,
                    ] {
                        found.extend(
                            schema::check_paths(&document, component, &connection.set)
                                .into_iter()
                                .map(|f| format!("connection {name}: {f}")),
                        );
                    }
                }
                let count = desired.connections.values().map(|c| c.set.len()).sum();
                (found, count)
            }
            Desired::TrailerProfiles(settings) => (
                schema::check_paths(&document, trailarr::TRAILER_PROFILE_READ, &settings.set),
                settings.set.len(),
            ),
            // Not reachable: the service check above rejects ntfy, bindery,
            // Seerr, Koel and SuggestArr specs.
            Desired::KoelRadioStations(_)
            | Desired::AudiobookshelfAuthSettings(_)
            | Desired::AudiobookshelfAdminPermissions(_)
            | Desired::AudiobookshelfLibraries(_)
            | Desired::SuggestArrConfiguration(_)
            | Desired::AccountSubscriptions(_)
            | Desired::BinderyEntries(..)
            | Desired::BinderySettings(_)
            | Desired::BinderyOidcProviders(_)
            | Desired::BinderyIndexers(_)
            | Desired::SeerrMain(_)
            | Desired::SeerrJellyfin(_)
            | Desired::SeerrServers(..)
            | Desired::SeerrWebhook(_) => (Vec::new(), 0),
            Desired::Naming(set) => (
                schema::check_paths(&document, servarr::NAMING, set),
                set.len(),
            ),
            Desired::IndexerConfig(set) => (
                schema::check_paths(&document, servarr::INDEXER_CONFIG, set),
                set.len(),
            ),
            // The fields are checked against the component; WHICH profile is
            // meant the task decides itself (the one without tags), so there
            // is nothing about that for a schema to say.
            Desired::DelayProfiles(set) => (
                schema::check_paths(&document, servarr::DELAY_PROFILE, set),
                set.len(),
            ),
            // The `fields` entries depend on the implementation and have no
            // schema (`Field.value` is untyped): only the top-level fields
            // are checked here, the entries against the answer at runtime.
            Desired::DownloadClients(desired)
            | Desired::Notifications(desired)
            | Desired::Applications(desired)
            | Desired::Indexers(desired)
            | Desired::IndexerProxies(desired) => {
                let kind = provider_kind(&spec.desired);
                match providers::ProviderApi::of(spec.service, kind) {
                    Some(api) => {
                        let mut found = Vec::new();
                        for (name, entry) in &desired.providers {
                            found.extend(
                                schema::check_paths(&document, api.component, &entry.set)
                                    .into_iter()
                                    .map(|f| format!("{} {name}: {f}", api.subject)),
                            );
                        }
                        (found, desired.providers.values().map(|p| p.set.len()).sum())
                    }
                    None => (vec![format!("{} has no such providers", service)], 0),
                }
            }
            // An account's and a source's fields are sent when they are added
            // and when they are updated, so each must be a property of both;
            // a group's fields are those of the membership the account answers
            // with (the endpoint's declared body is wrong, design §29).
            Desired::Dispatcharr(desired) => match &desired.task {
                DispatcharrTask::StreamSettings { .. } => (Vec::new(), 0),
                // Names only: a channel, a source, a tvg-id -- no field of a body.
                DispatcharrTask::ChannelEpg(_) => (Vec::new(), 0),
                DispatcharrTask::Entries(kind, entries, secrets) => {
                    let components = match kind {
                        dispatcharr::EntryKind::M3uAccount => [
                            dispatcharr::M3U_ACCOUNT_CREATE_COMPONENT,
                            dispatcharr::M3U_ACCOUNT_UPDATE_COMPONENT,
                        ],
                        dispatcharr::EntryKind::EpgSource => [
                            dispatcharr::EPG_SOURCE_CREATE_COMPONENT,
                            dispatcharr::EPG_SOURCE_UPDATE_COMPONENT,
                        ],
                    };
                    let mut found = Vec::new();
                    let mut count = 0;
                    for (name, fields) in entries {
                        // The secret fields by name only: their values are
                        // credentials, but the fields must exist all the same.
                        let mut named = fields.clone();
                        for field in secrets.get(name).into_iter().flat_map(|s| s.keys()) {
                            named.insert(field.clone(), serde_json::Value::Null);
                        }
                        count += named.len();
                        for component in components {
                            found.extend(
                                schema::check_paths(&document, component, &named)
                                    .into_iter()
                                    .map(|f| format!("{name}: {f}")),
                            );
                        }
                    }
                    (found, count)
                }
                DispatcharrTask::Groups(accounts) => {
                    let mut found = Vec::new();
                    let mut count = 0;
                    for (account, groups) in accounts {
                        for (group, fields) in groups {
                            count += fields.len();
                            found.extend(
                                schema::check_paths(
                                    &document,
                                    dispatcharr::GROUP_COMPONENT,
                                    &fields
                                        .iter()
                                        .filter(|(k, _)| {
                                            // Not fields of the view: the
                                            // profile by name and the name
                                            // filters live in custom_properties.
                                            k.as_str() != dispatcharr::GROUP_STREAM_PROFILE
                                                && !dispatcharr::GROUP_CUSTOM_FIELDS
                                                    .contains(&k.as_str())
                                        })
                                        .map(|(k, v)| (k.clone(), v.clone()))
                                        .collect(),
                                )
                                .into_iter()
                                .map(|f| format!("{account}/{group}: {f}")),
                            );
                        }
                    }
                    (found, count)
                }
            },
            Desired::MediaManagement(set) => (
                schema::check_paths(&document, servarr::MEDIA_MANAGEMENT, set),
                set.len(),
            ),
            Desired::DownloadClientConfig(set) => (
                schema::check_paths(&document, servarr::DOWNLOAD_CLIENT_CONFIG, set),
                set.len(),
            ),
            // A folder is sent with its path, its fields and its profiles as
            // ids; each of them must be a property of the root folder.
            Desired::RootFolders(desired) => {
                let mut found = Vec::new();
                let mut count = 0;
                for (path, folder) in &desired.folders {
                    let mut fields = folder.set.clone();
                    fields.insert("path".to_string(), serde_json::Value::from(path.clone()));
                    for field in folder.profiles.keys() {
                        fields.insert(field.clone(), serde_json::Value::from(1));
                    }
                    count += fields.len();
                    found.extend(
                        schema::check_paths(&document, servarr::ROOT_FOLDER, &fields)
                            .into_iter()
                            .map(|f| format!("root folder {path}: {f}")),
                    );
                }
                (found, count)
            }
        };
        checked_fields += count;
        findings.extend(
            spec_findings
                .into_iter()
                .map(|f| format!("{}: {f}", path.display())),
        );
    }
    if findings.is_empty() {
        println!(
            "{service}: {} endpoints and their wire types match {}",
            endpoints.len(),
            openapi.display()
        );
        if !specs.is_empty() {
            println!(
                "{service}: {} spec field(s) in {} spec(s) match",
                checked_fields,
                specs.len()
            );
        }
        ExitCode::SUCCESS
    } else {
        for finding in &findings {
            eprintln!("{service}: {finding}");
        }
        ExitCode::from(1)
    }
}
