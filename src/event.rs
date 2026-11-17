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
    /// A call the policy would not let through unasked. The run stops here,
    /// before the call has done anything.
    ApprovalRequested {
        call_id: String,
        name: String,
        arguments: String,
    },
    /// The decision an operator made about it, in a later process.
    ApprovalDecided {
        call_id: String,
        approved: bool,
    },
    Answer {
        text: String,
        turns: u32,
    },
    /// How the run ended when it did not answer. The value the loop returns and
    /// the value written here are the same type, so they cannot drift.
    Halt(Halt),
}

/// Why a run stopped without an answer.
///
/// These are not failures. Three of them are the harness working as configured,
/// and the fourth is the provider — which the caller still needs, because it is
/// the difference between "we stopped it" and "it broke".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "rule", rename_all = "snake_case")]
pub enum Halt {
    /// The turn ceiling: no answer within the calls allowed.
    Ceiling { turns: u32 },
    /// The money budget was passed.
    Budget {
        turns: u32,
        spent: f64,
        cap: f64,
    },
    /// The same call and result kept repeating.
    Stuck { turns: u32, repeats: u32 },
    /// The provider stopped answering, or answered with something unreadable.
    Provider { turns: u32, detail: String },
}

impl std::fmt::Display for Halt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ceiling { turns } => write!(f, "no answer after {turns} turn(s)"),
            Self::Budget { turns, spent, cap } => write!(
                f,
                "spent ${spent:.6} of a ${cap:.6} budget after {turns} turn(s)"
            ),
            Self::Stuck { turns, repeats } => write!(
                f,
                "the same call and result repeated {repeats} time(s) in a row, \
                 at turn {turns}"
            ),
            Self::Provider { turns, detail } => {
                write!(f, "the provider stopped answering after {turns} turn(s): {detail}")
            }
        }
    }
}
