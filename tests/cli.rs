mod support;

use std::{
    path::{Path, PathBuf},
    process::Command,
};

use support::Server;

const STATUS: &str = include_str!("fixtures/radarr-6.3.0.10514/system-status.json");
const LIST: &str = include_str!("fixtures/radarr-6.3.0.10514/qualitydefinition.json");
const DESIRED: &str = include_str!("fixtures/radarr-6.3.0.10514/desired.json");

fn converge() -> Command {
    Command::new(env!("CARGO_BIN_EXE_converge"))
}

fn write_spec(dir: &Path, file: &str, base: &str, desired: &str) -> PathBuf {
    let path = dir.join(file);
    let text = format!(
        r#"{{"service":"radarr","base_url":"{base}","api_key_credential":"radarr-api-key","task":"quality-definitions","desired":{desired}}}"#
    );
    std::fs::write(&path, text).unwrap();
    std::fs::write(dir.join("radarr-api-key"), "k\n").unwrap();
    path
}

fn radarr() -> Server {
    Server::start(vec![
        ("GET", "/api/v3/system/status", 200, STATUS.into()),
        ("GET", "/api/v3/qualitydefinition", 200, LIST.into()),
    ])
}

#[test]
fn no_arguments_is_usage_and_exit_1() {
    let out = converge().output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("usage:"));
}

