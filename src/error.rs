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
    #[error("entry {index} of {path} has no name")]
    MissingName { path: String, index: usize },
    #[error("no top-level item for these qualities: {}", .0.join("; "))]
    MissingItem(Vec<String>),
    #[error("the answer has no such field: {}", .0.join("; "))]
    MissingField(Vec<String>),
    #[error("not found on the service: {}", .0.join("; "))]
    NotFound(Vec<String>),
    #[error("the spec does not fit the service: {}", .0.join("; "))]
    Mismatch(Vec<String>),
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
