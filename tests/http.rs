mod support;

use std::time::Duration;

use converge::{
    client::{HttpTransport, Transport},
    secret::Secret,
};
use support::Server;

fn transport(base: &str) -> HttpTransport {
    HttpTransport::new(
        base,
        "X-Api-Key",
        Secret::new("s3cret-key-value".into()),
        Duration::from_secs(5),
    )
}

#[test]
fn get_sends_the_key_as_a_header_and_returns_non_2xx_as_a_reply() {
    let server = Server::start(vec![("GET", "/a", 401, "{}".into())]);
    let reply = transport(&server.base_url()).get("/a").unwrap();
    assert_eq!(reply.status, 401);
    let seen = &server.requests()[0];
    assert!(seen
        .headers
        .to_ascii_lowercase()
        .contains("x-api-key: s3cret-key-value"));
    assert!(!seen.path.contains("s3cret"));
}

#[test]
fn the_header_name_is_the_callers() {
    let server = Server::start(vec![("GET", "/a", 200, "{}".into())]);
    HttpTransport::new(
        &server.base_url(),
        "X-Emby-Token",
        Secret::new("t0ken".into()),
        Duration::from_secs(5),
    )
    .get("/a")
    .unwrap();
    let headers = server.requests()[0].headers.to_ascii_lowercase();
    assert!(headers.contains("x-emby-token: t0ken"), "{headers}");
    assert!(!headers.contains("x-api-key"), "{headers}");
}

