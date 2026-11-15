//! The one boundary in S1.
//!
//! These tests run against the stub in `common`: it records the request it
//! received and replies with a canned response. That is the honest way to test
//! an HTTP client — no key, no network, and the thing under test is the bytes
//! that actually leave the process.

mod common;

use std::net::SocketAddr;

use common::{response, serve_once};

use harness::client::{Client, Error};
use harness::config::Config;
use harness::request::Request;

const REPLY_BODY: &str = r#"{"choices":[{"message":{"content":"ok"}}],
                             "usage":{"prompt_tokens":3,"completion_tokens":1}}"#;

fn config_at(addr: SocketAddr, key: &str) -> Config {
    let entries = [
        ("LLM_BASE_URL".to_string(), format!("http://{addr}/v1")),
        ("LLM_API_KEY".to_string(), key.to_string()),
        ("LLM_MODEL".to_string(), "m".to_string()),
    ];
    Config::from_lookup(move |name| {
        entries
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
    })
    .unwrap()
}

#[tokio::test]
async fn a_successful_call_sends_the_right_request_and_returns_the_reply() {
    let (addr, seen) = serve_once(response("200 OK", REPLY_BODY)).await;
    let client = Client::new(config_at(addr, "k")).unwrap();
    let request = Request::new("m", "be brief", "reply with the word ok").unwrap();

    let reply = client.complete(&request).await.unwrap();
    assert_eq!(reply.text(), Some("ok"));
    assert_eq!(reply.usage().total(), 4);

    let seen = seen.await.unwrap();
    assert!(
        seen.starts_with("POST /v1/chat/completions HTTP/1.1"),
        "request line was: {seen}"
    );
    assert!(
        seen.to_ascii_lowercase()
            .contains("authorization: bearer k"),
        "headers were: {seen}"
    );
    assert!(seen.contains("\"model\":\"m\""), "body was: {seen}");
    assert!(
        seen.to_ascii_lowercase().contains("user-agent: harness/"),
        "a gateway may refuse a request that does not identify itself: {seen}"
    );
    assert!(
        seen.to_ascii_lowercase().contains("x-opencode-session: harness-"),
        "a gateway may refuse a request it cannot route: {seen}"
    );
    assert!(
        seen.contains("reply with the word ok"),
        "body was: {seen}"
    );
    assert!(
        seen.contains("\"temperature\":0"),
        "determinism must be stated on the wire: {seen}"
    );
}

#[tokio::test]
async fn a_rejection_carries_the_status_and_the_body() {
    let (addr, _seen) = serve_once(response("401 Unauthorized", "bad key")).await;
    let client = Client::new(config_at(addr, "wrong")).unwrap();
    let request = Request::new("m", "s", "t").unwrap();

    match client.complete(&request).await.unwrap_err() {
        Error::Status { code, body } => {
            assert_eq!(code, 401);
            assert_eq!(body, "bad key");
        }
        other => panic!("expected a status error, got {other:?}"),
    }
}

#[tokio::test]
async fn an_error_body_is_bounded_before_it_reaches_the_operator() {
    let huge = "x".repeat(5_000);
    let (addr, _seen) = serve_once(response("502 Bad Gateway", &huge)).await;
    let client = Client::new(config_at(addr, "k")).unwrap();
    let request = Request::new("m", "s", "t").unwrap();

    match client.complete(&request).await.unwrap_err() {
        Error::Status { body, .. } => {
            assert!(
                body.chars().count() <= 401,
                "body was {} chars",
                body.chars().count()
            );
            assert!(body.ends_with('…'), "body was: {body}");
        }
        other => panic!("expected a status error, got {other:?}"),
    }
}

#[tokio::test]
async fn a_dead_endpoint_is_a_transport_error_not_a_panic() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    let client = Client::new(config_at(addr, "k")).unwrap();
    let request = Request::new("m", "s", "t").unwrap();
    assert!(matches!(
        client.complete(&request).await.unwrap_err(),
        Error::Transport(_)
    ));
}
