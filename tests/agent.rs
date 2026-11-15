//! The loop, end to end over a stubbed provider.
//!
//! Each test scripts the provider's replies and then checks what the loop did:
//! how many calls it made, what it sent back, and how it ended.

mod common;

use common::{config_at, response, serve_many, TempWorkspace};

use harness::agent::{self, Error, Limits};
use harness::{Client, Request, Workspace};

fn calling(calls: &[(&str, &str, &str)], usage: (u32, u32)) -> String {
    let calls: Vec<serde_json::Value> = calls
        .iter()
        .map(|(id, name, arguments)| {
            serde_json::json!({
                "id": id, "type": "function",
                "function": {"name": name, "arguments": arguments}
            })
        })
        .collect();
    response(
        "200 OK",
        &serde_json::json!({
            "choices": [{"message": {"role": "assistant", "content": null, "tool_calls": calls}}],
            "usage": {"prompt_tokens": usage.0, "completion_tokens": usage.1}
        })
        .to_string(),
    )
}

fn answering(text: &str, usage: (u32, u32)) -> String {
    response(
        "200 OK",
        &serde_json::json!({
            "choices": [{"message": {"role": "assistant", "content": text}}],
            "usage": {"prompt_tokens": usage.0, "completion_tokens": usage.1}
        })
        .to_string(),
    )
}

async fn run_loop(
    replies: Vec<String>,
    max_turns: u32,
) -> (Result<agent::Outcome, Error>, Vec<String>, TempWorkspace) {
    let (addr, seen) = serve_many(replies).await;
    let workspace_dir = TempWorkspace::new("loop");
    std::fs::write(workspace_dir.path.join("a.txt"), "hello from disk\n").unwrap();

    let workspace = Workspace::open(&workspace_dir.path).unwrap();
    let registry = harness::tools::builtin();
    let client = Client::new(config_at(addr, "k"))
        .unwrap()
        .with_tools(registry.specs());
    let request = Request::new("m", "be brief", "what does a.txt say?").unwrap();

    let outcome = agent::run(
        &client,
        &registry,
        &workspace,
        request,
        Limits {
            max_turns,
            ..Limits::default()
        },
    )
    .await;

    let seen = seen.await.unwrap();
    (outcome, seen, workspace_dir)
}

#[tokio::test]
async fn a_tool_call_is_dispatched_and_its_result_goes_back_to_the_model() {
    let (outcome, seen, _dir) = run_loop(
        vec![
            calling(&[("call_1", "read_file", r#"{"path":"a.txt"}"#)], (100, 10)),
            answering("it says hello", (200, 20)),
        ],
        8,
    )
    .await;

    let outcome = outcome.unwrap();
    assert_eq!(outcome.answer, "it says hello");
    assert_eq!(outcome.turns, 2, "one turn to call, one to answer");
    assert_eq!(outcome.usage.input(), 300, "usage accumulates across turns");
    assert_eq!(outcome.usage.output(), 30);

    assert_eq!(seen.len(), 2);
    assert!(
        seen[0].contains("\"tools\":["),
        "the model must be told what it can call: {}",
        seen[0]
    );
    assert!(seen[0].contains("\"tool_choice\":\"auto\""));

    let second = &seen[1];
    assert!(second.contains("\"role\":\"tool\""), "body: {second}");
    assert!(second.contains("call_1"), "the result quotes its call id");
    assert!(
        second.contains("hello from disk"),
        "the result carries what the file said: {second}"
    );
    assert!(
        second.contains("\"role\":\"assistant\""),
        "the model's own call is echoed back before the result"
    );
}

#[tokio::test]
async fn several_calls_in_one_turn_cost_one_turn() {
    let (outcome, seen, _dir) = run_loop(
        vec![
            calling(
                &[
                    ("c1", "read_file", r#"{"path":"a.txt"}"#),
                    ("c2", "list_dir", "{}"),
                ],
                (100, 10),
            ),
            answering("both read", (200, 20)),
        ],
        8,
    )
    .await;

    let outcome = outcome.unwrap();
    assert_eq!(outcome.turns, 2, "two calls, still one model turn each way");

    let second = &seen[1];
    assert!(second.contains("\"tool_call_id\":\"c1\""));
    assert!(second.contains("\"tool_call_id\":\"c2\""));
}

#[tokio::test]
async fn the_ceiling_stops_a_model_that_never_answers() {
    let (outcome, seen, _dir) = run_loop(
        vec![
            calling(&[("c1", "list_dir", "{}")], (10, 1)),
            calling(&[("c2", "list_dir", "{}")], (10, 1)),
        ],
        2,
    )
    .await;

    match outcome {
        Err(Error::TurnsExceeded { turns, input, output }) => {
            assert_eq!(turns, 2);
            assert_eq!(input, 20);
            assert_eq!(output, 2);
        }
        other => panic!("expected the ceiling to bite, got {other:?}"),
    }
    assert_eq!(seen.len(), 2, "it stops calling once the ceiling is reached");
}

#[tokio::test]
async fn a_malformed_call_is_reported_to_the_model_rather_than_ending_the_run() {
    let (outcome, seen, _dir) = run_loop(
        vec![
            calling(&[("c1", "read_file", "path=a.txt")], (10, 1)),
            answering("I could not read it", (10, 1)),
        ],
        8,
    )
    .await;

    assert_eq!(outcome.unwrap().answer, "I could not read it");
    assert!(
        seen[1].contains("not valid JSON"),
        "the model must see why it failed: {}",
        seen[1]
    );
}

#[tokio::test]
async fn an_unknown_tool_is_reported_to_the_model() {
    let (outcome, seen, _dir) = run_loop(
        vec![
            calling(&[("c1", "make_coffee", "{}")], (10, 1)),
            answering("there is no coffee tool", (10, 1)),
        ],
        8,
    )
    .await;

    assert!(outcome.is_ok());
    assert!(
        seen[1].contains("no tool named make_coffee"),
        "the model must learn which tools exist: {}",
        seen[1]
    );
}

#[tokio::test]
async fn a_large_result_is_shaped_before_it_is_sent() {
    let (addr, seen) = serve_many(vec![
        calling(&[("c1", "bash", r#"{"command":"yes x | head -c 20000"}"#)], (10, 1)),
        answering("done", (10, 1)),
    ])
    .await;

    let dir = TempWorkspace::new("shaping");
    let workspace = Workspace::open(&dir.path).unwrap();
    let registry = harness::tools::builtin();
    let client = Client::new(config_at(addr, "k"))
        .unwrap()
        .with_tools(registry.specs());
    let request = Request::new("m", "s", "run it").unwrap();

    let outcome = agent::run(
        &client,
        &registry,
        &workspace,
        request,
        Limits {
            max_turns: 4,
            max_observation_bytes: 1_000,
        },
    )
    .await
    .unwrap();

    assert_eq!(outcome.answer, "done");
    let second = seen.await.unwrap().remove(1);
    assert!(
        second.contains("bytes elided"),
        "a 20 KB observation must not travel whole: {}",
        &second[second.len().saturating_sub(400)..]
    );
    assert!(
        second.len() < 5_000,
        "the second request stayed small: {} bytes",
        second.len()
    );
}
