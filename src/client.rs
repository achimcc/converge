use std::{fs, path::Path, time::Duration};

use serde::Deserialize;

use crate::{endpoint::Endpoint, error::Error, secret::Secret};

pub struct Reply {
    pub status: u16,
    pub body: String,
}

pub trait Transport {
    fn get(&self, path: &str) -> Result<Reply, Error>;
    fn put_json(&self, path: &str, body: &str) -> Result<Reply, Error>;
    fn post_json(&self, path: &str, body: &str) -> Result<Reply, Error>;
}

pub struct HttpTransport {
    agent: ureq::Agent,
    base_url: String,
    header: &'static str,
    key: Secret,
    accept_json: bool,
}

impl HttpTransport {
    /// `header` is where the key goes: `X-Api-Key` for Radarr and Sonarr,
    /// `X-Emby-Token` for Jellyfin.
    pub fn new(
        base_url: &str,
        header: &'static str,
        key: Secret,
        request_timeout: Duration,
    ) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(request_timeout))
            // A status is an answer, not a transport failure; the caller decides.
            .http_status_as_error(false)
            .build()
            .into();
        Self {
            agent,
            base_url: base_url.trim_end_matches('/').to_string(),
            header,
            key,
            accept_json: false,
        }
    }

    /// A transport without a key, for the one request that fetches one:
    /// SuggestArr's login. Its answer carries the token every later request
    /// travels with.
    pub fn anonymous(base_url: &str, request_timeout: Duration) -> Self {
        Self::new(base_url, "", Secret::new(String::new()), request_timeout)
    }

    /// Every request says `Accept: application/json`. Koel (Laravel) answers
    /// a request without it that it refuses with a redirect to its web page,
    /// which the agent would follow into an HTML answer with HTTP 200.
    pub fn accept_json(mut self) -> Self {
        self.accept_json = true;
        self
    }

    /// The key header, and `Accept` when asked for. An empty header name is
    /// the anonymous transport: it sends no key at all.
    fn headers<B>(&self, request: ureq::RequestBuilder<B>) -> ureq::RequestBuilder<B> {
        let request = if self.header.is_empty() {
            request
        } else {
            request.header(self.header, self.key.expose())
        };
        if self.accept_json {
            request.header("Accept", "application/json")
        } else {
            request
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
            .headers(self.agent.get(format!("{}{path}", self.base_url)))
            .call();
        Self::finish("GET", path, result)
    }

    fn put_json(&self, path: &str, body: &str) -> Result<Reply, Error> {
        let result = self
            .headers(self.agent.put(format!("{}{path}", self.base_url)))
            .content_type("application/json")
            .send(body);
        Self::finish("PUT", path, result)
    }

    fn post_json(&self, path: &str, body: &str) -> Result<Reply, Error> {
        let result = self
            .headers(self.agent.post(format!("{}{path}", self.base_url)))
            .content_type("application/json")
            .send(body);
        Self::finish("POST", path, result)
    }
}

/// Accepts the listed statuses. Anything else becomes an error that carries
/// the validation messages of the body -- and nothing else from it.
pub fn expect_status(ep: &Endpoint, reply: &Reply, accepted: &[u16]) -> Result<(), Error> {
    expect_status_at(ep.method, ep.path, reply, accepted)
}

/// The same for a path with its parameters filled in, so the error names the
/// object (`/api/v3/qualityprofile/7`) and not the template.
pub fn expect_status_at(
    method: &'static str,
    path: &str,
    reply: &Reply,
    accepted: &[u16],
) -> Result<(), Error> {
    if accepted.contains(&reply.status) {
        return Ok(());
    }
    Err(Error::Status {
        method,
        path: path.to_string(),
        status: reply.status,
        validation: validation_messages(&reply.body),
    })
}

fn validation_messages(body: &str) -> Vec<String> {
    // Laravel (Koel): `{"message": <summary>, "errors": {field: [messages]}}`.
    // The summary repeats the first message; only the fields are shown.
    #[derive(Deserialize)]
    struct Laravel {
        errors: std::collections::BTreeMap<String, Vec<String>>,
    }
    if let Ok(laravel) = serde_json::from_str::<Laravel>(body) {
        return laravel
            .errors
            .into_iter()
            .flat_map(|(field, messages)| {
                messages
                    .into_iter()
                    .map(move |message| format!("{field}: {message}"))
            })
            .collect();
    }
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
    fn expect_status_shows_only_validation_fields() {
        use crate::endpoint::Endpoint;
        const EP: Endpoint = Endpoint {
            method: "PUT",
            path: "/u",
            request: None,
            response: None,
        };
        let body = include_str!("../tests/fixtures/constructed-validation-error.json");
        let reply = Reply {
            status: 400,
            body: body.into(),
        };
        assert_eq!(
            expect_status(&EP, &reply, &[200]).err().unwrap().to_string(),
            "PUT /u answered HTTP 400: MaxSize: 'Max Size' must be greater than or equal to 'Min Size'."
        );
        let reply = Reply {
            status: 500,
            body: "<html>secret stack</html>".into(),
        };
        assert_eq!(
            expect_status(&EP, &reply, &[200])
                .err()
                .unwrap()
                .to_string(),
            "PUT /u answered HTTP 500"
        );
        let reply = Reply {
            status: 202,
            body: String::new(),
        };
        assert!(expect_status(&EP, &reply, &[200, 202]).is_ok());
    }

    #[test]
    fn expect_status_shows_laravel_validation_fields_and_nothing_else() {
        let body = include_str!("../tests/fixtures/koel-9.11.3/constructed-validation-error.json");
        let reply = Reply {
            status: 422,
            body: body.into(),
        };
        assert_eq!(
            expect_status_at("POST", "/api/radio/stations", &reply, &[201])
                .err()
                .unwrap()
                .to_string(),
            "POST /api/radio/stations answered HTTP 422: logo: Invalid image for logo; url: The url field must be a valid URL."
        );
        let reply = Reply {
            status: 401,
            body: include_str!("../tests/fixtures/koel-9.11.3/unauthenticated.json").into(),
        };
        assert_eq!(
            expect_status_at("GET", "/api/radio/stations", &reply, &[200])
                .err()
                .unwrap()
                .to_string(),
            "GET /api/radio/stations answered HTTP 401"
        );
    }

    #[test]
    fn empty_credential_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("k"), "\n").unwrap();
        assert!(read_credential(Some(dir.path()), "k").is_err());
    }
}
