//! The setup door (design §52): a service that makes its first account an
//! administrator, through a request that needs no sign-in, until some
//! account exists. Kavita (`POST /api/Account/register`) and bindery
//! (`POST /api/v1/auth/setup`) both work that way. Whoever asks first owns
//! the instance -- on a name the internet can reach, that is a race.
//!
//! The task closes the door with an account of its own, whose password is
//! drawn from `/dev/urandom` for the one request and then dropped: it is not
//! printed, not stored, and nobody can sign in with it. The host this was
//! written for signs in through OIDC, and reaches the API with a key of its
//! own making; the account exists so that the door is shut.
//!
//! What a spec declares is the account's name. What the task compares is
//! only whether the door is open -- neither service tells a caller without a
//! key who holds it. A door that closes between the read and the write is
//! therefore an error, not a success: the account that holds it is not this
//! run's, and nothing here can say whose.

use std::io::Read;

use serde::Deserialize;
use serde_json::Value;

use crate::{
    client::{expect_status, Reply, Transport},
    endpoint::Endpoint,
    engine::{Change, Probe, Task},
    error::Error,
    services::{bindery, kavita},
};

/// Which service's door.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Door {
    Kavita,
    Bindery,
}

pub struct Setup {
    pub door: Door,
    /// The account that closes the door.
    pub username: String,
}

const SUBJECT: &str = "setup";
const OPEN: &str = "(open)";

/// What Kavita's health check reports as a version: nothing.
pub const VERSION_NOT_REPORTED: &str = "(not reported)";

/// bindery's `GET /api/v1/auth/status`. The field is required: an answer
/// without it says nothing about the door.
#[derive(Deserialize)]
struct BinderyStatus {
    #[serde(rename = "setupRequired")]
    setup_required: bool,
}

impl Setup {
    fn status(&self) -> &'static Endpoint {
        match self.door {
            Door::Kavita => &kavita::ADMIN_EXISTS,
            Door::Bindery => &bindery::AUTH_STATUS,
        }
    }

    fn create(&self) -> &'static Endpoint {
        match self.door {
            Door::Kavita => &kavita::REGISTER,
            Door::Bindery => &bindery::AUTH_SETUP,
        }
    }

    fn closed(&self) -> String {
        format!("(closed by account {})", self.username)
    }

    /// The body of the one request that sets the account up. Kavita takes an
    /// e-mail address and fills in the name when there is none; the reserved
    /// top-level domain `.invalid` says that this one reaches nobody.
    fn body(&self, password: &str) -> String {
        match self.door {
            Door::Kavita => serde_json::to_string(&kavita::RegisterDto {
                username: self.username.clone(),
                email: Some(format!("{}@invalid", self.username)),
                password: password.to_string(),
            })
            .expect("a struct of strings serializes"),
            Door::Bindery => serde_json::json!({
                "username": self.username,
                "password": password,
            })
            .to_string(),
        }
    }
}

