# converge pilot — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A CLI that reconciles Radarr's and Sonarr's quality size limits with a JSON spec, checks every field name it relies on at three levels, and replaces the shell unit `qualitaetsgroessen` on the host.

**Architecture:** A library crate with a `Task` trait driven by a generic engine (probe → read → diff → write → read back), a `Transport` trait with an HTTP implementation over `ureq` and a scripted fake for tests, and a schema checker that compares `schemars`-derived wire types with a service's OpenAPI file. A thin `main.rs` parses arguments and prints results.

**Tech Stack:** Rust 2021 (rustc 1.95 from nixpkgs 26.05), `serde`/`serde_json`, `ureq` 3.4 without default features, `schemars` 1.2, `thiserror` 2; Nix flake with `rustPlatform.buildRustPackage`.

**Spec:** [`docs/design.md`](design.md)

## Global Constraints

- License `AGPL-3.0-only`; repository `https://github.com/achimcc/converge`, public; English throughout.
- Spec files are parsed with `deny_unknown_fields`; `min`/`preferred`/`max` must be present and are number or `null`.
- Absent nullable field on the service == `null`.
- The API key travels only as the `X-Api-Key` header; `Secret` has no `Debug`/`Display`.
- `200` and `202` accepted for the update PUT; any other status → error printing only `propertyName`/`errorMessage`.
- Timing defaults: request 10 s, readiness 120 s, read-back 60 s, poll 2 s, per-spec deadline 300 s (`--deadline`).
- Exit codes: `apply` 0/1; `plan` 0 equal, 2 differs, 1 error; `schema-check` 0/1; usage errors 1.
- No `unwrap()`/`panic!` outside tests. Plain HTTP only (`base_url` must start with `http://`).
- Every task ends green on all three: `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` (run inside `nix develop`).
- Stage files by name, never `git add -A`. Commits are signed (global git config).

## File structure

| file | responsibility |
|---|---|
| `src/lib.rs` | module list |
| `src/main.rs` | argument parsing, output, exit codes |
| `src/error.rs` | the one `Error` enum |
| `src/secret.rs` | `Secret` |
| `src/spec.rs` | spec parsing and validation |
| `src/endpoint.rs` | `Endpoint`, `Shape` — the only place paths are written |
| `src/client.rs` | `Transport`, `Reply`, `HttpTransport`, `read_credential`, `expect_status` |
| `src/clock.rs` | `Clock`, `SystemClock` |
| `src/engine.rs` | `Task`, `Probe`, `Change`, `Timing`, `Mode`, `Outcome`, `Report`, `run` |
| `src/services/mod.rs`, `src/services/arr.rs` | Radarr/Sonarr wire types, endpoints, `QualityDefinitions` |
| `src/schema.rs` | OpenAPI comparison |
| `src/testing.rs` | `FakeClock`, `FakeTransport` (`#[cfg(test)]`) |
| `tests/support/mod.rs` | a tiny std-only HTTP server for tests |
| `tests/http.rs`, `tests/schema.rs`, `tests/cli.rs` | integration tests |
| `tests/fixtures/…` | recorded answers + host table |
| `openapi/…` | vendored OpenAPI files |

---

### Task 1: Scaffold, errors, secret, credentials

**Files:** Create `Cargo.toml` (exists from `cargo init`; set metadata), `src/lib.rs`, `src/main.rs` (placeholder), `src/error.rs`, `src/secret.rs`, `src/endpoint.rs`, `src/client.rs` (credential part only), `CLAUDE.md`.

**Interfaces — Produces:**
- `converge::error::Error` (all variants below)
- `converge::secret::Secret::{new(String) -> Secret, expose(&self) -> &str}`
- `converge::client::read_credential(dir: Option<&Path>, name: &str) -> Result<Secret, Error>`
- `converge::endpoint::{Endpoint { method: &'static str, path: &'static str, request: Option<Shape>, response: Option<Shape> }, Shape::{One(&'static str), List(&'static str)}}`

- [ ] **Step 1: `Cargo.toml` package section**

```toml
[package]
name = "converge"
version = "0.1.0"
edition = "2021"
license = "AGPL-3.0-only"
description = "Reconcile self-hosted services with a desired state through their HTTP APIs"
repository = "https://github.com/achimcc/converge"

[dependencies]
schemars = "1.2.2"
serde = { version = "1.0.229", features = ["derive"] }
serde_json = "1.0.151"
thiserror = "2.0.20"
ureq = { version = "3.4.1", default-features = false }

[dev-dependencies]
tempfile = "3.27.0"
```

- [ ] **Step 2: `src/error.rs`**

```rust
use std::path::PathBuf;

/// Every way a run can fail. Messages name fields, paths and credentials --
/// never a credential's value, which no variant can even hold.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("cannot read spec {path}: {source}")]
    SpecRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("spec {path} is not valid: {reason}")]
    SpecInvalid { path: PathBuf, reason: String },
    #[error("credential {name}: {reason}")]
    Credential { name: String, reason: String },
    #[error("{method} {path}: {reason}")]
    Request {
        method: &'static str,
        path: String,
        reason: String,
    },
    #[error("{method} {path} answered HTTP {status}{}", validation_suffix(.validation))]
    Status {
        method: &'static str,
        path: String,
        status: u16,
        validation: Vec<String>,
    },
    #[error("{path}: the answer does not have the expected shape: {reason}")]
    Decode { path: String, reason: String },
    #[error("service not ready after {waited_secs} s, last: {last}")]
    NotReady { waited_secs: u64, last: String },
    #[error("{path} returned an empty list")]
    EmptyList { path: String },
    #[error("entry {index} of {path} has no quality name")]
    MissingName { path: String, index: usize },
    #[error("the service does not know these qualities from the spec: {}", .0.join(", "))]
    UnknownQualities(Vec<String>),
    #[error("written, but after {waited_secs} s these still differ: {}", .remaining.join("; "))]
    NotConverged {
        waited_secs: u64,
        remaining: Vec<String>,
    },
    #[error("overall deadline of {secs} s exceeded")]
    Deadline { secs: u64 },
}

fn validation_suffix(messages: &[String]) -> String {
    if messages.is_empty() {
        String::new()
    } else {
        format!(": {}", messages.join("; "))
    }
}
```

- [ ] **Step 3: `src/secret.rs`**

```rust
/// A string that must never reach a log line, an error message or a Debug
/// dump. It has neither `Debug` nor `Display`, so this does not compile:
///
/// ```compile_fail
/// let s = converge::secret::Secret::new("k".to_string());
/// println!("{:?}", s);
/// ```
pub struct Secret(String);

impl Secret {
    pub fn new(value: String) -> Self {
        Self(value)
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}
```

- [ ] **Step 4: `src/endpoint.rs`**

```rust
/// What a request or response body is, in OpenAPI component names.
#[derive(Clone, Copy, Debug)]
pub enum Shape {
    One(&'static str),
    List(&'static str),
}

/// One HTTP endpoint a task uses. Task code reads `path` from here, so the
/// schema check and the requests cannot name different endpoints.
#[derive(Clone, Copy, Debug)]
pub struct Endpoint {
    pub method: &'static str,
    pub path: &'static str,
    pub request: Option<Shape>,
    pub response: Option<Shape>,
}
```

- [ ] **Step 5: failing credential tests in `src/client.rs`**

```rust
use std::{fs, path::Path};

use crate::{error::Error, secret::Secret};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_and_trims_a_credential() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("radarr-api-key"), "abc123\n").unwrap();
        let key = read_credential(Some(dir.path()), "radarr-api-key").unwrap();
        assert_eq!(key.expose(), "abc123");
    }

    #[test]
    fn missing_directory_names_the_unit_setting() {
        let err = read_credential(None, "radarr-api-key").err().unwrap();
        assert!(err.to_string().contains("LoadCredential"), "{err}");
    }

    #[test]
    fn missing_file_names_the_credential_not_a_value() {
        let dir = tempfile::tempdir().unwrap();
        let err = read_credential(Some(dir.path()), "radarr-api-key").err().unwrap();
        assert!(err.to_string().starts_with("credential radarr-api-key:"), "{err}");
    }

