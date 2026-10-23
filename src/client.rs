use std::{
    fs,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use serde::Deserialize;

use crate::{endpoint::Endpoint, error::Error, secret::Secret};

pub struct Reply {
    pub status: u16,
    pub body: String,
    /// Every `Set-Cookie` line of the answer, whole. A cookie is a session,
    /// so the lines are secrets: no `Debug`, no `Display` (design §58).
    pub set_cookies: Vec<Secret>,
}

impl Reply {
    /// An answer without cookies -- what nearly every service sends.
    pub fn new(status: u16, body: String) -> Self {
        Self {
            status,
            body,
            set_cookies: Vec::new(),
        }
    }

    /// The value of the cookie `name` that this answer sets: the text
    /// between `name=` and the first `;` of a `Set-Cookie` line. An empty
    /// value is a cookie being cleared, and is `None`.
    pub fn cookie(&self, name: &str) -> Option<Secret> {
        self.set_cookies.iter().find_map(|line| {
            let pair = line.expose().split(';').next()?;
            let (key, value) = pair.split_once('=')?;
            (key.trim() == name && !value.trim().is_empty())
                .then(|| Secret::new(value.trim().to_string()))
        })
    }
}

pub trait Transport {
    fn get(&self, path: &str) -> Result<Reply, Error>;
    fn put_json(&self, path: &str, body: &str) -> Result<Reply, Error>;
    fn post_json(&self, path: &str, body: &str) -> Result<Reply, Error>;
    /// Audiobookshelf's settings take a partial body (design §27).
    fn patch_json(&self, path: &str, body: &str) -> Result<Reply, Error>;
    /// Removes one entry. Only a task with `exactly` sends this, and only
    /// for an entry `Task::surplus` named (design §39).
    fn delete(&self, path: &str) -> Result<Reply, Error>;
    /// A form body. Bazarr's settings endpoint reads `request.form` and
    /// nothing else (design §48).
    fn post_form(&self, path: &str, fields: &[(&str, &str)]) -> Result<Reply, Error>;
}

/// `fields` as `application/x-www-form-urlencoded`: everything but ASCII
/// letters, digits and `-._~` is percent-encoded, byte by byte.
pub fn form_body(fields: &[(&str, &str)]) -> String {
    fn encode(text: &str, out: &mut String) {
        for byte in text.bytes() {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
                out.push(byte as char);
            } else {
                out.push_str(&format!("%{byte:02X}"));
            }
        }
    }
    let mut body = String::new();
    for (i, (name, value)) in fields.iter().enumerate() {
        if i > 0 {
            body.push('&');
        }
        encode(name, &mut body);
        body.push('=');
        encode(value, &mut body);
    }
    body
}

pub struct HttpTransport {
    agent: ureq::Agent,
    base_url: String,
    header: &'static str,
    key: Secret,
    accept_json: bool,
    /// Every request of this transport is a read, whatever its verb: the
    /// POST of a sign-in changes nothing a spec describes.
    reads_only: bool,
    /// A write of this transport has gone out, answered or not (design §54).
    wrote: AtomicBool,
    /// Headers every request carries besides the key: Velvarr's `Origin`
    /// (design §58). Not secrets -- they come from the spec.
    fixed: Vec<(&'static str, String)>,
}

/// Where a request that got no answer was lost.
enum Lost {
    /// Before the request could travel: no address, no connection.
    BeforeSending,
    /// Somewhere after: the time limit ended, the connection broke.
    Unanswered,
    /// Not a lost answer at all: a malformed URI, a protocol violation.
    Otherwise,
}

fn lost(e: &ureq::Error) -> Lost {
    use ureq::{Error as E, Timeout as T};
    match e {
        E::HostNotFound | E::ConnectionFailed => Lost::BeforeSending,
        E::Timeout(T::Resolve | T::Connect) => Lost::BeforeSending,
        E::Io(io) if io.kind() == std::io::ErrorKind::ConnectionRefused => Lost::BeforeSending,
        E::Timeout(_) | E::Io(_) => Lost::Unanswered,
        _ => Lost::Otherwise,
    }
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
            // ureq drops `Authorization` and `Cookie` on a redirect, but not
            // `X-Api-Key` or `X-Emby-Token`: a 302 to another host carried the
            // key there (audit B39, measured with two listeners). converge
            // follows redirects itself -- see `get`.
            .max_redirects(0)
            .build()
            .into();
        Self {
            agent,
            base_url: base_url.trim_end_matches('/').to_string(),
            header,
            key,
            accept_json: false,
            reads_only: false,
            wrote: AtomicBool::new(false),
            fixed: Vec::new(),
        }
    }

