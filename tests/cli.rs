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
            .contains("radarr: 23 endpoints and their wire types match"),
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
        stdout.contains("jellyfin: 12 endpoints and their wire types match"),
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
        stdout.contains("trailarr: 6 endpoints and their wire types match"),
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
        stdout.contains("lidarr: 22 endpoints and their wire types match"),
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
        stdout.contains("prowlarr: 31 endpoints and their wire types match"),
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