#[test]
fn schema_check_passes_for_the_vendored_file() {
    let file = format!(
        "{}/openapi/radarr-6.3.0.10514.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let out = converge()
        .args(["schema-check", "--service", "radarr", "--openapi", &file])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Exit 0 alone would also pass for a program that does nothing.
    assert!(
        String::from_utf8_lossy(&out.stdout)
            .contains("radarr: 29 endpoints and their wire types match"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn schema_check_reads_jellyfin_specs_and_rejects_a_trigger_type_outside_the_enum() {
    let openapi = format!(
        "{}/openapi/jellyfin-10.11.11.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let dir = tempfile::tempdir().unwrap();
    let spec = |name: &str, trigger: &str| {
        let path = dir.path().join(name);
        std::fs::write(
            &path,
            format!(
                r#"{{"service":"jellyfin","base_url":"http://localhost:8096","api_key_credential":"k","task":"scheduled-task-triggers","desired":{{"key_prefix":"Merge","triggers":[{{"Type":"{trigger}","TimeOfDayTicks":198000000000}}]}}}}"#
            ),
        )
        .unwrap();
        path
    };
    let good = spec("good.json", "DailyTrigger");
    let out = converge()
        .args([
            "schema-check",
            "--service",
            "jellyfin",
            "--openapi",
            &openapi,
            "--spec",
        ])
        .arg(&good)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("jellyfin: 16 endpoints and their wire types match"),
        "{stdout}"
    );
    assert!(
        stdout.contains("2 spec field(s) in 1 spec(s) match"),
        "{stdout}"
    );

    let bad = spec("bad.json", "Daily");
    let out = converge()
        .args([
            "schema-check",
            "--service",
            "jellyfin",
            "--openapi",
            &openapi,
            "--spec",
        ])
        .arg(&bad)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("TaskTriggerInfo[0].Type: \"Daily\" is not one of"),
        "{stderr}"
    );
}

#[test]
fn schema_check_checks_a_named_configuration_against_the_component_of_its_key() {
    let openapi = format!(
        "{}/openapi/jellyfin-10.11.11.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let dir = tempfile::tempdir().unwrap();
    let spec = |name: &str, key: &str, set: &str| {
        let path = dir.path().join(name);
        std::fs::write(
            &path,
            format!(
                r#"{{"service":"jellyfin","base_url":"http://localhost:8096","api_key_credential":"k","task":"named-configuration","desired":{{"key":"{key}","set":{set}}}}}"#
            ),
        )
        .unwrap();
        path
    };
    let check = |path: &std::path::Path| {
        converge()
            .args([
                "schema-check",
                "--service",
                "jellyfin",
                "--openapi",
                &openapi,
                "--spec",
            ])
            .arg(path)
            .output()
            .unwrap()
    };

    let good = spec(
        "good.json",
        "network",
        r#"{"KnownProxies":["10.0.20.11"],"EnableUPnP":false}"#,
    );
    let out = check(&good);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(
        stdout.contains("2 spec field(s) in 1 spec(s) match"),
        "{stdout}"
    );

    // A branding field is not a network field: the key decides the component.
    let wrong_key = spec("wrong-key.json", "network", r#"{"LoginDisclaimer":"x"}"#);
    let out = check(&wrong_key);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("NetworkConfiguration.LoginDisclaimer: NetworkConfiguration has no property LoginDisclaimer"),
        "{stderr}"
    );
}

/// The account maps of a `user-policies` spec are checked field by field
/// against `UserPolicy`, and a finding names the account it came from.
#[test]
fn schema_check_counts_both_account_maps_and_names_the_account_of_a_finding() {
    let openapi = format!(
        "{}/openapi/jellyfin-10.11.11.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let dir = tempfile::tempdir().unwrap();
    let spec = |name: &str, desired: &str| {
        let path = dir.path().join(name);
        std::fs::write(
            &path,
            format!(
                r#"{{"service":"jellyfin","base_url":"http://localhost:8096","api_key_credential":"k","task":"user-policies","desired":{desired}}}"#
            ),
        )
        .unwrap();
        path
    };
    let check = |path: &std::path::Path| {
        converge()
            .args([
                "schema-check",
                "--service",
                "jellyfin",
                "--openapi",
                &openapi,
                "--spec",
            ])
            .arg(path)
            .output()
            .unwrap()
    };

    let good = spec(
        "good.json",
        r#"{"all":{"EnableAllFolders":false,"EnableLiveTvAccess":true,"IsAdministrator":false},
            "accounts":{"konto1":{"IsAdministrator":true}}}"#,
    );
    let out = check(&good);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(
        stdout.contains("4 spec field(s) in 1 spec(s) match"),
        "{stdout}"
    );

    let misspelt = spec(
        "misspelt.json",
        r#"{"accounts":{"konto1":{"IsAdminstrator":true}}}"#,
    );
    let out = check(&misspelt);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(
            "account konto1: UserPolicy.IsAdminstrator: UserPolicy has no property IsAdminstrator"
        ),
        "{stderr}"
    );

    // A display-preferences spec has no field a schema knows -- but its
    // endpoints are checked, so the run still says something.
    let prefs = dir.path().join("prefs.json");
    std::fs::write(
        &prefs,
        r#"{"service":"jellyfin","base_url":"http://localhost:8096","api_key_credential":"k","task":"display-preferences","desired":{"client":"emby","all":{"custom_prefs":{"livetv-favoritechannelsattop":"false"}}}}"#,
    )
    .unwrap();
    let out = check(&prefs);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(
        stdout.contains("jellyfin: 16 endpoints and their wire types match"),
        "{stdout}"
    );
}

#[test]
fn schema_check_fails_for_a_renamed_field() {
    let dir = tempfile::tempdir().unwrap();
    let original = std::fs::read_to_string(format!(
        "{}/openapi/radarr-6.3.0.10514.json",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let file = dir.path().join("renamed.json");
    std::fs::write(&file, original.replace("\"minSize\"", "\"minimumSize\"")).unwrap();
    let out = converge()
        .args(["schema-check", "--service", "radarr", "--openapi"])
        .arg(&file)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr)
        .contains("QualityDefinitionResource.minSize: not in the OpenAPI description"));
}

#[test]
fn plan_is_0_when_equal_and_2_when_it_differs() {
    let server = radarr();
    let dir = tempfile::tempdir().unwrap();

    let path = write_spec(dir.path(), "same.json", &server.base_url(), DESIRED);
    let out = converge()
        .arg("plan")
        .arg(&path)
        .env("CREDENTIALS_DIRECTORY", dir.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("radarr quality-definitions: service version 6.3.0.10514"),
        "{stdout}"
    );
    assert!(
        stdout.contains("radarr quality-definitions: unchanged"),
        "{stdout}"
    );

    let mut changed: serde_json::Value = serde_json::from_str(DESIRED).unwrap();
    changed["Bluray-1080p"]["min"] = 35.into();
    let path = write_spec(
        dir.path(),
        "changed.json",
        &server.base_url(),
        &changed.to_string(),
    );
    let out = converge()
        .arg("plan")
        .arg(&path)
        .env("CREDENTIALS_DIRECTORY", dir.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(2), "{stdout}");
    assert!(
        stdout.contains("would change Bluray-1080p: min 12.5 -> 35"),
        "{stdout}"
    );
    assert!(
        server.requests().iter().all(|r| r.method == "GET"),
        "plan must not write"
    );
}

#[test]
fn a_bad_spec_fails_but_the_next_spec_still_runs() {
    let server = radarr();
    let dir = tempfile::tempdir().unwrap();
    let bad = dir.path().join("bad.json");
    std::fs::write(&bad, r#"{"service":"radarr"}"#).unwrap();
    let good = write_spec(dir.path(), "good.json", &server.base_url(), DESIRED);
    let out = converge()
        .arg("apply")
        .arg(&bad)
        .arg(&good)
        .env("CREDENTIALS_DIRECTORY", dir.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("bad.json"));
    assert!(String::from_utf8_lossy(&out.stdout).contains("unchanged"));
}

#[test]
fn a_missing_credential_directory_is_reported_by_name() {
    let server = radarr();
    let dir = tempfile::tempdir().unwrap();
    let path = write_spec(dir.path(), "s.json", &server.base_url(), DESIRED);
    let out = converge()
        .arg("apply")
        .arg(&path)
        .env_remove("CREDENTIALS_DIRECTORY")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("credential radarr-api-key: CREDENTIALS_DIRECTORY is not set"),
        "{stderr}"
    );
    assert!(server.requests().is_empty(), "no request without a key");
}

#[test]
fn quality_profiles_plan_over_http() {
    const PROFILES: &str = include_str!("fixtures/sonarr-4.0.19.2979/qualityprofile.json");
    const SONARR_STATUS: &str = include_str!("fixtures/sonarr-4.0.19.2979/system-status.json");
    let mut off: serde_json::Value = serde_json::from_str(PROFILES).unwrap();
    for item in off[0]["items"].as_array_mut().unwrap() {
        if item["quality"]["name"] == "Unknown" {
            item["allowed"] = false.into();
        }
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("sonarr-api-key"), "k\n").unwrap();
    for (body, code, expect) in [
        (
            PROFILES.to_string(),
            0,
            "sonarr quality-profiles: unchanged",
        ),
        (
            off.to_string(),
            2,
            "would change Dual Language, sonst Deutsch (1080p): Unknown: allowed false -> true",
        ),
    ] {
        let server = Server::start(vec![
            ("GET", "/api/v3/system/status", 200, SONARR_STATUS.into()),
            ("GET", "/api/v3/qualityprofile", 200, body),
        ]);
        let spec = dir.path().join("profiles.json");
        std::fs::write(
            &spec,
            format!(
                r#"{{"service":"sonarr","base_url":"{}","api_key_credential":"sonarr-api-key","task":"quality-profiles","desired":{{"allow_in_every_profile":["Unknown"]}}}}"#,
                server.base_url()
            ),
        )
        .unwrap();
        let out = converge()
            .arg("plan")
            .arg(&spec)
            .env("CREDENTIALS_DIRECTORY", dir.path())
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert_eq!(
            out.status.code(),
            Some(code),
            "{stdout}{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(stdout.contains(expect), "{stdout}");
    }
}

#[test]
fn a_plugin_secret_never_reaches_the_output_even_when_the_write_fails() {
    const INFO: &str = include_str!("fixtures/jellyfin-10.11.11/system-info.json");
    const PLUGINS: &str = include_str!("fixtures/jellyfin-10.11.11/plugins.json");
    const OSCARS: &str = "c531afa3de204055aca5a7cc43adf783";
    let config = std::fs::read_to_string(format!(
        "{}/tests/fixtures/jellyfin-10.11.11/plugins/{OSCARS}.json",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let config_path = format!("/Plugins/{OSCARS}/Configuration");
    let secret = "very-secret-omdb-key-4711";

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("jellyfin-api-key"), "k\n").unwrap();
    std::fs::write(dir.path().join("jellyfin-omdb-key"), format!("{secret}\n")).unwrap();

    // Leaked into `routes` as a &'static str: the server outlives the test body.
    let config_path: &'static str = Box::leak(config_path.into_boxed_str());
    let server = Server::start(vec![
        ("GET", "/System/Info", 200, INFO.into()),
        ("GET", "/Plugins", 200, PLUGINS.into()),
        ("GET", config_path, 200, config),
        // The write is refused, with a body that would echo whatever it got.
        (
            "POST",
            config_path,
            400,
            r#"[{"propertyName":"OmdbApiKey","errorMessage":"rejected"}]"#.to_string(),
        ),
    ]);
    let spec = dir.path().join("plugins.json");
    std::fs::write(
        &spec,
        format!(
            r#"{{"service":"jellyfin","base_url":"{}","api_key_credential":"jellyfin-api-key","task":"plugin-configurations","desired":{{"{OSCARS}":{{"name":"Jellyfin Oscars","secrets":{{"OmdbApiKey":"jellyfin-omdb-key"}}}}}}}}"#,
            server.base_url()
        ),
    )
    .unwrap();

    for mode in ["plan", "apply"] {
        let out = converge()
            .arg(mode)
            .arg(&spec)
            .env("CREDENTIALS_DIRECTORY", dir.path())
            .output()
            .unwrap();
        let all = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(!all.contains(secret), "{mode}: {all}");
        assert!(
            !all.contains("<masked>"),
            "{mode}: the old value is not shown either: {all}"
        );
        if mode == "plan" {
            assert!(
                all.contains("OmdbApiKey (hidden) -> (hidden) from its credential"),
                "{all}"
            );
        } else {
            // A failed apply stops before printing changes; what it prints is
            // the error, and the error carries no value either.
            assert_eq!(out.status.code(), Some(1), "{all}");
            assert!(
                all.contains("answered HTTP 400: OmdbApiKey: rejected"),
                "{all}"
            );
        }
    }
    // The key did travel -- in the body of the refused POST, nowhere else.
    let posted = server
        .requests()
        .into_iter()
        .find(|r| r.method == "POST")
        .unwrap();
    assert!(posted.body.contains(secret));
    assert!(!posted.path.contains(secret) && !posted.headers.contains(secret));
}

fn trailarr_openapi() -> String {
    format!(
        "{}/openapi/trailarr-0.11.5.json",
        env!("CARGO_MANIFEST_DIR")
    )
}

const TRAILARR_CONNECTIONS: &str = r#"{"connections":{"Radarr":{"set":{"arr_type":"radarr","url":"http://127.0.0.1:7878","monitor_new_media":true,"external_url":"","path_mappings":[]},"api_key_credential":"radarr-api-key"},"Sonarr":{"set":{"arr_type":"sonarr","url":"http://127.0.0.1:8989","monitor_new_media":true,"external_url":"","path_mappings":[]},"api_key_credential":"sonarr-api-key"}}}"#;
const TRAILARR_PROFILES: &str = r#"{"set":{"search_query":"{title} {year} deutscher trailer","always_search":true,"exclude_words":"reaction,review","file_format":"mp4","video_format":"h264","audio_format":"aac"}}"#;

fn trailarr_spec(dir: &Path, file: &str, base: &str, task: &str, desired: &str) -> PathBuf {
    let path = dir.join(file);
    std::fs::write(
        &path,
        format!(
            r#"{{"service":"trailarr","base_url":"{base}","api_key_credential":"trailarr-api-key","task":"{task}","desired":{desired}}}"#
        ),
    )
    .unwrap();
    path
}

fn schema_check(service: &str, openapi: Option<&str>, specs: &[&Path]) -> std::process::Output {
    let mut command = converge();
    command.args(["schema-check", "--service", service]);
    if let Some(openapi) = openapi {
        command.args(["--openapi", openapi]);
    }
    for spec in specs {
        command.arg("--spec").arg(spec);
    }
    command.output().unwrap()
}

#[test]
fn schema_check_passes_the_hosts_trailarr_specs_and_rejects_monitor() {
    let dir = tempfile::tempdir().unwrap();
    let base = "http://localhost:7889";
    let connections = trailarr_spec(
        dir.path(),
        "c.json",
        base,
        "connections",
        TRAILARR_CONNECTIONS,
    );
    let profiles = trailarr_spec(
        dir.path(),
        "p.json",
        base,
        "trailer-profiles",
        TRAILARR_PROFILES,
    );
    let out = schema_check(
        "trailarr",
        Some(&trailarr_openapi()),
        &[&connections, &profiles],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("trailarr: 7 endpoints and their wire types match"),
        "{stdout}"
    );
    assert!(
        stdout.contains("16 spec field(s) in 2 spec(s) match"),
        "{stdout}"
    );

    // The field the host's shell unit sent; Trailarr 0.11.5 has
    // `monitor_new_media` and ignored `monitor` without a word.
    let old_shell = TRAILARR_CONNECTIONS.replace("monitor_new_media", "monitor");
    let bad = trailarr_spec(dir.path(), "bad.json", base, "connections", &old_shell);
    let out = schema_check("trailarr", Some(&trailarr_openapi()), &[&bad]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    for component in ["ConnectionCreate", "ConnectionUpdate"] {
        assert!(
            stderr.contains(&format!(
                "connection Radarr: {component}.monitor: {component} has no property monitor"
            )),
            "{stderr}"
        );
    }
}

/// bindery's `indexers` is a list of names, and a spec that would switch
/// everything off -- an empty list, or a name given twice -- is refused
/// before the service is ever called (design §23).
#[test]
fn the_bindery_indexer_switch_takes_a_list_of_names_and_refuses_an_empty_one() {
    let dir = tempfile::tempdir().unwrap();
    let spec = |file: &str, desired: &str| {
        let path = dir.path().join(file);
        std::fs::write(
            &path,
            format!(
                r#"{{"service":"bindery","base_url":"http://127.0.0.1:8787","api_key_credential":"bindery-api-key","task":"indexers","desired":{desired}}}"#
            ),
        )
        .unwrap();
        path
    };
    let good = spec(
        "good.json",
        r#"{"enabled":["MyAnonamouse","AudioBookBay"]}"#,
    );
    let out = schema_check("bindery", None, &[&good]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    for (file, desired, needle) in [
        ("empty.json", r#"{"enabled":[]}"#, "names no indexer"),
        ("twice.json", r#"{"enabled":["A","A"]}"#, "names A twice"),
        (
            "map.json",
            r#"{"indexers":{"A":{"set":{"enabled":true}}}}"#,
            "desired",
        ),
    ] {
        let out = schema_check("bindery", None, &[&spec(file, desired)]);
        assert_eq!(out.status.code(), Some(1), "{file}");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains(needle),
            "{file}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn schema_check_for_bindery_validates_its_four_tasks_without_an_openapi_file() {
    let dir = tempfile::tempdir().unwrap();
    let spec = |file: &str, task: &str, desired: &str| {
        let path = dir.path().join(file);
        std::fs::write(
            &path,
            format!(
                r#"{{"service":"bindery","base_url":"http://127.0.0.1:8787","api_key_credential":"bindery-api-key","task":"{task}","desired":{desired}}}"#
            ),
        )
        .unwrap();
        path
    };
    let clients = spec(
        "clients.json",
        "download-clients",
        r#"{"clients":{"sabnzbd":{"set":{"host":"10.0.10.10","port":8080},"secret_fields":{"apiKey":"sabnzbd-api-key"}}}}"#,
    );
    let instances = spec(
        "instances.json",
        "prowlarr-instances",
        r#"{"instances":{"media-01":{"set":{"syncOnStartup":true},"secret_fields":{"apiKey":"prowlarr-api-key"}}}}"#,
    );
    let folders = spec(
        "folders.json",
        "root-folders",
        r#"{"folders":{"/tank/data/media/books":{}}}"#,
    );
    let settings = spec(
        "settings.json",
        "settings",
        r#"{"settings":{"import.mode":"copy"}}"#,
    );
    let out = schema_check(
        "bindery",
        None,
        &[&clients, &instances, &folders, &settings],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("bindery: no OpenAPI description exists; 4 spec(s) valid"),
        "{stdout}"
    );

    // A secret bindery does not keep write-only, a root folder with fields,
    // and an ntfy spec handed to bindery are errors.
    let token = spec(
        "token.json",
        "download-clients",
        r#"{"clients":{"x":{"secret_fields":{"token":"t"}}}}"#,
    );
    let folder_fields = spec(
        "folder-fields.json",
        "root-folders",
        r#"{"folders":{"/b":{"set":{"x":1}}}}"#,
    );
    for (path, needle) in [
        (&token, "not one of bindery's write-only fields"),
        (&folder_fields, "cannot update a root folder"),
    ] {
        let out = schema_check("bindery", None, &[path]);
        assert_eq!(out.status.code(), Some(1));
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains(needle), "{stderr}");
    }
    let out = schema_check("bindery", Some(&trailarr_openapi()), &[&clients]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("bindery publishes no OpenAPI description")
    );
}

#[test]
fn schema_check_for_seerr_validates_its_five_tasks_without_an_openapi_file() {
    let dir = tempfile::tempdir().unwrap();
    let spec = |file: &str, task: &str, desired: &str| {
        let path = dir.path().join(file);
        std::fs::write(
            &path,
            format!(
                r#"{{"service":"seerr","base_url":"http://127.0.0.1:5055","api_key_credential":"seerr-api-key","task":"{task}","desired":{desired}}}"#
            ),
        )
        .unwrap();
        path
    };
    let main = spec(
        "main.json",
        "main",
        r#"{"set":{"locale":"de","newPlexLogin":true}}"#,
    );
    let jellyfin = spec(
        "jellyfin.json",
        "jellyfin",
        r#"{"set":{"ip":"10.0.30.10","port":8096},"api_key_credential":"jellyfin-api-key-seerr","libraries":["Filme","Serien"]}"#,
    );
    let servers = r#"{"servers":{"Radarr":{"set":{"hostname":"10.0.10.10","port":7878},"api_key_credential":"radarr-api-key","profile":"HD","root_folder":"/tank/data/media/movies"}}}"#;
    let radarr = spec("radarr.json", "radarr-servers", servers);
    let sonarr = spec(
        "sonarr.json",
        "sonarr-servers",
        &servers.replace("Radarr", "Sonarr"),
    );
    let webhook = spec(
        "webhook.json",
        "webhook",
        r#"{"set":{"enabled":true,"types":24},"payload":{"subject":"{{subject}}"},"headers":{"X-Webhook-Token":"signal-webhook-marke"}}"#,
    );
    let out = schema_check(
        "seerr",
        None,
        &[&main, &jellyfin, &radarr, &sonarr, &webhook],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("seerr: no OpenAPI description exists; 5 spec(s) valid"),
        "{stdout}"
    );

    // A resolved field in set, a library list that would disable every
    // library, and the payload set as a plain field are errors.
    let resolved = spec(
        "resolved.json",
        "radarr-servers",
        &servers.replace(r#""port":7878"#, r#""port":7878,"activeProfileId":7"#),
    );
    let no_library = spec(
        "no-library.json",
        "jellyfin",
        r#"{"set":{"ip":"10.0.30.10","port":8096},"api_key_credential":"jf","libraries":[]}"#,
    );
    let payload_as_field = spec(
        "payload-as-field.json",
        "webhook",
        r#"{"set":{"options.jsonPayload":"{}"},"payload":{"a":1}}"#,
    );
    for (path, needle) in [
        (&resolved, "activeProfileId is not set this way"),
        (&no_library, "names no library"),
        (&payload_as_field, "options.jsonPayload is not set this way"),
    ] {
        let out = schema_check("seerr", None, &[path]);
        assert_eq!(out.status.code(), Some(1));
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains(needle), "{stderr}");
    }
    let out = schema_check("seerr", Some(&trailarr_openapi()), &[&main]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("seerr publishes no OpenAPI description"));
}

fn koel_spec(dir: &Path, file: &str, base: &str, stations: &str) -> PathBuf {
    let path = dir.join(file);
    std::fs::write(
        &path,
        format!(
            r#"{{"service":"koel","base_url":"{base}","api_key_credential":"koel-token","task":"radio-stations","desired":{{"stations":{stations}}}}}"#
        ),
    )
    .unwrap();
    path
}

#[test]
fn schema_check_for_koel_validates_specs_without_an_openapi_file() {
    let dir = tempfile::tempdir().unwrap();
    let good = koel_spec(
        dir.path(),
        "good.json",
        "http://127.0.0.1:8080",
        r#"[{"name":"FSK","url":"https://streaming.fueralle.org/fsk.mp3","is_public":true,"logo_file":"/nix/store/not-read-at-build.png"}]"#,
    );
    let out = schema_check("koel", None, &[&good]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("koel: no OpenAPI description exists; 1 spec(s) valid"),
        "{stdout}"
    );
    let twice = koel_spec(
        dir.path(),
        "twice.json",
        "http://127.0.0.1:8080",
        r#"[{"name":"FSK","url":"https://a/1","is_public":true},{"name":"FSK","url":"https://a/2","is_public":true}]"#,
    );
    let out = schema_check("koel", None, &[&twice]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("names station FSK twice"));
    let out = schema_check("koel", Some(&trailarr_openapi()), &[&good]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("koel publishes no OpenAPI description"));
}

#[test]
fn koel_gets_a_bearer_token_and_json_and_neither_token_nor_logo_is_shown() {
    const RECORDED: &str = include_str!("fixtures/koel-9.11.3/radio-stations.json");
    const INVALID: &str = include_str!("fixtures/koel-9.11.3/constructed-validation-error.json");
    let token = "koel-token-7f3a9c-never-print-me";
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("koel-token"), format!("{token}\n")).unwrap();
    // A PNG signature and a mark that would show in the base64 of the logo.
    let logo = dir.path().join("logo.png");
    std::fs::write(&logo, b"\x89PNG\r\n\x1a\nlogo-mark-never-print-me").unwrap();
    // The recorded account, with include_public_media turned off: only then
    // does converge read the list as the account's own stations.
    let me = include_str!("fixtures/koel-9.11.3/me.json").replace(
        r#""include_public_media": true"#,
        r#""include_public_media": false"#,
    );
    assert!(me.contains(r#""include_public_media": false"#));
    let server = Server::start(vec![
        ("GET", "/api/me", 200, me),
        ("GET", "/api/radio/stations", 200, RECORDED.into()),
        ("POST", "/api/radio/stations", 422, INVALID.into()),
    ]);
    let stations = format!(
        r#"[{{"name":"FSK","url":"https://streaming.fueralle.org/fsk.mp3","is_public":true,"logo_file":"{}"}}]"#,
        logo.display()
    );
    let spec = koel_spec(dir.path(), "koel.json", &server.base_url(), &stations);
    let run = |mode: &str, spec: &Path| {
        let out = converge()
            .arg(mode)
            .arg(spec)
            .env("CREDENTIALS_DIRECTORY", dir.path())
            .output()
            .unwrap();
        let all = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(!all.contains(token), "{mode} shows the token: {all}");
        assert!(!all.contains("base64"), "{mode} shows the logo: {all}");
        (out.status.code(), all)
    };

    let (code, all) = run("plan", &spec);
    assert_eq!(code, Some(2), "{all}");
    assert!(
        all.contains("koel radio-stations: service version (not reported)"),
        "{all}"
    );
    assert!(
        all.contains("koel radio-stations: would change station FSK: (missing) -> (added)"),
        "{all}"
    );

    let (code, all) = run("apply", &spec);
    assert_eq!(code, Some(1), "{all}");
    assert!(
        all.contains("POST /api/radio/stations answered HTTP 422: logo: Invalid image for logo; url: The url field must be a valid URL."),
        "{all}"
    );
    let requests = server.requests();
    assert!(!requests.is_empty());
    for seen in &requests {
        let headers = seen.headers.to_ascii_lowercase();
        assert!(headers.contains("accept: application/json"), "{headers}");
        assert!(
            headers.contains(&format!("authorization: bearer {token}")),
            "{headers}"
        );
    }
    let posted = requests.iter().find(|r| r.method == "POST").unwrap();
    assert!(
        // base64 of the PNG signature and the mark after it.
        posted.body.contains(
            r#""logo":"data:image/png;base64,iVBORw0KGgpsb2dvLW1hcmstbmV2ZXItcHJpbnQtbWU=""#
        ),
        "{}",
        posted.body
    );

    // A logo file that is not there fails before any request.
    let before = server.requests().len();
    let missing = koel_spec(
        dir.path(),
        "missing.json",
        &server.base_url(),
        r#"[{"name":"FSK","url":"https://streaming.fueralle.org/fsk.mp3","is_public":true,"logo_file":"/nonexistent/logo.png"}]"#,
    );
    let (code, all) = run("plan", &missing);
    assert_eq!(code, Some(1), "{all}");
    assert!(
        all.contains("logo_file /nonexistent/logo.png cannot be read"),
        "{all}"
    );
    assert_eq!(server.requests().len(), before);

    // The account as recorded (include_public_media on): the list could hold
    // a person's public station of the same name, so apply stops before it
    // writes anything.
    let theirs = Server::start(vec![
        (
            "GET",
            "/api/me",
            200,
            include_str!("fixtures/koel-9.11.3/me.json").into(),
        ),
        (
            "GET",
            "/api/radio/stations",
            200,
            include_str!("fixtures/koel-9.11.3/constructed-radio-stations.json").into(),
        ),
        (
            "PUT",
            "/api/radio/stations/01K52Z6P7B8C9D0E1F2G3H4J5K",
            200,
            "{}".into(),
        ),
    ]);
    let spec = koel_spec(
        dir.path(),
        "theirs.json",
        &theirs.base_url(),
        r#"[{"name":"Somebody's own","url":"https://example.org/other.mp3","is_public":true}]"#,
    );
    let (code, all) = run("apply", &spec);
    assert_eq!(code, Some(1), "{all}");
    assert!(
        all.contains("koel radio-stations: error: refused: the account's preference include_public_media is on"),
        "{all}"
    );
    assert!(
        theirs.requests().iter().all(|r| r.method == "GET"),
        "{:?}",
        theirs.requests()
    );
}

#[test]
fn schema_check_for_ntfy_validates_specs_without_an_openapi_file() {
    let dir = tempfile::tempdir().unwrap();
    let text = r#"{"service":"ntfy","base_url":"http://localhost:2586","api_key_credential":"ntfy-token","task":"account-subscriptions","desired":{"base_url":"https://ntfy.rusty-vault.de","topics_credential":"ntfy-abo-topics"}}"#;
    let spec = dir.path().join("ntfy.json");
    std::fs::write(&spec, text).unwrap();
    let out = schema_check("ntfy", None, &[&spec]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(
        stdout.contains("ntfy: no OpenAPI description exists; 1 spec(s) valid"),
        "{stdout}"
    );
    assert!(
        stdout.contains("checked by recorded answers and at runtime only"),
        "{stdout}"
    );

    let out = schema_check("ntfy", Some(&trailarr_openapi()), &[&spec]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("ntfy publishes no OpenAPI description"),
        "{stderr}"
    );

    let broken = dir.path().join("broken.json");
    std::fs::write(&broken, text.replace("topics_credential", "topics")).unwrap();
    let out = schema_check("ntfy", None, &[&broken]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("broken.json"));

    // Every other service still needs the file.
    let out = schema_check("trailarr", None, &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("needs --service and --openapi"));
}

#[test]
fn trailarr_connections_plan_over_http_sends_the_key_as_x_api_key() {
    const SETTINGS: &str =
        include_str!("fixtures/trailarr-0.11.5/constructed-settings-version-only.json");
    const CONNECTIONS: &str = include_str!("fixtures/trailarr-0.11.5/connections.json");
    let server = Server::start(vec![
        ("GET", "/api/v1/settings/", 200, SETTINGS.into()),
        ("GET", "/api/v1/connections/", 200, CONNECTIONS.into()),
    ]);
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("trailarr-api-key"), "trailarr-key\n").unwrap();
    for name in ["radarr-api-key", "sonarr-api-key"] {
        std::fs::write(dir.path().join(name), "<masked>\n").unwrap();
    }
    let spec = trailarr_spec(
        dir.path(),
        "c.json",
        &server.base_url(),
        "connections",
        &TRAILARR_CONNECTIONS.replace("127.0.0.1:8989", "10.0.20.11:8989"),
    );
    let out = converge()
        .arg("plan")
        .arg(&spec)
        .env("CREDENTIALS_DIRECTORY", dir.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(2),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("trailarr connections: service version v0.11.5"),
        "{stdout}"
    );
    assert!(
        stdout.contains(
            r#"would change connection Sonarr: url "http://127.0.0.1:8989" -> "http://10.0.20.11:8989""#
        ),
        "{stdout}"
    );
    let headers = server.requests()[0].headers.to_ascii_lowercase();
    assert!(headers.contains("x-api-key: trailarr-key"), "{headers}");
}

/// The whole way from the spec's `"exactly": true` to the line a plan
/// prints, over HTTP -- and the same spec without the switch, which says the
/// connection is left alone (design §39).
#[test]
fn exactly_turns_a_connection_outside_the_spec_into_a_removal_in_the_plan() {
    const SETTINGS: &str =
        include_str!("fixtures/trailarr-0.11.5/constructed-settings-version-only.json");
    const CONNECTIONS: &str = include_str!("fixtures/trailarr-0.11.5/connections.json");
    // The spec names Radarr alone; the recording also holds Sonarr.
    let only_radarr = r#"{"connections":{"Radarr":{"set":{"arr_type":"radarr","url":"http://127.0.0.1:7878","monitor_new_media":true,"external_url":"","path_mappings":[]},"api_key_credential":"radarr-api-key"}}}"#;

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("trailarr-api-key"), "trailarr-key\n").unwrap();
    std::fs::write(dir.path().join("radarr-api-key"), "<masked>\n").unwrap();

    for (exactly, expected_code) in [(true, 2), (false, 0)] {
        let server = Server::start(vec![
            ("GET", "/api/v1/settings/", 200, SETTINGS.into()),
            ("GET", "/api/v1/connections/", 200, CONNECTIONS.into()),
        ]);
        let path = dir.path().join(format!("exactly-{exactly}.json"));
        std::fs::write(
            &path,
            format!(
                r#"{{"service":"trailarr","base_url":"{}","api_key_credential":"trailarr-api-key","task":"connections","exactly":{exactly},"desired":{only_radarr}}}"#,
                server.base_url()
            ),
        )
        .unwrap();
        let out = converge()
            .arg("plan")
            .arg(&path)
            .env("CREDENTIALS_DIRECTORY", dir.path())
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert_eq!(
            out.status.code(),
            Some(expected_code),
            "{stdout}{}",
            String::from_utf8_lossy(&out.stderr)
        );
        if exactly {
            assert!(
                stdout.contains(
                    "trailarr connections: would change connection Sonarr: (present) -> (removed)"
                ),
                "{stdout}"
            );
            assert!(stdout.contains("1 field(s) differ"), "{stdout}");
        } else {
            assert!(
                stdout.contains("trailarr connections: note: not in the spec: connection Sonarr"),
                "{stdout}"
            );
            assert!(
                stdout.contains("trailarr connections: unchanged"),
                "{stdout}"
            );
            assert!(!stdout.contains("removed"), "{stdout}");
        }
        // Neither run sends anything but the two reads.
        assert_eq!(server.requests().len(), 2, "{stdout}");
    }
}

#[test]
fn an_ntfy_topic_never_reaches_the_output_even_when_the_write_fails() {
    const HEALTH: &str = include_str!("fixtures/ntfy-2.26.0/constructed-health.json");
    const ACCOUNT: &str = include_str!("fixtures/ntfy-2.26.0/account.json");
    let new_topic = "very-secret-topic-4711";
    let topics = ["<masked-topic-1>", "<masked-topic-2>", new_topic];

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("ntfy-token"), "tk_ntfy-token-value\n").unwrap();
    let server = Server::start(vec![
        ("GET", "/v1/health", 200, HEALTH.into()),
        ("GET", "/v1/account", 200, ACCOUNT.into()),
        // Refused, with a body that echoes the topic it got.
        (
            "POST",
            "/v1/account/subscription",
            400,
            format!(r#"[{{"propertyName":"topic","errorMessage":"invalid topic {new_topic}"}}]"#),
        ),
    ]);
    let spec = dir.path().join("ntfy.json");
    std::fs::write(
        &spec,
        format!(
            r#"{{"service":"ntfy","base_url":"{}","api_key_credential":"ntfy-token","task":"account-subscriptions","desired":{{"base_url":"https://ntfy.rusty-vault.de","topics_credential":"ntfy-abo-topics"}}}}"#,
            server.base_url()
        ),
    )
    .unwrap();

    let run = |mode: &str| {
        let out = converge()
            .arg(mode)
            .arg(&spec)
            .env("CREDENTIALS_DIRECTORY", dir.path())
            .output()
            .unwrap();
        let all = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        for topic in topics {
            assert!(!all.contains(topic), "{mode} shows {topic}: {all}");
        }
        (out.status.code(), all)
    };

    // The recorded state: both topics already subscribed.
    std::fs::write(
        dir.path().join("ntfy-abo-topics"),
        "<masked-topic-1>\n\n<masked-topic-2>\n",
    )
    .unwrap();
    let (code, all) = run("plan");
    assert_eq!(code, Some(0), "{all}");
    assert!(
        all.contains("ntfy account-subscriptions: service version (not reported)"),
        "{all}"
    );
    assert!(
        all.contains("ntfy account-subscriptions: unchanged"),
        "{all}"
    );

    // A third topic.
    std::fs::write(dir.path().join("ntfy-abo-topics"), topics.join("\n")).unwrap();
    let (code, all) = run("plan");
    assert_eq!(code, Some(2), "{all}");
    assert!(
        all.contains("would change ntfy account: subscription 3 (missing) -> (added)"),
        "{all}"
    );
    let (code, all) = run("apply");
    assert_eq!(code, Some(1), "{all}");
    assert!(
        all.contains("POST /v1/account/subscription answered HTTP 400"),
        "{all}"
    );

    let requests = server.requests();
    let posted = requests.iter().find(|r| r.method == "POST").unwrap();
    assert!(
        posted.body.contains(new_topic),
        "the topic travels in the body"
    );
    assert!(!posted.path.contains(new_topic) && !posted.headers.contains(new_topic));
    let account_reads: Vec<_> = requests
        .iter()
        .filter(|r| r.path == "/v1/account")
        .collect();
    assert!(!account_reads.is_empty());
    assert!(
        account_reads
            .iter()
            .all(|r| r.headers.contains("Bearer tk_ntfy-token-value")),
        "the token travels as a bearer token"
    );
}

fn lidarr_spec(dir: &Path, file: &str, task: &str, desired: &str) -> PathBuf {
    let path = dir.join(file);
    std::fs::write(
        &path,
        format!(
            r#"{{"service":"lidarr","base_url":"http://localhost:8686","api_key_credential":"lidarr-api-key","task":"{task}","desired":{desired}}}"#
        ),
    )
    .unwrap();
    path
}

#[test]
fn schema_check_passes_lidarr_specs_and_names_a_provider_field_name() {
    let openapi = format!(
        "{}/openapi/lidarr-3.1.0.4875.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let dir = tempfile::tempdir().unwrap();
    let media = lidarr_spec(
        dir.path(),
        "m.json",
        "media-management",
        r#"{"copyUsingHardlinks":false,"allowFingerprinting":"newFiles"}"#,
    );
    let folders = lidarr_spec(
        dir.path(),
        "f.json",
        "root-folders",
        r#"{"folders":{"/tank/data/media/music":{"set":{"name":"Musik","defaultMonitorOption":"all"},"profiles":{"defaultQualityProfileId":"Standard"}}}}"#,
    );
    let out = schema_check("lidarr", Some(&openapi), &[&media, &folders]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("lidarr: 26 endpoints and their wire types match"),
        "{stdout}"
    );
    // path, name, defaultMonitorOption, defaultQualityProfileId; two fields.
    assert!(
        stdout.contains("6 spec field(s) in 2 spec(s) match"),
        "{stdout}"
    );

    // The name tofu's provider used; the API calls it copyUsingHardlinks.
    let tofu_name = lidarr_spec(
        dir.path(),
        "t.json",
        "media-management",
        r#"{"hardlinks_copy":false}"#,
    );
    let out = schema_check("lidarr", Some(&openapi), &[&tofu_name]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(
            "MediaManagementConfigResource.hardlinks_copy: MediaManagementConfigResource has no property hardlinks_copy"
        ),
        "{stderr}"
    );
}

const LIDARR_STATUS: &str = include_str!("fixtures/lidarr-3.1.0.4875/system-status.json");
const LIDARR_QUALITY: &str = include_str!("fixtures/lidarr-3.1.0.4875/qualityprofile.json");
const LIDARR_METADATA: &str = include_str!("fixtures/lidarr-3.1.0.4875/metadataprofile.json");

/// The profile `Standard` as Lidarr holds it, in the form a host writes into
/// its spec.
const LIDARR_STANDARD: &str = r#"{"profiles":{"Standard":{"upgrade_allowed":false,
    "cutoff":"Low Quality Lossy",
    "allowed":["MP3-192","OGG Vorbis Q6","AAC-192","WMA","MP3-224","OGG Vorbis Q7",
               "MP3-VBR-V2","MP3-256","OGG Vorbis Q8","AAC-256","MP3-VBR-V0","AAC-VBR",
               "MP3-320","OGG Vorbis Q9","AAC-320","OGG Vorbis Q10"]}}}"#;
const LIDARR_META_STANDARD: &str = r#"{"profiles":{"Standard":{
    "primary_album_types":["Album"],"secondary_album_types":["Studio"],
    "release_statuses":["Official"]}}}"#;

/// Both profile tasks are typed, so `schema-check` compares their wire types
/// against Lidarr's description and counts no spec field.
#[test]
fn schema_check_accepts_the_lidarr_profile_specs_and_refuses_an_unknown_key() {
    let openapi = format!(
        "{}/openapi/lidarr-3.1.0.4875.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let dir = tempfile::tempdir().unwrap();
    let quality = lidarr_spec(dir.path(), "q.json", "quality-profiles", LIDARR_STANDARD);
    let metadata = lidarr_spec(
        dir.path(),
        "m.json",
        "metadata-profiles",
        LIDARR_META_STANDARD,
    );
    let out = schema_check("lidarr", Some(&openapi), &[&quality, &metadata]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("lidarr: 26 endpoints and their wire types match"),
        "{stdout}"
    );

    // `cutoff_quality` is not a key of this spec, and a misspelt key is an
    // error rather than something ignored.
    let misspelt = lidarr_spec(
        dir.path(),
        "bad.json",
        "quality-profiles",
        &LIDARR_STANDARD.replace(r#""cutoff""#, r#""cutoff_quality""#),
    );
    let out = schema_check("lidarr", Some(&openapi), &[&misspelt]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("cutoff_quality"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `plan` against the recorded profiles: the spec written from them changes
/// nothing, and one quality taken out is one named change.
#[test]
fn a_lidarr_profile_plan_is_0_when_it_matches_the_recording_and_2_when_it_does_not() {
    let server = Server::start(vec![
        ("GET", "/api/v1/system/status", 200, LIDARR_STATUS.into()),
        ("GET", "/api/v1/qualityprofile", 200, LIDARR_QUALITY.into()),
        (
            "GET",
            "/api/v1/metadataprofile",
            200,
            LIDARR_METADATA.into(),
        ),
    ]);
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("lidarr-api-key"), "k\n").unwrap();
    let spec = |file: &str, task: &str, desired: &str| {
        let path = dir.path().join(file);
        std::fs::write(
            &path,
            format!(
                r#"{{"service":"lidarr","base_url":"{}","api_key_credential":"lidarr-api-key","task":"{task}","desired":{desired}}}"#,
                server.base_url()
            ),
        )
        .unwrap();
        path
    };
    let plan = |path: &Path| {
        let out = converge()
            .arg("plan")
            .arg(path)
            .env("CREDENTIALS_DIRECTORY", dir.path())
            .output()
            .unwrap();
        (
            out.status.code(),
            format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            ),
        )
    };

    let (code, all) = plan(&spec("q.json", "quality-profiles", LIDARR_STANDARD));
    assert_eq!(code, Some(0), "{all}");
    assert!(
        all.contains("lidarr quality-profiles: service version 3.1.0.4875"),
        "{all}"
    );
    assert!(all.contains("lidarr quality-profiles: unchanged"), "{all}");
    assert!(all.contains("note: not in the spec: profile Any"), "{all}");

    let (code, all) = plan(&spec("m.json", "metadata-profiles", LIDARR_META_STANDARD));
    assert_eq!(code, Some(0), "{all}");
    assert!(all.contains("lidarr metadata-profiles: unchanged"), "{all}");

    let (code, all) = plan(&spec(
        "less.json",
        "quality-profiles",
        &LIDARR_STANDARD.replace(r#""MP3-320","#, ""),
    ));
    assert_eq!(code, Some(2), "{all}");
    assert!(
        all.contains(
            "lidarr quality-profiles: would change profile Standard: items.MP3-320.allowed true -> false"
        ),
        "{all}"
    );

    // A quality no profile has fails before anything is written, naming it.
    let (code, all) = plan(&spec(
        "unknown.json",
        "quality-profiles",
        &LIDARR_STANDARD.replace(r#""MP3-320""#, r#""MP3-321""#),
    ));
    assert_eq!(code, Some(1), "{all}");
    assert!(
        all.contains(r#"profile Standard: it has no quality "MP3-321""#),
        "{all}"
    );

    // Neither task takes `exactly`: converge removes no Lidarr profile.
    let path = dir.path().join("exactly.json");
    std::fs::write(
        &path,
        format!(
            r#"{{"service":"lidarr","base_url":"{}","api_key_credential":"lidarr-api-key","task":"metadata-profiles","exactly":true,"desired":{LIDARR_META_STANDARD}}}"#,
            server.base_url()
        ),
    )
    .unwrap();
    let (code, all) = plan(&path);
    assert_eq!(code, Some(1), "{all}");
    assert!(
        all.contains("task metadata-profiles does not support exactly"),
        "{all}"
    );
}

const PROWLARR_STATUS: &str = include_str!("fixtures/prowlarr-2.5.2.5491/system-status.json");
const PROWLARR_CLIENTS: &str = include_str!("fixtures/prowlarr-2.5.2.5491/downloadclient.json");

#[test]
fn a_provider_password_travels_only_in_the_put_body_and_never_in_output() {
    let server = Server::start(vec![
        ("GET", "/api/v1/system/status", 200, PROWLARR_STATUS.into()),
        (
            "GET",
            "/api/v1/downloadclient",
            200,
            PROWLARR_CLIENTS.into(),
        ),
        (
            "PUT",
            "/api/v1/downloadclient/2?forceSave=true",
            202,
            String::new(),
        ),
    ]);
    let dir = tempfile::tempdir().unwrap();
    let password = "qb-pass-5d1e-must-not-leak";
    std::fs::write(dir.path().join("prowlarr-api-key"), "k\n").unwrap();
    std::fs::write(
        dir.path().join("qbittorrent-password"),
        format!("{password}\n"),
    )
    .unwrap();
    let spec = dir.path().join("p.json");
    std::fs::write(
        &spec,
        format!(
            r#"{{"service":"prowlarr","base_url":"{}","api_key_credential":"prowlarr-api-key","task":"download-clients","desired":{{"providers":{{"qBittorrent":{{"implementation":"QBittorrent","set":{{"enable":true}},"fields":{{"host":"10.0.10.11","port":8080,"category":"prowlarr"}},"secret_fields":{{"password":"qbittorrent-password"}}}}}}}}}}"#,
            server.base_url()
        ),
    )
    .unwrap();
    let run = |command: &str| {
        let out = converge()
            .arg(command)
            .arg(&spec)
            .env("CREDENTIALS_DIRECTORY", dir.path())
            .output()
            .unwrap();
        let all = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            !all.contains(password),
            "the password is in the output: {all}"
        );
        (out.status.code(), all)
    };

    let (code, all) = run("plan");
    assert_eq!(code, Some(0), "{all}");
    assert!(
        all.contains("prowlarr download-clients: unchanged"),
        "{all}"
    );
    assert!(
        all.contains("note: not in the spec: download client SABnzbd"),
        "{all}"
    );
    assert!(
        server.requests().iter().all(|r| r.method == "GET"),
        "plan hands nothing over"
    );

    let (code, all) = run("apply");
    assert_eq!(code, Some(0), "{all}");
    assert!(
        all.contains("prowlarr download-clients: download client qBittorrent: password handed over from credentials (hidden; the service compares)"),
        "{all}"
    );
    let requests = server.requests();
    let puts: Vec<_> = requests.iter().filter(|r| r.method == "PUT").collect();
    assert_eq!(
        puts.len(),
        1,
        "one hand-over, no write: nothing visible differs"
    );
    assert!(puts[0].body.contains(password));
    assert!(requests
        .iter()
        .all(|r| !r.path.contains(password) && !r.headers.contains(password)));
}

#[test]
fn schema_check_passes_a_prowlarr_applications_spec_and_rejects_an_own_field() {
    let openapi = format!(
        "{}/openapi/prowlarr-2.5.2.5491.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let dir = tempfile::tempdir().unwrap();
    let write = |file: &str, set: &str| {
        let path = dir.path().join(file);
        std::fs::write(
            &path,
            format!(
                r#"{{"service":"prowlarr","base_url":"http://localhost:9696","api_key_credential":"k","task":"applications","desired":{{"providers":{{"Radarr":{{"implementation":"Radarr","set":{set},"fields":{{"syncCategories":[2000]}},"secret_fields":{{"apiKey":"radarr-api-key"}}}}}}}}}}"#
            ),
        )
        .unwrap();
        path
    };
    let good = write("good.json", r#"{"syncLevel":"fullSync"}"#);
    let out = schema_check("prowlarr", Some(&openapi), &[&good]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("prowlarr: 34 endpoints and their wire types match"),
        "{stdout}"
    );
    assert!(
        stdout.contains("1 spec field(s) in 1 spec(s) match"),
        "{stdout}"
    );

    // Prowlarr 2.5.2's description lacks `enable` on ApplicationResource,
    // although the running service answers with it. The check follows the
    // description, so a spec cannot set it -- found writing this test.
    let enable = write("enable.json", r#"{"enable":true}"#);
    let out = schema_check("prowlarr", Some(&openapi), &[&enable]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr)
        .contains("ApplicationResource.enable: ApplicationResource has no property enable"));

    let bad = write("bad.json", r#"{"syncLevel":"everything"}"#);
    let out = schema_check("prowlarr", Some(&openapi), &[&bad]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(
            "application Radarr: ApplicationResource.syncLevel: \"everything\" is not one of"
        ),
        "{stderr}"
    );
}

fn suggestarr_spec(dir: &Path, file: &str, desired: &str) -> PathBuf {
    let path = dir.join(file);
    std::fs::write(
        &path,
        format!(
            r#"{{"service":"suggestarr","base_url":"http://127.0.0.1:5000","api_key_credential":"suggestarr-converge-passwort","task":"configuration","desired":{desired}}}"#
        ),
    )
    .unwrap();
    path
}

#[test]
fn schema_check_for_suggestarr_validates_specs_without_an_openapi_file() {
    let dir = tempfile::tempdir().unwrap();
    let good = suggestarr_spec(
        dir.path(),
        "good.json",
        r#"{"username":"converge","set":{"FILTER_RATING_SOURCE":"both"},"secrets":{"OMDB_API_KEY":"omdb-api-key"},"jellyfin_libraries":{"exclude_collection_types":["homevideos"]}}"#,
    );
    let out = schema_check("suggestarr", None, &[&good]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("suggestarr: no OpenAPI description exists; 1 spec(s) valid"),
        "{stdout}"
    );

    // A key in `set` instead of `secrets` would put it in the Nix store.
    let secret_in_set = suggestarr_spec(
        dir.path(),
        "secret-in-set.json",
        r#"{"username":"converge","set":{"OMDB_API_KEY":"abc"}}"#,
    );
    let out = schema_check("suggestarr", None, &[&secret_in_set]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("desired.secrets"));

    let out = schema_check("suggestarr", Some(&trailarr_openapi()), &[&good]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr)
        .contains("suggestarr publishes no OpenAPI description"));
}

#[test]
fn schema_check_checks_dispatcharr_entries_against_create_and_update() {
    let openapi = format!(
        "{}/openapi/dispatcharr-0.31.0.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let dir = tempfile::tempdir().unwrap();
    let spec = |name: &str, task: &str, desired: &str| {
        let path = dir.path().join(name);
        std::fs::write(
            &path,
            format!(
                r#"{{"service":"dispatcharr","base_url":"http://localhost:9191","api_key_credential":"k","task":"{task}","desired":{desired}}}"#
            ),
        )
        .unwrap();
        path
    };
    let check = |paths: &[&std::path::Path]| {
        let mut cmd = converge();
        cmd.args([
            "schema-check",
            "--service",
            "dispatcharr",
            "--openapi",
            &openapi,
        ]);
        for p in paths {
            cmd.arg("--spec").arg(p);
        }
        cmd.output().unwrap()
    };

    let accounts = spec(
        "accounts.json",
        "m3u-accounts",
        r#"{"username":"c","accounts":{"A":{"file_path":"/m3u/a.m3u","refresh_interval":24}}}"#,
    );
    let groups = spec(
        "groups.json",
        "m3u-groups",
        r#"{"username":"c","accounts":{"A":{"G":{"auto_channel_sync":true,"auto_sync_channel_end":99}}}}"#,
    );
    let out = check(&[&accounts, &groups]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(stdout.contains("21 endpoints"), "{stdout}");

    // A misspelt field is found in both bodies it would travel in.
    let typo = spec(
        "typo.json",
        "epg-sources",
        r#"{"username":"c","sources":{"E":{"sourcetype":"xmltv"}}}"#,
    );
    let out = check(&[&typo]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("EPGSource.sourcetype: EPGSource has no property sourcetype"),
        "{stderr}"
    );
    assert!(stderr.contains("PatchedEPGSource.sourcetype"), "{stderr}");

    // A profile's fields are sent when it is added and when it is changed;
    // a field the description does not know fails the check.
    let profiles = spec(
        "profiles.json",
        "m3u-profiles",
        r#"{"username":"c","accounts":{"X":{"X Default":{"search_pattern":"^https?://[^/]+","replace_pattern":"http://10.88.0.1:9201"},"X 2":{"max_streams":1,"is_active":true,"search_pattern":"^https?://[^/]+","replace_pattern":"http://10.88.0.1:9202"}}}}"#,
    );
    let out = check(&[&profiles]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(stdout.contains("6 spec field(s)"), "{stdout}");

    // A channel's number is a field of its override; the rest are names.
    let channels = spec(
        "channels.json",
        "channel-epg",
        r#"{"username":"c","channels":{"Das Erste":{"channel_number":1,"name":"Das Erste HD"},"ZDF":{"name":"ZDF HD"}}}"#,
    );
    let out = check(&[&channels]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(stdout.contains("1 spec field(s)"), "{stdout}");
}

#[test]
fn schema_check_checks_authentik_settings_against_the_answer_and_the_patch() {
    let openapi = format!(
        "{}/openapi/authentik-2026.5.6.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let dir = tempfile::tempdir().unwrap();
    let spec = |name: &str, desired: &str| {
        let path = dir.path().join(name);
        std::fs::write(
            &path,
            format!(
                r#"{{"service":"authentik","base_url":"http://10.0.10.10:9000","api_key_credential":"authentik-converge-token","task":"settings","desired":{desired}}}"#
            ),
        )
        .unwrap();
        path
    };

    let good = spec(
        "settings.json",
        r#"{"set":{"reputation_lower_limit":-10,"impersonation":false}}"#,
    );
    let out = schema_check("authentik", Some(&openapi), &[&good]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    // The endpoints live behind the description's server prefix (/api/v3).
    assert!(stdout.contains("3 endpoints"), "{stdout}");
    assert!(stdout.contains("2 spec field(s)"), "{stdout}");

    // A misspelt field is found in the answer and in the body of the PATCH.
    let typo = spec("typo.json", r#"{"set":{"impersonaton":false}}"#);
    let out = schema_check("authentik", Some(&openapi), &[&typo]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Settings.impersonaton: Settings has no property impersonaton"),
        "{stderr}"
    );
    assert!(
        stderr.contains("PatchedSettingsRequest.impersonaton"),
        "{stderr}"
    );
}

/// The whole way once, over HTTP: the token travels as a bearer token, both
/// paths keep their trailing slash, and the one differing field is the plan.
#[test]
fn authentik_plan_over_http_sends_a_bearer_token_and_never_shows_it() {
    const VERSION: &str = include_str!("fixtures/authentik-2026.5.6/version.json");
    const SETTINGS: &str = include_str!("fixtures/authentik-2026.5.6/settings.json");
    let mut on: serde_json::Value = serde_json::from_str(SETTINGS).unwrap();
    on["impersonation"] = true.into();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("authentik-converge-token"), "t0ken-xyz\n").unwrap();
    let server = Server::start(vec![
        ("GET", "/api/v3/admin/version/", 200, VERSION.into()),
        ("GET", "/api/v3/admin/settings/", 200, on.to_string()),
    ]);
    let spec = dir.path().join("settings.json");
    std::fs::write(
        &spec,
        format!(
            r#"{{"service":"authentik","base_url":"{}","api_key_credential":"authentik-converge-token","task":"settings","desired":{{"set":{{"impersonation":false,"reputation_lower_limit":-10}}}}}}"#,
            server.base_url()
        ),
    )
    .unwrap();
    let out = converge()
        .arg("plan")
        .arg(&spec)
        .env("CREDENTIALS_DIRECTORY", dir.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(2),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("authentik settings: service version 2026.5.6"),
        "{stdout}"
    );
    assert!(
        stdout.contains("would change Settings: impersonation true -> false"),
        "{stdout}"
    );
    assert!(stdout.contains("1 field(s) differ"), "{stdout}");
    assert!(!stdout.contains("t0ken"), "{stdout}");
    let seen = server.requests();
    assert_eq!(seen.len(), 2);
    for request in &seen {
        assert!(
            request
                .headers
                .to_ascii_lowercase()
                .contains("authorization: bearer t0ken-xyz"),
            "{}",
            request.headers
        );
        assert!(request.path.ends_with('/'), "{}", request.path);
    }
}

// --- custom formats, app profiles and format scores (design §41) ----------

const RADARR_FORMATS: &str = include_str!("fixtures/radarr-6.3.0.10514/customformat.json");
const PROWLARR_STATUS_V1: &str = include_str!("fixtures/prowlarr-2.5.2.5491/system-status.json");
const PROWLARR_APP_PROFILES: &str = include_str!("fixtures/prowlarr-2.5.2.5491/appprofile.json");

/// The three specs the host deploys, against the vendored descriptions: the
/// field names of an app profile, of a custom format and of each of its
/// specifications are properties of the components they are written to.
#[test]
fn schema_check_reads_the_new_arr_and_prowlarr_specs_and_names_a_wrong_field() {
    let dir = tempfile::tempdir().unwrap();
    let write = |file: &str, text: String| {
        let path = dir.path().join(file);
        std::fs::write(&path, text).unwrap();
        path
    };
    let radarr = format!(
        "{}/openapi/radarr-6.3.0.10514.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let prowlarr = format!(
        "{}/openapi/prowlarr-2.5.2.5491.json",
        env!("CARGO_MANIFEST_DIR")
    );

    let format_spec = |specification: &str| {
        format!(
            r#"{{"service":"radarr","base_url":"http://localhost:7878","api_key_credential":"k",
                 "task":"custom-formats","desired":{{"formats":{{"3D":{{
                   "include_custom_format_when_renaming":true,
                   "specifications":[{specification}]}}}}}}}}"#
        )
    };
    let good = write(
        "formats.json",
        format_spec(
            r#"{"name":"3D","implementation":"ReleaseTitleSpecification",
                "negate":false,"required":true,"fields":{"value":"(?i)\\b3d\\b"}}"#,
        ),
    );
    let out = schema_check("radarr", Some(&radarr), &[&good]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("5 spec field(s) in 1 spec(s) match"),
        "{stdout}"
    );

    // The count says the four fields of the specification were compared as
    // well, not only the format's switch: 1 + 4. What the comparison is for
    // is an upgrade that renames one of them -- `negate` is typed `bool` in
    // the spec, so a wrong VALUE never reaches the schema check:
    let bad = write(
        "negate.json",
        format_spec(
            r#"{"name":"3D","implementation":"ReleaseTitleSpecification",
                "negate":false,"required":true,"fields":{"value":"x"}}"#,
        )
        .replace(r#""negate":false"#, r#""negate":"no""#),
    );
    let out = schema_check("radarr", Some(&radarr), &[&bad]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("expected a boolean") && stderr.contains("negate.json"),
        "{stderr}"
    );

    let profile = |set: &str| {
        format!(
            r#"{{"service":"prowlarr","base_url":"http://localhost:9696","api_key_credential":"k",
                 "task":"app-profiles","desired":{{"profiles":{{"Standard":{set}}}}}}}"#
        )
    };
    let good = write("appprofile.json", profile(r#"{"minimumSeeders":1}"#));
    let out = schema_check("prowlarr", Some(&prowlarr), &[&good]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("1 spec field(s) in 1 spec(s) match"),
        "{stdout}"
    );

    let bad = write("seeders.json", profile(r#"{"minimumSeeders":"one"}"#));
    let out = schema_check("prowlarr", Some(&prowlarr), &[&bad]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(
            "app profile Standard: AppProfileResource.minimumSeeders: expects integer, \
             the spec has a string"
        ),
        "{stderr}"
    );
}

/// The host's own format, against a service that does not have it yet: one
/// `POST`, and the seventy TRaSH formats beside it are a counted note.
#[test]
fn custom_formats_plan_and_apply_over_http_leave_the_other_formats_alone() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("radarr-api-key"), "k\n").unwrap();
    let server = Server::start(vec![
        ("GET", "/api/v3/system/status", 200, STATUS.into()),
        ("GET", "/api/v3/customformat", 200, RADARR_FORMATS.into()),
    ]);
    let spec = dir.path().join("formats.json");
    std::fs::write(
        &spec,
        format!(
            r#"{{"service":"radarr","base_url":"{}","api_key_credential":"radarr-api-key",
                 "task":"custom-formats","desired":{{"formats":{{"3D":{{
                   "include_custom_format_when_renaming":true,
                   "specifications":[{{"name":"3D","implementation":"ReleaseTitleSpecification",
                     "negate":false,"required":true,
                     "fields":{{"value":"(?i)\\b(3d|hsbs|sbs)\\b"}}}}]}}}}}}}}"#,
            server.base_url()
        ),
    )
    .unwrap();
    let out = converge()
        .arg("plan")
        .arg(&spec)
        .env("CREDENTIALS_DIRECTORY", dir.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(2),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("would change custom format 3D: (missing) -> (added)"),
        "{stdout}"
    );
    // Counted, not listed: five recorded formats, one of them named.
    assert!(
        stdout.contains("not in the spec, left as they are: 5 other formats"),
        "{stdout}"
    );
    assert!(!stdout.contains("BR-DISK"), "{stdout}");
}

/// An app profile by name, and a name Prowlarr does not have: the refusal
/// names the indexer and the name it was given, and nothing from the body.
#[test]
fn an_indexer_app_profile_by_name_is_refused_by_name_and_nothing_is_written() {
    const INDEXERS: &str = include_str!("fixtures/prowlarr-2.5.2.5491/indexer.json");
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("prowlarr-api-key"), "k\n").unwrap();
    std::fs::write(dir.path().join("tnt-key"), "tnt-secret-never-print\n").unwrap();
    for (profile, code, expect) in [
        ("Standard", 0, "prowlarr indexers: unchanged"),
        (
            "Gibt es nicht",
            1,
            "indexer TNTracker: app_profile names \"Gibt es nicht\", which the service does not have",
        ),
    ] {
        let server = Server::start(vec![
            ("GET", "/api/v1/system/status", 200, PROWLARR_STATUS_V1.into()),
            ("GET", "/api/v1/indexer", 200, INDEXERS.into()),
            ("GET", "/api/v1/appprofile", 200, PROWLARR_APP_PROFILES.into()),
        ]);
        let spec = dir.path().join("indexers.json");
        std::fs::write(
            &spec,
            format!(
                r#"{{"service":"prowlarr","base_url":"{}","api_key_credential":"prowlarr-api-key",
                     "task":"indexers","desired":{{"providers":{{"TNTracker":{{
                       "implementation":"Torznab","app_profile":"{profile}",
                       "set":{{"priority":25}},
                       "secret_fields":{{"apiKey":"tnt-key"}}}}}}}}}}"#,
                server.base_url()
            ),
        )
        .unwrap();
        let out = converge()
            .arg("plan")
            .arg(&spec)
            .env("CREDENTIALS_DIRECTORY", dir.path())
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(code), "{stdout}{stderr}");
        assert!(
            stdout.contains(expect) || stderr.contains(expect),
            "{stdout}{stderr}"
        );
        assert!(!stdout.contains("tnt-secret-never-print"), "{stdout}");
        assert!(!stderr.contains("tnt-secret-never-print"), "{stderr}");
    }
}
