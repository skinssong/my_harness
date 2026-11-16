# harness

An agent harness, built from the loop outwards. The model call is a function of
tokens; everything that makes it an agent — what it sees, what it can do, when
it stops — is this program.

Phases so far: **S1** one call and its cost. **S2** tools, a chokepoint and the
loop. **S3** a record of every run, replay of that record, a money budget and a
stuck detector. The design, including the reasoning for each phase, is in
[DESIGN.md](DESIGN.md).

## Run it

Three settings, all required, no inference:

```
# harness/.env
LLM_BASE_URL=https://opencode.ai/zen/go/v1
LLM_API_KEY=…
LLM_MODEL=deepseek-v4.1-flash

# optional, for a money budget: dollars per million tokens
LLM_PRICE_IN=0.27
LLM_PRICE_OUT=1.10
```

```
cargo run -- "create greeting.txt containing hello, then tell me what it says"

It says: hello

tokens 2532 in · 113 out · 2645 total · 3 turn(s)
```

| Flag | Effect |
|---|---|
| `--workspace DIR` | the directory the tools work in (default: current) |
| `--max-turns N` | model calls allowed before stopping (default 8) |
| `--max-usd X` | stop once the run has cost more than X (needs the prices above) |
| `--stuck-after N` | stop when the same call and result repeat N times (default 3, 0 off) |
| `--log DIR` | where runs are recorded (default `~/.harness/runs`) |

## What it can do

| Tool | Does |
|---|---|
| `read_file` | reads a workspace file; `offset` and `limit` for long ones |
| `list_dir` | lists a directory, with `dir/` marked |
| `write_file` | writes a file, creating parents |
| `bash` | runs `sh -c` from the workspace root, with a timeout |

The file tools refuse paths that leave the workspace. **`bash` does not** — a
shell can go anywhere, and containing it is S5's job. Point `--workspace` at
something you are willing to lose.

## Record and replay

Every run writes one JSON object per event, flushed as it happens:

```
$ cargo run -- "create a.txt containing 'hi', then read it back"
a.txt created and reads: `hi`

tokens 2605 in · 142 out · 2747 total · 3 turn(s)
record /Users/you/.harness/runs/run-1794805969-60059-0.jsonl
```

```
$ cargo run -- replay ~/.harness/runs/run-1794805969-60059-0.jsonl
a.txt created and reads: `hi`

replayed 3 turn(s) · 3 tool result(s) · tokens 2605 in · 142 out · 2747 total
```

Replay calls nothing: it re-derives each decision from the record and checks the
recorded effects match, so a change to the loop that alters a decision shows up
as a mismatch. It needs no provider settings at all.

The record lives outside the workspace on purpose. A directory the tools can
reach is not an audit trail — `write_file` and `bash` would both reach it. Point
`--log` inside the workspace and the harness warns rather than stopping you.

## Layout

| Module | Holds |
|---|---|
| `config` | three settings, validated once |
| `message` / `reply` / `request` | the shapes on the wire |
| `client` | the single boundary: one POST, three ways to fail |
| `tool` | the contract, the registry, the chokepoint, observation shaping |
| `tools/` | `read_file`, `list_dir`, `write_file`, `bash` |
| `agent` | the loop, its ceiling, the budget and the stuck check |
| `event` / `log` | what a run records, and the file it records to |
| `replay` | re-deriving a recorded run |
| `workspace` | where the tools may look |

## Check it

```
cargo test      # 80 tests; the provider is a localhost stub, so no key is needed
cargo clippy --all-targets
```

## Not yet

No compaction (S4), so a long run eventually outgrows the window. No permissions
(S5): `bash` is unbounded, and nothing asks before a file is written. No
subagents or eval rig (S6), no MCP (S7). No retries anywhere.

Replay re-derives decisions but not halt reasons: a budget or stuck stop is
replayed by turn count alone.
