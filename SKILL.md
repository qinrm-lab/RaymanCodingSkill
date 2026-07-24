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
