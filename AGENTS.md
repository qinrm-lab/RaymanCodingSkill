# Shared agent contract

<!-- AGENT_CONTRACT: rayman-shared-v1 -->

This file is the single source of truth for rules that apply to every AI
coding client in this repository. Native client entry files may add only
client-specific integration details; they must not duplicate or weaken this
contract.

## Read order and precedence

1. Follow the user's request and the host platform's mandatory instructions.
2. Read this file before making a non-trivial change.
3. Then follow the native entry file for the active client: `SKILL.md` for
   Codex and `CLAUDE.md` for Claude Code.
4. When a native entry conflicts with this file, stop and ask for direction
   unless the native entry is enforcing a stricter platform safety contract.

## Shared working rules

- Keep text, paths, and command output in UTF-8. Do not replace Chinese or
  other Unicode characters with lossy placeholders.
- Treat `rayman` as the shared deterministic workflow CLI. Before a
  non-trivial source change, inspect the workspace; after a change, run the
  relevant project checks and report their actual result.
- A successful build only creates an artifact. Do not describe it as an
  installed or released CLI unless the supported installer and identity checks
  have completed.
- Do not install, publish, push, commit, delete, or overwrite user-managed
  state without the user's explicit authorization.
- Preserve unrelated work in a dirty checkout. Do not reset or discard it.

## Compatibility boundary

`SKILL.md` and `CLAUDE.md` are adapters, not independent policy copies. Any
change to this shared contract must keep their `AGENT_CONTRACT:
rayman-shared-v1` marker and pass `scripts/check-agent-instructions.ps1`.