    #[test]
    fn rejects_names_that_leave_the_directory() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["../etc/passwd", "a/b", "..", ""] {
            assert!(read_credential(Some(dir.path()), name).is_err(), "{name}");
        }
    }

    #[test]
    fn empty_credential_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("k"), "\n").unwrap();
        assert!(read_credential(Some(dir.path()), "k").is_err());
    }
}
```

`src/lib.rs`:

```rust
pub mod client;
pub mod endpoint;
pub mod error;
pub mod secret;
```

`src/main.rs`: `fn main() {}`

- [ ] **Step 6: run, expect FAIL** — `nix develop -c cargo test` → `cannot find function read_credential`.

- [ ] **Step 7: implement in `src/client.rs` above the tests**

```rust
/// Reads a systemd credential by name. The name comes from the spec; the
/// value never leaves the returned `Secret`.
pub fn read_credential(dir: Option<&Path>, name: &str) -> Result<Secret, Error> {
    let fail = |reason: &str| Error::Credential {
        name: name.to_string(),
        reason: reason.to_string(),
    };
    if name.is_empty() || name == "." || name == ".." || name.contains('/') {
        return Err(fail("is not a plain credential name"));
    }
    let dir = dir.ok_or_else(|| {
        fail("CREDENTIALS_DIRECTORY is not set -- is LoadCredential= missing from the unit?")
    })?;
    let raw = fs::read_to_string(dir.join(name))
        .map_err(|e| fail(&format!("cannot be read ({})", e.kind())))?;
    let value = raw.trim_end_matches(['\n', '\r']);
    if value.is_empty() {
        return Err(fail("is empty"));
    }
    Ok(Secret::new(value.to_string()))
}
```

- [ ] **Step 8: run all three, expect PASS** (the `compile_fail` doctest counts as passing).

- [ ] **Step 9: `CLAUDE.md`** — language rule (English), the test cycle (three commands), "ask for the result, not the exit code", the secret rule, and "field names come from recorded answers or the OpenAPI file, never from memory".

- [ ] **Step 10: commit** — `git add Cargo.toml Cargo.lock src/lib.rs src/main.rs src/error.rs src/secret.rs src/endpoint.rs src/client.rs CLAUDE.md LICENSE .gitignore renovate.json flake.nix flake.lock docs/design.md docs/plan-pilot.md` → `git commit -m "Scaffold: errors, Secret, credential reading"`.

---

### Task 2: Spec parsing and validation

**Files:** Create `src/spec.rs`; modify `src/lib.rs` (`pub mod spec;`).

**Interfaces — Produces:**
- `Service::{Radarr, Sonarr}`, `Service::name(self) -> &'static str`
- `SizeLimits { min: Option<f64>, preferred: Option<f64>, max: Option<f64> }` (`Clone, Copy, Debug, PartialEq`)
- `Desired::QualityDefinitions(BTreeMap<String, SizeLimits>)`
- `Spec { path: PathBuf, service: Service, base_url: String, api_key_credential: String, desired: Desired }`, `Spec::load(&Path)`, `Spec::parse(&Path, &str)`, `Spec::task_name(&self) -> &'static str`
- `spec::show(Option<f64>) -> String` (`None` → `"unlimited"`)

- [ ] **Step 1: failing tests (bottom of `src/spec.rs`)**

```rust
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
        let Desired::QualityDefinitions(map) = spec.desired;
        assert_eq!(map["Bluray-1080p"], SizeLimits { min: Some(12.5), preferred: None, max: None });
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
        assert!(parse(&text).err().unwrap().to_string().contains("dry_run"));
    }

    #[test]
    fn a_missing_limit_key_is_an_error_not_unlimited() {
        let text = GOOD.replace(r#""preferred": null, "#, "");
        let err = parse(&text).err().unwrap().to_string();
        assert!(err.contains("preferred"), "{err}");
    }

    #[test]
    fn ordering_is_checked_with_null_as_unlimited() {
        let text = GOOD.replace(r#""min": 0, "preferred": 95"#, r#""min": 96, "preferred": 95"#);
        assert!(parse(&text).err().unwrap().to_string().contains("CAM: min 96"));
        let text = GOOD.replace(r#""max": 100"#, r#""max": 90"#);
        assert!(parse(&text).is_err());
        let text = GOOD.replace(r#""preferred": 95, "max": 100"#, r#""preferred": null, "max": 100"#);
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
```

- [ ] **Step 2: run, expect FAIL** (`Spec` not defined).

- [ ] **Step 3: implement (top of `src/spec.rs`)**

```rust
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
}

impl Service {
    pub fn name(self) -> &'static str {
        match self {
            Service::Radarr => "radarr",
            Service::Sonarr => "sonarr",
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum TaskName {
    QualityDefinitions,
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
            return Err(format!("min {min} is above preferred {}", show(self.preferred)));
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

#[derive(Debug, Clone, PartialEq)]
pub enum Desired {
    QualityDefinitions(BTreeMap<String, SizeLimits>),
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
        let desired = match raw.task {
            TaskName::QualityDefinitions => {
                let map: BTreeMap<String, SizeLimits> = serde_json::from_value(raw.desired)
                    .map_err(|e| invalid(format!("desired: {e}")))?;
                if map.is_empty() {
                    return Err(invalid("desired names no quality".to_string()));
                }
                for (name, limits) in &map {
                    limits.check_order().map_err(|r| invalid(format!("{name}: {r}")))?;
                }
                Desired::QualityDefinitions(map)
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
        }
    }
}
```

- [ ] **Step 4: run all three, expect PASS.** If `a_misspelt_limit_key_is_an_error` fails because serde reports the *missing* `preferred` first instead of the unknown `prefered`, relax the assertion to `err.contains("prefered") || err.contains("preferred")` — both prove the typo is not silently accepted; note which one serde reports in the test comment.

- [ ] **Step 5: commit** `src/spec.rs src/lib.rs` — "Spec: strict parsing, required nullable limits, ordering check".

---

### Task 3: Recorded fixtures and Radarr/Sonarr wire types

**Files:** Create `tests/fixtures/radarr-6.3.0.10514/{system-status.json,qualitydefinition.json,desired.json,SOURCE.md}`, `tests/fixtures/sonarr-4.0.19.2979/{same}`, `tests/fixtures/constructed-validation-error.json`, `src/services/mod.rs`, `src/services/arr.rs` (types + endpoints); modify `src/lib.rs`.

**Interfaces — Produces:**
- `arr::{SystemResource { version: Option<String> }, Quality { name: Option<String>, rest: Map }, QualityDefinitionResource { quality, min_size, preferred_size, max_size: Option<f64>, rest: Map }}` — all `JsonSchema`
- `arr::{SYSTEM_STATUS, QUALITY_LIST, QUALITY_UPDATE}: Endpoint`, `arr::ENDPOINTS: [Endpoint; 3]`, `arr::wire_types() -> Vec<schemars::Schema>`

- [ ] **Step 1: copy fixtures.** The four JSON answers recorded 2026-09-13 11:47 CEST from the host (`GET /api/v3/system/status`, `GET /api/v3/qualitydefinition`, per service) go in verbatim. `desired.json` is the host's table rendered with `nix eval --json --file lib/qualitaetsgroessen.nix` and then `.radarr` / `.sonarr`. `SOURCE.md` per directory states date, version, the `systemd-run --machine=media-01 … curl` command, and that both endpoints carry no credentials (checked by hand; the only long token is a Nix store hash in `startupPath`). `constructed-validation-error.json`:

```json
[
  {
    "propertyName": "MaxSize",
    "errorMessage": "'Max Size' must be greater than or equal to 'Min Size'.",
    "attemptedValue": 1,
    "severity": "error"
  }
]
```

with a note in `tests/fixtures/README.md`: constructed from the FluentValidation shape Radarr returns, not recorded — provoking it on a live instance was not worth the risk.

- [ ] **Step 2: failing tests in `src/services/arr.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    pub const RADARR_LIST: &str =
        include_str!("../../tests/fixtures/radarr-6.3.0.10514/qualitydefinition.json");
    pub const SONARR_LIST: &str =
        include_str!("../../tests/fixtures/sonarr-4.0.19.2979/qualitydefinition.json");

    fn by_name<'a>(list: &'a [QualityDefinitionResource], name: &str) -> &'a QualityDefinitionResource {
        list.iter().find(|d| d.quality.name.as_deref() == Some(name)).unwrap()
    }

    #[test]
    fn recorded_answers_parse() {
        let radarr: Vec<QualityDefinitionResource> = serde_json::from_str(RADARR_LIST).unwrap();
        let sonarr: Vec<QualityDefinitionResource> = serde_json::from_str(SONARR_LIST).unwrap();
        assert!(radarr.len() > 20 && sonarr.len() > 20);
    }

    #[test]
    fn an_absent_nullable_field_is_none() {
        // Radarr omits null values: this entry has no maxSize key at all.
        let raw: serde_json::Value = serde_json::from_str(RADARR_LIST).unwrap();
        let entry = raw.as_array().unwrap().iter()
            .find(|e| e["quality"]["name"] == "Bluray-1080p").unwrap();
        assert!(entry.get("maxSize").is_none(), "fixture changed; pick another entry");
        let list: Vec<QualityDefinitionResource> = serde_json::from_str(RADARR_LIST).unwrap();
        assert_eq!(by_name(&list, "Bluray-1080p").max_size, None);
    }

    #[test]
    fn unknown_fields_survive_a_round_trip() {
        let list: Vec<QualityDefinitionResource> = serde_json::from_str(RADARR_LIST).unwrap();
        let back = serde_json::to_value(by_name(&list, "Unknown")).unwrap();
        assert_eq!(back["quality"]["modifier"], "none");
        assert_eq!(back["id"], 1);
        assert!(back.get("weight").is_some() && back.get("title").is_some());
    }

    #[test]
    fn none_is_written_as_explicit_null() {
        let list: Vec<QualityDefinitionResource> = serde_json::from_str(RADARR_LIST).unwrap();
        let back = serde_json::to_value(by_name(&list, "Bluray-1080p")).unwrap();
        assert_eq!(back.get("maxSize"), Some(&serde_json::Value::Null));
    }

    #[test]
    fn a_missing_quality_object_is_an_error() {
        assert!(serde_json::from_str::<QualityDefinitionResource>(r#"{"minSize":1}"#).is_err());
    }

    #[test]
    fn wire_types_are_named_after_their_components() {
        let titles: Vec<String> = wire_types().iter()
            .map(|s| s.as_value()["title"].as_str().unwrap().to_string()).collect();
        assert_eq!(titles, ["SystemResource", "QualityDefinitionResource"]);
    }
}
```

- [ ] **Step 3: run, expect FAIL.**

- [ ] **Step 4: implement (top of `src/services/arr.rs`)**

