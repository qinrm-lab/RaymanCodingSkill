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
canonical skill plus its delegated agent/workflow contract bundle. A leftover
state directory without that complete activation contract is orphan state and
does not authorize automatic use. A legacy six-field binding is inactive but
eligible for `workspace rebind --yes` only when the current bundle matches.

For explicit non-read-only use, apply the eligible rebind and continuation
rules in the shared workflow reference. Activation writes remain fail closed;
this adapter does not redefine their identity or permission contract.

## Installed-version currency

Polling, install consent, worker execution, restart authority, and identity-only
rebind are governed by the shared workflow reference. This adapter adds only
the Codex host boundary: after a successful worker reports
`restart_required=true`, stop and request a Codex restart because the loaded
adapter is still the old one. On the next invocation follow the shared
`workspace ensure-current` contract; do not infer that a restart or UI version
display migrated project automation.

## Codex adapter

- This repository installer deploys the Codex adapter as a global skill.
- The installed Codex Stop hook may continue active foreground work or reject
  an unsupported completion claim. Verify the real hook configuration and
  restart/trust boundary instead of inferring installation from UI state.
- Use `rayman codex-hook status|install --yes|uninstall --yes|stop` only for
  the Codex host integration. Preserve unrelated handlers.
- Claude Code uses the repository `CLAUDE.md` entrypoint; this installer does
  not claim a global Claude deployment, and Claude Code must not execute or
  emulate the Codex Stop hook.
- A human boundary is rendered with `rayman goal pending render --current`.
  Only the current Codex Stop event may perform the native, event-local output
  comparison described by the shared workflow; it creates no durable delivery
  or awareness receipt.
- Diagnose Codex Windows patch failures by signature: split-root enforcement
  requires the `elevated` sandbox; a `WindowsApps` shim requires the non-MSIX
  Codex install; `helper_unknown_error` is a host boundary. Stop retrying and
  use the managed-scratch `git apply` or whole-file fallback defined in the
  workflow reference.
- Keep OS identity separate from elevation transport. Inspect the execution
  principal/profile probe before retrying a broker. A principal fingerprint
  proves only the SID, not ACL capability. The shared workflow defines the
  required action-specific evidence.

## Shared workflow

For a repository enrolled in Rayman Continuity, apply the shared contract's
"Enrolled session-continuity owner" boundary. Recover user intent and cross-session
progress from the plugin's repository-local originals. Use Rayman context/map as
navigation and its Goals as required technical validation scopes. Preserve the
plugin task ID in the scope description; never claim a Rayman Goal alone proves
equivalence to the user's original request. Do not activate or invoke a second
copy of the coordinating plugin skill, and do not copy its state into a competing
session-recovery ledger.

Goal lifecycle, planning, validation, blocker/frontier handling, checkpoints,
concurrency, permissions, final authority, release transfer, and degradation
are governed exclusively by
[references/workflow-contract.md](references/workflow-contract.md). Follow it
without copying its shared policy back into this adapter.

## Degradation and boundary

This skill never manages host accounts, OAuth, IDE login, or connectors.
