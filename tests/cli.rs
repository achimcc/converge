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
            .contains("radarr: 5 endpoints and their wire types match"),
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
        stdout.contains("jellyfin: 10 endpoints and their wire types match"),
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