```rust
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::endpoint::{Endpoint, Shape};

pub const SYSTEM_STATUS: Endpoint = Endpoint {
    method: "GET",
    path: "/api/v3/system/status",
    request: None,
    response: Some(Shape::One("SystemResource")),
};
pub const QUALITY_LIST: Endpoint = Endpoint {
    method: "GET",
    path: "/api/v3/qualitydefinition",
    request: None,
    response: Some(Shape::List("QualityDefinitionResource")),
};
pub const QUALITY_UPDATE: Endpoint = Endpoint {
    method: "PUT",
    path: "/api/v3/qualitydefinition/update",
    request: Some(Shape::List("QualityDefinitionResource")),
    response: None,
};
pub const ENDPOINTS: [Endpoint; 3] = [SYSTEM_STATUS, QUALITY_LIST, QUALITY_UPDATE];

/// The wire types, each named exactly like its OpenAPI component. Their
/// fields are what `schema-check` compares -- derived, not listed by hand.
pub fn wire_types() -> Vec<schemars::Schema> {
    vec![
        schemars::schema_for!(SystemResource),
        schemars::schema_for!(QualityDefinitionResource),
    ]
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SystemResource {
    #[serde(default)]
    pub version: Option<String>,
}

/// Declares only what this program reads or writes. Everything else the
/// service sends is kept in `rest` and written back untouched.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Quality {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(flatten)]
    #[schemars(skip)]
    pub rest: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct QualityDefinitionResource {
    pub quality: Quality,
    // `default`: the service omits null values instead of writing `null`.
    #[serde(default)]
    pub min_size: Option<f64>,
    #[serde(default)]
    pub preferred_size: Option<f64>,
    #[serde(default)]
    pub max_size: Option<f64>,
    #[serde(flatten)]
    #[schemars(skip)]
    pub rest: Map<String, Value>,
}
```

`src/services/mod.rs`: `pub mod arr;` — `src/lib.rs`: add `pub mod services;`.

- [ ] **Step 5: run all three, expect PASS.**
- [ ] **Step 6: commit** fixtures, `src/services/*`, `src/lib.rs` — "Radarr/Sonarr wire types against recorded answers".

---

### Task 4: Transport, clock, fakes, HTTP

**Files:** Modify `src/client.rs`; create `src/clock.rs`, `src/testing.rs`, `tests/support/mod.rs`, `tests/http.rs`; modify `src/lib.rs`.

**Interfaces — Produces:**
- `client::Reply { status: u16, body: String }`
- `client::Transport { fn get(&self, path: &str) -> Result<Reply, Error>; fn put_json(&self, path: &str, body: &str) -> Result<Reply, Error>; }`
- `client::HttpTransport::new(base_url: &str, key: Secret, request_timeout: Duration)`
- `client::expect_status(ep: &Endpoint, reply: &Reply, accepted: &[u16]) -> Result<(), Error>`
- `clock::{Clock { now, sleep }, SystemClock}`
- `testing::{FakeClock::new(), FakeClock::elapsed(), Step::{Answer(u16, String), Refused}, ok(&str) -> Step, FakeTransport::default().on_get(path, Vec<Step>).on_put(Vec<Step>), FakeTransport.written: RefCell<Vec<(String, String)>>}`
- `tests/support::Server::start(routes: Vec<(&'static str, &'static str, u16, String)>) -> Server` with `.base_url()` and `.requests() -> Vec<Recorded { method, path, headers: String, body: String }>`

- [ ] **Step 1: `tests/support/mod.rs`** — std-only server: binds `127.0.0.1:0`, a thread accepts connections in a loop, reads until `\r\n\r\n`, reads `Content-Length` bytes, records the request, answers the first route whose method and path match (`404` otherwise) with `Content-Length` and `Connection: close`.

```rust
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    sync::{Arc, Mutex},
    thread,
};

#[derive(Clone, Debug)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    pub headers: String,
    pub body: String,
}

pub struct Server {
    port: u16,
    seen: Arc<Mutex<Vec<Recorded>>>,
}

impl Server {
    pub fn start(routes: Vec<(&'static str, &'static str, u16, String)>) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut head = String::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    head.push_str(&line);
                }
                let mut parts = head.split_whitespace();
                let method = parts.next().unwrap_or("").to_string();
                let path = parts.next().unwrap_or("").to_string();
                let length = head
                    .lines()
                    .find_map(|l| {
                        let (k, v) = l.split_once(':')?;
                        k.eq_ignore_ascii_case("content-length").then(|| v.trim().parse::<usize>().ok())?
                    })
                    .unwrap_or(0);
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                log.lock().unwrap().push(Recorded {
                    method: method.clone(),
                    path: path.clone(),
                    headers: head.clone(),
                    body: String::from_utf8_lossy(&body).into_owned(),
                });
                let (status, answer) = routes
                    .iter()
                    .find(|(m, p, _, _)| *m == method && *p == path)
                    .map(|(_, _, s, b)| (*s, b.clone()))
                    .unwrap_or((404, String::new()));
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}",
                    answer.len()
                );
            }
        });
        Server { port, seen }
    }

    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    pub fn requests(&self) -> Vec<Recorded> {
        self.seen.lock().unwrap().clone()
    }
}
```

- [ ] **Step 2: failing `tests/http.rs`**

```rust
mod support;

use std::time::Duration;

use converge::{client::{HttpTransport, Transport}, secret::Secret};
use support::Server;

fn transport(base: &str) -> HttpTransport {
    HttpTransport::new(base, Secret::new("s3cret-key-value".into()), Duration::from_secs(5))
}

#[test]
fn get_sends_the_key_as_a_header_and_returns_non_2xx_as_a_reply() {
    let server = Server::start(vec![("GET", "/a", 401, "{}".into())]);
    let reply = transport(&server.base_url()).get("/a").unwrap();
    assert_eq!(reply.status, 401);
    let seen = &server.requests()[0];
    assert!(seen.headers.to_ascii_lowercase().contains("x-api-key: s3cret-key-value"));
    assert!(!seen.path.contains("s3cret"));
}

#[test]
fn put_sends_json() {
    let server = Server::start(vec![("PUT", "/u", 202, String::new())]);
    let reply = transport(&server.base_url()).put_json("/u", r#"[{"a":1}]"#).unwrap();
    assert_eq!(reply.status, 202);
    let seen = &server.requests()[0];
    assert_eq!(seen.body, r#"[{"a":1}]"#);
    assert!(seen.headers.to_ascii_lowercase().contains("content-type: application/json"));
}

#[test]
fn a_refused_connection_is_an_error_without_the_key() {
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let err = transport(&format!("http://127.0.0.1:{port}")).get("/a").err().unwrap().to_string();
    assert!(err.starts_with("GET /a:"), "{err}");
    assert!(!err.contains("s3cret"));
}
```

Plus unit tests in `src/client.rs` for `expect_status`:

```rust
    #[test]
    fn expect_status_shows_only_validation_fields() {
        use crate::endpoint::Endpoint;
        const EP: Endpoint = Endpoint { method: "PUT", path: "/u", request: None, response: None };
        let body = include_str!("../tests/fixtures/constructed-validation-error.json");
        let err = expect_status(&EP, &Reply { status: 400, body: body.into() }, &[200]).err().unwrap();
        assert_eq!(
            err.to_string(),
            "PUT /u answered HTTP 400: MaxSize: 'Max Size' must be greater than or equal to 'Min Size'."
        );
        let err = expect_status(&EP, &Reply { status: 500, body: "<html>secret stack</html>".into() }, &[200]).err().unwrap();
        assert_eq!(err.to_string(), "PUT /u answered HTTP 500");
        assert!(expect_status(&EP, &Reply { status: 202, body: String::new() }, &[200, 202]).is_ok());
    }
```

- [ ] **Step 3: run, expect FAIL.**

- [ ] **Step 4: implement `src/client.rs` additions**

```rust
use std::time::Duration;

use serde::Deserialize;

use crate::endpoint::Endpoint;

pub struct Reply {
    pub status: u16,
    pub body: String,
}

pub trait Transport {
    fn get(&self, path: &str) -> Result<Reply, Error>;
    fn put_json(&self, path: &str, body: &str) -> Result<Reply, Error>;
}

pub struct HttpTransport {
    agent: ureq::Agent,
    base_url: String,
    key: Secret,
}

impl HttpTransport {
    pub fn new(base_url: &str, key: Secret, request_timeout: Duration) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(request_timeout))
            // A status is an answer, not a transport failure; the caller decides.
            .http_status_as_error(false)
            .build()
            .into();
        Self {
            agent,
            base_url: base_url.trim_end_matches('/').to_string(),
            key,
        }
    }

    fn finish(
        method: &'static str,
        path: &str,
        result: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
    ) -> Result<Reply, Error> {
        let fail = |reason: String| Error::Request {
            method,
            path: path.to_string(),
            reason,
        };
        let mut response = result.map_err(|e| fail(e.to_string()))?;
        let status = response.status().as_u16();
        let body = response
            .body_mut()
            .read_to_string()
            .map_err(|e| fail(e.to_string()))?;
        Ok(Reply { status, body })
    }
}

impl Transport for HttpTransport {
    fn get(&self, path: &str) -> Result<Reply, Error> {
        let result = self
            .agent
            .get(format!("{}{path}", self.base_url))
            .header("X-Api-Key", self.key.expose())
            .call();
        Self::finish("GET", path, result)
    }

    fn put_json(&self, path: &str, body: &str) -> Result<Reply, Error> {
        let result = self
            .agent
            .put(format!("{}{path}", self.base_url))
            .header("X-Api-Key", self.key.expose())
            .content_type("application/json")
            .send(body);
        Self::finish("PUT", path, result)
    }
}

/// Accepts the listed statuses. Anything else becomes an error that carries
/// the validation messages of the body -- and nothing else from it.
pub fn expect_status(ep: &Endpoint, reply: &Reply, accepted: &[u16]) -> Result<(), Error> {
    if accepted.contains(&reply.status) {
        return Ok(());
    }
    Err(Error::Status {
        method: ep.method,
        path: ep.path.to_string(),
        status: reply.status,
        validation: validation_messages(&reply.body),
    })
}

fn validation_messages(body: &str) -> Vec<String> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Failure {
        property_name: Option<String>,
        error_message: Option<String>,
    }
    let Ok(list) = serde_json::from_str::<Vec<Failure>>(body) else {
        return Vec::new();
    };
    list.into_iter()
        .filter_map(|f| {
            let message = f.error_message?;
            Some(match f.property_name {
                Some(p) if !p.is_empty() => format!("{p}: {message}"),
                _ => message,
            })
        })
        .collect()
}
```

