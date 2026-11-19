# S7 — extend without rebuilding

**Status:** open. Not designed, and not scheduled.

[← design index](../DESIGN.md)

S1–S6 were designed one at a time, each forced by something the phase before it could not do. S7 is recorded as a requirement only: its building blocks are not designed, because writing them down now would be fiction — I would be inventing shapes for problems I have not yet met.

For that reason the cumulative system diagram stops at [S6](06-s6-compose.md): a phase with no blocks has nothing to draw green.

**Requirement.** New tools arrive over a protocol rather than as compiled-in code.

**Why.** S2's tools are compiled in, so every capability is a rebuild and a release. The tool contract at S2 is already a name, a description, a schema and a call — which is exactly what a protocol can carry.

**Roughly what it needs.** A client for the protocol, and a projection from remote tools into the S2 contract: `tools/list` supplies name, description and `inputSchema`; `tools/call` supplies execution. The loop does not change, because it only ever asks "name, what does it accept, run it".

**Questions to settle first.**

- **Where is the boundary?** A remote tool runs with the *server's* privileges. S5's approval can decide whether to call it; nothing can bound what it does. That has to be said out loud in the policy, not discovered later.
- **Names.** Two servers can both offer `read_file`. Namespacing is cheap now and awkward to add after records exist.
- **Errors.** The protocol reports tool failure in the result rather than the transport, which maps onto the existing rule — but the mapping has to be written down and tested, and a server that dies mid-call is a transport failure, not an observation.
- **When to discover.** Once at startup, or per run? Tool lists change; a record has to say which list a run was given.

*Building blocks not designed yet.*
