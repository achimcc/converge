use std::fmt;

use crate::{client::Transport, error::Error};

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
