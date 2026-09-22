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
    /// Entries the service refused to delete (design §39): each by name and
    /// with the status it answered -- never anything from its body. The
    /// removals after it were still tried.
    #[error("not removed: {}", .0.join("; "))]
    NotRemoved(Vec<String>),
    #[error("the service does not know these qualities from the spec: {}", .0.join(", "))]
    UnknownQualities(Vec<String>),
    #[error("written, but after {waited_secs} s these still differ: {}", .remaining.join("; "))]
    NotConverged {
        waited_secs: u64,
        remaining: Vec<String>,
    },
    /// The service is in a state converge will not write in, however long it
    /// waits.
    #[error("refused: {0}")]
    Refused(String),
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

/// What a decode error may say about a FOREIGN answer: its kind, where it
/// broke, and what was expected -- never the value it found (audit B40).
/// serde_json quotes the offending value (`invalid type: string "…",
/// expected f64`), and that value can be a key the service handed back; the
/// message goes to the journal and from there to Loki.
///
/// Kept: `missing field` and `duplicate field` name a field of converge's own
/// types, and everything after `, expected` is what those types expect.
pub fn shape(e: &serde_json::Error) -> String {
    let message = e.to_string();
    if message.starts_with("missing field `") || message.starts_with("duplicate field `") {
        return message;
    }
    match message.rfind(", expected ") {
        Some(i) => format!("{:?} error: {}", e.classify(), &message[i + 2..]),
        None => format!(
            "{:?} error at line {} column {}",
            e.classify(),
            e.line(),
            e.column()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::shape;

    const MARKER: &str = "ERFUNDENES-GEHEIMNIS-XYZ";

    #[derive(Debug, serde::Deserialize)]
    #[allow(dead_code)]
    struct Target {
        size: f64,
        mode: Mode,
    }

    #[derive(Debug, serde::Deserialize)]
    enum Mode {
        On,
    }

    fn error_for(json: &str) -> serde_json::Error {
        serde_json::from_str::<Target>(json).unwrap_err()
    }

    #[test]
    fn a_foreign_value_never_reaches_the_message() {
        for json in [
            format!(r#"{{"size": "{MARKER}", "mode": "On"}}"#),
            format!(r#"{{"size": 1, "mode": "{MARKER}"}}"#),
        ] {
            let e = error_for(&json);
            // Without this the test would prove nothing: serde_json itself
            // quotes the value.
            assert!(e.to_string().contains(MARKER), "{e}");
            let said = shape(&e);
            assert!(!said.contains(MARKER), "{said}");
            assert!(said.contains("line"), "{said}");
        }
    }

    #[test]
    fn what_was_expected_and_missing_fields_stay() {
        let said = shape(&error_for(&format!(
            r#"{{"size": "{MARKER}", "mode": "On"}}"#
        )));
        assert!(said.contains("expected f64"), "{said}");
        let said = shape(&error_for(r#"{"mode": "On"}"#));
        assert!(said.starts_with("missing field `size`"), "{said}");
    }
}