`src/clock.rs`:

```rust
use std::time::{Duration, Instant};

pub trait Clock {
    fn now(&self) -> Instant;
    fn sleep(&self, duration: Duration);
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}
```

`src/testing.rs`:

```rust
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, VecDeque},
    time::{Duration, Instant},
};

use crate::{client::{Reply, Transport}, clock::Clock, error::Error};

pub struct FakeClock {
    start: Instant,
    offset: Cell<Duration>,
}

impl FakeClock {
    pub fn new() -> Self {
        Self { start: Instant::now(), offset: Cell::new(Duration::ZERO) }
    }
    pub fn elapsed(&self) -> Duration {
        self.offset.get()
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Instant {
        self.start + self.offset.get()
    }
    fn sleep(&self, duration: Duration) {
        self.offset.set(self.offset.get() + duration);
    }
}

#[derive(Clone)]
pub enum Step {
    Answer(u16, String),
    Refused,
}

pub fn ok(body: &str) -> Step {
    Step::Answer(200, body.to_string())
}

/// Scripted answers per path. The last step repeats, so a test only scripts
/// what changes.
#[derive(Default)]
pub struct FakeTransport {
    gets: RefCell<HashMap<String, VecDeque<Step>>>,
    puts: RefCell<VecDeque<Step>>,
    pub written: RefCell<Vec<(String, String)>>,
}

impl FakeTransport {
    pub fn on_get(self, path: &str, steps: Vec<Step>) -> Self {
        self.gets.borrow_mut().insert(path.to_string(), steps.into());
        self
    }
    pub fn on_put(self, steps: Vec<Step>) -> Self {
        *self.puts.borrow_mut() = steps.into();
        self
    }
}

fn take(queue: Option<&mut VecDeque<Step>>) -> Option<Step> {
    let queue = queue?;
    if queue.len() > 1 { queue.pop_front() } else { queue.front().cloned() }
}

fn play(step: Option<Step>, method: &'static str, path: &str) -> Result<Reply, Error> {
    match step {
        Some(Step::Answer(status, body)) => Ok(Reply { status, body }),
        Some(Step::Refused) => Err(Error::Request {
            method,
            path: path.to_string(),
            reason: "connection refused".to_string(),
        }),
        None => panic!("test did not script {method} {path}"),
    }
}

impl Transport for FakeTransport {
    fn get(&self, path: &str) -> Result<Reply, Error> {
        let step = take(self.gets.borrow_mut().get_mut(path));
        play(step, "GET", path)
    }
    fn put_json(&self, path: &str, body: &str) -> Result<Reply, Error> {
        self.written.borrow_mut().push((path.to_string(), body.to_string()));
        let step = take(Some(&mut *self.puts.borrow_mut()));
        play(step, "PUT", path)
    }
}
```

`src/lib.rs`: add `pub mod clock;` and `#[cfg(test)] pub mod testing;`.

- [ ] **Step 5: run all three, expect PASS.** (Clippy may ask for `impl Default for FakeClock`; add it.)
- [ ] **Step 6: commit** — "Transport over ureq, fakes, status handling".

---

### Task 5: The `quality-definitions` task

**Files:** Create `src/engine.rs` (trait and types only); modify `src/services/arr.rs`, `src/lib.rs`.

**Interfaces — Produces (`engine`):**
- `Change { subject: String, field: &'static str, current: String, desired: String }` + `Display` → `"{subject}: {field} {current} -> {desired}"`
- `Probe::{NotYet(String), Fatal(Error)}`
- `trait Task { type Current; fn probe(&self, t: &dyn Transport) -> Result<String, Probe>; fn read(&self, t: &dyn Transport) -> Result<Self::Current, Error>; fn diff(&self, c: &Self::Current) -> Result<Vec<Change>, Error>; fn notes(&self, c: &Self::Current) -> Vec<String>; fn write(&self, t: &dyn Transport, c: &Self::Current) -> Result<(), Error>; }`
- `arr::QualityDefinitions { desired: BTreeMap<String, SizeLimits> }` implementing `Task<Current = Vec<QualityDefinitionResource>>`

- [ ] **Step 1: failing tests (in `arr.rs` tests module)**

```rust
    use std::collections::BTreeMap;
    use crate::{engine::{Probe, Task}, spec::SizeLimits, testing::{ok, FakeTransport, Step}};

    const RADARR_DESIRED: &str = include_str!("../../tests/fixtures/radarr-6.3.0.10514/desired.json");
    const SONARR_DESIRED: &str = include_str!("../../tests/fixtures/sonarr-4.0.19.2979/desired.json");
    const RADARR_STATUS: &str = include_str!("../../tests/fixtures/radarr-6.3.0.10514/system-status.json");

    fn task(desired: &str) -> QualityDefinitions {
        let desired: BTreeMap<String, SizeLimits> = serde_json::from_str(desired).unwrap();
        QualityDefinitions { desired }
    }

    fn listing(body: &str) -> FakeTransport {
        FakeTransport::default().on_get(QUALITY_LIST.path, vec![ok(body)])
    }

    #[test]
    fn the_hosts_table_matches_what_the_service_holds() {
        // The shell unit this replaces wrote on every run against exactly
        // this state, because it compared an absent key with `null`.
        for (desired, list) in [(RADARR_DESIRED, RADARR_LIST), (SONARR_DESIRED, SONARR_LIST)] {
            let task = task(desired);
            let current = task.read(&listing(list)).unwrap();
            assert_eq!(task.diff(&current).unwrap(), vec![]);
        }
    }

    #[test]
    fn one_changed_field_is_one_change() {
        let changed = RADARR_DESIRED.replacen(r#""min": 12.5"#, r#""min": 35"#, 1);
        let task = task(&changed);
        let current = task.read(&listing(RADARR_LIST)).unwrap();
        let changes = task.diff(&current).unwrap();
        assert_eq!(changes.len(), 1, "{changes:?}");
        assert_eq!(changes[0].to_string(), "Bluray-1080p: min 12.5 -> 35");
    }

    #[test]
    fn an_unknown_quality_aborts_with_all_names() {
        let task = task(r#"{"Bluray-9000p": {"min":0,"preferred":null,"max":null}, "VHS": {"min":0,"preferred":null,"max":null}}"#);
        let current = task.read(&listing(RADARR_LIST)).unwrap();
        let err = task.diff(&current).err().unwrap().to_string();
        assert!(err.ends_with("Bluray-9000p, VHS"), "{err}");
    }

    #[test]
    fn qualities_outside_the_spec_are_a_note() {
        let task = task(r#"{"CAM": {"min":0,"preferred":95,"max":100}}"#);
        let current = task.read(&listing(RADARR_LIST)).unwrap();
        let notes = task.notes(&current);
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("Bluray-1080p") && !notes[0].contains("CAM,"));
    }

    #[test]
    fn empty_list_and_nameless_entry_are_errors() {
        let t = task(RADARR_DESIRED);
        assert!(t.read(&listing("[]")).err().unwrap().to_string().contains("empty list"));
        let nameless = r#"[{"quality":{"id":0},"minSize":0}]"#;
        assert!(t.read(&listing(nameless)).err().unwrap().to_string().contains("no quality name"));
        let err = t.read(&listing(r#"[{"minSize":0}]"#)).err().unwrap().to_string();
        assert!(err.contains("quality"), "{err}");
    }

    #[test]
    fn write_sends_the_full_list_with_explicit_nulls_and_unknown_fields() {
        let changed = RADARR_DESIRED.replacen(r#""min": 12.5"#, r#""min": 35"#, 1);
        let task = task(&changed);
        let transport = listing(RADARR_LIST).on_put(vec![Step::Answer(202, String::new())]);
        let current = task.read(&transport).unwrap();
        task.write(&transport, &current).unwrap();
        let written = transport.written.borrow();
        assert_eq!(written[0].0, QUALITY_UPDATE.path);
        let sent: serde_json::Value = serde_json::from_str(&written[0].1).unwrap();
        let entry = sent.as_array().unwrap().iter().find(|e| e["quality"]["name"] == "Bluray-1080p").unwrap();
        assert_eq!(entry["minSize"], 35.0);
        assert_eq!(entry["maxSize"], serde_json::Value::Null);
        assert_eq!(entry["quality"]["modifier"], "none");
        assert_eq!(sent.as_array().unwrap().len(), current.len());
    }

    #[test]
    fn write_rejects_other_statuses() {
        let task = task(RADARR_DESIRED);
        let transport = listing(RADARR_LIST).on_put(vec![Step::Answer(500, "boom".into())]);
        let current = task.read(&transport).unwrap();
        assert_eq!(task.write(&transport, &current).err().unwrap().to_string(),
            "PUT /api/v3/qualitydefinition/update answered HTTP 500");
    }

    #[test]
    fn probe_distinguishes_not_yet_from_fatal() {
        let t = task(RADARR_DESIRED);
        let up = FakeTransport::default().on_get(SYSTEM_STATUS.path, vec![ok(RADARR_STATUS)]);
        assert_eq!(t.probe(&up).ok().unwrap(), "6.3.0.10514");
        let refused = FakeTransport::default().on_get(SYSTEM_STATUS.path, vec![Step::Refused]);
        assert!(matches!(t.probe(&refused), Err(Probe::NotYet(_))));
        let starting = FakeTransport::default().on_get(SYSTEM_STATUS.path, vec![Step::Answer(503, String::new())]);
        assert!(matches!(t.probe(&starting), Err(Probe::NotYet(_))));
        let wrong_key = FakeTransport::default().on_get(SYSTEM_STATUS.path, vec![Step::Answer(401, String::new())]);
        assert!(matches!(t.probe(&wrong_key), Err(Probe::Fatal(_))));
    }
```

