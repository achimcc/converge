use std::{
    fmt,
    time::{Duration, Instant},
};

use crate::{client::Transport, clock::Clock, error::Error};

/// One field that differs between the service and the spec.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub subject: String,
    pub field: &'static str,
    pub current: String,
    pub desired: String,
}

impl fmt::Display for Change {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: {} {} -> {}",
            self.subject, self.field, self.current, self.desired
        )
    }
}

/// Why a readiness probe did not return a version.
pub enum Probe {
    /// Worth waiting for: refused, 5xx, no version yet.
    NotYet(String),
    /// Waiting will not help: a wrong key stays wrong.
    Fatal(Error),
}

/// One kind of reconciliation. The engine owns waiting, reading back and the
/// deadline; a task only knows its service's API.
pub trait Task {
    type Current;
    fn probe(&self, t: &dyn Transport) -> Result<String, Probe>;
    fn read(&self, t: &dyn Transport) -> Result<Self::Current, Error>;
    fn diff(&self, current: &Self::Current) -> Result<Vec<Change>, Error>;
    fn notes(&self, current: &Self::Current) -> Vec<String>;
    fn write(&self, t: &dyn Transport, current: &Self::Current) -> Result<(), Error>;
}

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
    /// Written and read back.
    Changed(Vec<Change>),
    /// `plan` only: what `apply` would change.
    Differs(Vec<Change>),
}

#[derive(Debug)]
pub struct Report {
    pub version: String,
    pub notes: Vec<String>,
    pub outcome: Outcome,
}

