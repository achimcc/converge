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
    // A failing spec does not skip the next one; it only decides the exit code.
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
fn reconcile_one(
    mode: Mode,
    path: &Path,
    credentials: Option<&Path>,
    timing: Timing,
) -> Result<bool, ()> {
    let spec = Spec::load(path).map_err(|e| eprintln!("{}: error: {e}", path.display()))?;
    let label = format!("{} {}", spec.service.name(), spec.task_name());
    let fail = |e: Error| eprintln!("{label}: error: {e}");
    let key = read_credential(credentials, &spec.api_key_credential).map_err(fail)?;
    let transport = HttpTransport::new(&spec.base_url, key, REQUEST_TIMEOUT);
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
            };
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
            println!(
                "{label}: changed {} field(s), read back and confirmed",
                changes.len()
            );
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
