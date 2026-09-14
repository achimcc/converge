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