/// 32 bytes from the kernel, as 64 hex digits: above Kavita's six and
/// bindery's eight characters, below Kavita's 256, and with no character
/// either service could trim or reject.
fn fresh_password() -> Result<String, Error> {
    let mut bytes = [0u8; 32];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .map_err(|e| Error::Refused(format!("no password can be drawn: /dev/urandom: {e}")))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// What a refused setup may show: a single short line the service wrote,
/// never the body as a whole. Kavita answers a sentence, or ASP.NET
/// Identity's errors as a list (only their `code` is shown -- the
/// description can quote the name); bindery answers `{"error": "..."}`.
fn refusal(reply: &Reply) -> Vec<String> {
    let short = |s: &str| {
        let s = s.trim();
        (!s.is_empty() && s.len() <= 200 && !s.contains('\n')).then(|| s.to_string())
    };
    match serde_json::from_str::<Value>(&reply.body) {
        Ok(Value::String(s)) => short(&s).into_iter().collect(),
        Ok(Value::Object(map)) => map
            .get("error")
            .and_then(Value::as_str)
            .and_then(short)
            .into_iter()
            .collect(),
        Ok(Value::Array(items)) => items
            .iter()
            .filter_map(|i| i.get("code").and_then(Value::as_str).and_then(short))
            .collect(),
        Ok(_) => Vec::new(),
        Err(_) => short(&reply.body).into_iter().collect(),
    }
}

impl Task for Setup {
    /// Whether the door is open.
    type Current = bool;

    fn probe(&self, t: &dyn Transport) -> Result<String, Probe> {
        match self.door {
            // Without a key the health check is all Kavita answers, and it
            // names no version.
            Door::Kavita => {
                let reply = t
                    .get(kavita::HEALTH.path)
                    .map_err(|e| Probe::NotYet(e.to_string()))?;
                match reply.status {
                    200 => Ok(VERSION_NOT_REPORTED.to_string()),
                    other => Err(Probe::NotYet(format!("HTTP {other}"))),
                }
            }
            Door::Bindery => bindery::probe(t),
        }
    }

    fn read(&self, t: &dyn Transport) -> Result<bool, Error> {
        let status = self.status();
        let reply = t.get(status.path)?;
        expect_status(status, &reply, &[200])?;
        let decode = |e: serde_json::Error| Error::Decode {
            path: status.path.to_string(),
            reason: crate::error::shape(&e),
        };
        match self.door {
            Door::Kavita => serde_json::from_str::<bool>(&reply.body)
                .map(|admin_exists| !admin_exists)
                .map_err(decode),
            Door::Bindery => serde_json::from_str::<BinderyStatus>(&reply.body)
                .map(|s| s.setup_required)
                .map_err(decode),
        }
    }

    fn diff(&self, open: &bool) -> Result<Vec<Change>, Error> {
        Ok(if *open {
            vec![Change {
                subject: SUBJECT.to_string(),
                field: String::new(),
                current: OPEN.to_string(),
                desired: self.closed(),
            }]
        } else {
            Vec::new()
        })
    }

    fn notes(&self, _open: &bool) -> Vec<String> {
        Vec::new()
    }

    fn write(&self, t: &dyn Transport, _open: &bool) -> Result<(), Error> {
        let create = self.create();
        let reply = {
            let password = fresh_password()?;
            t.post_json(create.path, &self.body(&password))?
        };
        if reply.status == 200 {
            return Ok(());
        }
        let mut validation = refusal(&reply);
        // Refused -- by the door having closed meanwhile, or for the values?
        // Only the first may be told apart, and it is an error as well.
        if !self.read(t)? {
            validation.push(
                "the door was closed by another request meanwhile; \
                 whose account holds it, this run cannot tell"
                    .to_string(),
            );
        }
        Err(Error::Status {
            method: create.method,
            path: create.path.to_string(),
            status: reply.status,
            validation,
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;
    use crate::{
        engine::{run, Mode, Outcome, Timing},
        testing::{ok, FakeClock, FakeTransport, Step},
    };

    const KAVITA_CLOSED: &str =
        include_str!("../../tests/fixtures/kavita-0.9.1.4/admin-exists.json");
    const KAVITA_OPEN: &str =
        include_str!("../../tests/fixtures/kavita-0.9.1.4/constructed-admin-exists-false.json");
    const BINDERY_CLOSED: &str =
        include_str!("../../tests/fixtures/bindery-1.33.2/auth-status.json");
    const BINDERY_OPEN: &str =
        include_str!("../../tests/fixtures/bindery-1.33.2/constructed-auth-status-open.json");
    const BINDERY_HEALTH: &str = include_str!("../../tests/fixtures/bindery-1.33.2/health.json");

    fn task(door: Door) -> Setup {
        Setup {
            door,
            username: "bootstrap".to_string(),
        }
    }

    fn kavita(states: Vec<Step>) -> FakeTransport {
        FakeTransport::default()
            .on_get(kavita::HEALTH.path, vec![ok("Ok")])
            .on_get(kavita::ADMIN_EXISTS.path, states)
    }

    fn bindery(states: Vec<Step>) -> FakeTransport {
        FakeTransport::default()
            .on_get(bindery::HEALTH.path, vec![ok(BINDERY_HEALTH)])
            .on_get(bindery::AUTH_STATUS.path, states)
    }

    fn is_fresh_password(value: &Value) -> bool {
        value
            .as_str()
            .is_some_and(|p| p.len() == 64 && p.chars().all(|c| c.is_ascii_hexdigit()))
    }

    #[test]
    fn a_plan_says_the_open_door_would_close_and_writes_nothing() {
        let t = kavita(vec![ok(KAVITA_OPEN)]);
        let report = run(
            Mode::Plan,
            &task(Door::Kavita),
            &t,
            &FakeClock::new(),
            Timing::default(),
        )
        .unwrap();
        assert_eq!(report.version, VERSION_NOT_REPORTED);
        let Outcome::Differs(changes) = report.outcome else {
            panic!("expected a difference");
        };
        assert_eq!(
            changes[0].to_string(),
            "setup: (open) -> (closed by account bootstrap)"
        );
        assert!(t.written.borrow().is_empty());
    }

    #[test]
    fn kavita_registers_its_first_account_with_a_password_drawn_for_the_request() {
        let t = kavita(vec![ok(KAVITA_OPEN), ok(KAVITA_CLOSED)]).on_put(vec![ok("{}")]);
        let report = run(
            Mode::Apply,
            &task(Door::Kavita),
            &t,
            &FakeClock::new(),
            Timing::default(),
        )
        .unwrap();
        assert!(matches!(report.outcome, Outcome::Changed(_)));
        let written = t.written.borrow();
        assert_eq!(written.len(), 1);
        let (path, body) = &written[0];
        assert_eq!(path, "/api/Account/register");
        let body: Value = serde_json::from_str(body).unwrap();
        assert_eq!(body["username"], "bootstrap");
        assert_eq!(body["email"], "bootstrap@invalid");
        assert!(is_fresh_password(&body["password"]), "{body}");
        assert_eq!(body.as_object().unwrap().len(), 3);
    }

    #[test]
    fn bindery_sets_up_its_first_account_the_same_way() {
        let t =
            bindery(vec![ok(BINDERY_OPEN), ok(BINDERY_CLOSED)]).on_put(vec![ok(r#"{"ok":true}"#)]);
        let report = run(
            Mode::Apply,
            &task(Door::Bindery),
            &t,
            &FakeClock::new(),
            Timing::default(),
        )
        .unwrap();
        assert_eq!(report.version, "1.33.2");
        assert!(matches!(report.outcome, Outcome::Changed(_)));
        let written = t.written.borrow();
        let (path, body) = &written[0];
        assert_eq!(path, "/api/v1/auth/setup");
        let body: Value = serde_json::from_str(body).unwrap();
        assert_eq!(body["username"], "bootstrap");
        assert!(is_fresh_password(&body["password"]), "{body}");
        assert_eq!(body.as_object().unwrap().len(), 2);
    }

    #[test]
    fn a_closed_door_is_left_alone() {
        for (door, t) in [
            (Door::Kavita, kavita(vec![ok(KAVITA_CLOSED)])),
            (Door::Bindery, bindery(vec![ok(BINDERY_CLOSED)])),
        ] {
            let report = run(
                Mode::Apply,
                &task(door),
                &t,
                &FakeClock::new(),
                Timing::default(),
            )
            .unwrap();
            assert_eq!(report.outcome, Outcome::Unchanged, "{door:?}");
            assert!(t.written.borrow().is_empty(), "{door:?}");
        }
    }

    #[test]
    fn two_runs_draw_two_passwords() {
        assert_ne!(fresh_password().unwrap(), fresh_password().unwrap());
    }

    #[test]
    fn a_door_closed_by_another_request_is_an_error_not_a_success() {
        let t = bindery(vec![ok(BINDERY_OPEN), ok(BINDERY_CLOSED)]).on_put(vec![Step::Answer(
            409,
            r#"{"error":"setup already complete"}"#.to_string(),
        )]);
        let err = run(
            Mode::Apply,
            &task(Door::Bindery),
            &t,
            &FakeClock::new(),
            Timing::default(),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("HTTP 409"), "{err}");
        assert!(err.contains("setup already complete"), "{err}");
        assert!(err.contains("closed by another request"), "{err}");
    }

    #[test]
    fn a_refusal_for_the_values_says_why_and_never_the_password() {
        let t = kavita(vec![ok(KAVITA_OPEN)]).on_put(vec![Step::Answer(
            400,
            r#"[{"code":"InvalidUserName","description":"Username 'bootstrap' is invalid"}]"#
                .to_string(),
        )]);
        let err = run(
            Mode::Apply,
            &task(Door::Kavita),
            &t,
            &FakeClock::new(),
            Timing::default(),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("HTTP 400: InvalidUserName"), "{err}");
        assert!(!err.contains("closed by another request"), "{err}");
        let body: Value = serde_json::from_str(&t.written.borrow()[0].1).unwrap();
        assert!(!err.contains(body["password"].as_str().unwrap()));
    }

    #[test]
    fn an_answer_that_says_nothing_about_the_door_is_an_error() {
        for (door, t) in [
            (Door::Kavita, kavita(vec![ok(r#"{"exists":true}"#)])),
            (Door::Bindery, bindery(vec![ok(r#"{"mode":"enabled"}"#)])),
        ] {
            let err = run(
                Mode::Plan,
                &task(door),
                &t,
                &FakeClock::new(),
                Timing::default(),
            )
            .unwrap_err();
            assert!(matches!(err, Error::Decode { .. }), "{door:?}: {err}");
        }
    }

    #[test]
    fn kavita_is_waited_for_until_its_health_check_answers() {
        let t = FakeTransport::default()
            .on_get(
                kavita::HEALTH.path,
                vec![Step::Refused, Step::Answer(503, String::new()), ok("Ok")],
            )
            .on_get(kavita::ADMIN_EXISTS.path, vec![ok(KAVITA_CLOSED)]);
        let report = run(
            Mode::Apply,
            &task(Door::Kavita),
            &t,
            &FakeClock::new(),
            Timing::default(),
        )
        .unwrap();
        assert_eq!(report.outcome, Outcome::Unchanged);
    }
}