/// probe -> read -> diff -> (write -> read back until equal).
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
        return Ok(Report {
            version,
            notes,
            outcome: Outcome::Unchanged,
        });
    }
    if mode == Mode::Plan {
        return Ok(Report {
            version,
            notes,
            outcome: Outcome::Differs(changes),
        });
    }
    task.write(t, &current)?;
    let written = clock.now();
    loop {
        // An accepted write is not a saved one: ask for the content.
        let remaining = task.diff(&task.read(t)?)?;
        if remaining.is_empty() {
            return Ok(Report {
                version,
                notes,
                outcome: Outcome::Changed(changes),
            });
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
    start: Instant,
) -> Result<String, Error> {
    let began = clock.now();
    loop {
        match task.probe(t) {
            Ok(version) => return Ok(version),
            Err(Probe::Fatal(e)) => return Err(e),
            Err(Probe::NotYet(last)) => {
                let waited = clock.now().duration_since(began);
                if waited >= timing.ready_timeout {
                    return Err(Error::NotReady {
                        waited_secs: waited.as_secs(),
                        last,
                    });
                }
                check_deadline(clock, start, timing)?;
                clock.sleep(timing.poll_interval);
            }
        }
    }
}

fn check_deadline(clock: &dyn Clock, start: Instant, timing: Timing) -> Result<(), Error> {
    if clock.now().duration_since(start) >= timing.deadline {
        Err(Error::Deadline {
            secs: timing.deadline.as_secs(),
        })
    } else {
        Ok(())
    }
}

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

    fn list_with_bluray_min(min: f64) -> String {
        let mut list: serde_json::Value = serde_json::from_str(LIST).unwrap();
        for entry in list.as_array_mut().unwrap() {
            if entry["quality"]["name"] == "Bluray-1080p" {
                entry["minSize"] = min.into();
            }
        }
        list.to_string()
    }

    fn desired_with_bluray_min(min: f64) -> String {
        let mut desired: serde_json::Value = serde_json::from_str(DESIRED).unwrap();
        desired["Bluray-1080p"]["min"] = min.into();
        desired.to_string()
    }

    fn up() -> FakeTransport {
        FakeTransport::default().on_get(SYSTEM_STATUS.path, vec![ok(STATUS)])
    }

    #[test]
    fn unchanged_never_writes() {
        let t = up().on_get(QUALITY_LIST.path, vec![ok(LIST)]);
        let report = run(
            Mode::Apply,
            &task(DESIRED),
            &t,
            &FakeClock::new(),
            Timing::default(),
        )
        .unwrap();
        assert_eq!(report.outcome, Outcome::Unchanged);
        assert_eq!(report.version, "6.3.0.10514");
        assert!(t.written.borrow().is_empty());
    }

    #[test]
    fn waits_for_readiness_then_proceeds() {
        let t = FakeTransport::default()
            .on_get(
                SYSTEM_STATUS.path,
                vec![Step::Refused, Step::Answer(503, String::new()), ok(STATUS)],
            )
            .on_get(QUALITY_LIST.path, vec![ok(LIST)]);
        let clock = FakeClock::new();
        run(Mode::Apply, &task(DESIRED), &t, &clock, Timing::default()).unwrap();
        assert_eq!(clock.elapsed(), Duration::from_secs(4));
    }

    #[test]
    fn gives_up_on_readiness_after_its_timeout() {
        let t = FakeTransport::default().on_get(SYSTEM_STATUS.path, vec![Step::Refused]);
        let err = run(
            Mode::Apply,
            &task(DESIRED),
            &t,
            &FakeClock::new(),
            Timing::default(),
        )
        .err()
        .unwrap()
        .to_string();
        assert!(err.starts_with("service not ready after 120 s"), "{err}");
        assert!(err.contains("connection refused"), "{err}");
    }

    #[test]
    fn a_refused_key_fails_at_once() {
        let t = FakeTransport::default()
            .on_get(SYSTEM_STATUS.path, vec![Step::Answer(401, String::new())]);
        let clock = FakeClock::new();
        assert!(run(Mode::Apply, &task(DESIRED), &t, &clock, Timing::default()).is_err());
        assert_eq!(clock.elapsed(), Duration::ZERO);
    }

    #[test]
    fn plan_reports_without_writing() {
        let t = up().on_get(QUALITY_LIST.path, vec![ok(LIST)]);
        let report = run(
            Mode::Plan,
            &task(&desired_with_bluray_min(35.0)),
            &t,
            &FakeClock::new(),
            Timing::default(),
        )
        .unwrap();
        assert!(matches!(report.outcome, Outcome::Differs(ref c) if c.len() == 1));
        assert!(t.written.borrow().is_empty());
    }

    #[test]
    fn accepted_is_not_saved_so_it_reads_back_until_it_is() {
        // Radarr's 202: two reads after the write still show the old value.
        let new = list_with_bluray_min(35.0);
        let t = up()
            .on_get(
                QUALITY_LIST.path,
                vec![ok(LIST), ok(LIST), ok(LIST), ok(&new)],
            )
            .on_put(vec![Step::Answer(202, String::new())]);
        let clock = FakeClock::new();
        let report = run(
            Mode::Apply,
            &task(&desired_with_bluray_min(35.0)),
            &t,
            &clock,
            Timing::default(),
        )
        .unwrap();
        match report.outcome {
            Outcome::Changed(changes) => {
                assert_eq!(changes.len(), 1);
                assert_eq!(changes[0].to_string(), "Bluray-1080p: min 12.5 -> 35");
            }
            other => panic!("expected Changed, got {other:?}"),
        }
        assert_eq!(t.written.borrow().len(), 1, "no second write");
        assert_eq!(clock.elapsed(), Duration::from_secs(4));
    }

    #[test]
    fn never_landing_is_an_error_naming_what_differs() {
        let t = up()
            .on_get(QUALITY_LIST.path, vec![ok(LIST)])
            .on_put(vec![Step::Answer(202, String::new())]);
        let err = run(
            Mode::Apply,
            &task(&desired_with_bluray_min(35.0)),
            &t,
            &FakeClock::new(),
            Timing::default(),
        )
        .err()
        .unwrap()
        .to_string();
        assert_eq!(
            err,
            "written, but after 60 s these still differ: Bluray-1080p: min 12.5 -> 35"
        );
    }

    #[test]
    fn the_overall_deadline_wins_over_the_step_timeouts() {
        let t = FakeTransport::default().on_get(SYSTEM_STATUS.path, vec![Step::Refused]);
        let timing = Timing {
            deadline: Duration::from_secs(30),
            ..Timing::default()
        };
        let err = run(Mode::Apply, &task(DESIRED), &t, &FakeClock::new(), timing)
            .err()
            .unwrap()
            .to_string();
        assert_eq!(err, "overall deadline of 30 s exceeded");
    }
}
