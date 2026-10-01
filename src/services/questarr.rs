//! Questarr (1.4.2): the account, download clients, the Prowlarr sync and the
//! import configuration (design §47). Questarr publishes no OpenAPI
//! description, so field names are checked against recorded answers
//! (`tests/fixtures/questarr-1.4.2`).
//!
//! No API key, no cookie: every route behind `/api` wants
//! `Authorization: Bearer <JWT>`, and the only way to a token is a sign-in
//! with the account's name and password. Questarr has exactly one account,
//! created by `POST /api/auth/setup` while there is none — so the first run
//! sets it up, and every later one signs in.

use serde::Deserialize;
use serde_json::json;

use crate::{
    client::{expect_status_at, Transport},
    endpoint::Endpoint,
    engine::Probe,
    error::Error,
    secret::Secret,
};

pub const HEALTH: Endpoint = Endpoint {
    method: "GET",
    path: "/api/health",
    request: None,
    response: None,
};

pub const AUTH_STATUS: Endpoint = Endpoint {
    method: "GET",
    path: "/api/auth/status",
    request: None,
    response: None,
};

pub const SETUP: Endpoint = Endpoint {
    method: "POST",
    path: "/api/auth/setup",
    request: None,
    response: None,
};

pub const LOGIN: Endpoint = Endpoint {
    method: "POST",
    path: "/api/auth/login",
    request: None,
    response: None,
};

/// Questarr's API names no version anywhere.
pub const VERSION_NOT_REPORTED: &str = "(not reported)";

#[derive(Deserialize)]
struct Health {
    status: Option<String>,
}

/// `GET /api/auth/status`. The field is required: an answer without it is
/// not Questarr's, and "no account" must never be guessed.
#[derive(Deserialize)]
struct AuthStatus {
    #[serde(rename = "hasUsers")]
    has_users: bool,
}

/// What a setup or a login answered. Only the token is read.
#[derive(Deserialize)]
struct TokenAnswer {
    token: Secret,
}

/// Readiness: `GET /api/health` answers `status: ok`, without a token.
pub fn probe(t: &dyn Transport) -> Result<String, Probe> {
    let reply = t
        .get(HEALTH.path)
        .map_err(|e| Probe::NotYet(e.to_string()))?;
    if reply.status != 200 {
        return Err(Probe::NotYet(format!("HTTP {}", reply.status)));
    }
    let health: Health = serde_json::from_str(&reply.body)
        .map_err(|e| Probe::NotYet(format!("unexpected answer: {}", crate::error::shape(&e))))?;
    match health.status.as_deref() {
        Some("ok") => Ok(VERSION_NOT_REPORTED.to_string()),
        _ => Err(Probe::NotYet("health is not ok yet".to_string())),
    }
}

/// The body of both the setup and the login. Built here, so the password
/// never travels in `argv`; a serializing failure names the path only.
fn account_body(endpoint: &Endpoint, username: &str, password: &Secret) -> Result<String, Error> {
    serde_json::to_string(&json!({
        "username": username,
        "password": password.expose(),
    }))
    .map_err(|_| Error::Request {
        method: endpoint.method,
        path: endpoint.path.to_string(),
        reason: "cannot serialize the body".to_string(),
    })
}

fn token_of(endpoint: &Endpoint, body: &str) -> Result<Secret, Error> {
    let answer: TokenAnswer = serde_json::from_str(body).map_err(|e| Error::Decode {
        path: endpoint.path.to_string(),
        reason: crate::error::shape(&e),
    })?;
    Ok(answer.token)
}

