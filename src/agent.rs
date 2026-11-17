//! The loop: call, act, observe, repeat until something ends it.
//!
//! Three rules live here.
//!
//! * The ceiling counts **model calls**, not tool executions, so a turn that
//!   asks for four things still costs one turn.
//! * The stopping rule is **"are there calls?"**, not `finish_reason`, which
//!   providers populate inconsistently.
//! * A tool result goes back as an observation whatever happened — including a
//!   time-out or arguments that did not parse.
//!
//! A run has exactly four ways to end, and [`Ending`] names them: it answered,
//! a rule stopped it, it is waiting for a decision, or the harness could not do
//! its own bookkeeping. Only the last is an `Err`. The three rule-stops are
//! [`Halt`] values, which are also what the record stores, so the value the
//! caller sees and the value in the file cannot describe different things.
//!
//! A run may also pause. The policy is consulted before any call in a turn is
//! executed — all of them, not just the one being asked about — so a suspension
//! never leaves half a turn's effects behind. `resume` picks up from the record.

use crate::client::Client;
use crate::config::Prices;
use crate::context::{self, ContextPolicy};
use crate::event::{Event, Halt, LoggedCall};
use crate::log::{LogError, Recorder};
use crate::message::{Message, ToolCall};
use crate::policy::{Permission, Policy};
use crate::reply::{Reply, Usage};
use crate::request::Request;
use crate::tool::{Observation, Registry, MAX_OBSERVATION_BYTES};
use crate::workspace::Workspace;

#[derive(Debug, Clone, PartialEq)]
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
    /// What the run may do without asking. Bounds *what*, where the ceiling and
    /// the budget bound how long and how much.
    pub policy: Policy,
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
            policy: Policy::default(),
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

/// How a run stopped.
#[derive(Debug, Clone, PartialEq)]
pub enum Ending {
    /// It finished the task.
    Answered(Outcome),
    /// A rule stopped it. Not a failure: the rules are the harness working.
    Halted(Halt),
    /// It is waiting for a decision, and has done nothing that needs it.
    Suspended(Suspension),
}

/// A call awaiting a decision, with the record that holds the pause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suspension {
    pub call: ToolCall,
}

/// An operator's answer to a suspension.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Approval {
    pub call_id: String,
    pub approved: bool,
}

/// What can go wrong that is not a way for a run to end: the record could not be
/// written, or there is nothing to resume.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Log(#[from] LogError),
    #[error("nothing to resume: the record is not waiting for a decision")]
    NothingToResume,
    #[error("the record has no pending call with id {0}")]
    UnknownCall(String),
    #[error("the record does not begin with a start event")]
    BadRecord,
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
    limits: &Limits,
    recorder: &mut dyn Recorder,
) -> Result<Ending, Error> {
    recorder.record(Event::Start {
        model: request.model().to_string(),
        system: request.system().unwrap_or_default().to_string(),
        task: request.task().unwrap_or_default().to_string(),
        max_turns: limits.max_turns,
        stuck_after: limits.stuck_after,
        max_usd: limits.max_usd,
    })?;

    drive(
        client,
        registry,
        workspace,
        request,
        limits,
        1,
        Usage::new(0, 0),
        recorder,
    )
    .await
}

/// Continue a run that stopped for a decision.
///
/// The record already holds the reply that asked, so this dispatches that
/// turn's calls — the approved one, and the rest of the same turn, which never
/// ran because the pause came before any of them — and then carries on live.
pub async fn resume(
    client: &Client,
    registry: &Registry,
    workspace: &Workspace,
    events: &[Event],
    approval: &Approval,
    limits: &Limits,
    recorder: &mut dyn Recorder,
) -> Result<Ending, Error> {
    let (request, turns, spent) = rebuild(events)?;
    let pending = pending_turn(events).ok_or(Error::NothingToResume)?;
    if !pending.iter().any(|call| call.id == approval.call_id) {
        return Err(Error::UnknownCall(approval.call_id.clone()));
    }

    recorder.record(Event::ApprovalDecided {
        call_id: approval.call_id.clone(),
        approved: approval.approved,
    })?;

    let mut request = request;
    let mut messages = Vec::new();
    for logged in &pending {
        let call = ToolCall::from(logged);
        let observed = observe(
            &call,
            Some(approval),
            &limits.policy,
            registry,
            workspace,
            limits.max_observation_bytes,
        )
        .await;
        record_tool(recorder, &call, &observed)?;
        messages.push(Message::tool_result(
            call.id.clone(),
            call.name.clone(),
            observed.text,
        ));
    }
    request = request.continued(messages);

    drive(
        client,
        registry,
        workspace,
        request,
        limits,
        turns + 1,
        spent,
        recorder,
    )
    .await
}

