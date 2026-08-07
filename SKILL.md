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

For explicit non-read-only use, apply the eligible rebind and continuation
rules in the shared workflow reference. Activation writes remain fail closed;
this adapter does not redefine their identity or permission contract.

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

Goal lifecycle, planning, validation, blocker/frontier handling, checkpoints,
concurrency, permissions, final authority, release transfer, and degradation
are governed exclusively by
[references/workflow-contract.md](references/workflow-contract.md). Follow it
without copying its shared policy back into this adapter.

## Degradation and boundary

This skill never manages host accounts, OAuth, IDE login, or connectors.
