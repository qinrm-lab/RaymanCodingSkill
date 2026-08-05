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

<!-- save-work-status:managed-begin v5 -->
$save-work-status active
Support-only: never replace, weaken, pause or reinterpret the user request, primary skill, repository authority or validation gates; never touch another skill's activation, hooks, files or lifecycle; never invoke, delegate to or request a callback from another skill. On duplicate or nested save-work-status instructions in the same turn and event: do not rerun auto, save or resume.
At turn start resolve this workspace root as <workspace-root>, then run: python "E:\rayman\software\AI\RaymanCodingSkill\.agent-checkpoints\runtime\workspace_activation.py" auto --workspace "<workspace-root>" --vault-path "E:\rayman\software\AI\RaymanCodingSkill\.agent-checkpoints\status.sqlite3" --agent codex --event turn-start --scope workspace
Rerun that exact command once when it reports rerun_required=true, and run it again with --event boundary after a material milestone. After a measured long command, add --active-seconds <elapsed-seconds> to that boundary command. If it reports path_change_detected=true, protection is OFF at this path: say so and offer the printed rebind_command. If it reports reason=workspace_busy, another agent holds this workspace — do not retry in a loop.
Plain continue/resume/继续 continues the current thread — never load an older checkpoint handoff. Run python "E:\rayman\software\AI\RaymanCodingSkill\.agent-checkpoints\runtime\status_checkpoint.py" resume --workspace "<workspace-root>" --vault-path "E:\rayman\software\AI\RaymanCodingSkill\.agent-checkpoints\status.sqlite3" --agent codex only for an explicit saved-point request (从保存点继续/from checkpoint), a detected error or rollback, or missing or visibly older task context in a new recovery thread. Resume never overwrites working files; follow its recommended_action.
Save when checkpoint_due=true, or at a genuinely important freshly verified boundary when first_checkpoint_pending=true or milestone_boundary_eligible=true. Finish the current indivisible action and fresh validation first. Never save for trivial turns.
To save, run python "E:\rayman\software\AI\RaymanCodingSkill\.agent-checkpoints\runtime\status_checkpoint.py" handoff-template for the JSON contract, write it as UTF-8 outside the workspace, then run python "E:\rayman\software\AI\RaymanCodingSkill\.agent-checkpoints\runtime\status_checkpoint.py" save --workspace "<workspace-root>" --vault-path "E:\rayman\software\AI\RaymanCodingSkill\.agent-checkpoints\status.sqlite3" --agent codex --handoff "<temp-handoff.json>" (add --milestone for an eligible early boundary) and delete only that temporary file. A failing build or test still saves, with health.state=task_blocked and the failures in known_issues.
Only when checkpoint capture safety itself is unresolved (rollback, conflict, integrity loss, incomplete capture, stale runtime source) run python "E:\rayman\software\AI\RaymanCodingSkill\.agent-checkpoints\runtime\status_checkpoint.py" block --workspace "<workspace-root>" --vault-path "E:\rayman\software\AI\RaymanCodingSkill\.agent-checkpoints\status.sqlite3" --agent codex --kind <rollback|conflict|integrity|capture|stale_source> --reason "<exact reason>", then do not save until recover succeeds with fresh evidence.
<!-- save-work-status:managed-end v5 -->