    /// One more header on every request, with a value from the spec -- never
    /// a secret, which belongs in the key. Velvarr refuses a write whose
    /// `Origin` is not the one it was configured with (design §58).
    pub fn with_header(mut self, name: &'static str, value: String) -> Self {
        self.fixed.push((name, value));
        self
    }

    /// A transport whose requests are all reads, for a sign-in that only
    /// fetches a token: its POST creates nothing a spec describes, so one
    /// that gets no answer is "no answer", not an unconfirmed write (§54).
    pub fn reads_only(mut self) -> Self {
        self.reads_only = true;
        self
    }

    /// Whether a write of this transport has gone out -- answered, refused
    /// by status or left without an answer. A write that could not even be
    /// connected does not count: nothing reached the service.
    pub fn wrote(&self) -> bool {
        self.wrote.load(Ordering::Relaxed)
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
        let mut request = if self.header.is_empty() {
            request
        } else {
            request.header(self.header, self.key.expose())
        };
        for (name, value) in &self.fixed {
            request = request.header(*name, value);
        }
        if self.accept_json {
            request.header("Accept", "application/json")
        } else {
            request
        }
    }

    /// Reads the answer, and says which kind of failure a missing one is
    /// (design §54): a read without an answer, or a request that never
    /// left, is `NoAnswer`; a write that went out and was not answered is
    /// `Unconfirmed`. Notes on the way whether a write has gone out.
    fn finish(
        &self,
        method: &'static str,
        path: &str,
        result: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
    ) -> Result<Reply, Error> {
        let write = method != "GET" && !self.reads_only;
        let answer = result.and_then(|mut response| {
            let status = response.status().as_u16();
            // A line that is not text is no cookie converge could send back.
            let set_cookies = response
                .headers()
                .get_all("set-cookie")
                .iter()
                .filter_map(|v| v.to_str().ok())
                .map(|v| Secret::new(v.to_string()))
                .collect();
            let body = response.body_mut().read_to_string()?;
            Ok(Reply {
                status,
                body,
                set_cookies,
            })
        });
        let e = match answer {
            Ok(reply) => {
                if write {
                    self.wrote.store(true, Ordering::Relaxed);
                }
                return Ok(reply);
            }
            Err(e) => e,
        };
        let (path, reason) = (path.to_string(), e.to_string());
        match lost(&e) {
            Lost::BeforeSending => Err(Error::NoAnswer {
                method,
                path,
                reason,
            }),
            Lost::Unanswered if !write => Err(Error::NoAnswer {
                method,
                path,
                reason,
            }),
            Lost::Unanswered => {
                self.wrote.store(true, Ordering::Relaxed);
                Err(Error::Unconfirmed {
                    method,
                    path,
                    reason,
                })
            }
            Lost::Otherwise => {
                if write {
                    self.wrote.store(true, Ordering::Relaxed);
                }
                Err(Error::Request {
                    method,
                    path,
                    reason,
                })
            }
        }
    }
}

/// How many redirects a GET follows. A trailing-slash redirect (Flask,
/// Django) takes one.
const MAX_REDIRECTS: usize = 5;

/// `scheme://host:port` of a URL, lower-cased; `None` if it is not absolute.
fn origin(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.is_empty() {
        return None;
    }
    Some(format!("{scheme}://{authority}").to_ascii_lowercase())
}

/// Where a redirect from `base_url` leads, if it stays on the same origin:
/// an absolute path (`/x`), or an absolute URL with the same scheme, host
/// and port. Anything else -- another host, a scheme change, a
/// protocol-relative `//host` -- is `None`: the key would travel with it.
fn same_origin_target(base_url: &str, location: &str) -> Option<String> {
    let own = origin(base_url)?;
    if location.starts_with('/') && !location.starts_with("//") {
        return Some(format!("{own}{location}"));
    }
    (origin(location)? == own).then(|| location.to_string())
}

