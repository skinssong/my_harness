# harness

An agent harness, built from the loop outwards. The model call is a function of
tokens; everything that makes it an agent — what it sees, what it can do, when
it stops — is this program.

Phases so far: **S1** one call and its cost. **S2** tools, a chokepoint and the
loop. The design, including the reasoning for each phase, is in
[DESIGN.md](DESIGN.md).

## Run it

Three settings, all required, no inference:

```
# harness/.env
LLM_BASE_URL=https://opencode.ai/zen/go/v1
LLM_API_KEY=…
LLM_MODEL=deepseek-v4.1-flash
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

## Layout

| Module | Holds |
|---|---|
| `config` | three settings, validated once |
| `message` / `reply` / `request` | the shapes on the wire |
| `client` | the single boundary: one POST, three ways to fail |
| `tool` | the contract, the registry, the chokepoint, observation shaping |
| `tools/` | `read_file`, `list_dir`, `write_file`, `bash` |
| `agent` | the loop and its ceiling |
| `workspace` | where the tools may look |

## Check it

```
cargo test      # 63 tests; the provider is a localhost stub, so no key is needed
cargo clippy --all-targets
```

## Not yet

No record and no replay (S3), so a run cannot be inspected afterwards or
reproduced. No compaction (S4), no permissions (S5), no subagents or eval rig
(S6), no MCP (S7). No retries anywhere.
