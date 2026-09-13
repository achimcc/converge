//! ntfy: the subscriptions of an account (design §10). A topic name is all it
//! takes to read a topic, so topics are secrets here: they come from a
//! credential, are held as `Secret`, and are referred to by their line in
//! that credential -- `subscription 2` -- in every change, note and error.
//!
//! ntfy publishes no OpenAPI description. The fields below are checked
//! against a recorded answer and at runtime only, as for Jellyfin's plugin
//! configurations.

use serde::Deserialize;
use serde_json::json;

use crate::{
    client::{Reply, Transport},
    endpoint::Endpoint,
    engine::{Change, Probe, Task},
    error::Error,
    secret::Secret,
};

pub const HEALTH: Endpoint = Endpoint {
    method: "GET",
    path: "/v1/health",
    request: None,
    response: None,
};
pub const ACCOUNT: Endpoint = Endpoint {
    method: "GET",
    path: "/v1/account",
    request: None,
    response: None,
};
pub const SUBSCRIPTION_ADD: Endpoint = Endpoint {
    method: "POST",
    path: "/v1/account/subscription",
    request: None,
    response: None,
};
pub const ENDPOINTS: [Endpoint; 3] = [HEALTH, ACCOUNT, SUBSCRIPTION_ADD];

/// ntfy tells its version only to administrators; a user token gets none.
pub const VERSION_NOT_REPORTED: &str = "(not reported)";

#[derive(Deserialize)]
struct Health {
    #[serde(default)]
    healthy: Option<bool>,
}

/// Only the subscriptions. The account object also carries the user's
/// tokens, its sync topic and its name; they are never kept.
///
/// ntfy 2.26.0 omits `subscriptions` when there are none
/// (`json:"subscriptions,omitempty"` in `server/types.go`), so a missing key
/// is an empty list. `username` is required instead, and only its presence
/// is checked: it tells an account answer from any other JSON object.
#[derive(Deserialize)]
pub struct Account {
    #[allow(dead_code)]
    username: serde::de::IgnoredAny,
    #[serde(default)]
    pub subscriptions: Vec<Subscription>,
}

/// `display_name` is the person's to choose and is never read or written.
#[derive(Deserialize)]
pub struct Subscription {
    pub base_url: String,
    pub topic: Secret,
}

/// The topics of a credential: one per line, surrounding whitespace and blank
/// lines ignored. Errors name lines, never a topic.
pub fn topics(credential: &str, content: &Secret) -> Result<Vec<Secret>, Error> {
    let lines: Vec<&str> = content
        .expose()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let fail = |reason: String| Error::Credential {
        name: credential.to_string(),
        reason,
    };
    if lines.is_empty() {
        return Err(fail("holds no topic".to_string()));
    }
    for (i, line) in lines.iter().enumerate() {
        if let Some(first) = lines[..i].iter().position(|earlier| earlier == line) {
            return Err(fail(format!(
                "names one topic twice (topics {} and {})",
                first + 1,
                i + 1
            )));
        }
    }
    Ok(lines
        .into_iter()
        .map(|line| Secret::new(line.to_string()))
        .collect())
}

/// Readiness: `/v1/health` answers `{"healthy": true}`. It needs no token, so
/// a refused token shows up at the first read instead.
pub fn probe(t: &dyn Transport) -> Result<String, Probe> {
    let reply = t
        .get(HEALTH.path)
        .map_err(|e| Probe::NotYet(e.to_string()))?;
    if reply.status != 200 {
        return Err(Probe::NotYet(format!("HTTP {}", reply.status)));
    }
    let health: Health = serde_json::from_str(&reply.body)
        .map_err(|e| Probe::NotYet(format!("unexpected answer: {e}")))?;
    match health.healthy {
        Some(true) => Ok(VERSION_NOT_REPORTED.to_string()),
        _ => Err(Probe::NotYet("not healthy yet".to_string())),
    }
}

/// A status error that carries nothing from the body: ntfy's messages may
/// quote the request, and the request holds a topic.
fn refuse(ep: &Endpoint, reply: &Reply, accepted: u16) -> Result<(), Error> {
    if reply.status == accepted {
        return Ok(());
    }
    Err(Error::Status {
        method: ep.method,
        path: ep.path.to_string(),
        status: reply.status,
        validation: match reply.status {
            401 | 403 => vec!["the token was refused".to_string()],
            _ => Vec::new(),
        },
    })
}

/// Subscriptions to add to the account: `base_url` with each topic.
pub struct AccountSubscriptions {
    pub base_url: String,
    pub topics: Vec<Secret>,
}

const SUBJECT: &str = "ntfy account";