impl Transport for HttpTransport {
    /// Follows a redirect only on the service's own origin. The key header
    /// goes with every request, so a redirect elsewhere is refused before it
    /// is sent (audit B39). Writes do not follow at all: a 3xx comes back
    /// as a status, which the caller refuses.
    fn get(&self, path: &str) -> Result<Reply, Error> {
        let mut url = format!("{}{path}", self.base_url);
        for _ in 0..=MAX_REDIRECTS {
            let result = self.headers(self.agent.get(&url)).call();
            let response = match result {
                Ok(r) if r.status().is_redirection() => r,
                other => return self.finish("GET", path, other),
            };
            let status = response.status().as_u16();
            let location = response
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok());
            url = match location.and_then(|l| same_origin_target(&self.base_url, l)) {
                Some(next) => next,
                None => {
                    return Err(Error::Request {
                        method: "GET",
                        path: path.to_string(),
                        reason: format!(
                            "HTTP {status} redirects off the service's own origin; \
                             not followed, the key stays here"
                        ),
                    });
                }
            };
        }
        Err(Error::Request {
            method: "GET",
            path: path.to_string(),
            reason: format!("more than {MAX_REDIRECTS} redirects"),
        })
    }

    fn put_json(&self, path: &str, body: &str) -> Result<Reply, Error> {
        let result = self
            .headers(self.agent.put(format!("{}{path}", self.base_url)))
            .content_type("application/json")
            .send(body);
        self.finish("PUT", path, result)
    }

    fn post_json(&self, path: &str, body: &str) -> Result<Reply, Error> {
        let result = self
            .headers(self.agent.post(format!("{}{path}", self.base_url)))
            .content_type("application/json")
            .send(body);
        self.finish("POST", path, result)
    }

    fn patch_json(&self, path: &str, body: &str) -> Result<Reply, Error> {
        let result = self
            .headers(self.agent.patch(format!("{}{path}", self.base_url)))
            .content_type("application/json")
            .send(body);
        self.finish("PATCH", path, result)
    }

    /// No body, and no redirect is followed: a 3xx comes back as a status
    /// the caller refuses, as for the other writes.
    fn delete(&self, path: &str) -> Result<Reply, Error> {
        let result = self
            .headers(self.agent.delete(format!("{}{path}", self.base_url)))
            .call();
        self.finish("DELETE", path, result)
    }

    fn post_form(&self, path: &str, fields: &[(&str, &str)]) -> Result<Reply, Error> {
        let result = self
            .headers(self.agent.post(format!("{}{path}", self.base_url)))
            .content_type("application/x-www-form-urlencoded")
            .send(form_body(fields));
        self.finish("POST", path, result)
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
        let reply = Reply::new(400, body.into());
        assert_eq!(
            expect_status(&EP, &reply, &[200]).err().unwrap().to_string(),
            "PUT /u answered HTTP 400: MaxSize: 'Max Size' must be greater than or equal to 'Min Size'."
        );
        let reply = Reply::new(500, "<html>secret stack</html>".into());
        assert_eq!(
            expect_status(&EP, &reply, &[200])
                .err()
                .unwrap()
                .to_string(),
            "PUT /u answered HTTP 500"
        );
        let reply = Reply::new(202, String::new());
        assert!(expect_status(&EP, &reply, &[200, 202]).is_ok());
    }

    #[test]
    fn expect_status_shows_laravel_validation_fields_and_nothing_else() {
        let body = include_str!("../tests/fixtures/koel-9.11.3/constructed-validation-error.json");
        let reply = Reply::new(422, body.into());
        assert_eq!(
            expect_status_at("POST", "/api/radio/stations", &reply, &[201])
                .err()
                .unwrap()
                .to_string(),
            "POST /api/radio/stations answered HTTP 422: logo: Invalid image for logo; url: The url field must be a valid URL."
        );
        let reply = Reply::new(
            401,
            include_str!("../tests/fixtures/koel-9.11.3/unauthenticated.json").into(),
        );
        assert_eq!(
            expect_status_at("GET", "/api/radio/stations", &reply, &[200])
                .err()
                .unwrap()
                .to_string(),
            "GET /api/radio/stations answered HTTP 401"
        );
    }

    #[test]
    fn a_cookie_is_read_by_name_from_its_set_cookie_line() {
        let reply = Reply {
            status: 200,
            body: String::new(),
            set_cookies: vec![
                Secret::new("other=1; Path=/".into()),
                Secret::new(
                    "velvarr_session=tok-3f9a; Path=/; HttpOnly; SameSite=Strict; Max-Age=604800"
                        .into(),
                ),
            ],
        };
        assert_eq!(
            reply.cookie("velvarr_session").unwrap().expose(),
            "tok-3f9a"
        );
        assert_eq!(reply.cookie("other").unwrap().expose(), "1");
        assert!(reply.cookie("velvarr").is_none());
        assert!(Reply::new(200, String::new())
            .cookie("velvarr_session")
            .is_none());
        // A logout clears the cookie with an empty value: no session.
        let cleared = Reply {
            status: 200,
            body: String::new(),
            set_cookies: vec![Secret::new(
                "velvarr_session=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0".into(),
            )],
        };
        assert!(cleared.cookie("velvarr_session").is_none());
    }

    #[test]
    fn empty_credential_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("k"), "\n").unwrap();
        assert!(read_credential(Some(dir.path()), "k").is_err());
    }
}
