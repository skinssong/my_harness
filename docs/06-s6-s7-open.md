# S6 and S7 — deliberately open

**Status:** open. Not designed, and not scheduled.

[← design index](../DESIGN.md)

S1–S5 were designed one at a time, each forced by something the phase before could not do. These two are recorded as requirements only: their building blocks are not designed, because writing them down now would be fiction — I would be inventing shapes for problems I have not yet met.

For the same reason the cumulative system diagram stops at [S5](05-s5-permissions.md): a phase that has no blocks has nothing to draw green.

## S6 — compose

**Requirement.** Sub-tasks run without contaminating the main context, reusable procedures load on demand, and a task suite can be run and graded.

**Why.** Two failures arrive together. A long task's context fills with work that belonged to a sub-problem — S4 masks *bulk*, but it cannot know that an entire line of investigation is irrelevant. And until now "does it work" has meant "did this one run work", which is not a measurement.

**Roughly what it needs.** An isolation boundary that returns a conclusion rather than a transcript; procedure files the model discovers instead of carrying; and a rig that runs N tasks, grades them, and compares against a weaker configuration. The eval rig is described separately in the [design index](../DESIGN.md#the-eval-rig--a-parallel-track) because it is required by the deliverable rather than forced by the previous phase.

**Questions to settle first.**

- What is a *task*? A prompt, a starting directory, and a checkable outcome — but "checkable" is the whole difficulty. Code-based grading needs a fixture filesystem and a deterministic assertion; an LLM judge needs a rubric and a reason to trust it.
- What does a subagent return — text, a structured result, or a diff? Whatever it is, it is the only thing the parent sees, so it is the interface.
- Does a subagent get the same tools, a subset, or its own policy? S5's suspension inside a subagent is a decision, not an implementation detail.

## S7 — extend without rebuilding

**Requirement.** New tools arrive over a protocol rather than as compiled-in code.

**Why.** S2's tools are compiled in, so every capability is a rebuild and a release. The tool contract at S2 is already a name, a description, a schema and a call — which is exactly what a protocol can carry.

**Roughly what it needs.** A client for the protocol, and a projection from remote tools into the S2 contract: `tools/list` supplies name, description and `inputSchema`; `tools/call` supplies execution. The loop does not change, because it only ever asks "name, what does it accept, run it".

**Questions to settle first.**

- **Where is the boundary?** A remote tool runs with the *server's* privileges. S5's approval can decide whether to call it; nothing can bound what it does. That has to be said out loud in the policy, not discovered later.
- **Names.** Two servers can both offer `read_file`. Namespacing is cheap now and awkward to add after records exist.
- **Errors.** The protocol reports tool failure in the result rather than the transport, which maps onto the existing rule — but the mapping has to be written down and tested, and a server that dies mid-call is a transport failure, not an observation.
- **When to discover.** Once at startup, or per run? Tool lists change; a record has to say which list a run was given.
