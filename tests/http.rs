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
