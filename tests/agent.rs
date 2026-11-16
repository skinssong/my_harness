//! The loop, end to end over a stubbed provider.
//!
//! Each test scripts the provider's replies and then checks what the loop did:
//! how many calls it made, what it sent back, and how it ended.

mod common;

use common::{config_at, response, serve_many, TempWorkspace};

use harness::agent::{self, Error, Limits};
use harness::config::Prices;
use harness::event::Event;
use harness::log::Log;
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
    run_loop_with(
        replies,
        Limits {
            max_turns,
            ..Limits::default()
        },
    )
    .await
}

async fn run_loop_with(
    replies: Vec<String>,
    limits: Limits,
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
        limits,
        &mut harness::log::Discard,
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
            ..Limits::default()
        },
        &mut harness::log::Discard,
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

#[tokio::test]
async fn a_run_stops_when_it_overspends() {
    let (addr, seen) = serve_many(vec![calling(&[("c1", "list_dir", "{}")], (100, 10))]).await;
    let dir = TempWorkspace::new("budget");
    let workspace = Workspace::open(&dir.path).unwrap();
    let registry = harness::tools::builtin();
    let client = Client::new(config_at(addr, "k"))
        .unwrap()
        .with_tools(registry.specs());
    let request = Request::new("m", "s", "look around").unwrap();

    let mut log = Log::create(dir.path.join("runs")).unwrap();
    let record = log.path().to_path_buf();
    let outcome = agent::run(
        &client,
        &registry,
        &workspace,
        request,
        Limits {
            // 110 tokens at $1/M is $0.00011, so the cap is passed on turn one.
            max_usd: Some(0.00005),
            prices: Some(Prices {
                input_per_mtok: 1.0,
                output_per_mtok: 1.0,
            }),
            ..Limits::default()
        },
        &mut log,
    )
    .await;
    drop(log);

    match outcome {
        Err(Error::BudgetExceeded { turns, spent, cap }) => {
            assert_eq!(turns, 1);
            assert!(spent > cap, "spent {spent} should exceed {cap}");
        }
        other => panic!("expected the budget to stop it, got {other:?}"),
    }

    assert_eq!(
        seen.await.unwrap().len(),
        1,
        "it stops before spending on another call"
    );
    let events = Log::read(&record).unwrap();
    assert!(
        matches!(events.last(), Some(Event::Halt { reason, .. }) if reason.starts_with("budget")),
        "the record says why it stopped: {events:?}"
    );
    assert!(
        !events.iter().any(|event| matches!(event, Event::Tool { .. })),
        "the budget was checked before acting: {events:?}"
    );
}

#[tokio::test]
async fn a_run_stops_when_the_same_call_and_result_repeat() {
    let (addr, _seen) = serve_many(vec![
        calling(&[("c1", "list_dir", "{}")], (10, 1)),
        calling(&[("c2", "list_dir", "{}")], (10, 1)),
    ])
    .await;
    let dir = TempWorkspace::new("stuck");
    let workspace = Workspace::open(&dir.path).unwrap();
    let registry = harness::tools::builtin();
    let client = Client::new(config_at(addr, "k"))
        .unwrap()
        .with_tools(registry.specs());
    let request = Request::new("m", "s", "look around").unwrap();

    let mut log = Log::create(dir.path.join("runs")).unwrap();
    let record = log.path().to_path_buf();
    let outcome = agent::run(
        &client,
        &registry,
        &workspace,
        request,
        Limits {
            stuck_after: 2,
            ..Limits::default()
        },
        &mut log,
    )
    .await;
    drop(log);

    match outcome {
        Err(Error::Stuck { turns, repeats }) => {
            assert_eq!(turns, 2);
            assert_eq!(repeats, 2);
        }
        other => panic!("expected the repetition to stop it, got {other:?}"),
    }
    let events = Log::read(&record).unwrap();
    assert!(
        matches!(events.last(), Some(Event::Halt { reason, .. }) if reason.starts_with("stuck")),
        "the record says why it stopped: {events:?}"
    );
}

#[tokio::test]
async fn a_run_is_not_stuck_when_each_turn_does_something_different() {
    let (addr, _seen) = serve_many(vec![
        calling(&[("c1", "read_file", r#"{"path":"a.txt"}"#)], (10, 1)),
        calling(&[("c2", "read_file", r#"{"path":"b.txt"}"#)], (10, 1)),
        calling(&[("c3", "list_dir", "{}")], (10, 1)),
        answering("done", (10, 1)),
    ])
    .await;
    let dir = TempWorkspace::new("notstuck");
    std::fs::write(dir.path.join("a.txt"), "first").unwrap();
    std::fs::write(dir.path.join("b.txt"), "second").unwrap();
    let workspace = Workspace::open(&dir.path).unwrap();
    let registry = harness::tools::builtin();
    let client = Client::new(config_at(addr, "k"))
        .unwrap()
        .with_tools(registry.specs());
    let request = Request::new("m", "s", "look around").unwrap();

    let outcome = agent::run(
        &client,
        &registry,
        &workspace,
        request,
        Limits {
            stuck_after: 2,
            ..Limits::default()
        },
        &mut harness::log::Discard,
    )
    .await
    .expect("different work each turn is not being stuck");

    assert_eq!(outcome.turns, 4);
}

/// Everything S4 claims, in one run: the request that leaves is not the request
/// that was built, and the record never notices.
#[tokio::test]
async fn the_wire_is_masked_but_the_record_is_not() {
    let (addr, seen) = serve_many(vec![
        calling(&[("call_1", "read_file", r#"{"path":"a.txt"}"#)], (100, 10)),
        calling(&[("call_2", "list_dir", "{}")], (100, 10)),
        calling(&[("call_3", "read_file", r#"{"path":"a.txt"}"#)], (100, 10)),
        answering("done", (100, 10)),
    ])
    .await;

    let workspace_dir = TempWorkspace::new("mask");
    std::fs::write(workspace_dir.path.join("a.txt"), "hello from disk\n").unwrap();
    let log_dir = TempWorkspace::new("mask-log");
    let mut log = harness::log::Log::create(&log_dir.path).unwrap();
    let record = log.path().to_path_buf();

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
            max_turns: 8,
            // Small enough that the third request has to lose something.
            context: harness::ContextPolicy {
                window: 150,
                keep_turns: 1,
                trigger: 0.6,
                reserved: 0,
            },
            ..Limits::default()
        },
        &mut log,
    )
    .await;
    assert!(outcome.is_ok(), "masking must not break the run");

    let seen = seen.await.unwrap();
    let third = &seen[2];
    assert!(
        third.contains("[masked: was 15 bytes;"),
        "the oldest result should be a marker by the third request: {third}"
    );
    assert!(
        third.contains("\"tool_call_id\":\"call_1\""),
        "the masked result keeps its id: {third}"
    );
    assert_eq!(
        third.matches("\"id\":\"call_1\"").count(),
        1,
        "its assistant call is still above it, so the pair travels together"
    );

    let recorded = std::fs::read_to_string(&record).unwrap();
    assert!(
        recorded.contains("hello from disk"),
        "the record keeps what the wire dropped"
    );
    assert!(
        !recorded.contains("[masked:"),
        "a marker is a request artefact, never a recorded one"
    );
}