impl AccountSubscriptions {
    /// The 1-based numbers of the topics the account lacks at `base_url`.
    fn lacking(&self, current: &[Subscription]) -> Vec<usize> {
        self.topics
            .iter()
            .enumerate()
            .filter(|(_, topic)| {
                !current
                    .iter()
                    .any(|s| s.base_url == self.base_url && s.topic.expose() == topic.expose())
            })
            .map(|(i, _)| i + 1)
            .collect()
    }
}

impl Task for AccountSubscriptions {
    type Current = Vec<Subscription>;

    fn probe(&self, t: &dyn Transport) -> Result<String, Probe> {
        probe(t)
    }

    fn read(&self, t: &dyn Transport) -> Result<Self::Current, Error> {
        let reply = t.get(ACCOUNT.path)?;
        refuse(&ACCOUNT, &reply, 200)?;
        // serde's message could quote a value; say only where it failed.
        let account: Account = serde_json::from_str(&reply.body).map_err(|e| Error::Decode {
            path: ACCOUNT.path.to_string(),
            reason: format!(
                "not an account with subscriptions ({:?} error at line {} column {})",
                e.classify(),
                e.line(),
                e.column()
            ),
        })?;
        Ok(account.subscriptions)
    }

    fn diff(&self, current: &Self::Current) -> Result<Vec<Change>, Error> {
        Ok(self
            .lacking(current)
            .into_iter()
            .map(|number| Change {
                subject: SUBJECT.to_string(),
                field: format!("subscription {number}"),
                current: "(missing)".to_string(),
                desired: "(added)".to_string(),
            })
            .collect())
    }

    fn notes(&self, current: &Self::Current) -> Vec<String> {
        let mut notes = Vec::new();
        for (i, topic) in self.topics.iter().enumerate() {
            for other in current
                .iter()
                .filter(|s| s.topic.expose() == topic.expose() && s.base_url != self.base_url)
            {
                notes.push(format!(
                    "subscription {} also exists with base_url {}",
                    i + 1,
                    other.base_url
                ));
            }
        }
        notes
    }

