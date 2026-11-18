//! Fanning out over a directory.
//!
//! The provider here answers each request on its own terms, because the
//! children arrive concurrently and a scripted list of replies would be handed
//! out in whatever order the sockets happened to open.

mod common;

use std::net::SocketAddr;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

use common::{config_at, response, TempWorkspace};

use harness::agent::Limits;
use harness::compose::{fan_out, Claim, DEFAULT_WIDTH};
use harness::{Client, Workspace};

/// The four chunks, named so that name order is not story order.
const FILES: [&str; 4] = ["alpha.md", "beta.md", "delta.md", "gamma.md"];

fn calling_read(file: &str) -> String {
    let arguments = serde_json::json!({ "path": format!("notes/{file}") }).to_string();
    response(
        "200 OK",
        &serde_json::json!({
            "choices": [{"message": {"role": "assistant", "content": null, "tool_calls": [
                {"id": "call_1", "type": "function",
                 "function": {"name": "read_file", "arguments": arguments}}
            ]}}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 1}
        })
        .to_string(),
    )
}

fn answering(text: &str) -> String {
    response(
        "200 OK",
        &serde_json::json!({
            "choices": [{"message": {"role": "assistant", "content": text}}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 1}
        })
        .to_string(),
    )
}

/// Answer whatever arrives: a child's first call asks for the file, its second
/// reports on it, and later files are made to answer *sooner* so the children
/// finish in reverse order.
async fn serve_child_calls(files: &[&str]) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let files: Vec<String> = files.iter().map(|f| f.to_string()).collect();
    let expected = files.len() * 2;

    tokio::spawn(async move {
        for _ in 0..expected {
            let (mut socket, _) = listener.accept().await.unwrap();
            let request = common::read_request(&mut socket).await;

            let (delay, body) = match files.iter().position(|f| request.contains(f.as_str())) {
                Some(rank) if request.contains("\"role\":\"tool\"") => {
                    // Reporting. The earlier the file, the later it answers.
                    let delay = Duration::from_millis((files.len() - rank) as u64 * 40);
                    (delay, answering(&files[rank]))
                }
                Some(rank) => (Duration::ZERO, calling_read(&files[rank])),
                None => (Duration::ZERO, answering("no file named")),
            };

            tokio::time::sleep(delay).await;
            let _ = socket.write_all(body.as_bytes()).await;
            let _ = socket.flush().await;
        }
    });

    addr
}

async fn run(width: usize) -> (Vec<Claim>, TempWorkspace, TempWorkspace) {
    let workspace_dir = TempWorkspace::new("compose");
    let notes = workspace_dir.path.join("notes");
    std::fs::create_dir_all(&notes).unwrap();
    for (index, file) in FILES.iter().enumerate() {
        std::fs::write(notes.join(file), format!("chunk {index}")).unwrap();
    }
    let log_dir = TempWorkspace::new("compose-log");

    let addr = serve_child_calls(&FILES).await;
    let workspace = Workspace::open(&workspace_dir.path).unwrap();
    let registry = harness::tools::builtin();
    let client = Client::new(config_at(addr, "k"))
        .unwrap()
        .with_tools(registry.specs());

    let claims = fan_out(
        &client,
        &workspace,
        &Limits::default(),
        &log_dir.path,
        "notes",
        width,
    )
    .await
    .unwrap();

    (claims, workspace_dir, log_dir)
}

#[tokio::test]
async fn claims_come_back_in_file_order_even_though_children_finish_backwards() {
    let (claims, _workspace, _log) = run(DEFAULT_WIDTH).await;

    let order: Vec<&str> = claims.iter().map(|claim| claim.file.as_str()).collect();
    assert_eq!(
        order,
        FILES,
        "the join is by index, not by the order the provider answered"
    );
    for claim in &claims {
        assert!(
            claim.text.contains(&claim.file),
            "the claim answers about its own file: {claim:?}"
        );
    }
}

#[tokio::test]
async fn every_child_writes_its_own_record() {
    let (claims, _workspace, log_dir) = run(DEFAULT_WIDTH).await;

    let records = std::fs::read_dir(&log_dir.path).unwrap().count();
    assert_eq!(
        records,
        claims.len(),
        "one record per child, all in the fan-out's log directory"
    );
}

#[tokio::test]
async fn a_narrower_fan_out_still_joins_in_order() {
    let (claims, _workspace, _log) = run(1).await;

    let order: Vec<&str> = claims.iter().map(|claim| claim.file.as_str()).collect();
    assert_eq!(order, FILES, "bounded to one at a time, same result");
}

#[tokio::test]
async fn a_directory_with_no_files_is_an_empty_fan_out() {
    let workspace_dir = TempWorkspace::new("compose-empty");
    std::fs::create_dir_all(workspace_dir.path.join("notes")).unwrap();
    let log_dir = TempWorkspace::new("compose-empty-log");

    let addr = "127.0.0.1:1".parse().unwrap();
    let workspace = Workspace::open(&workspace_dir.path).unwrap();
    let registry = harness::tools::builtin();
    let client = Client::new(config_at(addr, "k"))
        .unwrap()
        .with_tools(registry.specs());

    let claims = fan_out(
        &client,
        &workspace,
        &Limits::default(),
        &log_dir.path,
        "notes",
        DEFAULT_WIDTH,
    )
    .await
    .unwrap();

    assert!(claims.is_empty());
}
