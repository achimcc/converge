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
