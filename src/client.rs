use std::{fs, path::Path};

use crate::{error::Error, secret::Secret};

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
        let err = read_credential(Some(dir.path()), "radarr-api-key")
            .err()
            .unwrap();
        assert!(
            err.to_string().starts_with("credential radarr-api-key:"),
            "{err}"
        );
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
