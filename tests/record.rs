//! The record, the replay that reads it, and the resume that continues it.
//!
//! The interesting cases are the ones where the record and the loop disagree:
//! those are what replay exists to catch.

mod common;

use common::{config_at, response, serve_many, TempWorkspace};

use harness::agent::{self, Approval, Ending, Limits};
use harness::event::{Event, Halt, LoggedCall};
use harness::log::Log;
use harness::replay::{replay, ReplayError};
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

fn start(max_turns: u32, stuck_after: u32, max_usd: Option<f64>) -> Event {
    Event::Start {
        model: "m".to_string(),
        system: "s".to_string(),
        task: "t".to_string(),
        max_turns,
        stuck_after,
        max_usd,
    }
}

fn logged(id: &str, name: &str, arguments: &str) -> LoggedCall {
    LoggedCall {
        id: id.to_string(),
        name: name.to_string(),
        arguments: arguments.to_string(),
    }
}

async fn recorded_run(
    replies: Vec<String>,
    max_turns: u32,
) -> (Vec<Event>, Option<Ending>, TempWorkspace) {
    let (addr, _seen) = serve_many(replies).await;
    let dir = TempWorkspace::new("record");
    std::fs::write(dir.path.join("a.txt"), "hi\n").unwrap();

    let workspace = Workspace::open(&dir.path).unwrap();
    let registry = harness::tools::builtin();
    let client = Client::new(config_at(addr, "k"))
        .unwrap()
        .with_tools(registry.specs());
    let request = Request::new("m", "be brief", "read a.txt").unwrap();
    let limits = Limits {
        max_turns,
        ..Limits::default()
    };

    let mut log = Log::create(dir.path.join("runs")).unwrap();
    let path = log.path().to_path_buf();
    let ending = agent::run(&client, &registry, &workspace, request, &limits, &mut log)
        .await
        .ok();
    drop(log);

    let events = Log::read(&path).unwrap();
    (events, ending, dir)
}

fn answered(ending: Option<Ending>) -> agent::Outcome {
    match ending {
        Some(Ending::Answered(outcome)) => outcome,
        other => panic!("expected an answer, got {other:?}"),
    }
}

