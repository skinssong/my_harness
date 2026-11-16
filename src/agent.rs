//! The loop: call, act, observe, repeat until an answer.
//!
//! Three rules live here.
//!
//! * The ceiling counts **model calls**, not tool executions, so a turn that
//!   asks for four things still costs one turn.
//! * The stopping rule is **"are there calls?"**, not `finish_reason`, which
//!   providers populate inconsistently.
//! * A tool result goes back as an observation whatever happened — including a
//!   time-out or arguments that did not parse. Only a broken provider response
//!   ends the run, and that error comes from the client.
//!
//! Two things can stop it besides an answer: money, and going nowhere. The
//! ceiling alone is not enough — a loop can be cheap per turn and ruinous in
//! total, and it can repeat one action forever without ever asking for more
//! than the ceiling allows.
//!
//! Every turn is recorded before it is acted on, so a run that dies mid-tool
//! still leaves the reply that asked for it.

use crate::client::{Client, Error as ClientError};
use crate::config::Prices;
use crate::context::{self, ContextPolicy};
use crate::event::{Event, LoggedCall};
use crate::log::{LogError, Recorder};
use crate::message::{Message, ToolCall};
use crate::reply::{Reply, Usage};
use crate::request::Request;
use crate::tool::{Registry, MAX_OBSERVATION_BYTES};
use crate::workspace::Workspace;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    pub max_turns: u32,
    pub max_observation_bytes: usize,
    /// Stop once the run has cost more than this. Needs `prices` to mean
    /// anything; a budget without a price per token cannot be computed.
    pub max_usd: Option<f64>,
    /// How many times the same call may produce the same result before the run
    /// is declared stuck. Zero disables the check.
    pub stuck_after: u32,
    pub prices: Option<Prices>,
    /// How much of the conversation goes on the wire. The record is never
    /// touched by it.
    pub context: ContextPolicy,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_turns: 8,
            max_observation_bytes: MAX_OBSERVATION_BYTES,
            max_usd: None,
            stuck_after: 3,
            prices: None,
            context: ContextPolicy::default(),
        }
    }
}

impl Limits {
    pub fn spent_usd(&self, usage: Usage) -> Option<f64> {
        self.prices.map(|prices| prices.cost(usage.input(), usage.output()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub answer: String,
    pub usage: Usage,
    pub turns: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Client(#[from] ClientError),
    #[error("{0}")]
    Log(#[from] LogError),
    #[error("no answer after {turns} turns ({input} in · {output} out)")]
    TurnsExceeded {
        turns: u32,
        input: u32,
        output: u32,
    },
    #[error("stopped after {turns} turns: spent ${spent:.6} of a ${cap:.6} budget")]
    BudgetExceeded { turns: u32, spent: f64, cap: f64 },
    #[error("stopped after {turns} turns: the same call and result repeated {repeats} times")]
    Stuck { turns: u32, repeats: u32 },
}

/// What a reply means for the loop, separated from how the reply was obtained.
/// Replay judges a recorded reply with the same function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Act {
        calls: Vec<ToolCall>,
        text: Option<String>,
    },
    Answer {
        text: String,
    },
}

pub fn decide(reply: Reply) -> Decision {
    match reply {
        Reply::Answer { text, .. } => Decision::Answer { text },
        Reply::Calling { calls, text, .. } => Decision::Act { calls, text },
    }
}

pub async fn run(
    client: &Client,
    registry: &Registry,
    workspace: &Workspace,
    request: Request,
    limits: Limits,
    recorder: &mut dyn Recorder,
) -> Result<Outcome, Error> {
    recorder.record(Event::Start {
        model: request.model().to_string(),
        system: request.system().unwrap_or_default().to_string(),
        task: request.task().unwrap_or_default().to_string(),
        max_turns: limits.max_turns,
        stuck_after: limits.stuck_after,
        max_usd: limits.max_usd,
    })?;

    let mut request = request;
    let mut spent = Usage::new(0, 0);
    let mut previous_turn: Option<String> = None;
    let mut repeats = 0u32;
    let mut said_it_masks = false;

    for turn in 1..=limits.max_turns {
        // What goes on the wire is a projection of what the run has said so far.
        // The transcript itself is not touched, so the record keeps every byte.
        let sent = context::project(request.messages(), &limits.context);
        if !said_it_masks {
            let masked = context::masked_count(request.messages(), &sent);
            if masked > 0 {
                eprintln!(
                    "context: {masked} older tool result(s) masked for this request; \
                     the record keeps them whole"
                );
                said_it_masks = true;
            }
        }

        let reply = client.complete(&request.replaced(sent)).await?;
        let usage = reply.usage();
        spent = spent.plus(usage);

        recorder.record(Event::Reply {
            text: reply.text().map(str::to_string),
            calls: reply.calls().iter().map(LoggedCall::from).collect(),
            input: usage.input(),
            output: usage.output(),
        })?;

        // Checked before acting: passing the budget should not also run tools.
        if let (Some(cap), Some(spent_usd)) = (limits.max_usd, limits.spent_usd(spent)) {
            if spent_usd > cap {
                recorder.record(Event::Halt {
                    reason: format!(
                        "budget: spent ${spent_usd:.6} of ${cap:.6} after {turn} turn(s)"
                    ),
                    turns: turn,
                })?;
                return Err(Error::BudgetExceeded {
                    turns: turn,
                    spent: spent_usd,
                    cap,
                });
            }
        }

        match decide(reply) {
            Decision::Answer { text } => {
                recorder.record(Event::Answer {
                    text: text.clone(),
                    turns: turn,
                })?;
                return Ok(Outcome {
                    answer: text,
                    usage: spent,
                    turns: turn,
                });
            }
            Decision::Act { calls, text } => {
                let mut messages = vec![Message::calling(calls.clone(), text)];
                let mut signature = String::new();
                for call in &calls {
                    let observed = registry.dispatch(call, workspace).await;
                    let shaped = observed.shaped(limits.max_observation_bytes);
                    recorder.record(Event::Tool {
                        call_id: call.id.clone(),
                        name: call.name.clone(),
                        ok: shaped.ok,
                        bytes: shaped.text.len(),
                        text: shaped.text.clone(),
                    })?;
                    signature.push_str(&call.name);
                    signature.push('\u{1}');
                    signature.push_str(&call.arguments);
                    signature.push('\u{1}');
                    signature.push_str(&shaped.text);
                    signature.push('\u{2}');
                    messages.push(Message::tool_result(
                        call.id.clone(),
                        call.name.clone(),
                        shaped.text,
                    ));
                }

                repeats = match &previous_turn {
                    Some(previous) if *previous == signature => repeats + 1,
                    _ => 1,
                };
                previous_turn = Some(signature);

                if limits.stuck_after > 0 && repeats >= limits.stuck_after {
                    recorder.record(Event::Halt {
                        reason: format!(
                            "stuck: the same call and result repeated {repeats} times"
                        ),
                        turns: turn,
                    })?;
                    return Err(Error::Stuck {
                        turns: turn,
                        repeats,
                    });
                }

                request = request.continued(messages);
            }
        }
    }

    recorder.record(Event::Halt {
        reason: format!("no answer after {} turns", limits.max_turns),
        turns: limits.max_turns,
    })?;
    Err(Error::TurnsExceeded {
        turns: limits.max_turns,
        input: spent.input(),
        output: spent.output(),
    })
}
