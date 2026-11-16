//! Keeping a long task inside the window.
//!
//! The projection is a pure function over a message slice, so every rule the
//! design leans on can be checked here — no provider, no key, no network.

use harness::context::{estimate, masked_count, project, ContextPolicy};
use harness::{Message, ToolCall};

fn policy(window: usize, keep_turns: usize) -> ContextPolicy {
    ContextPolicy {
        window,
        keep_turns,
        trigger: 1.0,
        reserved: 0,
    }
}

/// system, task, then `turns` of (assistant call, tool result).
fn transcript(turns: usize, bytes: usize) -> Vec<Message> {
    let mut messages = vec![
        Message::system("be brief"),
        Message::user("find the failing test"),
    ];
    for turn in 0..turns {
        let id = format!("call_{turn}");
        messages.push(Message::calling(
            vec![ToolCall {
                id: id.clone(),
                name: "bash".to_string(),
                arguments: format!("{{\"command\":\"cat log{turn}\"}}"),
            }],
            None,
        ));
        messages.push(Message::tool_result(
            id,
            "bash".to_string(),
            "x".repeat(bytes),
        ));
    }
    messages
}

#[test]
fn below_the_budget_nothing_changes() {
    let messages = transcript(3, 40);
    assert_eq!(project(&messages, &policy(100_000, 2)), messages);
}

#[test]
fn the_prefix_and_the_tail_survive() {
    let messages = transcript(6, 4_000);
    let sent = project(&messages, &policy(4_000, 2));

    assert!(sent.len() == messages.len(), "nothing is removed");
    assert_eq!(sent[0], messages[0], "the system message is pinned");
    assert_eq!(sent[1], messages[1], "the task is pinned");
    assert_eq!(&sent[8..], &messages[8..], "the last two turns are verbatim");
}

#[test]
fn a_masked_result_keeps_its_call() {
    let messages = transcript(6, 4_000);
    let sent = project(&messages, &policy(4_000, 2));

    let masked = sent
        .iter()
        .filter_map(|message| match message {
            Message::Tool {
                content,
                tool_call_id,
                ..
            } if content.starts_with("[masked:") => Some(tool_call_id.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        !masked.is_empty(),
        "a transcript this size has to lose something"
    );

    for id in masked {
        let has_call = sent.iter().any(|message| match message {
            Message::Assistant { tool_calls, .. } => tool_calls.iter().any(|call| call.id == id),
            _ => false,
        });
        assert!(
            has_call,
            "the assistant call for {id} is gone with its result"
        );
    }
}

#[test]
fn the_marker_states_what_it_replaced() {
    let messages = transcript(6, 4_000);
    let sent = project(&messages, &policy(4_000, 2));
    let marker = sent
        .iter()
        .find_map(|message| match message {
            Message::Tool { content, .. } if content.starts_with("[masked:") => {
                Some(content.clone())
            }
            _ => None,
        })
        .expect("something was masked");
    assert!(marker.contains("4000 bytes"), "got {marker}");
}

#[test]
fn masking_stops_as_soon_as_it_fits() {
    let messages = transcript(8, 4_000);
    // Room for roughly the prefix, two turns and a little more.
    let sent = project(&messages, &policy(6_000, 2));
    let masked = masked_count(&messages, &sent);
    assert!(masked > 0 && masked < 8, "masked {masked} of 8 turns");
}

#[test]
fn the_oldest_results_go_first() {
    let messages = transcript(6, 4_000);
    let sent = project(&messages, &policy(4_000, 2));
    // The first tool result sits at index 3 and the newest at 13; if the oldest
    // is masked and the newest is not, the order is right.
    assert!(matches!(&sent[3], Message::Tool { content, .. } if content.starts_with("[masked:")));
    assert!(matches!(&sent[11], Message::Tool { content, .. } if !content.starts_with("[masked:")));
}

#[test]
fn a_short_conversation_is_never_masked() {
    let messages = transcript(1, 40_000);
    assert_eq!(project(&messages, &policy(4_000, 2)), messages);
}

#[test]
fn the_estimate_grows_with_the_content() {
    let small = estimate(&transcript(1, 100));
    let large = estimate(&transcript(1, 10_000));
    assert!(large > small + 2_000);
}
