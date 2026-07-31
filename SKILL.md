---
name: raymancodingskill
description: Lean, owner-minded coding-workflow helper for a workspace with a valid workspace_skill.yaml activation contract, or when the user explicitly invokes rayman in the current turn. Drives safe local work to a stable, evidence-bound finish; provides workspace activation, context indexing, goal planning and validation, structured blocker ownership, read-only audits, and managed temp. Do not use for unrelated repos, host-app accounts, IDE assistant login, OAuth, or connectors.
---

# RaymanCodingSkill

<!-- AGENT_CONTRACT: rayman-shared-v1 -->

## Required reading

Read [AGENTS.md](AGENTS.md) before non-trivial work. It is the mandatory
client-neutral contract. For standard/release work, goal lifecycle, blocker
handling, checkpoints, concurrency, or evidence claims, also read
[references/workflow-contract.md](references/workflow-contract.md).

## Activation

Use this skill when the user explicitly invokes Rayman or when
`.RaymanCodingSkill/workspace_skill.yaml` is valid and hash-bound to this
canonical file. A leftover state directory without that activation contract
is orphan state and does not authorize automatic use.

## Codex adapter

- This repository installer deploys the Codex adapter as a global skill.
- The installed Codex Stop hook may continue active foreground work or reject
  an unsupported completion claim. Verify the real hook configuration and
  restart/trust boundary instead of inferring installation from UI state.
- Use `rayman codex-hook status|install --yes|uninstall --yes|stop` only for
  the Codex host integration. Preserve unrelated handlers.
- Claude Code uses the repository `CLAUDE.md` entrypoint; this installer does
  not claim a global Claude deployment.
- Under the Codex Windows restricted-token sandbox, the built-in
  `apply_patch` tool has two distinct failure modes with distinct fixes.
  `cannot enforce split writable root sets` (or split read / deny-read
  restrictions) is a host configuration defect, not a patch defect: the
  `unelevated` sandbox cannot express the managed profile's read-only
  `.git`/`.agents`/`.codex` carve-outs. Report it and ask for Codex
  `[windows] sandbox = "elevated"`, which supports all three. `Access is
  denied.` from `apply_patch.bat` is a different defect with a different fix:
  Codex writes that shim as a direct absolute-path call to its own
  executable, and an MSIX/Store install lives under
  `C:\Program Files\WindowsApps\`, which Windows refuses to launch by path at
  all — not an ACL to repair, and it fails the same way for an ordinary
  interactive user. Read the newest
  `~/.codex/tmp/arg0/*/apply_patch.bat` to see which build the session is
  bound to; a `WindowsApps` target fails every call, a
  `%LOCALAPPDATA%\OpenAI\Codex\bin\...` target works. The fix is to run
  Codex from the non-MSIX install. Escalation fixes neither mode. Until the
  host is fixed, patch via `git apply` and stop retrying the tool.
- Request escalated permissions upfront for rayman state writes, git
  stage/commit, repository gates, and installer runs; over-long command
  lines fail to spawn under the sandbox wrapper, so split them. Details:
  the sandbox and permission boundaries section of the workflow reference.

## Working flow

1. Confirm activation, start a goal, and run `prepare --goal <id>`.
2. Persist a pre-mutation plan for multi-file work and extend it before
   touching newly discovered paths.
3. Implement conservatively, run focused project checks, and record staged
   progress without treating it as authority.
4. Use atomic `--must-proof KIND::TEXT` requirements and matching
   `goal validate` receipts for delivery claims.
5. Record a stable final gate with `--authority --repeat 2`, review a
   high-priority final fingerprint, close packages and goal, then run
   `finish --goal <id>`.
6. For release transfer, create a clean-HEAD `goal handoff start` contract and
   complete its installation, audit, and source-fresh stages.

Keep working while safe agent-owned work remains. Human/external consultations
must be complete solution packages. Commit, push, install, publish, deploy,
destructive deletion, and account changes still require user authority.

## Detailed contract

The shared command, evidence, lifecycle, checkpoint, and release rules live in
[references/workflow-contract.md](references/workflow-contract.md).

## Degradation and boundary

Prefer `rayman` on PATH, then a built release binary, then
`cargo run -p rayman --`. If unavailable, work manually and state that Rayman
gates were not verified. This skill covers programming workflows only; it does
not manage host accounts, OAuth, IDE login, or connectors.
