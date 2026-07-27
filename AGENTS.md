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
- Do not permit direct or indirect skill-invocation cycles. When one skill
  routes work to another, refuse to invoke any skill already active in that
  invocation chain; end the handoff and return control to the original task
  context. Supporting skills must not call back into their caller to satisfy
  their own trigger or lifecycle.

## Shared workflow authority

- For standard/release work, goal lifecycle, blocker handling, checkpoints,
  concurrency, or evidence claims, read and follow
  [references/workflow-contract.md](references/workflow-contract.md).
- Continue safe agent-owned work to a stable finish. A human/external pause
  must be a complete solution package, not an open-ended question.
- Treat unbound `rayman check` as workspace health only. Task completion
  requires a goal-bound gate; release completion additionally requires
  installation, repository audit, and source-fresh proof.
- Use `--must-proof KIND::TEXT` for atomic mandatory evidence and split
  compound claims. A typed requirement accepts only a matching validation
  command.
- Start release transfer with `goal handoff start --from-goal <id> --commit
  <sha>`. Do not hand off an uncommitted, dirty, authority-less, or
  source-stale implementation.
- Checkpoint save is lossless by default. Pruning requires explicit user
  authorization or an already confirmed retention policy.
- Sandbox and permission boundary handling (ACL denials, probe-first,
  upfront escalation) is defined in the shared workflow reference.

## Compatibility boundary

`SKILL.md` and `CLAUDE.md` are adapters, not independent policy copies. Any
change to this shared contract must keep their `AGENT_CONTRACT:
rayman-shared-v1` marker, preserve the workflow reference link, and pass
`scripts/check-agent-instructions.ps1`.