- [ ] **Step 2: run, expect FAIL.**

- [ ] **Step 3: `src/engine.rs` (first part)**

```rust
use std::fmt;

use crate::{client::Transport, error::Error};

#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub subject: String,
    pub field: &'static str,
    pub current: String,
    pub desired: String,
}

impl fmt::Display for Change {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {} {} -> {}", self.subject, self.field, self.current, self.desired)
    }
}

/// Why a readiness probe did not return a version.
pub enum Probe {
    /// Worth waiting for: refused, 5xx, no version yet.
    NotYet(String),
    /// Waiting will not help: a wrong key stays wrong.
    Fatal(Error),
}

pub trait Task {
    type Current;
    fn probe(&self, t: &dyn Transport) -> Result<String, Probe>;
    fn read(&self, t: &dyn Transport) -> Result<Self::Current, Error>;
    fn diff(&self, current: &Self::Current) -> Result<Vec<Change>, Error>;
    fn notes(&self, current: &Self::Current) -> Vec<String>;
    fn write(&self, t: &dyn Transport, current: &Self::Current) -> Result<(), Error>;
}
```

`src/lib.rs`: add `pub mod engine;`.

- [ ] **Step 4: `QualityDefinitions` in `arr.rs`** (additional imports: `std::collections::{BTreeMap, BTreeSet}`, `crate::{client::{expect_status, Transport}, engine::{Change, Probe, Task}, error::Error, spec::{show, SizeLimits}}`)

```rust
pub struct QualityDefinitions {
    pub desired: BTreeMap<String, SizeLimits>,
}

impl Task for QualityDefinitions {
    type Current = Vec<QualityDefinitionResource>;

    fn probe(&self, t: &dyn Transport) -> Result<String, Probe> {
        let reply = t.get(SYSTEM_STATUS.path).map_err(|e| Probe::NotYet(e.to_string()))?;
        match reply.status {
            200 => {}
            401 | 403 => {
                return Err(Probe::Fatal(Error::Status {
                    method: SYSTEM_STATUS.method,
                    path: SYSTEM_STATUS.path.to_string(),
                    status: reply.status,
                    validation: vec!["the API key was refused".to_string()],
                }))
            }
            other => return Err(Probe::NotYet(format!("HTTP {other}"))),
        }
        let status: SystemResource = serde_json::from_str(&reply.body)
            .map_err(|e| Probe::NotYet(format!("unexpected answer: {e}")))?;
        status
            .version
            .filter(|v| !v.is_empty())
            .ok_or_else(|| Probe::NotYet("the answer has no version".to_string()))
    }

    fn read(&self, t: &dyn Transport) -> Result<Self::Current, Error> {
        let path = QUALITY_LIST.path.to_string();
        let reply = t.get(QUALITY_LIST.path)?;
        expect_status(&QUALITY_LIST, &reply, &[200])?;
        let list: Vec<QualityDefinitionResource> = serde_json::from_str(&reply.body)
            .map_err(|e| Error::Decode { path: path.clone(), reason: e.to_string() })?;
        if list.is_empty() {
            return Err(Error::EmptyList { path });
        }
        if let Some(index) = list.iter().position(|d| d.quality.name.is_none()) {
            return Err(Error::MissingName { path, index });
        }
        Ok(list)
    }

    fn diff(&self, current: &Self::Current) -> Result<Vec<Change>, Error> {
        let known: BTreeSet<&str> =
            current.iter().filter_map(|d| d.quality.name.as_deref()).collect();
        let unknown: Vec<String> = self
            .desired
            .keys()
            .filter(|name| !known.contains(name.as_str()))
            .cloned()
            .collect();
        if !unknown.is_empty() {
            return Err(Error::UnknownQualities(unknown));
        }
        let mut changes = Vec::new();
        for entry in current {
            let Some(name) = entry.quality.name.as_deref() else { continue };
            let Some(want) = self.desired.get(name) else { continue };
            for (field, have, wanted) in [
                ("min", entry.min_size, want.min),
                ("preferred", entry.preferred_size, want.preferred),
                ("max", entry.max_size, want.max),
            ] {
                if have != wanted {
                    changes.push(Change {
                        subject: name.to_string(),
                        field,
                        current: show(have),
                        desired: show(wanted),
                    });
                }
            }
        }
        Ok(changes)
    }

    fn notes(&self, current: &Self::Current) -> Vec<String> {
        let untouched: Vec<&str> = current
            .iter()
            .filter_map(|d| d.quality.name.as_deref())
            .filter(|name| !self.desired.contains_key(*name))
            .collect();
        if untouched.is_empty() {
            Vec::new()
        } else {
            vec![format!("not in the spec, left as they are: {}", untouched.join(", "))]
        }
    }

    fn write(&self, t: &dyn Transport, current: &Self::Current) -> Result<(), Error> {
        let updated: Vec<QualityDefinitionResource> = current
            .iter()
            .cloned()
            .map(|mut entry| {
                let want = entry.quality.name.as_deref().and_then(|n| self.desired.get(n)).copied();
                if let Some(want) = want {
                    entry.min_size = want.min;
                    entry.preferred_size = want.preferred;
                    entry.max_size = want.max;
                }
                entry
            })
            .collect();
        let body = serde_json::to_string(&updated).map_err(|e| Error::Request {
            method: QUALITY_UPDATE.method,
            path: QUALITY_UPDATE.path.to_string(),
            reason: format!("cannot serialize: {e}"),
        })?;
        let reply = t.put_json(QUALITY_UPDATE.path, &body)?;
        // 202 is what Radarr actually answers (observed 2026-09-06), although
        // its OpenAPI file lists only 200 -- and 202 means accepted, not
        // saved, which is why the engine reads back afterwards.
        expect_status(&QUALITY_UPDATE, &reply, &[200, 202])
    }
}
```

- [ ] **Step 5: run all three, expect PASS.** If `one_changed_field_is_one_change` finds the `"min": 12.5` string belongs to a quality other than Bluray-1080p (the table has several at 12.5? check: `jq` over `desired.json`), change the replacement to target the Bluray-1080p object explicitly via `serde_json::Value` mutation instead of `replacen`.
- [ ] **Step 6: commit** — "quality-definitions: read, diff, notes, write".

---

### Task 6: The engine — readiness, apply, plan, read-back, deadline

**Files:** Modify `src/engine.rs`.

**Interfaces — Produces:** `Timing { ready_timeout, readback_timeout, poll_interval, deadline: Duration }` with `Default` (120/60/2/300 s); `Mode::{Apply, Plan}`; `Outcome::{Unchanged, Changed(Vec<Change>), Differs(Vec<Change>)}`; `Report { version: String, notes: Vec<String>, outcome: Outcome }`; `run<T: Task>(mode: Mode, task: &T, t: &dyn Transport, clock: &dyn Clock, timing: Timing) -> Result<Report, Error>`.

- [ ] **Step 1: failing tests (`src/engine.rs`)**

