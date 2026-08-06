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

After an installed identity update, `rayman workspace rebind --yes` repairs only complete,
enabled `raymancodingskill` identity drift when current skill bytes match the canonical
`SKILL.md` embedded in the running CLI. It updates only the identity scalars under the
shared activation lock, fails closed otherwise, and never scans other workspaces. On explicit
Rayman use, rebind a non-read-only task and continue it; read-only work only
reports recovery. The intent-blind Stop Hook never writes the binding; it still
evaluates current goals and blocks unfinished Owner Mode work. The shared
workflow reference defines the exact boundary.

## Codex adapter

- This repository installer deploys the Codex adapter as a global skill.
- The installed Codex Stop hook may continue active foreground work or reject
  an unsupported completion claim. Verify the real hook configuration and
  restart/trust boundary instead of inferring installation from UI state.
- Use `rayman codex-hook status|install --yes|uninstall --yes|stop` only for
  the Codex host integration. Preserve unrelated handlers.
- Claude Code uses the repository `CLAUDE.md` entrypoint; this installer does
  not claim a global Claude deployment.
- Diagnose Codex Windows patch failures by signature: split-root enforcement
  requires the `elevated` sandbox; a `WindowsApps` shim requires the non-MSIX
  Codex install; `helper_unknown_error` is a host boundary. Stop retrying and
  use the managed-scratch `git apply` or whole-file fallback defined in the
  workflow reference.
- Request escalated permissions upfront for rayman state writes, git
  stage/commit, repository gates, and installer runs; over-long command
  lines fail to spawn under the sandbox wrapper, so split them. Details:
  the sandbox and permission boundaries section of the workflow reference.
- Keep OS identity separate from elevation transport. Inspect the execution
  principal/profile probe before retrying a broker. A principal fingerprint
  proves only the SID, not ACL capability; require fresh evidence for the
  boundary that matters (SID, profile, or an action-specific permission probe)
  instead of treating elevated, COM, Terminal, or Task Scheduler as evidence.

## Working flow

1. Confirm activation, perform an eligible explicit-use rebind when the task is
   not read-only, then start a goal and run `prepare --goal <id>`. Every later
   prepare reconciles the live goal-baseline delta with the effective plan; it
   never auto-extends a plan after the source changed.
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
must be complete, capability-keyed solution packages. An askable `ready`
frontier only authorizes asking: run `goal pending render --current`, emit that
exact workspace aggregate as the complete final message, and let only the
current Codex Stop event compare its `last_assistant_message`. No persisted
receipt proves delivery, visibility, reading, or user awareness. Commit, push,
install, publish, deploy, destructive deletion, and account changes still require user authority.

## Detailed contract

The shared command, evidence, lifecycle, checkpoint, and release rules live in
[references/workflow-contract.md](references/workflow-contract.md).

## Degradation and boundary

Prefer `rayman` on PATH, then a built release binary, then `cargo run -p rayman --`; if unavailable, work manually and report that Rayman gates were not verified. This skill never manages host accounts, OAuth, IDE login, or connectors.
