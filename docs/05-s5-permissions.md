# S5 — bound what the agent may do unasked

**Status:** designed, 2026-11-16. No code yet.

[← design index](../DESIGN.md)

**Requirement.** No side-effecting action happens unless policy allows it, and an action needing approval can pause and resume.

**Why.** The tools write files and run a shell. Once the loop is reliable enough to leave running — which S3's record and budget made possible — the risk moves from correctness to blast radius. A turn ceiling and a money budget bound how long and how much; neither bounds what.

## The exchange, walked through

Default policy: **allow** `read_file`, `list_dir`; **ask** `write_file`, `bash`; anything unlisted is **deny**. Task: *"summarise team-notes.txt into report.txt"*.

**First run — it stops before acting.**

```
$ cargo run -- --workspace /tmp/w "summarise team-notes.txt into report.txt"
approval needed  call_7f2  write_file  {"path":"report.txt","content":"Q3 planning…"}
record  /Users/you/.harness/runs/run-1794812000-81234-0.jsonl
decide  cargo run -- resume <record> --approve call_7f2
        cargo run -- resume <record> --deny call_7f2
exit 3
```

The record at that moment:

```json
{"event":"start","model":"deepseek-v4.1-flash", …}
{"event":"reply","text":null,"calls":[{"id":"call_7f2","name":"write_file",
  "arguments":"{\"path\":\"report.txt\",\"content\":\"Q3 planning…\"}"}],"input":820,"output":44}
{"event":"approval_requested","call_id":"call_7f2","name":"write_file",
  "arguments":"{\"path\":\"report.txt\",\"content\":\"Q3 planning…\"}"}
```

Nothing was written. The suspension happens at the chokepoint, before `dispatch` — which is why S2 insisted every action passes through one place.

**Second run — the decision, then it continues.**

```
$ cargo run -- resume <record> --approve call_7f2
wrote report.txt; it says: Q3 planning — hire two engineers in Q1, migrate billing by March.

tokens 4201 in · 96 out · 4297 total · 2 turn(s)
record (appended to the same file)
```

Four events are appended, not rewritten:

```json
{"event":"approval_decided","call_id":"call_7f2","approved":true}
{"event":"tool","call_id":"call_7f2","name":"write_file","ok":true,"text":"wrote 92 bytes…"}
{"event":"reply","text":"wrote report.txt; it says: …","calls":[],"input":1733,"output":52}
{"event":"answer","text":"wrote report.txt; it says: …","turns":2}
```

**Denied — and the model carries on.**

```
$ cargo run -- resume <record> --deny call_7f2
I could not write it — the operator denied the call — so here is the summary instead: …

{"event":"approval_decided","call_id":"call_7f2","approved":false}
{"event":"tool","call_id":"call_7f2","name":"write_file","ok":false,
 "text":"tool_error: denied by the operator"}
```

A denial is an observation, not an abort: the same rule as a failing tool. The model is told why and can choose again.

## What this forces

**The run no longer has two endings.** `run` returns `Result<Outcome, Error>` today; suspension is neither. It becomes:

```rust
pub enum Ending {
    Answered(Outcome),
    Suspended(Suspension),   // needs a decision before it can continue
    Stopped(Error),
}
pub struct Suspension { pub call: ToolCall }
```

That is S5's amber change — the driver gains a stop-mid-run path — and the CLI maps it to exit 3, distinct from 1 so a script can tell "needs you" from "broke".

**Resume is replay plus continuation.** It rebuilds the conversation from the record (`Start` ⇒ system and task, each `Reply` ⇒ assistant, each `Tool` ⇒ tool result), applies the decision, dispatches, and then continues *live* — new model calls, new events appended to the same file. S3's append-only record is what makes this possible: nothing has to be re-derived from a model that no longer exists.

**The policy is per tool, not per argument.** `write_file` is asked about regardless of path; `bash` regardless of command. Argument-level rules ("may write inside `out/`, never `../`") are a later question, and the file tools already refuse paths outside the workspace.

```rust
pub enum Permission { Allow, Ask, Deny }
pub struct Policy { /* by tool name; unlisted ⇒ Deny */ }
// LLM_TOOLS_ALLOW=read_file,list_dir
// LLM_TOOLS_ASK=write_file,bash
// LLM_TOOLS_DENY=
```

## What it does not do

- **`bash` is not sandboxed, and will not be.** `read_file`, `write_file` and `list_dir` resolve paths lexically and cannot leave the workspace. A shell can. `sandbox-exec` is macOS-only and deprecated, a container is out of scope for a local tool, and a shell that can run `curl` can do anything a shell can do. **The approval gate is the control; that is a gate, not a cage**, and it should be described that way rather than implying containment.
- **No remember-my-answer.** Each `bash` call asks again. A per-run "allow all bash" would be one line, and is deliberately not there yet.
- **No revision of a decision.** The record is append-only; a second decision is a second event, and the first stands.

## How it is checked

- An allowed tool runs without interrupting anything.
- An asked-about tool suspends: the run exits 3, the record holds `approval_requested`, and **the effect has not happened**.
- `resume --approve` dispatches the pending call, appends the decision, and finishes the run.
- `resume --deny` returns `tool_error: denied by the operator` to the model, which can still answer.
- A tool named in no list is denied rather than allowed.
- A suspended run replays like any other — suspension is just events.
