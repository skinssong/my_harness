//! The record and the replay that reads it.
//!
//! The interesting cases are the ones where the record and the loop disagree:
//! those are what replay exists to catch.

mod common;

use common::{config_at, response, serve_many, TempWorkspace};

use harness::agent::{self, Limits};
use harness::event::{Event, HaltRule, LoggedCall};
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

async fn recorded_run(
    replies: Vec<String>,
    max_turns: u32,
) -> (Vec<Event>, Option<agent::Outcome>, TempWorkspace) {
    let (addr, _seen) = serve_many(replies).await;
    let dir = TempWorkspace::new("record");
    std::fs::write(dir.path.join("a.txt"), "hi\n").unwrap();

    let workspace = Workspace::open(&dir.path).unwrap();
    let registry = harness::tools::builtin();
    let client = Client::new(config_at(addr, "k"))
        .unwrap()
        .with_tools(registry.specs());
    let request = Request::new("m", "be brief", "read a.txt").unwrap();

    let mut log = Log::create(dir.path.join("runs")).unwrap();
    let path = log.path().to_path_buf();
    let outcome = agent::run(
        &client,
        &registry,
        &workspace,
        request,
        Limits {
            max_turns,
            ..Limits::default()
        },
        &mut log,
    )
    .await
    .ok();
    drop(log);

    let events = Log::read(&path).unwrap();
    (events, outcome, dir)
}

#[tokio::test]
async fn a_recorded_run_replays_to_the_same_answer_and_the_same_spend() {
    let (events, outcome, _dir) = recorded_run(
        vec![
            calling(&[("call_1", "read_file", r#"{"path":"a.txt"}"#)], (100, 10)),
            answering("it says hi", (200, 20)),
        ],
        8,
    )
    .await;

    let outcome = outcome.expect("the run answered");
    let replayed = replay(&events).unwrap();
    assert_eq!(replayed.answer.as_deref(), Some(outcome.answer.as_str()));
    assert_eq!(replayed.turns, outcome.turns);
    assert_eq!(replayed.usage.total(), outcome.usage.total());
    assert_eq!(replayed.tools, 1);
}

#[tokio::test]
async fn the_record_holds_every_turn_in_order() {
    let (events, _outcome, _dir) = recorded_run(
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
            Event::Answer { .. } => "answer",
            Event::Halt { .. } => "halt",
        })
        .collect();
    assert_eq!(shape, ["start", "reply", "tool", "reply", "answer"]);
}

#[tokio::test]
async fn a_record_whose_answer_does_not_match_its_reply_is_caught() {
    let (mut events, _outcome, _dir) = recorded_run(
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
    let (mut events, _outcome, _dir) = recorded_run(
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
    let (mut events, _outcome, _dir) = recorded_run(
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
    let (events, _outcome, _dir) = recorded_run(
        vec![calling(&[("c1", "list_dir", "{}")], (10, 1))],
        1,
    )
    .await;

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
        Event::Start {
            model: "m".to_string(),
            system: "s".to_string(),
            task: "t".to_string(),
            max_turns: 8,
            stuck_after: 3,
            max_usd: None,
        },
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
    log.append(&Event::Start {
        model: "m".to_string(),
        system: "s".to_string(),
        task: "t".to_string(),
        max_turns: 8,
        stuck_after: 3,
        max_usd: None,
    })
    .unwrap();
    log.append(&Event::Reply {
        text: None,
        calls: vec![LoggedCall {
            id: "c1".to_string(),
            name: "list_dir".to_string(),
            arguments: "{}".to_string(),
        }],
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
        Event::Start {
            model: "m".to_string(),
            system: "s".to_string(),
            task: "t".to_string(),
            max_turns: 8,
            stuck_after: 3,
            max_usd: Some(0.001),
        },
        Event::Reply {
            text: None,
            calls: vec![LoggedCall {
                id: "c1".to_string(),
                name: "list_dir".to_string(),
                arguments: "{}".to_string(),
            }],
            input: 100,
            output: 10,
        },
        Event::Halt {
            reason: "budget: spent $0.001100 of $0.001000 after 1 turn(s)".to_string(),
            turns: 1,
            rule: Some(HaltRule::Budget),
        },
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
        Event::Start {
            model: "m".to_string(),
            system: "s".to_string(),
            task: "t".to_string(),
            max_turns: 8,
            stuck_after: 3,
            max_usd: None,
        },
        Event::Reply {
            text: None,
            calls: vec![LoggedCall {
                id: "c1".to_string(),
                name: "list_dir".to_string(),
                arguments: "{}".to_string(),
            }],
            input: 10,
            output: 1,
        },
        Event::Halt {
            reason: "no answer after 8 turns".to_string(),
            turns: 1,
            rule: Some(HaltRule::Ceiling),
        },
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