/// Exchanges the account's name and password for a JWT — and sets the
/// account up first when Questarr has none yet.
///
/// The setup sends the account alone. Questarr would also take IGDB
/// credentials there, but what it stores that way shadows the ones from its
/// environment for good.
///
/// A setup answered with 403 lost a race against another run (the account
/// check comes before everything else there), so the sign-in follows. A
/// refused sign-in is final: Questarr allows 20 of them per 15 minutes and
/// address, successful ones included, and a wrong password stays wrong.
pub fn sign_in(t: &dyn Transport, username: &str, password: &Secret) -> Result<Secret, Error> {
    let reply = t.get(AUTH_STATUS.path)?;
    expect_status_at(AUTH_STATUS.method, AUTH_STATUS.path, &reply, &[200])?;
    let status: AuthStatus = serde_json::from_str(&reply.body).map_err(|e| Error::Decode {
        path: AUTH_STATUS.path.to_string(),
        reason: crate::error::shape(&e),
    })?;

    if !status.has_users {
        let reply = t.post_json(SETUP.path, &account_body(&SETUP, username, password)?)?;
        if reply.status != 403 {
            expect_status_at(SETUP.method, SETUP.path, &reply, &[200])?;
            return token_of(&SETUP, &reply.body);
        }
    }

    let reply = t.post_json(LOGIN.path, &account_body(&LOGIN, username, password)?)?;
    let refused = |reason: String| Error::Status {
        method: LOGIN.method,
        path: LOGIN.path.to_string(),
        status: reply.status,
        validation: vec![reason],
    };
    match reply.status {
        401 | 403 => Err(refused(format!(
            "the account {username} or its password was refused"
        ))),
        429 => Err(refused(
            "Questarr allows 20 sign-ins per 15 minutes and address; this one was over the limit"
                .to_string(),
        )),
        _ => {
            expect_status_at(LOGIN.method, LOGIN.path, &reply, &[200])?;
            token_of(&LOGIN, &reply.body)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{ok, FakeTransport, Step};
    use serde_json::Value;

    const HEALTH_OK: &str = include_str!("../../tests/fixtures/questarr-1.4.2/health.json");
    const FRESH: &str = include_str!("../../tests/fixtures/questarr-1.4.2/auth-status-fresh.json");
    const SET_UP: &str =
        include_str!("../../tests/fixtures/questarr-1.4.2/auth-status-set-up.json");
    const SETUP_ANSWER: &str =
        include_str!("../../tests/fixtures/questarr-1.4.2/setup-answer.json");
    const SETUP_REFUSED: &str =
        include_str!("../../tests/fixtures/questarr-1.4.2/setup-refused.json");
    const LOGIN_ANSWER: &str =
        include_str!("../../tests/fixtures/questarr-1.4.2/login-answer.json");
    const LOGIN_REFUSED: &str =
        include_str!("../../tests/fixtures/questarr-1.4.2/login-refused.json");

    /// A value nobody would type, so a leak is easy to search for.
    const PASSWORD: &str = "account-pw-4c1e7a-never-print-me";

    fn password() -> Secret {
        Secret::new(PASSWORD.to_string())
    }

    fn questarr(status: &str, writes: Vec<Step>) -> FakeTransport {
        FakeTransport::default()
            .on_get("/api/auth/status", vec![ok(status)])
            .on_put(writes)
    }

    fn sent(t: &FakeTransport, index: usize) -> (String, Value) {
        let (path, body) = t.written.borrow()[index].clone();
        (path, serde_json::from_str(&body).unwrap())
    }

    #[test]
    fn the_probe_wants_status_ok_and_names_no_version() {
        let t = FakeTransport::default().on_get("/api/health", vec![ok(HEALTH_OK)]);
        assert_eq!(probe(&t).ok().unwrap(), VERSION_NOT_REPORTED);

        let t =
            FakeTransport::default().on_get("/api/health", vec![Step::Answer(503, String::new())]);
        assert!(matches!(probe(&t), Err(Probe::NotYet(_))));

        let t =
            FakeTransport::default().on_get("/api/health", vec![ok(r#"{"status":"starting"}"#)]);
        assert!(matches!(probe(&t), Err(Probe::NotYet(_))));
    }

    #[test]
    fn a_fresh_questarr_gets_its_account_and_the_setup_answers_the_token() {
        let t = questarr(FRESH, vec![ok(SETUP_ANSWER)]);
        let token = sign_in(&t, "achim", &password()).unwrap();
        assert_eq!(token.expose(), "<masked>");
        let (path, body) = sent(&t, 0);
        assert_eq!(path, "/api/auth/setup");
        // Only the account: IGDB sent here would shadow the environment's
        // credentials in Questarr's database for good.
        assert_eq!(body, json!({"username": "achim", "password": PASSWORD}));
        assert_eq!(t.written.borrow().len(), 1);
    }

    #[test]
    fn a_questarr_with_an_account_is_signed_in_to() {
        let t = questarr(SET_UP, vec![ok(LOGIN_ANSWER)]);
        let token = sign_in(&t, "achim", &password()).unwrap();
        assert_eq!(token.expose(), "<masked>");
        let (path, body) = sent(&t, 0);
        assert_eq!(path, "/api/auth/login");
        assert_eq!(body, json!({"username": "achim", "password": PASSWORD}));
    }

    #[test]
    fn a_setup_that_lost_the_race_signs_in_instead() {
        // Two runs start together: both read "no account", one sets it up,
        // the other gets 403 "Setup already completed".
        let t = questarr(
            FRESH,
            vec![Step::Answer(403, SETUP_REFUSED.into()), ok(LOGIN_ANSWER)],
        );
        let token = sign_in(&t, "achim", &password()).unwrap();
        assert_eq!(token.expose(), "<masked>");
        assert_eq!(
            *t.calls.borrow(),
            [
                ("POST", "/api/auth/setup".to_string()),
                ("POST", "/api/auth/login".to_string())
            ]
        );
    }

    #[test]
    fn a_refused_password_names_the_account_and_never_the_password() {
        let t = questarr(SET_UP, vec![Step::Answer(401, LOGIN_REFUSED.into())]);
        let error = sign_in(&t, "achim", &password()).err().unwrap();
        assert!(matches!(error, Error::Status { status: 401, .. }));
        let text = error.to_string();
        assert!(
            text.contains("the account achim or its password was refused"),
            "{text}"
        );
        assert!(!text.contains(PASSWORD), "{text}");
    }

    #[test]
    fn the_sign_in_limit_is_named() {
        // 20 sign-ins per 15 minutes and address, successful ones included
        // (`authRateLimiter`); waiting a poll interval will not help.
        let t = questarr(SET_UP, vec![Step::Answer(429, "Too many requests".into())]);
        let error = sign_in(&t, "achim", &password()).err().unwrap();
        assert!(matches!(error, Error::Status { status: 429, .. }));
        let text = error.to_string();
        assert!(text.contains("20 sign-ins per 15 minutes"), "{text}");
    }

    #[test]
    fn a_status_answer_without_has_users_is_not_guessed_at() {
        let t = questarr(r#"{"status":"ok"}"#, vec![ok(LOGIN_ANSWER)]);
        let error = sign_in(&t, "achim", &password()).err().unwrap();
        assert!(matches!(error, Error::Decode { .. }), "{error}");
        assert!(t.written.borrow().is_empty());
    }

    #[test]
    fn a_setup_refused_for_its_values_is_an_error_not_a_login() {
        // 400: the username or password breaks Questarr's rules (3..50
        // characters, password at least 6). Signing in cannot fix that.
        let t = questarr(
            FRESH,
            vec![Step::Answer(
                400,
                r#"{"error":"Password must be at least 6 characters"}"#.into(),
            )],
        );
        let error = sign_in(&t, "achim", &password()).err().unwrap();
        assert!(matches!(error, Error::Status { status: 400, .. }));
        assert_eq!(t.written.borrow().len(), 1);
        assert!(!error.to_string().contains(PASSWORD));
    }
}
