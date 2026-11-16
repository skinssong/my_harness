//! Keeping a long task inside the window.
//!
//! The record is the truth and grows forever. The request is a *projection* of
//! it: the same messages in the same order, with the bulk of the old tool
//! results replaced by a marker that says how much was there and how to get it
//! back. Masking, not summarising — nothing is reordered, nothing is dropped,
//! and no message loses its partner.
//!
//! That last point is the one that bites. A provider rejects a `tool` message
//! whose `tool_call_id` has no `assistant` call above it in the array, so "keep
//! the last few messages" is wrong where "replace the content of an old result"
//! is not. Because this function only ever rewrites a `content` field, the call
//! and its result stay together by construction rather than by a check.
//!
//! Nothing here is async and nothing here calls a model, so the whole policy is
//! tested without a network.

use crate::message::Message;

/// Where the line is drawn. It rides in [`Limits`](crate::agent::Limits) rather
/// than arriving on its own, because it is one more bound on a run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContextPolicy {
    /// What the model will accept, in tokens.
    pub window: usize,
    /// Turns kept verbatim at the end — what the model is actually working on.
    pub keep_turns: usize,
    /// Mask once the estimate passes this fraction of the window. Projecting
    /// every turn would rewrite history the model is still using.
    pub trigger: f32,
    /// Tokens spent on what `messages` does not contain: the tool schemas,
    /// which are re-sent with every request.
    pub reserved: usize,
}

impl Default for ContextPolicy {
    fn default() -> Self {
        Self {
            window: 64_000,
            keep_turns: 2,
            trigger: 0.6,
            reserved: 800,
        }
    }
}

impl ContextPolicy {
    /// The estimate the projection is measured against.
    pub fn budget(&self) -> usize {
        ((self.window as f32) * self.trigger) as usize
    }
}

/// What stands in for a result that was there and is not any more. It has to be
/// actionable: "rerun the command" is a real option, "content removed" is not.
pub fn marker(bytes: usize) -> String {
    format!("[masked: was {bytes} bytes; rerun the command if needed]")
}

/// Rough size in tokens: four bytes to the token, which is close enough for a
/// decision as coarse as mask-or-don't. A tokenizer would buy precision this
/// threshold does not need.
pub fn estimate(messages: &[Message]) -> usize {
    let bytes: usize = messages
        .iter()
        .map(|message| serde_json::to_string(message).map_or(0, |text| text.len()))
        .sum();
    bytes / 4
}

/// The same conversation, sized to fit the policy's budget.
///
/// Below the budget the slice comes back untouched — byte for byte, so an
/// unchanged request is genuinely unchanged rather than merely equivalent.
/// Above it, old tool results are masked oldest first, stopping as soon as the
/// estimate fits: the further back a result sits, the less it is worth keeping,
/// and stopping early keeps as much of the conversation as the window allows.
pub fn project(messages: &[Message], policy: &ContextPolicy) -> Vec<Message> {
    let budget = policy.budget();
    if estimate(messages) + policy.reserved <= budget {
        return messages.to_vec();
    }

    let mut projected = messages.to_vec();
    let tail = tail_start(messages, policy.keep_turns);

    for index in 0..tail {
        let masked = match &projected[index] {
            Message::Tool {
                content,
                tool_call_id,
                name,
            } if !content.starts_with("[masked:") => Message::Tool {
                content: marker(content.len()),
                tool_call_id: tool_call_id.clone(),
                name: name.clone(),
            },
            _ => continue,
        };
        projected[index] = masked;

        if estimate(&projected) + policy.reserved <= budget {
            break;
        }
    }
    projected
}

/// How many messages were masked, for a run to say so once.
pub fn masked_count(messages: &[Message], projected: &[Message]) -> usize {
    messages
        .iter()
        .zip(projected)
        .filter(|(before, after)| before != after)
        .count()
}

/// The index where the verbatim tail begins: the `keep_turns`-th assistant
/// message counting back from the end, and everything after it. With fewer
/// turns than that, nothing is old enough to mask.
///
/// A turn is counted by its assistant message, which is the only element of one
/// the array marks as such.
fn tail_start(messages: &[Message], keep_turns: usize) -> usize {
    if keep_turns == 0 {
        return messages.len();
    }
    let mut seen = 0;
    for (index, message) in messages.iter().enumerate().rev() {
        if matches!(message, Message::Assistant { .. }) {
            seen += 1;
            if seen == keep_turns {
                return index;
            }
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

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
                vec![crate::message::ToolCall {
                    id: id.clone(),
                    name: "bash".to_string(),
                    arguments: format!("{{\"command\":\"cat log{turn}\"}}"),
                }],
                None,
            ));
            messages.push(Message::tool_result(id, "bash".to_string(), "x".repeat(bytes)));
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
        assert!(!masked.is_empty(), "a transcript this size has to lose something");

        for id in masked {
            let has_call = sent.iter().any(|message| match message {
                Message::Assistant { tool_calls, .. } => {
                    tool_calls.iter().any(|call| call.id == id)
                }
                _ => false,
            });
            assert!(has_call, "the assistant call for {id} is gone with its result");
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
        // The first tool result sits at index 3 and the newest at 13; if the
        // oldest is masked and the newest is not, the order is right.
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
}
