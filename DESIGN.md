# Rust agent harness — design

**Status:** S1–S5 built and running; S6 and S7 deliberately open.

## Context

The CS 329Z project needs a system whose evaluation is rigorous, and the harness is that system's core. Building it in Rust from scratch is the point: no framework, so the loop is yours to understand.

Standalone project. It shares nothing with the trading screener elsewhere in this workspace — that project is unrelated and must not be imported, called or designed around.

## How to read this

Each phase is one **functional requirement** — a capability the harness lacked — and each requirement exists because the previous phase fails at something. The arc is:

*a call → an act → a record → forgetting selectively → a bound → composition → extension.*

The eval rig is drawn beside the spine rather than on it: it is required by the deliverable, not forced by the phase before it.

Diagrams are cumulative. **Grey** blocks were built in an earlier phase and are unchanged; **green** blocks are new in this phase; **amber** marks an existing block whose *shape* changes; a **red dotted** edge is a connection the phase removes.

Every phase has its own document, and every phase document carries the whole system drawn at that phase — directly on top of the one before it, so the difference *is* the phase. S1–S5 are built; S6 and S7 are open, with their blocks deliberately undesigned.

Every diagram is rendered from its `.mmd` source by [`diagrams/build.sh`](diagrams/build.sh), which then applies [`diagrams/size.py`](diagrams/size.py): mermaid otherwise leaves the size to the container, which draws small in a narrow column. Edit the source, run the script, and the SVG matches what was drawn. Any `.svg` also opens at full size on its own.

**Three phases are amber.** S3 extracts the loop's decisions so they can be replayed; S5 adds suspension to the driver; S6 makes it re-entrant. Each is a small, local change — but a change, and marking it is the honest drawing.

**There are no rebuild-from-scratch phases.** Every later phase is drawn as additions with, at most, one amber block.

---

## The arc

Read the arrows as **gaps** and the boxes as **capabilities**: each arrow names something the phase on its left cannot do, and that specific inability is what makes the next phase necessary. So the first arrow says "S2 exists because S1 cannot produce an answer that depends on an action."

It is not a schedule. The phases cannot usefully be reordered or skipped — each removes a limitation that blocks real work at the phase before it.

![The seven phases and what forces each next](diagrams/arc.svg)

*Source: [diagrams/arc.mmd](diagrams/arc.mmd)*

| Gap the arrow names | In plain terms |
|---|---|
| S1 cannot produce an answer that depends on an action | If the task needs a file read or a command run, the reply depends on something that doesn't exist yet. One call cannot do it. |
| S2 leaves no record of a run | When a run goes wrong at turn nine there is nothing to inspect afterwards — state was in memory, the model is non-deterministic — so you re-run and hope. |
| S3's history outgrows the window | Every turn kept in the prompt eventually exceeds the model's context, and quality degrades before the hard limit. |
| S4 cannot stop a tool from doing damage | Once the loop is reliable enough to leave running, the risk stops being "does it work" and becomes "what if it overwrites something". |
| S5 can only forget, and only globally | Compaction throws away old detail uniformly; it cannot scope a sub-problem into its own context and return just the conclusion. And with no way to discover a procedure on demand, every procedure has to sit in the prompt at once. |
| S6 still needs a rebuild per tool | Tools are compiled in, so a new capability means editing Rust, rebuilding, and shipping. |
| *(side track)* the eval rig needs a record | Not a gap in any phase — a gap in the evidence, present from S1 onward. Its prerequisite is S3's record, which is why it hangs off S3 and not off the last phase. |

---

## Phases

Each phase has one document: the requirement, why the previous phase could not meet it, how the data moves through it, and how it is checked.

| Phase | Requirement | Status |
|---|---|---|
| [S1](docs/01-s1-call.md) | one prompt in, one answer out, cost known | **built** |
| [S2](docs/02-s2-tools.md) | the model acts, observes, continues | **built** |
| [S3](docs/03-s3-record.md) | explain and reproduce any run, offline | **built** |
| [S4](docs/04-s4-context.md) | keep a long task inside the window | **built** |
| [S5](docs/05-s5-permissions.md) | bound what the agent may do unasked | **built** |
| [S6, S7](docs/06-s6-s7-open.md) | compose and measure; extend without rebuilding | open |

## Structs, and what crosses between them

Boxes act; every arrow is labelled with the value that crosses it. `main` builds a `Config` and the tools array once, hands a `Request` and `Limits` to the loop, and prints the `Outcome`. The loop sends a `Request` per turn, receives a `Reply`, and either stops or turns that reply's `ToolCall` into an `Observation` by way of the registry.

The drawing is cumulative, and it lives in the phase documents — one per phase, each built on the one before it:

| Phase | The system at that phase |
|---|---|
| S1 | [one call](docs/01-s1-call.md#the-system-at-this-phase) |
| S2 | [the loop and the tools](docs/02-s2-tools.md#the-system-at-this-phase) |
| S3 | [the record and replay](docs/03-s3-record.md#the-system-at-this-phase) |
| S4 | [projection](docs/04-s4-context.md#the-system-at-this-phase) |
| S5 | [permissions and suspension](docs/05-s5-permissions.md#the-system-at-this-phase) |

Read any one on its own: grey is what earlier phases built, green is what that phase adds, amber is an existing block whose shape changes, red dotted is a connection it removes. No phase is a redraw.

Every phase is drawn from what was built, so the colours describe commits rather than intentions.

| Actor | Acts by |
|---|---|
| `main` | parsing argv, assembling the pieces, printing |
| `agent::run` | deciding the next step, and when to stop |
| `Client` | one HTTP POST |
| `Registry` | finding, validating and running a call |
| the four tools | touching the filesystem and the shell |
| `Log` | appending events, and reading them back |
| `replay()` | re-deriving decisions, calling nothing |
| `resume()` | continuing a suspended run, live |
| the provider | not ours |

Three carriers are worth a second look. `Reply` is data, but its variant is what the loop branches on — an enum doing control's job. `Observation.ok` is the reverse: the loop never reads it, only the model does, which is why a failed tool cannot stop a run by accident. And `Limits` started as two numbers and now carries the prices as well, because a money budget is meaningless without a rate to convert tokens into dollars — a carrier that grew a second purpose rather than gaining a sibling.

---

## The eval rig — a parallel track

**Requirement.** Run the harness over a set of tasks, grade the results, and compare against a weaker configuration.

**Why it is not on the spine.** Every other box is forced by an inability of the phase before it. This one isn't: you couldn't prove anything at S1 either, so nothing about S5 in particular makes it necessary. It's forced by the deliverable — the course grades evaluation, and Results are the largest single block of the report. What it *does* depend on is a record to grade, which is why it hangs off S3 rather than off the last phase.

**Why it can't wait until the end.** The harness is worth measuring as soon as it can be run twice. A rig built at the end measures whatever the harness happens to have become; a rig built early changes what you decide to build next.

**Required.** A task suite with fixed inputs and recorded outcomes; a grader; and at least one weaker baseline, so a rising tide doesn't read as skill. Task and grader design are not done yet.

*Building blocks not designed yet.*

---

## Cross-phase decisions

Cheap now, expensive later.

1. **The record is the state.** Transcript, turn count and spend are derived from it. A mutable transcript plus a separate log gives two sources of truth, and replay drifts.
2. **Tool failure is an observation; transport failure ends the run.** A tool that times out must be seen by the model as a result it can react to; a 502 must stop the run. Never convert one into the other. Since S5 the second one is `Halt::Provider` rather than an error, because from the caller's side it is an ending like any other — but it stays a distinct one, so the blame is not lost.
3. **One chokepoint for execution.** Every action passes through one function that validates, runs, shapes and records. S3's recording attaches to that seam rather than being retrofitted into each tool. S5's permission gate sits *before* the loop calls it, because "may this run" is a question about the run, and a resumed run has to answer it from a record instead of asking again.
4. **The record lives outside the workspace.** `~/.harness/runs` by default. A record the tools can reach is not an audit trail — a live run listed its own log directory, which is how this got noticed. `--log` inside the workspace still works and warns, because keeping records beside a throwaway sandbox is a legitimate choice; `bash` can reach the default anyway, so this is placement, not a boundary.
5. **A run has four endings, and only one of them is an error.** Answered, halted by a rule, waiting for a decision, or the harness failed at its own bookkeeping. The first three are `Ok`; the record stores the same `Halt` value the caller receives, so the file and the return value cannot describe different things.

## Toolchain

- Toolchain in use: **rustc 1.100.0**, `edition = "2021"`.
- Dependencies, all of them: `tokio` (runtime, process, fs, time), `reqwest` (rustls, so no OpenSSL), `serde`/`serde_json`, `thiserror`, `dotenvy`, and — added at S2 — `schemars` for schemas derived from types, and `async-trait` so one registry can hold unlike tools.
- The crate is its own git repository; S1–S3 are committed.

## Model access

OpenAI-compatible HTTP, three settings and no inference: `LLM_BASE_URL`, `LLM_API_KEY`, `LLM_MODEL`, all required. Optional `LLM_PRICE_IN` and `LLM_PRICE_OUT` — dollars per million tokens, both or neither — are what make a money budget computable.

The client is provider-neutral: any endpoint that speaks `/chat/completions` with a `tools` array works. That is how the harness moved from a provider's own API to an OpenAI-compatible gateway without a code change, and why S1's two gateway quirks — a required `User-Agent`, a required session header — live in the adapter rather than in the domain.

Tool-calling quality is model-dependent. A weak model produces malformed calls, which usefully exercises the error path but slows development.
