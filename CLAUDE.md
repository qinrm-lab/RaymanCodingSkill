# Claude Code entrypoint

<!-- AGENT_CONTRACT: rayman-shared-v1 -->

Before non-trivial work, read and follow [AGENTS.md](AGENTS.md). For
standard/release work and any goal, blocker, checkpoint, concurrency, or
evidence operation, also read
[references/workflow-contract.md](references/workflow-contract.md).

This file is the repository-scoped Claude Code adapter. The repository
installer does not deploy it globally and makes no global Claude Code claim.
Use the shared `rayman` CLI when it is available, but do not assume Codex Stop
hook behavior exists in Claude Code. Preserve Claude-specific platform safety
requirements when they are stricter than the shared contract.

Do not duplicate shared workflow rules here; update `AGENTS.md` or the shared
workflow reference instead.