```rust
#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::{
        services::arr::{QualityDefinitions, QUALITY_LIST, SYSTEM_STATUS},
        spec::SizeLimits,
        testing::{ok, FakeClock, FakeTransport, Step},
    };

    const STATUS: &str = include_str!("../tests/fixtures/radarr-6.3.0.10514/system-status.json");
    const LIST: &str = include_str!("../tests/fixtures/radarr-6.3.0.10514/qualitydefinition.json");
    const DESIRED: &str = include_str!("../tests/fixtures/radarr-6.3.0.10514/desired.json");

    fn task(desired: &str) -> QualityDefinitions {
        let desired: BTreeMap<String, SizeLimits> = serde_json::from_str(desired).unwrap();
        QualityDefinitions { desired }
    }

    fn with_bluray_min(min: f64) -> String {
        let mut list: serde_json::Value = serde_json::from_str(LIST).unwrap();
        for e in list.as_array_mut().unwrap() {
            if e["quality"]["name"] == "Bluray-1080p" {
                e["minSize"] = min.into();
            }
        }
        list.to_string()
    }

    fn desired_bluray_min(min: f64) -> String {
        let mut d: serde_json::Value = serde_json::from_str(DESIRED).unwrap();
        d["Bluray-1080p"]["min"] = min.into();
        d.to_string()
    }

    #[test]
    fn unchanged_never_writes() {
        let t = FakeTransport::default().on_get(SYSTEM_STATUS.path, vec![ok(STATUS)]).on_get(QUALITY_LIST.path, vec![ok(LIST)]);
        let report = run(Mode::Apply, &task(DESIRED), &t, &FakeClock::new(), Timing::default()).unwrap();
        assert_eq!(report.outcome, Outcome::Unchanged);
        assert_eq!(report.version, "6.3.0.10514");
        assert!(t.written.borrow().is_empty());
    }

    #[test]
    fn waits_for_readiness_then_proceeds() {
        let t = FakeTransport::default()
            .on_get(SYSTEM_STATUS.path, vec![Step::Refused, Step::Answer(503, String::new()), ok(STATUS)])
            .on_get(QUALITY_LIST.path, vec![ok(LIST)]);
        let clock = FakeClock::new();
        run(Mode::Apply, &task(DESIRED), &t, &clock, Timing::default()).unwrap();
        assert_eq!(clock.elapsed(), Duration::from_secs(4));
    }

    #[test]
    fn gives_up_on_readiness_after_its_timeout() {
        let t = FakeTransport::default().on_get(SYSTEM_STATUS.path, vec![Step::Refused]);
        let err = run(Mode::Apply, &task(DESIRED), &t, &FakeClock::new(), Timing::default()).err().unwrap().to_string();
        assert!(err.starts_with("service not ready after 120 s"), "{err}");
        assert!(err.contains("connection refused"));
    }

    #[test]
    fn a_refused_key_fails_at_once() {
        let t = FakeTransport::default().on_get(SYSTEM_STATUS.path, vec![Step::Answer(401, String::new())]);
        let clock = FakeClock::new();
        assert!(run(Mode::Apply, &task(DESIRED), &t, &clock, Timing::default()).is_err());
        assert_eq!(clock.elapsed(), Duration::ZERO);
    }

    #[test]
    fn plan_reports_without_writing() {
        let t = FakeTransport::default().on_get(SYSTEM_STATUS.path, vec![ok(STATUS)]).on_get(QUALITY_LIST.path, vec![ok(LIST)]);
        let report = run(Mode::Plan, &task(&desired_bluray_min(35.0)), &t, &FakeClock::new(), Timing::default()).unwrap();
        assert!(matches!(report.outcome, Outcome::Differs(ref c) if c.len() == 1));
        assert!(t.written.borrow().is_empty());
    }

    #[test]
    fn accepted_is_not_saved_so_it_reads_back_until_it_is() {
        // Radarr's 202: the first two reads after the write still show the old value.
        let t = FakeTransport::default()
            .on_get(SYSTEM_STATUS.path, vec![ok(STATUS)])
            .on_get(QUALITY_LIST.path, vec![ok(LIST), ok(LIST), ok(LIST), ok(&with_bluray_min(35.0))])
            .on_put(vec![Step::Answer(202, String::new())]);
        let clock = FakeClock::new();
        let report = run(Mode::Apply, &task(&desired_bluray_min(35.0)), &t, &clock, Timing::default()).unwrap();
        assert!(matches!(report.outcome, Outcome::Changed(ref c) if c[0].to_string() == "Bluray-1080p: min 12.5 -> 35"));
        assert_eq!(t.written.borrow().len(), 1, "no second write");
        assert_eq!(clock.elapsed(), Duration::from_secs(4));
    }

    #[test]
    fn never_landing_is_an_error_naming_what_differs() {
        let t = FakeTransport::default()
            .on_get(SYSTEM_STATUS.path, vec![ok(STATUS)])
            .on_get(QUALITY_LIST.path, vec![ok(LIST)])
            .on_put(vec![Step::Answer(202, String::new())]);
        let err = run(Mode::Apply, &task(&desired_bluray_min(35.0)), &t, &FakeClock::new(), Timing::default()).err().unwrap().to_string();
        assert_eq!(err, "written, but after 60 s these still differ: Bluray-1080p: min 12.5 -> 35");
    }

    #[test]
    fn the_overall_deadline_wins_over_the_step_timeouts() {
        let t = FakeTransport::default().on_get(SYSTEM_STATUS.path, vec![Step::Refused]);
        let timing = Timing { deadline: Duration::from_secs(30), ..Timing::default() };
        let err = run(Mode::Apply, &task(DESIRED), &t, &FakeClock::new(), timing).err().unwrap().to_string();
        assert_eq!(err, "overall deadline of 30 s exceeded");
    }
}
```

- [ ] **Step 2: run, expect FAIL.**

- [ ] **Step 3: implement (append to `src/engine.rs`; add `use std::time::Duration;` and `crate::clock::Clock`)**

```rust
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    pub ready_timeout: Duration,
    pub readback_timeout: Duration,
    pub poll_interval: Duration,
    pub deadline: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            ready_timeout: Duration::from_secs(120),
            readback_timeout: Duration::from_secs(60),
            poll_interval: Duration::from_secs(2),
            deadline: Duration::from_secs(300),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Apply,
    Plan,
}

#[derive(Debug, PartialEq)]
pub enum Outcome {
    Unchanged,
    Changed(Vec<Change>),
    Differs(Vec<Change>),
}

#[derive(Debug)]
pub struct Report {
    pub version: String,
    pub notes: Vec<String>,
    pub outcome: Outcome,
}

pub fn run<T: Task>(
    mode: Mode,
    task: &T,
    t: &dyn Transport,
    clock: &dyn Clock,
    timing: Timing,
) -> Result<Report, Error> {
    let start = clock.now();
    let version = wait_ready(task, t, clock, timing, start)?;
    let current = task.read(t)?;
    let notes = task.notes(&current);
    let changes = task.diff(&current)?;
    if changes.is_empty() {
        return Ok(Report { version, notes, outcome: Outcome::Unchanged });
    }
    if mode == Mode::Plan {
        return Ok(Report { version, notes, outcome: Outcome::Differs(changes) });
    }
    task.write(t, &current)?;
    let written = clock.now();
    loop {
        let remaining = task.diff(&task.read(t)?)?;
        if remaining.is_empty() {
            return Ok(Report { version, notes, outcome: Outcome::Changed(changes) });
        }
        let waited = clock.now().duration_since(written);
        if waited >= timing.readback_timeout {
            return Err(Error::NotConverged {
                waited_secs: waited.as_secs(),
                remaining: remaining.iter().map(ToString::to_string).collect(),
            });
        }
        check_deadline(clock, start, timing)?;
        clock.sleep(timing.poll_interval);
    }
}

fn wait_ready<T: Task>(
    task: &T,
    t: &dyn Transport,
    clock: &dyn Clock,
    timing: Timing,
    start: std::time::Instant,
) -> Result<String, Error> {
    let began = clock.now();
    loop {
        match task.probe(t) {
            Ok(version) => return Ok(version),
            Err(Probe::Fatal(e)) => return Err(e),
            Err(Probe::NotYet(last)) => {
                let waited = clock.now().duration_since(began);
                if waited >= timing.ready_timeout {
                    return Err(Error::NotReady { waited_secs: waited.as_secs(), last });
                }
                check_deadline(clock, start, timing)?;
                clock.sleep(timing.poll_interval);
            }
        }
    }
}

fn check_deadline(clock: &dyn Clock, start: std::time::Instant, timing: Timing) -> Result<(), Error> {
    if clock.now().duration_since(start) >= timing.deadline {
        Err(Error::Deadline { secs: timing.deadline.as_secs() })
    } else {
        Ok(())
    }
}
```

- [ ] **Step 4: run all three, expect PASS.**
- [ ] **Step 5: commit** — "Engine: readiness, plan, apply with read-back and deadline".

---

### Task 7: Schema check against OpenAPI

**Files:** Create `src/schema.rs`, `openapi/radarr-6.3.0.10514.json`, `openapi/sonarr-4.0.19.2979.json`, `openapi/SOURCE.md`, `tests/schema.rs`; modify `src/lib.rs`.

**Interfaces — Produces:** `schema::check(openapi: &serde_json::Value, endpoints: &[Endpoint], wire: &[schemars::Schema]) -> Vec<String>` (empty = match).

- [ ] **Step 1: vendor** `https://raw.githubusercontent.com/Radarr/Radarr/v6.3.0.10514/src/Radarr.Api.V3/openapi.json` and `https://raw.githubusercontent.com/Sonarr/Sonarr/v4.0.19.2979/src/Sonarr.Api.V3/openapi.json` byte-for-byte; `SOURCE.md` lists URL and `sha256sum` of each.

- [ ] **Step 2: failing `tests/schema.rs`**

