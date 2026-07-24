# Claude Code entrypoint

<!-- AGENT_CONTRACT: rayman-shared-v1 -->

Before making a non-trivial change, read and follow [AGENTS.md](AGENTS.md).
It is the shared contract for Codex and Claude Code in this repository.

Claude Code may use the same installed `rayman` CLI as Codex. Treat the CLI
as a deterministic local tool: inspect the workspace before substantive work,
run the applicable checks afterwards, and do not claim that a build is an
installation or release.

This file adds only the Claude Code entrypoint. Do not duplicate shared rules
here; update `AGENTS.md` instead.