/// The loop itself. `from_turn` is 1 for a new run and the next unused turn for
/// a resumed one, so the ceiling and the turn numbers mean the same thing
/// either way.
#[allow(clippy::too_many_arguments)]
async fn drive(
    client: &Client,
    registry: &Registry,
    workspace: &Workspace,
    mut request: Request,
    limits: &Limits,
    from_turn: u32,
    mut spent: Usage,
    recorder: &mut dyn Recorder,
) -> Result<Ending, Error> {
    let mut previous_turn: Option<String> = None;
    let mut repeats = 0u32;
    let mut said_it_masks = false;

    for turn in from_turn..=limits.max_turns {
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

        let reply = match client.complete(&request.replaced(sent)).await {
            Ok(reply) => reply,
            Err(err) => {
                // The provider is not a rule and not a tool: the run is over,
                // but it still has to say so, or the record cannot be replayed.
                // `turns` is the calls that finished, since this one did not.
                return halt(
                    recorder,
                    Halt::Provider {
                        turns: turn - 1,
                        detail: err.to_string(),
                    },
                );
            }
        };
        let usage = reply.usage();
        spent = spent.plus(usage);

        recorder.record(Event::Reply {
            text: reply.text().map(str::to_string),
            calls: reply.calls().iter().map(LoggedCall::from).collect(),
            input: usage.input(),
            output: usage.output(),
        })?;

        match decide(reply) {
            Decision::Answer { text } => {
                recorder.record(Event::Answer {
                    text: text.clone(),
                    turns: turn,
                })?;
                return Ok(Ending::Answered(Outcome {
                    answer: text,
                    usage: spent,
                    turns: turn,
                }));
            }
            Decision::Act { calls, text } => {
                // A run that cannot afford to act should not ask permission to
                // act, so the budget is settled first. It is deliberately not
                // checked before deciding: an answer already paid for is worth
                // returning even when it is the call that crossed the cap.
                if let (Some(cap), Some(spent_usd)) = (limits.max_usd, limits.spent_usd(spent)) {
                    if spent_usd > cap {
                        return halt(
                            recorder,
                            Halt::Budget {
                                turns: turn,
                                spent: spent_usd,
                                cap,
                            },
                        );
                    }
                }

                // The gate, before any call in this turn is executed. Asking
                // about one call after running another would make "nothing has
                // happened yet" false.
                if let Some(call) = asking(&calls, &limits.policy, registry) {
                    recorder.record(Event::ApprovalRequested {
                        call_id: call.id.clone(),
                        name: call.name.clone(),
                        arguments: call.arguments.clone(),
                    })?;
                    return Ok(Ending::Suspended(Suspension { call }));
                }

                let mut messages = vec![Message::calling(calls.clone(), text)];
                let mut signature = String::new();
                for call in &calls {
                    let observed = observe(
                        call,
                        None,
                        &limits.policy,
                        registry,
                        workspace,
                        limits.max_observation_bytes,
                    )
                    .await;
                    record_tool(recorder, call, &observed)?;
                    signature.push_str(&call.name);
                    signature.push('\u{1}');
                    signature.push_str(&call.arguments);
                    signature.push('\u{1}');
                    signature.push_str(&observed.text);
                    signature.push('\u{2}');
                    messages.push(Message::tool_result(
                        call.id.clone(),
                        call.name.clone(),
                        observed.text,
                    ));
                }

                // Repetitions, not occurrences: the first turn of a streak is
                // the one being repeated, so it counts zero. Otherwise
                // `--stuck-after 1` would stop a run on its very first call.
                repeats = match &previous_turn {
                    Some(previous) if *previous == signature => repeats + 1,
                    _ => 0,
                };
                previous_turn = Some(signature);

                if limits.stuck_after > 0 && repeats >= limits.stuck_after {
                    return halt(
                        recorder,
                        Halt::Stuck {
                            turns: turn,
                            repeats,
                        },
                    );
                }

                request = request.continued(messages);
            }
        }
    }

    halt(
        recorder,
        Halt::Ceiling {
            turns: limits.max_turns,
        },
    )
}