```rust
use converge::{schema::check, services::arr};
use schemars::JsonSchema;
use serde_json::Value;

fn doc(name: &str) -> Value {
    let path = format!("{}/openapi/{name}.json", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn findings(openapi: &Value) -> Vec<String> {
    check(openapi, &arr::ENDPOINTS, &arr::wire_types())
}

#[test]
fn the_deployed_versions_match() {
    assert_eq!(findings(&doc("radarr-6.3.0.10514")), Vec::<String>::new());
    assert_eq!(findings(&doc("sonarr-4.0.19.2979")), Vec::<String>::new());
}

#[test]
fn a_renamed_property_is_found() {
    let mut d = doc("radarr-6.3.0.10514");
    let props = d.pointer_mut("/components/schemas/QualityDefinitionResource/properties").unwrap().as_object_mut().unwrap();
    let v = props.remove("minSize").unwrap();
    props.insert("minimumSize".into(), v);
    assert_eq!(findings(&d), ["QualityDefinitionResource.minSize: not in the OpenAPI description"]);
}

#[test]
fn a_nested_rename_is_found() {
    let mut d = doc("sonarr-4.0.19.2979");
    let props = d.pointer_mut("/components/schemas/Quality/properties").unwrap().as_object_mut().unwrap();
    props.remove("name");
    assert_eq!(findings(&d), ["Quality.name: not in the OpenAPI description"]);
}

#[test]
fn a_changed_type_is_found() {
    let mut d = doc("radarr-6.3.0.10514");
    *d.pointer_mut("/components/schemas/QualityDefinitionResource/properties/maxSize/type").unwrap() = "integer".into();
    assert_eq!(findings(&d), ["QualityDefinitionResource.maxSize: integer there, number here"]);
}

#[test]
fn a_missing_endpoint_or_wrong_body_is_found() {
    let mut d = doc("radarr-6.3.0.10514");
    d["paths"].as_object_mut().unwrap().remove("/api/v3/qualitydefinition/update");
    assert_eq!(findings(&d), ["PUT /api/v3/qualitydefinition/update does not exist"]);

    let mut d = doc("radarr-6.3.0.10514");
    d["paths"]["/api/v3/qualitydefinition"]["get"]["responses"]["200"]["content"]["application/json"]["schema"] =
        serde_json::json!({"$ref": "#/components/schemas/QualityDefinitionResource"});
    assert_eq!(findings(&d), ["GET /api/v3/qualitydefinition 200 response is not a list of QualityDefinitionResource"]);
}

mod strict {
    use super::JsonSchema;
    #[allow(dead_code)]
    #[derive(JsonSchema)]
    #[serde(rename_all = "camelCase")]
    pub struct QualityDefinitionResource {
        pub min_size: f64,
    }
    #[allow(dead_code)]
    #[derive(JsonSchema)]
    pub struct SystemResource {}
}

#[test]
fn nullable_there_but_required_here_is_found() {
    let wire = vec![schemars::schema_for!(strict::SystemResource), schemars::schema_for!(strict::QualityDefinitionResource)];
    let found = check(&doc("radarr-6.3.0.10514"), &arr::ENDPOINTS, &wire);
    assert!(found.contains(&"QualityDefinitionResource.minSize: nullable there, but not an Option here".to_string()), "{found:?}");
    // A wire type with no properties checks nothing and must say so.
    assert!(found.contains(&"wire type SystemResource declares no properties".to_string()), "{found:?}");
}

#[test]
fn a_missing_wire_type_is_found() {
    let wire = vec![schemars::schema_for!(strict::SystemResource)];
    let found = check(&doc("radarr-6.3.0.10514"), &arr::ENDPOINTS, &wire);
    assert!(found.iter().any(|f| f.ends_with("uses QualityDefinitionResource, but this program has no wire type of that name")), "{found:?}");
}
```

- [ ] **Step 3: run, expect FAIL.**

- [ ] **Step 4: implement `src/schema.rs`**

```rust
//! Compares this program's wire types with a service's OpenAPI description.
//! It checks names and types. It cannot check behaviour: Radarr's file lists
//! only 200 for the quality update, and the service answers 202.

use serde_json::Value;

use crate::endpoint::{Endpoint, Shape};

const COMPONENTS: &str = "#/components/schemas/";

pub fn check(openapi: &Value, endpoints: &[Endpoint], wire: &[schemars::Schema]) -> Vec<String> {
    let mut findings = Vec::new();
    for ep in endpoints {
        let pointer = format!("/paths/{}/{}", escape(ep.path), ep.method.to_ascii_lowercase());
        let Some(operation) = openapi.pointer(&pointer) else {
            findings.push(format!("{} {} does not exist", ep.method, ep.path));
            continue;
        };
        if let Some(shape) = ep.request {
            let schema = operation.pointer("/requestBody/content/application~1json/schema");
            check_body(&mut findings, ep, "request body", schema, shape, openapi, wire);
        }
        if let Some(shape) = ep.response {
            let schema = operation.pointer("/responses/200/content/application~1json/schema");
            check_body(&mut findings, ep, "200 response", schema, shape, openapi, wire);
        }
    }
    findings
}

/// JSON pointer escaping (RFC 6901).
fn escape(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

fn check_body(
    findings: &mut Vec<String>,
    ep: &Endpoint,
    what: &str,
    schema: Option<&Value>,
    shape: Shape,
    openapi: &Value,
    wire: &[schemars::Schema],
) {
    let label = format!("{} {} {what}", ep.method, ep.path);
    let Some(schema) = schema else {
        findings.push(format!("{label} has no application/json schema"));
        return;
    };
    let (component, reference, kind) = match shape {
        Shape::One(c) => (c, schema.get("$ref"), "one"),
        Shape::List(c) => {
            let is_array = schema.get("type").and_then(Value::as_str) == Some("array");
            (c, if is_array { schema.pointer("/items/$ref") } else { None }, "a list of")
        }
    };
    if reference.and_then(Value::as_str) != Some(format!("{COMPONENTS}{component}").as_str()) {
        findings.push(format!("{label} is not {kind} {component}"));
        return;
    }
    let Some(ours) = wire
        .iter()
        .map(schemars::Schema::as_value)
        .find(|s| s.get("title").and_then(Value::as_str) == Some(component))
    else {
        findings.push(format!("{label} uses {component}, but this program has no wire type of that name"));
        return;
    };
    compare(findings, component, ours, ours.get("$defs"), openapi);
}

fn compare(findings: &mut Vec<String>, component: &str, ours: &Value, defs: Option<&Value>, openapi: &Value) {
    let Some(theirs) = openapi
        .pointer(&format!("/components/schemas/{}/properties", escape(component)))
        .and_then(Value::as_object)
    else {
        findings.push(format!("component {component} does not exist or has no properties"));
        return;
    };
    let our_properties = ours.get("properties").and_then(Value::as_object);
    let Some(our_properties) = our_properties.filter(|p| !p.is_empty()) else {
        findings.push(format!("wire type {component} declares no properties"));
        return;
    };
    for (name, our) in our_properties {
        let at = format!("{component}.{name}");
        let Some(their) = theirs.get(name) else {
            findings.push(format!("{at}: not in the OpenAPI description"));
            continue;
        };
        let (our_type, our_nullable) = rust_type(our);
        if their.get("nullable").and_then(Value::as_bool) == Some(true) && !our_nullable {
            findings.push(format!("{at}: nullable there, but not an Option here"));
        }
        let their_ref = their.get("$ref").and_then(Value::as_str);
        match (our_type, their_ref) {
            (RustType::Ref(our_name), Some(their_ref)) => {
                let their_name = their_ref.strip_prefix(COMPONENTS).unwrap_or(their_ref);
                if our_name != their_name {
                    findings.push(format!("{at}: refers to {their_name} there, {our_name} here"));
                } else if let Some(nested) = defs.and_then(|d| d.get(&our_name)) {
                    compare(findings, &our_name, nested, defs, openapi);
                } else {
                    findings.push(format!("{at}: wire type {our_name} is not defined"));
                }
            }
            (RustType::Plain(ours), None) => {
                let their_type = their.get("type").and_then(Value::as_str).unwrap_or("untyped");
                if their_type != ours {
                    findings.push(format!("{at}: {their_type} there, {ours} here"));
                }
            }
            (ours, theirs) => findings.push(format!(
                "{at}: {} here, {} there",
                ours.describe(),
                theirs.map_or("a plain type".to_string(), |r| format!("a reference to {r}"))
            )),
        }
    }
}

enum RustType {
    Plain(String),
    Ref(String),
    Unknown,
}

impl RustType {
    fn describe(&self) -> String {
        match self {
            RustType::Plain(t) => format!("plain {t}"),
            RustType::Ref(r) => format!("a reference to {r}"),
            RustType::Unknown => "an unrecognised schema".to_string(),
        }
    }
}

/// The base type of a schemars property and whether it admits null.
fn rust_type(v: &Value) -> (RustType, bool) {
    if let Some(r) = v.get("$ref").and_then(Value::as_str) {
        return (RustType::Ref(r.trim_start_matches("#/$defs/").to_string()), false);
    }
    // `Option<Struct>` is rendered as anyOf [ {$ref}, {type: null} ].
    if let Some(any) = v.get("anyOf").and_then(Value::as_array) {
        let is_null = |x: &&Value| x.get("type").and_then(Value::as_str) == Some("null");
        let nullable = any.iter().any(|x| is_null(&x));
        let inner = any.iter().find(|x| !is_null(x));
        return (inner.map_or(RustType::Unknown, |i| rust_type(i).0), nullable);
    }
    match v.get("type") {
        Some(Value::String(t)) => (RustType::Plain(t.clone()), false),
        Some(Value::Array(types)) => {
            let names: Vec<&str> = types.iter().filter_map(Value::as_str).collect();
            let nullable = names.contains(&"null");
            let base = names.into_iter().find(|t| *t != "null");
            (base.map_or(RustType::Unknown, |t| RustType::Plain(t.to_string())), nullable)
        }
        _ => (RustType::Unknown, false),
    }
}
```

`src/lib.rs`: add `pub mod schema;`.

- [ ] **Step 5: run all three, expect PASS.** `a_nested_rename_is_found` depends on `Quality` being reached through `QualityDefinitionResource.quality`'s `$ref`; if schemars renders the nested definition under a different title than `Quality`, the test shows it — fix by `#[schemars(rename = "Quality")]`, not by loosening the check.
- [ ] **Step 6: commit** — "schema-check: wire types against the OpenAPI file of the deployed version".

---

### Task 8: CLI, README, green flake

**Files:** Replace `src/main.rs`; create `tests/cli.rs`, `README.md`.