#[test]
fn put_sends_json() {
    let server = Server::start(vec![("PUT", "/u", 202, String::new())]);
    let reply = transport(&server.base_url())
        .put_json("/u", r#"[{"a":1}]"#)
        .unwrap();
    assert_eq!(reply.status, 202);
    let seen = &server.requests()[0];
    assert_eq!(seen.body, r#"[{"a":1}]"#);
    assert!(seen
        .headers
        .to_ascii_lowercase()
        .contains("content-type: application/json"));
}

#[test]
fn post_sends_json() {
    let server = Server::start(vec![("POST", "/p", 204, String::new())]);
    let reply = transport(&server.base_url())
        .post_json("/p", r#"{"b":2}"#)
        .unwrap();
    assert_eq!(reply.status, 204);
    let seen = &server.requests()[0];
    assert_eq!(
        (seen.method.as_str(), seen.body.as_str()),
        ("POST", r#"{"b":2}"#)
    );
}

#[test]
fn a_form_is_percent_encoded_and_says_what_it_is() {
    let server = Server::start(vec![("POST", "/f", 204, String::new())]);
    let json = r#"[{"name":"DE+EN","tag":null,"n":"ä & b=c"}]"#;
    let reply = transport(&server.base_url())
        .post_form("/f", &[("languages-profiles", json), ("b", "1")])
        .unwrap();
    assert_eq!(reply.status, 204);
    let seen = &server.requests()[0];
    assert!(seen
        .headers
        .to_ascii_lowercase()
        .contains("content-type: application/x-www-form-urlencoded"));
    assert!(seen
        .headers
        .to_ascii_lowercase()
        .contains("x-api-key: s3cret-key-value"));
    assert_eq!(
        seen.body,
        "languages-profiles=%5B%7B%22name%22%3A%22DE%2BEN%22%2C%22tag%22%3Anull%2C%22n%22%3A%22%C3%A4%20%26%20b%3Dc%22%7D%5D&b=1"
    );
}

#[test]
fn a_refused_connection_is_an_error_without_the_key() {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let err = transport(&format!("http://127.0.0.1:{port}"))
        .get("/a")
        .err()
        .unwrap()
        .to_string();
    assert!(err.starts_with("GET /a:"), "{err}");
    assert!(!err.contains("s3cret"));
}

#[test]
fn accept_json_asks_every_verb_for_json() {
    // Koel answers a request it does not take for an API call with a
    // redirect to its web page -- unless the client says it wants JSON.
    let server = Server::start(vec![
        ("GET", "/g", 200, "[]".into()),
        ("PUT", "/u", 200, "{}".into()),
        ("POST", "/p", 201, "{}".into()),
    ]);
    let t = HttpTransport::new(
        &server.base_url(),
        "Authorization",
        Secret::new("Bearer t0ken".into()),
        Duration::from_secs(5),
    )
    .accept_json();
    t.get("/g").unwrap();
    t.put_json("/u", "{}").unwrap();
    t.post_json("/p", "{}").unwrap();
    let requests = server.requests();
    assert_eq!(requests.len(), 3);
    for seen in &requests {
        let headers = seen.headers.to_ascii_lowercase();
        assert!(headers.contains("accept: application/json"), "{headers}");
        assert!(headers.contains("authorization: bearer t0ken"), "{headers}");
    }

    let plain = Server::start(vec![("GET", "/g", 200, "[]".into())]);
    transport(&plain.base_url()).get("/g").unwrap();
    let headers = plain.requests()[0].headers.to_ascii_lowercase();
    assert!(!headers.contains("accept: application/json"), "{headers}");
}

#[test]
fn the_anonymous_transport_sends_no_key_and_a_login_answers_a_token() {
    let server = Server::start(vec![(
        "POST",
        "/api/auth/login",
        200,
        r#"{"access_token":"jwt-value","role":"admin","username":"converge"}"#.into(),
    )]);
    let anonymous = HttpTransport::anonymous(&server.base_url(), Duration::from_secs(5));
    let token = converge::services::suggestarr::login(
        &anonymous,
        "converge",
        &Secret::new("pa55word".into()),
    )
    .expect("the login answered a token");
    assert_eq!(token.expose(), "jwt-value");

    let seen = &server.requests()[0];
    let headers = seen.headers.to_ascii_lowercase();
    // No key header at all -- the request that fetches the token carries none.
    assert!(!headers.contains("x-api-key"), "{headers}");
    assert!(!headers.contains("authorization"), "{headers}");
    // The password travels in the body, never in the path.
    assert!(seen.body.contains("pa55word"), "{}", seen.body);
    assert!(!seen.path.contains("pa55word"), "{}", seen.path);
}

#[test]
fn questarr_is_set_up_anonymously_and_the_password_stays_in_the_body() {
    // A fresh Questarr: the status says "no account", the setup answers the
    // token. Neither request may carry a key -- there is none yet.
    let server = Server::start(vec![
        (
            "GET",
            "/api/auth/status",
            200,
            r#"{"hasUsers":false}"#.into(),
        ),
        (
            "POST",
            "/api/auth/setup",
            200,
            r#"{"token":"jwt-value","user":{"id":"u-1","username":"achim"}}"#.into(),
        ),
    ]);
    let anonymous = HttpTransport::anonymous(&server.base_url(), Duration::from_secs(5));
    let token =
        converge::services::questarr::sign_in(&anonymous, "achim", &Secret::new("pa55word".into()))
            .expect("the setup answered a token");
    assert_eq!(token.expose(), "jwt-value");

    let seen = server.requests();
    assert_eq!(seen.len(), 2);
    for request in &seen {
        let headers = request.headers.to_ascii_lowercase();
        assert!(!headers.contains("authorization"), "{headers}");
        assert!(!headers.contains("x-api-key"), "{headers}");
        assert!(!request.path.contains("pa55word"), "{}", request.path);
    }
    assert_eq!(seen[1].path, "/api/auth/setup");
    assert!(seen[1].body.contains("pa55word"), "{}", seen[1].body);
}

// --- Redirects (audit B39) -------------------------------------------------
//
// Two listeners, as in the audit's measurement: A is the service, B is
// somewhere else. Before the fix ureq followed A's 302 to B with the
// `X-Api-Key` header still attached.

fn saw_key(server: &Server) -> bool {
    server
        .requests()
        .iter()
        .any(|r| r.headers.contains("s3cret-key-value"))
}

#[test]
fn a_redirect_to_another_host_is_refused_and_the_key_never_leaves() {
    let elsewhere = Server::start(vec![("GET", "/leak", 200, "{}".into())]);
    for location in [
        format!("{}/leak", elsewhere.base_url()),
        // protocol-relative: the same host part, no scheme
        format!(
            "//{}/leak",
            elsewhere.base_url().trim_start_matches("http://")
        ),
    ] {
        let target: &'static str = Box::leak(location.into_boxed_str());
        let service = Server::start(vec![("GET", "/api/v3/system/status", 302, target.into())]);
        let err = transport(&service.base_url())
            .get("/api/v3/system/status")
            .err()
            .expect("a redirect off the origin is an error")
            .to_string();
        assert!(err.contains("not followed"), "{err}");
        assert!(!err.contains("s3cret"), "{err}");
    }
    assert!(elsewhere.requests().is_empty(), "B was contacted");
    assert!(!saw_key(&elsewhere));
}

#[test]
fn a_redirect_on_the_same_origin_is_followed() {
    // Flask and Django send a trailing-slash redirect.
    let server = Server::start(vec![
        ("GET", "/api/users", 308, "/api/users/".into()),
        ("GET", "/api/users/", 200, "[]".into()),
    ]);
    let reply = transport(&server.base_url()).get("/api/users").unwrap();
    assert_eq!(reply.status, 200);
    assert_eq!(reply.body, "[]");
    assert_eq!(server.requests().len(), 2);
}

#[test]
fn a_redirect_to_another_port_is_refused() {
    let server = Server::start(vec![("GET", "/b", 200, "{}".into())]);
    let own = format!("{}/b", server.base_url());
    let target: &'static str = Box::leak(own.into_boxed_str());
    let front = Server::start(vec![("GET", "/a", 302, target.into())]);
    // `front` redirects to `server` -- another port is another origin.
    assert!(transport(&front.base_url()).get("/a").is_err());
    assert!(server.requests().is_empty());
}

#[test]
fn a_write_does_not_follow_a_redirect() {
    let elsewhere = Server::start(vec![("GET", "/leak", 200, "{}".into())]);
    let target: &'static str = Box::leak(format!("{}/leak", elsewhere.base_url()).into_boxed_str());
    let service = Server::start(vec![("PUT", "/a", 302, target.into())]);
    let reply = transport(&service.base_url()).put_json("/a", "{}").unwrap();
    assert_eq!(reply.status, 302);
    assert!(elsewhere.requests().is_empty());
}

#[test]
fn a_redirect_loop_ends() {
    let server = Server::start(vec![("GET", "/a", 302, "/a".into())]);
    let err = transport(&server.base_url())
        .get("/a")
        .err()
        .unwrap()
        .to_string();
    assert!(err.contains("redirects"), "{err}");
}

// --- No answer (design §54) -------------------------------------------------
//
// A real listener that takes the request and never answers, and a port
// nothing listens on. What the transport makes of the silence decides
// whether a run may be tried again as it is.

fn impatient(base: &str) -> HttpTransport {
    HttpTransport::new(
        base,
        "X-Api-Key",
        Secret::new("s3cret-key-value".into()),
        Duration::from_secs(1),
    )
}

fn closed_port() -> String {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    format!("http://127.0.0.1:{port}")
}

#[test]
fn a_read_that_gets_no_answer_is_no_answer_and_nothing_was_written() {
    let server = Server::start(vec![("GET", "/a", support::SILENT, String::new())]);
    let t = impatient(&server.base_url());
    let err = t.get("/a").err().expect("the listener never answers");
    assert!(
        matches!(err, converge::error::Error::NoAnswer { .. }),
        "{err}"
    );
    assert!(err.is_no_answer());
    assert!(err.to_string().starts_with("GET /a: timeout"), "{err}");
    assert!(!t.wrote());
}

#[test]
fn a_write_that_was_sent_and_not_answered_is_unconfirmed() {
    let server = Server::start(vec![("PUT", "/u", support::SILENT, String::new())]);
    let t = impatient(&server.base_url());
    let err = t.put_json("/u", r#"{"a":1}"#).err().unwrap();
    assert!(
        matches!(err, converge::error::Error::Unconfirmed { .. }),
        "{err}"
    );
    // Without this the test would not prove the write went out.
    assert_eq!(server.requests()[0].body, r#"{"a":1}"#);
    assert!(!err.is_no_answer());
    assert!(t.wrote());
    let said = err.to_string();
    assert!(
        said.contains("whether the service applied it is unknown"),
        "{said}"
    );
    assert!(!said.contains("s3cret"), "{said}");
}

#[test]
fn a_write_that_could_not_be_connected_is_no_answer_and_did_not_go_out() {
    let t = impatient(&closed_port());
    for err in [
        t.put_json("/u", "{}").err().unwrap(),
        t.post_json("/p", "{}").err().unwrap(),
        t.delete("/d").err().unwrap(),
        t.get("/a").err().unwrap(),
    ] {
        assert!(
            matches!(err, converge::error::Error::NoAnswer { .. }),
            "{err}"
        );
    }
    assert!(!t.wrote());
}

#[test]
fn an_answered_write_counts_as_written_whatever_its_status() {
    let server = Server::start(vec![("PUT", "/u", 500, String::new())]);
    let t = impatient(&server.base_url());
    assert!(!t.wrote());
    assert_eq!(t.put_json("/u", "{}").unwrap().status, 500);
    assert!(t.wrote());
}

#[test]
fn a_sign_in_that_gets_no_answer_is_a_read() {
    let server = Server::start(vec![(
        "POST",
        "/api/auth/login",
        support::SILENT,
        String::new(),
    )]);
    let anonymous =
        HttpTransport::anonymous(&server.base_url(), Duration::from_secs(1)).reads_only();
    let err = anonymous.post_json("/api/auth/login", "{}").err().unwrap();
    assert!(
        matches!(err, converge::error::Error::NoAnswer { .. }),
        "{err}"
    );
    assert!(!anonymous.wrote());
}

#[test]
fn a_set_cookie_line_comes_back_with_the_reply_and_fixed_headers_go_out() {
    let server = Server::start(vec![(
        "POST",
        "/api/login",
        200,
        "Set-Cookie: velvarr_session=sess-77c1-never-print-me; Path=/; HttpOnly; SameSite=Strict; Max-Age=604800\r\n\r\n{\"account\":{}}"
            .into(),
    )]);
    let reply = HttpTransport::anonymous(&server.base_url(), Duration::from_secs(5))
        .with_header("Origin", "https://velvarr.example".into())
        .post_json("/api/login", "{}")
        .unwrap();
    assert_eq!(reply.status, 200);
    assert_eq!(reply.body, r#"{"account":{}}"#);
    assert_eq!(
        reply.cookie("velvarr_session").unwrap().expose(),
        "sess-77c1-never-print-me"
    );
    let headers = server.requests()[0].headers.to_ascii_lowercase();
    assert!(
        headers.contains("origin: https://velvarr.example"),
        "{headers}"
    );
    assert!(
        headers.contains("content-type: application/json"),
        "{headers}"
    );
}

#[test]
fn a_session_cookie_travels_as_the_cookie_header() {
    let server = Server::start(vec![("GET", "/api/admin/users", 200, "{}".into())]);
    HttpTransport::new(
        &server.base_url(),
        "Cookie",
        Secret::new("velvarr_session=sess-1".into()),
        Duration::from_secs(5),
    )
    .with_header("X-Forwarded-Proto", "https".into())
    .get("/api/admin/users")
    .unwrap();
    let headers = server.requests()[0].headers.to_ascii_lowercase();
    assert!(
        headers.contains("cookie: velvarr_session=sess-1"),
        "{headers}"
    );
    assert!(headers.contains("x-forwarded-proto: https"), "{headers}");
}
