//! The reply, reduced to what S1 needs: the text and what it cost.
//!
//! The provider sends a large object and we keep two things. `total_tokens` is
//! on the wire and is deliberately not stored — the total is derived from the
//! two parts, so the number cannot contradict itself. Usage is required rather
//! than optional: "cost known" is the requirement, so a reply that omits it is
//! a failure, not a success with an unknown price.

use serde::Deserialize;
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ReplyError {
    #[error("the reply carried no choices")]
    NoChoices,
    #[error("the reply carried no message content")]
    NoContent,
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
    pub fn input(&self) -> u32 {
        self.input
    }

    pub fn output(&self) -> u32 {
        self.output
    }

    pub fn total(&self) -> u32 {
        self.input + self.output
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    text: String,
    usage: Usage,
}

impl Reply {
    pub fn from_json(raw: &str) -> Result<Self, ReplyError> {
        let wire: Wire =
            serde_json::from_str(raw).map_err(|err| ReplyError::Malformed(err.to_string()))?;
        let choice = wire.choices.into_iter().next().ok_or(ReplyError::NoChoices)?;
        let text = choice.message.content.ok_or(ReplyError::NoContent)?;
        let usage = wire.usage.ok_or(ReplyError::NoUsage)?;
        Ok(Self {
            text,
            usage: Usage {
                input: usage.prompt_tokens,
                output: usage.completion_tokens,
            },
        })
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn usage(&self) -> Usage {
        self.usage
    }

    /// What the operator sees: the answer, then what it cost.
    pub fn report(&self) -> String {
        format!(
            "{}\n\ntokens {} in · {} out · {} total",
            self.text,
            self.usage.input,
            self.usage.output,
            self.usage.total()
        )
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
}

#[derive(Deserialize)]
struct WireUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
}