- [ ] **Step 1: failing `tests/cli.rs`**

```rust
mod support;

use std::process::Command;

use support::Server;

const STATUS: &str = include_str!("fixtures/radarr-6.3.0.10514/system-status.json");
const LIST: &str = include_str!("fixtures/radarr-6.3.0.10514/qualitydefinition.json");
const DESIRED: &str = include_str!("fixtures/radarr-6.3.0.10514/desired.json");

fn converge() -> Command {
    Command::new(env!("CARGO_BIN_EXE_converge"))
}

fn spec(dir: &std::path::Path, base: &str, desired: &str) -> std::path::PathBuf {
    let path = dir.join("spec.json");
    let text = format!(
        r#"{{"service":"radarr","base_url":"{base}","api_key_credential":"radarr-api-key","task":"quality-definitions","desired":{desired}}}"#
    );
    std::fs::write(&path, text).unwrap();
    std::fs::write(dir.join("radarr-api-key"), "k\n").unwrap();
    path
}

#[test]
fn no_arguments_is_usage_and_exit_1() {
    let out = converge().output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("usage:"));
}

#[test]
fn schema_check_passes_for_the_vendored_file() {
    let file = format!("{}/openapi/radarr-6.3.0.10514.json", env!("CARGO_MANIFEST_DIR"));
    let out = converge().args(["schema-check", "--service", "radarr", "--openapi", &file]).output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn plan_is_0_when_equal_and_2_when_it_differs() {
    let server = Server::start(vec![
        ("GET", "/api/v3/system/status", 200, STATUS.into()),
        ("GET", "/api/v3/qualitydefinition", 200, LIST.into()),
    ]);
    let dir = tempfile::tempdir().unwrap();

    let path = spec(dir.path(), &server.base_url(), DESIRED);
    let out = converge().arg("plan").arg(&path).env("CREDENTIALS_DIRECTORY", dir.path()).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}{}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("radarr quality-definitions: unchanged"), "{stdout}");

    let mut changed: serde_json::Value = serde_json::from_str(DESIRED).unwrap();
    changed["Bluray-1080p"]["min"] = 35.into();
    let path = spec(dir.path(), &server.base_url(), &changed.to_string());
    let out = converge().arg("plan").arg(&path).env("CREDENTIALS_DIRECTORY", dir.path()).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(2), "{stdout}");
    assert!(stdout.contains("would change Bluray-1080p: min 12.5 -> 35"), "{stdout}");
    assert!(server.requests().iter().all(|r| r.method == "GET"), "plan must not write");
}

#[test]
fn a_bad_spec_fails_but_the_next_spec_still_runs() {
    let server = Server::start(vec![
        ("GET", "/api/v3/system/status", 200, STATUS.into()),
        ("GET", "/api/v3/qualitydefinition", 200, LIST.into()),
    ]);
    let dir = tempfile::tempdir().unwrap();
    let bad = dir.path().join("bad.json");
    std::fs::write(&bad, r#"{"service":"radarr"}"#).unwrap();
    let good = spec(dir.path(), &server.base_url(), DESIRED);
    let out = converge().arg("apply").arg(&bad).arg(&good).env("CREDENTIALS_DIRECTORY", dir.path()).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("bad.json"));
    assert!(String::from_utf8_lossy(&out.stdout).contains("unchanged"));
}
```

- [ ] **Step 2: run, expect FAIL.**

- [ ] **Step 3: `src/main.rs`**

```rust
use std::{
    path::{Path, PathBuf},
    process::ExitCode,
    time::Duration,
};

use converge::{
    client::{read_credential, HttpTransport},
    clock::SystemClock,
    engine::{run, Mode, Outcome, Timing},
    schema,
    services::arr,
    spec::{Desired, Spec},
};

const USAGE: &str = "usage:
  converge apply [--deadline <seconds>] <spec.json>...
  converge plan [--deadline <seconds>] <spec.json>...
  converge schema-check --service <radarr|sonarr> --openapi <file>";

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

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
    for path in &paths {
        match reconcile_one(mode, path, credentials.as_deref(), timing) {
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

/// Prints its own lines. `Ok(true)` means a plan found a difference.
fn reconcile_one(mode: Mode, path: &Path, credentials: Option<&Path>, timing: Timing) -> Result<bool, ()> {
    let spec = Spec::load(path).map_err(|e| eprintln!("{}: error: {e}", path.display()))?;
    let label = format!("{} {}", spec.service.name(), spec.task_name());
    let fail = |e: converge::error::Error| eprintln!("{label}: error: {e}");
    let key = read_credential(credentials, &spec.api_key_credential).map_err(fail)?;
    let transport = HttpTransport::new(&spec.base_url, key, REQUEST_TIMEOUT);
    let report = match &spec.desired {
        Desired::QualityDefinitions(desired) => {
            let task = arr::QualityDefinitions { desired: desired.clone() };
            run(mode, &task, &transport, &SystemClock, timing)
        }
    }
    .map_err(fail)?;
    println!("{label}: service version {}", report.version);
    for note in &report.notes {
        println!("{label}: note: {note}");
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
            println!("{label}: changed {} field(s), read back and confirmed", changes.len());
            Ok(false)
        }
    }
}

fn schema_check(args: &[String]) -> ExitCode {
    let (mut service, mut openapi) = (None, None);
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--service" => service = rest.next().cloned(),
            "--openapi" => openapi = rest.next().map(PathBuf::from),
            other => return usage(Some(&format!("unexpected argument {other}"))),
        }
    }
    let (Some(service), Some(openapi)) = (service, openapi) else {
        return usage(Some("schema-check needs --service and --openapi"));
    };
    if !matches!(service.as_str(), "radarr" | "sonarr") {
        return usage(Some(&format!("unknown service {service:?}")));
    }
    let document = match std::fs::read_to_string(&openapi)
        .map_err(|e| e.to_string())
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).map_err(|e| e.to_string()))
    {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{service}: cannot read {}: {e}", openapi.display());
            return ExitCode::from(1);
        }
    };
    let findings = schema::check(&document, &arr::ENDPOINTS, &arr::wire_types());
    if findings.is_empty() {
        println!(
            "{service}: {} endpoints and their wire types match {}",
            arr::ENDPOINTS.len(),
            openapi.display()
        );
        ExitCode::SUCCESS
    } else {
        for finding in &findings {
            eprintln!("{service}: {finding}");
        }
        ExitCode::from(1)
    }
}
```

- [ ] **Step 4: run all three, expect PASS.**
- [ ] **Step 5: README.md** — what it does, the spec format, the three commands with exit codes, the three levels of field checks, a NixOS unit example with `LoadCredential`, license.
- [ ] **Step 6: `nix flake check`** — all of `package`, `clippy`, `fmt` build. Fix `renovate.json`'s description (it still names signal-cli and Seerr).
- [ ] **Step 7: commit** — "CLI, README".

---

### Task 9: Publish

- [ ] `gh repo create achimcc/converge --public --source ~/Projects/converge --description "Reconcile self-hosted services with a desired state through their HTTP APIs"`; `git push -u origin main`.
- [ ] Tag `v0.1.0` (signed: `git tag -s v0.1.0 -m v0.1.0`), push the tag.
- [ ] Verify: `nix build github:achimcc/converge/v0.1.0 --no-link` succeeds and `gh repo view achimcc/converge --json visibility` says `PUBLIC`.

---

### Task 10: Host integration (homeserver repo, German, own worktree)

Follows the host's own `CLAUDE.md`: own worktree, stage by name, `origin/main` fetched before deploy.

- [ ] Flake input `converge = { url = "github:achimcc/converge/v0.1.0"; inputs.nixpkgs.follows = "nixpkgs"; };` with a German comment; `nix flake lock --update-input converge`.
- [ ] `media-01.nix`: the unit `qualitaetsgroessen` keeps name, timer, `after` and `LoadCredential`; `script` is replaced by `ExecStart = "${converge}/bin/converge apply ${specDatei "radarr"} ${specDatei "sonarr"}"` where `specDatei` renders `{ service; base_url = "http://localhost:${port}"; api_key_credential = "${dienst}-api-key"; task = "quality-definitions"; desired = qg.${dienst}; }` via `builtins.toJSON`. `TimeoutStartSec` stays `10min` (> 2 × 300 s).
- [ ] Remove the now unused `groessenDatei` and adjust any comment or check that describes the shell script (`grep -n qualitaetsgroessen checks.nix scripts/`).
- [x] Built into the unit's spec derivation instead of a flake check (design §4). Originally planned as flake check `converge-schema`: a table `lib/openapi-quellen.nix` `{ radarr."6.3.0.10514" = "<sha256>"; sonarr."4.0.19.2979" = "<sha256>"; }`; a derivation fetching the file for the *deployed* package's version (`config.services.radarr.package.version` of `media-01`) and running `converge schema-check`. A version missing from the table throws with the `nix-prefetch-url` command in the message. Prove it can go red: temporarily point the table's radarr hash at a file with `minSize` renamed (sabotage case, taken back by copying the file, not via git).
- [ ] Build: `nix build .#nixosConfigurations.server.config.system.build.toplevel --no-link` and the new flake check.
- [ ] Commit (German message), merge `origin/main`, push, `just deploy`.
- [ ] Acceptance on the running host: `systemctl -M media-01 start qualitaetsgroessen`, journal shows `service version` and `unchanged` for both, and **no PUT** in Radarr's log for that minute; `converge plan` run by hand inside the unit's credentials (`systemd-run --machine=media-01 -p LoadCredential=radarr-api-key … converge plan <spec>`) exits 0.