#[tokio::test]
async fn a_recorded_run_replays_to_the_same_answer_and_the_same_spend() {
    let (events, ending, _dir) = recorded_run(
        vec![
            calling(&[("call_1", "read_file", r#"{"path":"a.txt"}"#)], (100, 10)),
            answering("it says hi", (200, 20)),
        ],
        8,
    )
    .await;

    let outcome = answered(ending);
    let replayed = replay(&events).unwrap();
    assert_eq!(replayed.answer.as_deref(), Some(outcome.answer.as_str()));
    assert_eq!(replayed.turns, outcome.turns);
    assert_eq!(replayed.usage.total(), outcome.usage.total());
    assert_eq!(replayed.tools, 1);
}

#[tokio::test]
async fn the_record_holds_every_turn_in_order() {
    let (events, _ending, _dir) = recorded_run(
        vec![
            calling(&[("call_1", "read_file", r#"{"path":"a.txt"}"#)], (100, 10)),
            answering("done", (200, 20)),
        ],
        8,
    )
    .await;

    let shape: Vec<&str> = events
        .iter()
        .map(|event| match event {
            Event::Start { .. } => "start",
            Event::Reply { .. } => "reply",
            Event::Tool { .. } => "tool",
            Event::ApprovalRequested { .. } => "requested",
            Event::ApprovalDecided { .. } => "decided",
            Event::Answer { .. } => "answer",
            Event::Halt(_) => "halt",
        })
        .collect();
    assert_eq!(shape, ["start", "reply", "tool", "reply", "answer"]);
}

#[tokio::test]
async fn a_record_whose_answer_does_not_match_its_reply_is_caught() {
    let (mut events, _ending, _dir) = recorded_run(
        vec![calling(&[("c1", "list_dir", "{}")], (1, 1)), answering("done", (1, 1))],
        8,
    )
    .await;

    for event in &mut events {
        if let Event::Answer { text, .. } = event {
            *text = "something else".to_string();
        }
    }

    match replay(&events) {
        Err(ReplayError::AnswerDiffers { decided, recorded }) => {
            assert_eq!(decided, "done");
            assert_eq!(recorded, "something else");
        }
        other => panic!("expected the mismatch to be caught, got {other:?}"),
    }
}

#[tokio::test]
async fn tool_results_out_of_order_are_caught() {
    let (mut events, _ending, _dir) = recorded_run(
        vec![
            calling(
                &[("c1", "read_file", r#"{"path":"a.txt"}"#), ("c2", "list_dir", "{}")],
                (1, 1),
            ),
            answering("done", (1, 1)),
        ],
        8,
    )
    .await;

    let positions: Vec<usize> = events
        .iter()
        .enumerate()
        .filter(|(_, event)| matches!(event, Event::Tool { .. }))
        .map(|(index, _)| index)
        .collect();
    events.swap(positions[0], positions[1]);

    assert!(matches!(
        replay(&events),
        Err(ReplayError::ToolOrder { .. })
    ));
}

#[tokio::test]
async fn a_record_that_stops_before_the_tool_result_is_incomplete() {
    let (mut events, _ending, _dir) = recorded_run(
        vec![calling(&[("c1", "list_dir", "{}")], (1, 1)), answering("done", (1, 1))],
        8,
    )
    .await;

    events.retain(|event| !matches!(event, Event::Tool { .. }));
    assert!(matches!(
        replay(&events),
        Err(ReplayError::ExpectedTool { .. })
    ));
}

#[tokio::test]
async fn a_halted_run_replays_as_having_no_answer() {
    let (events, ending, _dir) = recorded_run(
        vec![calling(&[("c1", "list_dir", "{}")], (10, 1))],
        1,
    )
    .await;

    assert!(matches!(
        ending,
        Some(Ending::Halted(Halt::Ceiling { turns: 1 }))
    ));
    let replayed = replay(&events).unwrap();
    assert_eq!(replayed.answer, None);
    assert_eq!(replayed.turns, 1);
    assert_eq!(replayed.usage.input(), 10);
}

#[test]
fn a_record_with_no_start_event_is_refused() {
    let events = vec![Event::Answer {
        text: "done".to_string(),
        turns: 1,
    }];
    assert_eq!(replay(&events), Err(ReplayError::NotStarted));
}

#[test]
fn a_reply_with_neither_text_nor_calls_is_refused() {
    let events = vec![
        start(8, 3, None),
        Event::Reply {
            text: None,
            calls: Vec::new(),
            input: 1,
            output: 1,
        },
    ];
    assert_eq!(replay(&events), Err(ReplayError::EmptyReply));
}

#[test]
fn the_log_file_is_one_json_object_per_line() {
    let dir = TempWorkspace::new("jsonl");
    let mut log = Log::create(dir.path.join("runs")).unwrap();
    log.append(&start(8, 3, None)).unwrap();
    log.append(&Event::Reply {
        text: None,
        calls: vec![logged("c1", "list_dir", "{}")],
        input: 5,
        output: 2,
    })
    .unwrap();

    let path = log.path().to_path_buf();
    drop(log);

    let raw = std::fs::read_to_string(&path).unwrap();
    assert_eq!(raw.lines().count(), 2);
    assert!(raw.contains("\"event\":\"start\""));
    assert!(raw.contains("\"event\":\"reply\""));

    let events = Log::read(&path).unwrap();
    assert_eq!(events.len(), 2);
    assert!(matches!(&events[1], Event::Reply { calls, .. } if calls.len() == 1));
}

/// A budget stop happens between deciding and acting, so the record holds a
/// reply and a halt with no results between them. That is an ending, not a
/// torn file, and replay has to accept it.
#[test]
fn a_budget_stop_replays_even_though_it_acted_on_nothing() {
    let events = vec![
        start(8, 3, Some(0.001)),
        Event::Reply {
            text: None,
            calls: vec![logged("c1", "list_dir", "{}")],
            input: 100,
            output: 10,
        },
        Event::Halt(Halt::Budget {
            turns: 1,
            spent: 0.0011,
            cap: 0.001,
        }),
    ];

    let replayed = replay(&events).unwrap();
    assert_eq!(replayed.turns, 1);
    assert_eq!(replayed.answer, None);
    assert_eq!(replayed.tools, 0, "it stopped before acting");
}

/// Replay holds a recorded halt against the limits the run started with, so a
/// loop that stopped halting correctly cannot replay clean.
#[test]
fn a_halt_the_limits_do_not_allow_is_refused() {
    let events = vec![
        start(8, 3, None),
        Event::Reply {
            text: None,
            calls: vec![logged("c1", "list_dir", "{}")],
            input: 10,
            output: 1,
        },
        Event::Halt(Halt::Ceiling { turns: 1 }),
    ];

    assert!(matches!(
        replay(&events),
        Err(ReplayError::HaltRuleDiffers { .. })
    ));
}

/// The record is written a line at a time and flushed per event, so a process
/// killed mid-write leaves a fragment. The events before it are still the run.
#[test]
fn a_record_cut_mid_line_keeps_what_landed() {
    let dir = TempWorkspace::new("torn");
    let path = dir.path.join("torn.jsonl");
    std::fs::write(
        &path,
        concat!(
            r#"{"event":"start","model":"m","system":"s","task":"t","max_turns":8,"stuck_after":3,"max_usd":null}"#,
            "\n",
            r#"{"event":"reply","text":"done","calls":[],"input":10,"output":1}"#,
            "\n",
            r#"{"event":"answer","text":"do"#
        ),
    )
    .unwrap();

    let events = Log::read(&path).unwrap();
    assert_eq!(events.len(), 2, "the torn tail is dropped, not the record");
}

// -- suspension and resume ------------------------------------------------

/// Run until the policy stops it, leaving a record that ends at the pause.
async fn suspended_run() -> (TempWorkspace, std::path::PathBuf, String) {
    let (addr, _seen) = serve_many(vec![calling(
        &[("call_1", "write_file", r#"{"path":"out.txt","content":"written"}"#)],
        (10, 1),
    )])
    .await;
    let dir = TempWorkspace::new("resume");
    let workspace = Workspace::open(&dir.path).unwrap();
    let registry = harness::tools::builtin();
    let client = Client::new(config_at(addr, "k"))
        .unwrap()
        .with_tools(registry.specs());
    let request = Request::new("m", "be brief", "write out.txt").unwrap();

    let mut log = Log::create(dir.path.join("runs")).unwrap();
    let path = log.path().to_path_buf();
    let ending = agent::run(
        &client,
        &registry,
        &workspace,
        request,
        &Limits::default(),
        &mut log,
    )
    .await
    .unwrap();
    drop(log);

    let call_id = match ending {
        Ending::Suspended(suspension) => suspension.call.id,
        other => panic!("expected a suspension, got {other:?}"),
    };
    (dir, path, call_id)
}

#[tokio::test]
async fn a_suspended_record_replays_as_waiting() {
    let (_dir, path, _call_id) = suspended_run().await;
    let events = Log::read(&path).unwrap();

    let replayed = replay(&events).unwrap();
    assert_eq!(replayed.turns, 1);
    assert_eq!(replayed.tools, 0, "it stopped before acting");
    assert_eq!(replayed.answer, None);
}

#[tokio::test]
async fn approving_a_suspension_runs_the_call_and_finishes_the_run() {
    let (dir, path, call_id) = suspended_run().await;
    assert!(!dir.path.join("out.txt").exists(), "nothing ran while waiting");

    let (addr, seen) = serve_many(vec![answering("written", (20, 2))]).await;
    let workspace = Workspace::open(&dir.path).unwrap();
    let registry = harness::tools::builtin();
    let client = Client::new(config_at(addr, "k"))
        .unwrap()
        .with_tools(registry.specs());
    let events = Log::read(&path).unwrap();
    let mut log = Log::open(&path).unwrap();

    let ending = agent::resume(
        &client,
        &registry,
        &workspace,
        &events,
        &Approval {
            call_id: call_id.clone(),
            approved: true,
        },
        &Limits::default(),
        &mut log,
    )
    .await
    .unwrap();
    drop(log);

    assert!(matches!(ending, Ending::Answered(_)), "got {ending:?}");
    assert_eq!(
        std::fs::read_to_string(dir.path.join("out.txt")).unwrap(),
        "written"
    );

    let seen = seen.await.unwrap();
    assert!(
        seen[0].contains("written"),
        "the model sees the result it was waiting for: {}",
        seen[0]
    );

    // The resumed run appends, and the whole record still replays.
    let events = Log::read(&path).unwrap();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::ApprovalDecided { approved: true, .. })),
        "{events:?}"
    );
    let replayed = replay(&events).unwrap();
    assert_eq!(replayed.answer.as_deref(), Some("written"));
    assert_eq!(replayed.tools, 1);
}

#[tokio::test]
async fn denying_a_suspension_tells_the_model_and_writes_nothing() {
    let (dir, path, call_id) = suspended_run().await;

    let (addr, seen) = serve_many(vec![answering("not allowed", (20, 2))]).await;
    let workspace = Workspace::open(&dir.path).unwrap();
    let registry = harness::tools::builtin();
    let client = Client::new(config_at(addr, "k"))
        .unwrap()
        .with_tools(registry.specs());
    let events = Log::read(&path).unwrap();
    let mut log = Log::open(&path).unwrap();

    let ending = agent::resume(
        &client,
        &registry,
        &workspace,
        &events,
        &Approval {
            call_id: call_id.clone(),
            approved: false,
        },
        &Limits::default(),
        &mut log,
    )
    .await
    .unwrap();
    drop(log);

    assert!(matches!(ending, Ending::Answered(_)), "got {ending:?}");
    assert!(
        !dir.path.join("out.txt").exists(),
        "a denial writes nothing"
    );
    let seen = seen.await.unwrap();
    assert!(
        seen[0].contains("denied by the operator"),
        "the model is told why: {}",
        seen[0]
    );
    assert!(replay(&Log::read(&path).unwrap()).is_ok());
}

#[tokio::test]
async fn resuming_the_wrong_call_is_refused() {
    let (dir, path, _call_id) = suspended_run().await;

    let (addr, _seen) = serve_many(Vec::new()).await;
    let workspace = Workspace::open(&dir.path).unwrap();
    let registry = harness::tools::builtin();
    let client = Client::new(config_at(addr, "k"))
        .unwrap()
        .with_tools(registry.specs());
    let events = Log::read(&path).unwrap();
    let mut log = Log::open(&path).unwrap();

    let result = agent::resume(
        &client,
        &registry,
        &workspace,
        &events,
        &Approval {
            call_id: "not_the_one".to_string(),
            approved: true,
        },
        &Limits::default(),
        &mut log,
    )
    .await;

    assert!(matches!(result, Err(agent::Error::UnknownCall(_))));
}