/// The first call in a turn that policy will not let through unasked. An
/// unknown tool is skipped: its answer is `dispatch`'s "no tool named …", which
/// tells the model more than a policy refusal would.
fn asking(calls: &[ToolCall], policy: &Policy, registry: &Registry) -> Option<ToolCall> {
    calls
        .iter()
        .find(|call| {
            registry.knows(&call.name) && policy.decide(&call.name) == Permission::Ask
        })
        .cloned()
}

/// May this call run? The operator's answer settles the call it was asked
/// about; everything else is the policy's. An unknown tool is let through so
/// that `dispatch` can say "no tool named …", which tells the model more than a
/// refusal would.
fn allowed(
    call: &ToolCall,
    approval: Option<&Approval>,
    policy: &Policy,
    registry: &Registry,
) -> bool {
    if let Some(approval) = approval {
        if approval.call_id == call.id {
            return approval.approved;
        }
    }
    if !registry.knows(&call.name) {
        return true;
    }
    policy.decide(&call.name) == Permission::Allow
}

/// Run one call, or refuse it, without deciding anything about the run. A
/// denial is an observation: the model is told why and may choose again.
async fn observe(
    call: &ToolCall,
    approval: Option<&Approval>,
    policy: &Policy,
    registry: &Registry,
    workspace: &Workspace,
    max_bytes: usize,
) -> Observation {
    match allowed(call, approval, policy, registry) {
        false => Observation::failed("denied by the operator"),
        true => registry
            .dispatch(call, workspace)
            .await
            .shaped(max_bytes),
    }
}

fn record_tool(
    recorder: &mut dyn Recorder,
    call: &ToolCall,
    observed: &Observation,
) -> Result<(), LogError> {
    recorder.record(Event::Tool {
        call_id: call.id.clone(),
        name: call.name.clone(),
        ok: observed.ok,
        bytes: observed.text.len(),
        text: observed.text.clone(),
    })
}

fn halt(recorder: &mut dyn Recorder, halt: Halt) -> Result<Ending, Error> {
    recorder.record(Event::Halt(halt.clone()))?;
    Ok(Ending::Halted(halt))
}

/// The calls of the turn that is waiting, taken from the reply that asked.
fn pending_turn(events: &[Event]) -> Option<Vec<LoggedCall>> {
    match events.last()? {
        Event::ApprovalRequested { .. } => {}
        _ => return None,
    }
    events.iter().rev().find_map(|event| match event {
        Event::Reply { calls, .. } => Some(calls.clone()),
        _ => None,
    })
}

/// Rebuild the conversation and the counters from a record, so a resumed run
/// continues rather than starts over.
fn rebuild(events: &[Event]) -> Result<(Request, u32, Usage), Error> {
    let Some(Event::Start {
        model,
        system,
        task,
        ..
    }) = events.first()
    else {
        return Err(Error::BadRecord);
    };
    let request = Request::new(model, system, task).map_err(|_| Error::BadRecord)?;
    let mut messages = Vec::new();
    let mut turns = 0;
    let mut spent = Usage::new(0, 0);

    for event in events.iter().skip(1) {
        match event {
            Event::Reply {
                text,
                calls,
                input,
                output,
            } => {
                turns += 1;
                spent = spent.plus(Usage::new(*input, *output));
                messages.push(Message::calling(
                    calls.iter().map(ToolCall::from).collect(),
                    text.clone(),
                ));
            }
            Event::Tool {
                call_id,
                name,
                text,
                ..
            } => messages.push(Message::tool_result(call_id.clone(), name.clone(), text.clone())),
            Event::Answer { .. } | Event::Halt(_) => break,
            Event::Start { .. } | Event::ApprovalRequested { .. } | Event::ApprovalDecided { .. } => {}
        }
    }

    Ok((request.continued(messages), turns, spent))
}
