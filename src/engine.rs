use std::{
    fmt,
    time::{Duration, Instant},
};

use crate::{client::Transport, clock::Clock, error::Error};

/// One field that differs between the service and the spec. A change about
/// the subject as a whole (`connection Radarr: (missing) -> (added)`) has an
/// empty `field`.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub subject: String,
    pub field: String,
    pub current: String,
    pub desired: String,
}

impl fmt::Display for Change {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.field.is_empty() {
            return write!(f, "{}: {} -> {}", self.subject, self.current, self.desired);
        }
        write!(
            f,
            "{}: {} {} -> {}",
            self.subject, self.field, self.current, self.desired
        )
    }
}

/// Shown instead of a secret's value, in changes and in errors alike.
pub const HIDDEN: &str = "(hidden)";

/// A value short enough for one line of a change. A script of three
/// kilobytes says nothing more in full than its beginning and its length.
pub fn shortened(value: &serde_json::Value) -> String {
    const KEEP: usize = 60;
    let text = value.to_string();
    let length = text.chars().count();
    if length <= KEEP + 20 {
        return text;
    }
    let start: String = text.chars().take(KEEP).collect();
    format!("{start}… ({length} characters)")
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

    /// Values the service hides when it answers (Servarr returns every
    /// password as `********`), so no diff can see them. `apply` sends them
    /// after every run, changed or not, and the service compares them itself;
    /// each returned line names what was handed over, never its value.
    /// `plan` does not call this.
    fn hand_over(
        &self,
        _t: &dyn Transport,
        _current: &Self::Current,
    ) -> Result<Vec<String>, Error> {
        Ok(Vec::new())
    }

    /// The subjects `remove` would take away: entries of the reconciled
    /// collection the spec does not name. Empty unless the spec says
    /// `"exactly": true`, and empty for every task that cannot delete
    /// (design §39) -- so by default converge removes nothing.
    ///
    /// A task's guard against a removal it should not make (half the list,
    /// a writer that has not run yet) belongs here, not in `remove`: this
    /// runs in `plan` as well, and before anything is written.
    fn surplus(&self, _current: &Self::Current) -> Result<Vec<String>, Error> {
        Ok(Vec::new())
    }

    /// Takes the surplus away. `apply` calls this **before** `write`, and a
    /// failure here stops the run before a single write goes out.
    fn remove(&self, _t: &dyn Transport, _current: &Self::Current) -> Result<(), Error> {
        Ok(())
    }
}

/// A surplus subject as a change line: `station WDR 2: (present) -> (removed)`,
/// the counterpart of the `(missing) -> (added)` a list task prints.
fn removal(subject: &str) -> Change {
    Change {
        subject: subject.to_string(),
        field: String::new(),
        current: "(present)".to_string(),
        desired: "(removed)".to_string(),
    }
}

