//! The reply, reduced to what the harness needs.
//!
//! A reply is one of two things, and the enum says so: an answer with text, or
//! a request for one or more tool calls. A reply that is neither cannot be
//! constructed, which is the S2 correction to S1's "content is required" —
//! a tool-calling reply carries `"content": null` and is perfectly valid.
//!
//! `total_tokens` is on the wire and is deliberately not stored: the total is
//! derived from the two parts, so the number cannot contradict itself.

use serde::Deserialize;
use thiserror::Error;

use crate::message::ToolCall;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ReplyError {
    #[error("the reply carried no choices")]
    NoChoices,
    #[error("the reply carried neither text nor a tool call")]
    Empty,
    #[error("the reply carried no token usage")]
    NoUsage,
    #[error("the reply was not readable: {0}")]
    Malformed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    input: u32,
    output: u32,
}

impl Usage {
    pub fn new(input: u32, output: u32) -> Self {
        Self { input, output }
    }

    pub fn input(&self) -> u32 {
        self.input
    }

    pub fn output(&self) -> u32 {
        self.output
    }

    pub fn total(&self) -> u32 {
        self.input + self.output
    }

    /// Running cost across every call in a loop.
    pub fn plus(self, other: Self) -> Self {
        Self {
            input: self.input + other.input,
            output: self.output + other.output,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    Answer {
        text: String,
        usage: Usage,
    },
    Calling {
        calls: Vec<ToolCall>,
        /// Whatever the model said alongside the calls, which is often nothing.
        text: Option<String>,
        usage: Usage,
    },
}

impl Reply {
    pub fn from_json(raw: &str) -> Result<Self, ReplyError> {
        let wire: Wire =
            serde_json::from_str(raw).map_err(|err| ReplyError::Malformed(err.to_string()))?;
        let choice = wire
            .choices
            .into_iter()
            .next()
            .ok_or(ReplyError::NoChoices)?;
        let usage = wire.usage.ok_or(ReplyError::NoUsage)?.into();

        let calls: Vec<ToolCall> = choice
            .message
            .tool_calls
            .into_iter()
            .map(|call| ToolCall {
                id: call.id,
                name: call.function.name,
                arguments: call.function.arguments,
            })
            .collect();
        let text = choice.message.content;

        if !calls.is_empty() {
            Ok(Self::Calling { calls, text, usage })
        } else if let Some(text) = text {
            Ok(Self::Answer { text, usage })
        } else {
            Err(ReplyError::Empty)
        }
    }

    pub fn usage(&self) -> Usage {
        match self {
            Self::Answer { usage, .. } | Self::Calling { usage, .. } => *usage,
        }
    }

    pub fn text(&self) -> Option<&str> {
        match self {
            Self::Answer { text, .. } => Some(text),
            Self::Calling { text, .. } => text.as_deref(),
        }
    }

    pub fn calls(&self) -> &[ToolCall] {
        match self {
            Self::Answer { .. } => &[],
            Self::Calling { calls, .. } => calls,
        }
    }

    /// True when the reply ends the run. Prefer inspecting `calls()` in a
    /// loop: `from_json` never yields a `Calling` with an empty call list, but
    /// the variant is writable by hand, and a loop that trusted `is_answer` for
    /// its stopping rule could spin on one.
    pub fn is_answer(&self) -> bool {
        matches!(self, Self::Answer { .. })
    }

    /// What the operator sees. Only an answer ends a run, so only an answer
    /// carries the cost line.
    pub fn report(&self) -> String {
        match self {
            Self::Answer { text, usage } => format!(
                "{}\n\ntokens {} in · {} out · {} total",
                text,
                usage.input,
                usage.output,
                usage.total()
            ),
            Self::Calling { calls, .. } => {
                let names: Vec<&str> = calls.iter().map(|call| call.name.as_str()).collect();
                format!(
                    "the model asked for {} call(s): {}",
                    calls.len(),
                    names.join(", ")
                )
            }
        }
    }
}

impl From<WireUsage> for Usage {
    fn from(wire: WireUsage) -> Self {
        Self {
            input: wire.prompt_tokens,
            output: wire.completion_tokens,
        }
    }
}

// The wire shape, kept private: these types exist to be read once and thrown
// away, so nothing outside this module can come to depend on them.
#[derive(Deserialize)]
struct Wire {
    #[serde(default)]
    choices: Vec<WireChoice>,
    #[serde(default)]
    usage: Option<WireUsage>,
}

#[derive(Deserialize)]
struct WireChoice {
    message: WireMessage,
}

#[derive(Deserialize)]
struct WireMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<WireToolCall>,
}

#[derive(Deserialize)]
struct WireToolCall {
    id: String,
    function: WireFunction,
}

#[derive(Deserialize)]
struct WireFunction {
    name: String,
    #[serde(default)]
    arguments: String,
}

#[derive(Deserialize)]
struct WireUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
}
