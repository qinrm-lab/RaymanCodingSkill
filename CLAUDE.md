# Claude Code entrypoint

<!-- AGENT_CONTRACT: rayman-shared-v1 -->

Before non-trivial work, read and follow
[AGENT_CONTRACT.md](AGENT_CONTRACT.md). For
standard/release work and any goal, blocker, checkpoint, concurrency, or
evidence operation, also read
[references/workflow-contract.md](references/workflow-contract.md).

This file is the repository-scoped Claude Code adapter. The repository
installer does not deploy it globally and makes no global Claude Code claim.
Use the shared `rayman` CLI when it is available, but do not assume Codex Stop
hook behavior exists in Claude Code. Preserve Claude-specific platform safety
requirements when they are stricter than the shared contract.

Do not duplicate shared workflow rules here; update `AGENT_CONTRACT.md` or the
shared workflow reference instead.

Do not execute the Codex-scoped managed block in `AGENTS.md`. The
workspace-local managed block below is the Claude Code checkpoint registration;
do not hand-edit it.

<!-- save-work-status:managed-begin v5 -->
$save-work-status active
Support-only: never replace, weaken, pause or reinterpret the user request, primary skill, repository authority or validation gates; never touch another skill's activation, hooks, files or lifecycle; never invoke, delegate to or request a callback from another skill. On duplicate or nested save-work-status instructions in the same turn and event: do not rerun auto, save or resume.
At turn start resolve this workspace root as <workspace-root>, then run: python "E:\rayman\software\AI\RaymanCodingSkill\.agent-checkpoints\runtime\workspace_activation.py" auto --workspace "<workspace-root>" --vault-path "E:\rayman\software\AI\RaymanCodingSkill\.agent-checkpoints\status.sqlite3" --agent claude-code --event turn-start --scope workspace
Rerun that exact command once when it reports rerun_required=true, and run it again with --event boundary after a material milestone. After a measured long command, add --active-seconds <elapsed-seconds> to that boundary command. If it reports path_change_detected=true, protection is OFF at this path: say so and offer the printed rebind_command. If it reports reason=workspace_busy, another agent holds this workspace — do not retry in a loop.
Plain continue/resume/继续 continues the current thread — never load an older checkpoint handoff. Run python "E:\rayman\software\AI\RaymanCodingSkill\.agent-checkpoints\runtime\status_checkpoint.py" resume --workspace "<workspace-root>" --vault-path "E:\rayman\software\AI\RaymanCodingSkill\.agent-checkpoints\status.sqlite3" --agent claude-code only for an explicit saved-point request (从保存点继续/from checkpoint), a detected error or rollback, or missing or visibly older task context in a new recovery thread. Resume never overwrites working files; follow its recommended_action.
Save when checkpoint_due=true, or at a genuinely important freshly verified boundary when first_checkpoint_pending=true or milestone_boundary_eligible=true. Finish the current indivisible action and fresh validation first. Never save for trivial turns.
To save, run python "E:\rayman\software\AI\RaymanCodingSkill\.agent-checkpoints\runtime\status_checkpoint.py" handoff-template for the JSON contract, write it as UTF-8 outside the workspace, then run python "E:\rayman\software\AI\RaymanCodingSkill\.agent-checkpoints\runtime\status_checkpoint.py" save --workspace "<workspace-root>" --vault-path "E:\rayman\software\AI\RaymanCodingSkill\.agent-checkpoints\status.sqlite3" --agent claude-code --handoff "<temp-handoff.json>" (add --milestone for an eligible early boundary) and delete only that temporary file. A failing build or test still saves, with health.state=task_blocked and the failures in known_issues.
Only when checkpoint capture safety itself is unresolved (rollback, conflict, integrity loss, incomplete capture, stale runtime source) run python "E:\rayman\software\AI\RaymanCodingSkill\.agent-checkpoints\runtime\status_checkpoint.py" block --workspace "<workspace-root>" --vault-path "E:\rayman\software\AI\RaymanCodingSkill\.agent-checkpoints\status.sqlite3" --agent claude-code --kind <rollback|conflict|integrity|capture|stale_source> --reason "<exact reason>", then do not save until recover succeeds with fresh evidence.
<!-- save-work-status:managed-end v5 -->