/// What a run would do: first what it would take away, then what it would
/// set. An empty result is an unchanged service.
fn pending<T: Task>(task: &T, current: &T::Current) -> Result<Vec<Change>, Error> {
    let mut changes: Vec<Change> = task.surplus(current)?.iter().map(|s| removal(s)).collect();
    changes.extend(task.diff(current)?);
    Ok(changes)
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
    /// `apply` only: what `Task::hand_over` sent.
    pub handed_over: Vec<String>,
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
    let changes = pending(task, &current)?;
    if changes.is_empty() {
        let handed_over = match mode {
            Mode::Apply => task.hand_over(t, &current)?,
            Mode::Plan => Vec::new(),
        };
        return Ok(Report {
            version,
            notes,
            outcome: Outcome::Unchanged,
            handed_over,
        });
    }
    if mode == Mode::Plan {
        return Ok(Report {
            version,
            notes,
            outcome: Outcome::Differs(changes),
            handed_over: Vec::new(),
        });
    }
    // Removal first. A Koel station renamed in the spec but keeping its URL
    // is a removal and an addition; the other way round the addition would
    // be a `POST` the service refuses, the URL still being taken (§39).
    task.remove(t, &current)?;
    task.write(t, &current)?;
    let written = clock.now();
    loop {
        // An accepted write is not a saved one: ask for the content. An
        // accepted removal is not a finished one either -- `surplus` has to
        // come back empty as well.
        let now = task.read(t)?;
        let remaining = pending(task, &now)?;
        if remaining.is_empty() {
            // Handed over against what was read back, so an entry the write
            // just added gets its hidden values too.
            let handed_over = task.hand_over(t, &now)?;
            return Ok(Report {
                version,
                notes,
                outcome: Outcome::Changed(changes),
                handed_over,
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

    /// A task whose hand-over is visible: it records what it was handed.
    struct Handing {
        inner: QualityDefinitions,
        handed: std::cell::RefCell<Vec<usize>>,
    }

    impl Task for Handing {
        type Current = Vec<crate::services::arr::QualityDefinitionResource>;
        fn probe(&self, t: &dyn Transport) -> Result<String, Probe> {
            self.inner.probe(t)
        }
        fn read(&self, t: &dyn Transport) -> Result<Self::Current, Error> {
            self.inner.read(t)
        }
        fn diff(&self, current: &Self::Current) -> Result<Vec<Change>, Error> {
            self.inner.diff(current)
        }
        fn notes(&self, current: &Self::Current) -> Vec<String> {
            self.inner.notes(current)
        }
        fn write(&self, t: &dyn Transport, current: &Self::Current) -> Result<(), Error> {
            self.inner.write(t, current)
        }
        fn hand_over(
            &self,
            _t: &dyn Transport,
            current: &Self::Current,
        ) -> Result<Vec<String>, Error> {
            self.handed.borrow_mut().push(current.len());
            Ok(vec!["password handed over".to_string()])
        }
    }

    fn handing(desired: &str) -> Handing {
        Handing {
            inner: task(desired),
            handed: std::cell::RefCell::new(Vec::new()),
        }
    }

    /// A task with a surplus: it names entries the spec does not, removes
    /// them with one `DELETE`, and -- unless `keeps_them` -- the service
    /// stops listing them afterwards.
    struct Removing {
        inner: QualityDefinitions,
        surplus: Vec<String>,
        keeps_them: bool,
        removed: std::cell::Cell<bool>,
    }

    impl Task for Removing {
        type Current = Vec<crate::services::arr::QualityDefinitionResource>;
        fn probe(&self, t: &dyn Transport) -> Result<String, Probe> {
            self.inner.probe(t)
        }
        fn read(&self, t: &dyn Transport) -> Result<Self::Current, Error> {
            self.inner.read(t)
        }
        fn diff(&self, current: &Self::Current) -> Result<Vec<Change>, Error> {
            self.inner.diff(current)
        }
        fn notes(&self, current: &Self::Current) -> Vec<String> {
            self.inner.notes(current)
        }
        /// Like the three tasks that can remove: only what differs is
        /// written, so a run with nothing but a surplus writes nothing.
        /// (`QualityDefinitions` itself always sends the whole list.)
        fn write(&self, t: &dyn Transport, current: &Self::Current) -> Result<(), Error> {
            if self.inner.diff(current)?.is_empty() {
                return Ok(());
            }
            self.inner.write(t, current)
        }
        fn surplus(&self, _current: &Self::Current) -> Result<Vec<String>, Error> {
            if self.removed.get() && !self.keeps_them {
                return Ok(Vec::new());
            }
            Ok(self.surplus.clone())
        }
        fn remove(&self, t: &dyn Transport, _current: &Self::Current) -> Result<(), Error> {
            for name in &self.surplus {
                let path = format!("/api/v3/qualitydefinition/{name}");
                let reply = t.delete(&path)?;
                crate::client::expect_status_at("DELETE", &path, &reply, &[200, 204])?;
            }
            self.removed.set(true);
            Ok(())
        }
    }

    fn removing(desired: &str, surplus: &[&str], keeps_them: bool) -> Removing {
        Removing {
            inner: task(desired),
            surplus: surplus.iter().map(ToString::to_string).collect(),
            keeps_them,
            removed: std::cell::Cell::new(false),
        }
    }

    #[test]
    fn plan_names_a_surplus_as_a_change_and_removes_nothing() {
        let t = up().on_get(QUALITY_LIST.path, vec![ok(LIST)]);
        let report = run(
            Mode::Plan,
            &removing(DESIRED, &["Raw-HD"], false),
            &t,
            &FakeClock::new(),
            Timing::default(),
        )
        .unwrap();
        match report.outcome {
            Outcome::Differs(changes) => {
                assert_eq!(changes.len(), 1);
                assert_eq!(changes[0].to_string(), "Raw-HD: (present) -> (removed)");
            }
            other => panic!("expected Differs, got {other:?}"),
        }
        assert!(t.deleted.borrow().is_empty(), "plan removes nothing");
        assert!(t.written.borrow().is_empty());
    }

    #[test]
    fn apply_removes_before_it_writes() {
        let new = list_with_bluray_min(35.0);
        let t = up()
            .on_get(QUALITY_LIST.path, vec![ok(LIST), ok(&new)])
            .on_put(vec![Step::Answer(202, String::new())])
            .on_delete(vec![Step::Answer(204, String::new())]);
        let report = run(
            Mode::Apply,
            &removing(&desired_with_bluray_min(35.0), &["Raw-HD"], false),
            &t,
            &FakeClock::new(),
            Timing::default(),
        )
        .unwrap();
        match report.outcome {
            // The removal first, then the field that differs.
            Outcome::Changed(changes) => {
                assert_eq!(changes.len(), 2);
                assert_eq!(changes[0].to_string(), "Raw-HD: (present) -> (removed)");
                assert_eq!(changes[1].to_string(), "Bluray-1080p: min 12.5 -> 35");
            }
            other => panic!("expected Changed, got {other:?}"),
        }
        let calls = t.calls.borrow();
        assert_eq!(calls[0].0, "DELETE", "{calls:?}");
        assert_eq!(calls[1].0, "PUT", "{calls:?}");
        assert_eq!(calls.len(), 2, "{calls:?}");
    }

    #[test]
    fn a_surplus_alone_is_enough_to_act_and_nothing_is_unchanged() {
        let t = up()
            .on_get(QUALITY_LIST.path, vec![ok(LIST)])
            .on_delete(vec![Step::Answer(204, String::new())]);
        let report = run(
            Mode::Apply,
            &removing(DESIRED, &["Raw-HD"], false),
            &t,
            &FakeClock::new(),
            Timing::default(),
        )
        .unwrap();
        assert!(matches!(report.outcome, Outcome::Changed(ref c) if c.len() == 1));
        assert_eq!(t.deleted.borrow().len(), 1);
        assert!(t.written.borrow().is_empty(), "nothing to write");
    }

    #[test]
    fn an_entry_still_there_after_its_removal_is_not_confirmed() {
        let t = up()
            .on_get(QUALITY_LIST.path, vec![ok(LIST)])
            .on_delete(vec![Step::Answer(204, String::new())]);
        let err = run(
            Mode::Apply,
            &removing(DESIRED, &["Raw-HD"], true),
            &t,
            &FakeClock::new(),
            Timing::default(),
        )
        .err()
        .unwrap()
        .to_string();
        assert_eq!(
            err,
            "written, but after 60 s these still differ: Raw-HD: (present) -> (removed)"
        );
    }

    #[test]
    fn a_refused_removal_stops_the_run_before_anything_is_written() {
        let t = up()
            .on_get(QUALITY_LIST.path, vec![ok(LIST)])
            .on_put(vec![Step::Answer(202, String::new())])
            .on_delete(vec![Step::Answer(409, String::new())]);
        let err = run(
            Mode::Apply,
            &removing(&desired_with_bluray_min(35.0), &["Raw-HD"], false),
            &t,
            &FakeClock::new(),
            Timing::default(),
        )
        .err()
        .unwrap()
        .to_string();
        assert!(err.contains("409"), "{err}");
        assert!(t.written.borrow().is_empty(), "nothing written");
    }

    #[test]
    fn without_a_surplus_and_without_a_diff_it_stays_unchanged() {
        let t = up().on_get(QUALITY_LIST.path, vec![ok(LIST)]);
        let report = run(
            Mode::Apply,
            &removing(DESIRED, &[], false),
            &t,
            &FakeClock::new(),
            Timing::default(),
        )
        .unwrap();
        assert_eq!(report.outcome, Outcome::Unchanged);
        assert!(t.deleted.borrow().is_empty());
    }

    #[test]
    fn apply_hands_over_when_unchanged_and_after_reading_back_plan_never() {
        let t = up().on_get(QUALITY_LIST.path, vec![ok(LIST)]);
        let unchanged = handing(DESIRED);
        let report = run(
            Mode::Apply,
            &unchanged,
            &t,
            &FakeClock::new(),
            Timing::default(),
        )
        .unwrap();
        assert_eq!(report.outcome, Outcome::Unchanged);
        assert_eq!(report.handed_over, ["password handed over"]);
        assert_eq!(unchanged.handed.borrow().len(), 1);

        let planned = handing(&desired_with_bluray_min(35.0));
        let report = run(
            Mode::Plan,
            &planned,
            &t,
            &FakeClock::new(),
            Timing::default(),
        )
        .unwrap();
        assert!(report.handed_over.is_empty());
        assert!(
            planned.handed.borrow().is_empty(),
            "plan hands nothing over"
        );
        let report = run(
            Mode::Plan,
            &unchanged,
            &t,
            &FakeClock::new(),
            Timing::default(),
        )
        .unwrap();
        assert!(report.handed_over.is_empty());
        assert_eq!(
            unchanged.handed.borrow().len(),
            1,
            "not even when unchanged"
        );

        let new = list_with_bluray_min(35.0);
        let t = up()
            .on_get(QUALITY_LIST.path, vec![ok(LIST), ok(&new)])
            .on_put(vec![Step::Answer(202, String::new())]);
        let changed = handing(&desired_with_bluray_min(35.0));
        let report = run(
            Mode::Apply,
            &changed,
            &t,
            &FakeClock::new(),
            Timing::default(),
        )
        .unwrap();
        assert!(matches!(report.outcome, Outcome::Changed(_)));
        assert_eq!(report.handed_over, ["password handed over"]);
        assert_eq!(changed.handed.borrow().len(), 1, "once, after reading back");
    }
}
