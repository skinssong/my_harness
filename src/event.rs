//! What a run records.
//!
//! One line per event, appended as it happens, so a run can be explained
//! afterwards without calling the model again. The record holds the decisions
//! (what the model said) and the effects (what the tools returned) separately,
//! because replay re-derives one from the other and a mismatch is the thing
//! worth catching.
//!
//! `LoggedCall` is this file's own shape, not the wire's: the provider's
//! `tool_calls` nesting is a detail of the request format, and a record should
//! not inherit it.

use serde::{Deserialize, Serialize};

use crate::message::ToolCall;

/// A tool call as the record keeps it: flat, and enough to re-issue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoggedCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

impl From<&ToolCall> for LoggedCall {
    fn from(call: &ToolCall) -> Self {
        Self {
            id: call.id.clone(),
            name: call.name.clone(),
            arguments: call.arguments.clone(),
        }
    }
}

impl From<&LoggedCall> for ToolCall {
    fn from(call: &LoggedCall) -> Self {
        Self {
            id: call.id.clone(),
            name: call.name.clone(),
            arguments: call.arguments.clone(),
        }
    }
}

// Not `Eq`: a budget is a float.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// Everything needed to describe the run without the environment.
    Start {
        model: String,
        system: String,
        task: String,
        max_turns: u32,
        /// Defaulted so a record written before these limits existed still reads.
        #[serde(default)]
        stuck_after: u32,
        #[serde(default)]
        max_usd: Option<f64>,
    },
    /// What the model said, and what it cost.
    Reply {
        text: Option<String>,
        calls: Vec<LoggedCall>,
        input: u32,
        output: u32,
    },
    /// What a tool returned. `text` is what the model was shown, after shaping.
    Tool {
        call_id: String,
        name: String,
        ok: bool,
        bytes: usize,
        text: String,
    },
    Answer {
        text: String,
        turns: u32,
    },
    Halt {
        reason: String,
        turns: u32,
        /// Which rule stopped the run. Optional because records written before
        /// S3's review carry only the reason, and replay must still read them.
        #[serde(default)]
        rule: Option<HaltRule>,
    },
}

/// The loop has four ways to stop without an answer, and replay checks a
/// recorded halt against the limits the run started with. A reason string could
/// not be compared; this can.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HaltRule {
    /// The turn ceiling.
    Ceiling,
    /// The money budget.
    Budget,
    /// The same call and result repeating.
    Stuck,
    /// The provider stopped answering.
    Transport,
}