    /// Adds, never changes or removes: no PATCH, no DELETE, and a subscription
    /// under another base_url stays as it is.
    fn write(&self, t: &dyn Transport, current: &Self::Current) -> Result<(), Error> {
        for number in self.lacking(current) {
            let topic = &self.topics[number - 1];
            let body = json!({ "base_url": self.base_url, "topic": topic.expose() }).to_string();
            let reply = t.post_json(SUBSCRIPTION_ADD.path, &body)?;
            refuse(&SUBSCRIPTION_ADD, &reply, 200)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;
    use crate::testing::{ok, FakeTransport, Step};

    const HEALTH_JSON: &str =
        include_str!("../../tests/fixtures/ntfy-2.26.0/constructed-health.json");
    const ACCOUNT_JSON: &str = include_str!("../../tests/fixtures/ntfy-2.26.0/account.json");
    const HOST: &str = "https://ntfy.rusty-vault.de";

    /// The fixture's topics are masked; a credential holding exactly those
    /// lines is the state the host wants.
    fn task(base_url: &str, lines: &str) -> AccountSubscriptions {
        AccountSubscriptions {
            base_url: base_url.to_string(),
            topics: topics("ntfy-abo-topics", &Secret::new(lines.to_string())).unwrap(),
        }
    }

    fn account(body: &str) -> FakeTransport {
        FakeTransport::default()
            .on_get(ACCOUNT.path, vec![ok(body)])
            .on_put(vec![ok(r#"{"base_url":"x"}"#)])
    }

    #[test]
    fn topics_are_lines_and_errors_name_no_topic() {
        let found = topics("t", &Secret::new("\n  alpha-7 \n\nbeta-9\r\n".to_string())).unwrap();
        let found: Vec<&str> = found.iter().map(Secret::expose).collect();
        assert_eq!(found, ["alpha-7", "beta-9"]);

        let err = topics("t", &Secret::new(" \n\n".to_string()))
            .err()
            .unwrap()
            .to_string();
        assert_eq!(err, "credential t: holds no topic");
        let err = topics("t", &Secret::new("alpha-7\nbeta-9\nalpha-7".to_string()))
            .err()
            .unwrap()
            .to_string();
        assert_eq!(err, "credential t: names one topic twice (topics 1 and 3)");
    }

    #[test]
    fn probe_needs_healthy_and_reports_no_version() {
        let t = FakeTransport::default().on_get(HEALTH.path, vec![ok(HEALTH_JSON)]);
        assert_eq!(probe(&t).ok().unwrap(), "(not reported)");
        for body in [r#"{"healthy":false}"#, "{}"] {
            let t = FakeTransport::default().on_get(HEALTH.path, vec![ok(body)]);
            assert!(matches!(probe(&t), Err(Probe::NotYet(_))), "{body}");
        }
        let t =
            FakeTransport::default().on_get(HEALTH.path, vec![Step::Answer(503, String::new())]);
        assert!(matches!(probe(&t), Err(Probe::NotYet(_))));
    }

    #[test]
    fn recorded_account_already_has_both_subscriptions() {
        let task = task(HOST, "<masked-topic-1>\n<masked-topic-2>\n");
        let t = account(ACCOUNT_JSON);
        let current = task.read(&t).unwrap();
        assert_eq!(task.diff(&current).unwrap(), vec![]);
        assert_eq!(task.notes(&current), Vec::<String>::new());
    }

    #[test]
    fn a_third_topic_is_one_change_and_one_post() {
        let secret = "brand-new-topic-4711";
        let task = task(
            HOST,
            &format!("<masked-topic-1>\n<masked-topic-2>\n{secret}\n"),
        );
        let t = account(ACCOUNT_JSON);
        let current = task.read(&t).unwrap();
        let changes = task.diff(&current).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(
            changes[0].to_string(),
            "ntfy account: subscription 3 (missing) -> (added)"
        );
        assert!(!format!("{changes:?}").contains(secret));
        task.write(&t, &current).unwrap();
        let written = t.written.borrow();
        assert_eq!(written.len(), 1);
        assert_eq!(written[0].0, "/v1/account/subscription");
        let sent: Value = serde_json::from_str(&written[0].1).unwrap();
        assert_eq!(sent, json!({"base_url": HOST, "topic": secret}));
    }

    #[test]
    fn the_same_topic_under_another_base_url_is_a_change_and_a_note() {
        let task = task("https://ntfy.example.org", "<masked-topic-2>\n");
        let t = account(ACCOUNT_JSON);
        let current = task.read(&t).unwrap();
        assert_eq!(
            task.diff(&current).unwrap()[0].to_string(),
            "ntfy account: subscription 1 (missing) -> (added)"
        );
        assert_eq!(
            task.notes(&current),
            ["subscription 1 also exists with base_url https://ntfy.rusty-vault.de"]
        );
    }

    #[test]
    fn a_refused_token_or_write_carries_nothing_from_the_body() {
        let task = task(HOST, "<masked-topic-1>\nsecret-topic\n");
        let t = FakeTransport::default().on_get(
            ACCOUNT.path,
            vec![Step::Answer(401, r#"{"error":"unauthorized"}"#.into())],
        );
        assert_eq!(
            task.read(&t).err().unwrap().to_string(),
            "GET /v1/account answered HTTP 401: the token was refused"
        );

        let t = FakeTransport::default()
            .on_get(ACCOUNT.path, vec![ok(ACCOUNT_JSON)])
            .on_put(vec![Step::Answer(
                400,
                r#"[{"propertyName":"topic","errorMessage":"secret-topic"}]"#.into(),
            )]);
        let current = task.read(&t).unwrap();
        assert_eq!(
            task.write(&t, &current).err().unwrap().to_string(),
            "POST /v1/account/subscription answered HTTP 400"
        );
    }

    #[test]
    fn an_account_without_subscriptions_gets_its_first_one() {
        // What ntfy 2.26.0 sends for an account with no subscriptions: the key
        // is omitted, not an empty list.
        let mut recorded: Value = serde_json::from_str(ACCOUNT_JSON).unwrap();
        recorded.as_object_mut().unwrap().remove("subscriptions");
        let task = task(HOST, "<masked-topic-1>\n");
        let t = account(&recorded.to_string());
        let current = task.read(&t).unwrap();
        assert_eq!(
            task.diff(&current).unwrap()[0].to_string(),
            "ntfy account: subscription 1 (missing) -> (added)"
        );
    }

    #[test]
    fn an_answer_that_is_no_account_is_an_error_without_values() {
        let task = task(HOST, "t\n");
        let err = task
            .read(&account(r#"{"role":"user"}"#))
            .err()
            .unwrap()
            .to_string();
        assert!(err.contains("not an account"), "{err}");
        let wrong = r#"{"subscriptions":[{"base_url":"x","topic":"leaky-topic","extra":1},{"topic":"leaky-2"}]}"#;
        let err = task.read(&account(wrong)).err().unwrap().to_string();
        assert!(
            err.starts_with("/v1/account: the answer does not have"),
            "{err}"
        );
        assert!(!err.contains("leaky"), "{err}");
    }
}
