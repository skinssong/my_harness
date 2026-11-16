# S4 — keep a long task inside the window

**Status:** designed, 2026-11-16. No code yet.

[← design index](../DESIGN.md)

**Requirement.** A task that outlives the model's context still finishes coherently.

**Why.** The record grows monotonically and the request is rebuilt from it every turn. Eventually the request is larger than the window, and well before that the model attends worse — the transcript fills with tool output that mattered once. The record cannot be truncated, because S3 needs it whole. So the *prompt* has to become a projection of the record, with a rule for what falls out and what stands in its place.

## What a context actually does — measured

Two turns of a real run, one tool result of twenty bytes:

```
turn 1  request 756 tokens
turn 2  request 797 tokens   (+41 for a 20-byte result and the assistant's call)
```

So the fixed cost is about **740 tokens** — the system prompt, the task, and the four tool schemas — and each turn adds roughly `40 + bytes/4`. The schemas are the largest fixed item and are re-sent on every request; that is S2's bill, not S4's.

Now the same shape with every result at the 8 KiB cap, one call per turn (`740 + (turn−1) × 2088`):

| Turn | Request | | Turn | Request |
|---:|---:|---|---:|---:|
| 1 | 740 | | 6 | 11,180 |
| 2 | 2,828 | | 7 | 13,268 |
| 3 | 4,916 | | 8 | **15,356** ← default ceiling |
| 4 | 7,004 | | 20 | 40,412 |
| 5 | 9,092 | | 28 | 57,164 |

**What this says about priority.** At today's defaults — 8 turns, 8 KiB per observation — a run tops out near 15k tokens, which fits comfortably inside a 64k window. S4 is not urgent; it is needed when turns are raised past ~25, when a model with a 16k window is used, or when every turn returns something near the cap. That is worth stating plainly rather than building machinery for a problem that does not yet bite.

## The projection

Masking, not summarising. Older tool results keep their place and lose their bulk:

```
before (turn 6)                       after the projection
────────────────────────────────────  ────────────────────────────────────────
system   You work inside a small…     system   You work inside a small…
user     find the failing test        user     find the failing test
assistant → bash {cat build.log}      assistant → bash {cat build.log}
tool     exit 0\n<8,192 bytes>        tool     [masked: was 8,192 bytes;
                                                 rerun the command if needed]
…                                     …
assistant → bash {grep FAIL log}      assistant → bash {grep FAIL log}
tool     exit 0\nFAILED …             tool     exit 0\nFAILED …        ← last 2 turns kept
assistant → read_file {log}           assistant → read_file {log}
```

**Rules.**

1. The system message and the original task are never touched — they are the pinned prefix, and losing them is how compaction destroys a run.
2. The last *K* turns stay verbatim (default 2), because that is what the model is actually working on.
3. Older tool results are replaced by a marker that says how much was there and how to get it back. The marker must be actionable: "rerun the command" is a real option; "content removed" is not.
4. **A tool message and the assistant call that produced it are masked together or not at all.** A provider rejects a `tool` message whose `tool_call_id` has no preceding call. This is the constraint that makes "just drop the oldest messages" wrong.
5. Trigger when the estimate exceeds a fraction of the window (60%), not on every turn.

**Where it lives.** `fn project(messages: &[Message], policy: &ContextPolicy) -> Vec<Message>` in `context.rs` — pure, so it is tested without a network and without a model. The loop calls it when it builds each request; nothing else changes. The record keeps the full text, so **replay is unaffected**: it reads the record, never the projection.

**Estimating size.** `bytes / 4` plus the fixed cost, with no tokenizer dependency. The decision it drives is coarse — mask or don't — and a tokenizer would buy precision the threshold does not need. If it ever drives a hard limit, that changes.

## Where it sits in the system

The [struct diagram](../DESIGN.md#structs-and-what-crosses-between-them) gains one node and splits one edge; nothing else moves.

```
agent::run ──"Request, full"──▶ project() ──"Request, masked"──▶ Client
```

| What | Kind | Why it does — or does not — become a node |
|---|---|---|
| `project()` | actor | it transforms, the same way `replay()` does |
| `ContextPolicy` | carrier | rides in `Limits`, which already flows `main → agent::run`; a second edge would make it look like an independent input |
| the mask marker | carrier | text inside an existing `Message`; a type for a string buys nothing |

In the diagram `project()` is drawn in outline, because it is designed and not built. `Log` still receives whole events and `replay()` still reads the record, so masking is invisible to both — which is this phase's own claim made visible: the record is the truth, and the prompt is a projection of it.

## What it does not do

- **No model-summarised compaction.** Summarising costs a call, can summarise the task away, and needs its own guard against a summariser that loops. Rules first; this is the second step, and only if masking proves insufficient.
- **No per-tool retention.** Every tool result ages the same way. A file read once may matter later more than a directory listing; nothing here knows that.
- **No hard failure on overflow.** If a single turn still exceeds the window after masking, the provider errors and the run stops — correctly, but without a specific message.

## How it is checked

- A projection of a long transcript keeps the system message, the task and the last *K* turns intact.
- Every masked tool message keeps its `tool_call_id`, and its assistant call is still present.
- A transcript under the threshold is returned unchanged — byte for byte.
- Replay of a run whose request was projected still reproduces the run, because the record was never masked.
