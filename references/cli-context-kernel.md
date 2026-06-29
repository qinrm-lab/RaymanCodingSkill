# Extracted Skill Rules

Source: `docs/CLI.md`

## Context Kernel

`rayman context status` prints the workspace-level context summary, including whether the cached Context Index or derived Context OS state is missing or stale. `rayman context status --check` exits non-zero when any Context Index record or Context OS state record is stale; `rayman gate status --check`, successful goal/session closure, and broad delivery evidence treat stale context as a hard blocker.

For non-trivial project understanding, follow [Project understanding](PROJECT_UNDERSTANDING.md): run context status, retrieve task-scoped context, reread current files from disk, and use impact/regression planning for touched paths.

`rayman context list` prints each aggregated context record.

`rayman context refresh` rebuilds `.RaymanCodingSkill/context/index.json` from current workspace files and rewrites the workspace-local Context OS snapshot. The index stores file hashes, project inputs, file inventory, symbols, manifests, entry points, verification commands, and default task evidence. It is a navigation layer only; implementation and review must reread current source files.

`rayman context os --write` derives `.RaymanCodingSkill/context/state.json` and appends `.RaymanCodingSkill/context/events.jsonl` from current Context Index status, goal/session state, pending work, audit findings, and asset retirement state. `rayman context os --check` fails when the snapshot is missing or its digest no longer matches those current workspace facts. This is the stronger Context OS / Content OS shape in RaymanCodingSkill: a local state graph and event log, not a daemon, database, vector index, or cross-project memory.

`rayman context task "..."` prints task-relevant indexed evidence without writing state. It narrows the model context to likely files and symbols, then points the agent back to the referenced source files for exact details. It never refreshes or mutates the Context Index; use `rayman context refresh` explicitly when freshness is required.

`rayman context explain` prints JSON explaining how RaymanCodingSkill uses the Context Kernel to build a workspace-local Context OS state graph without introducing a daemon, database, vector index, or cross-project memory.

`rayman goal` is the resumable autonomy loop for substantial customer goals. `rayman goal clarify "<request>" --format text|json` is read-only and produces a deterministic hidden-requirement clarification with recommended defaults, inferred requirements, acceptance criteria, validation suggestions, out-of-scope items, and customer-confirmation questions. `goal start` stores the same clarification under `contract.clarification` so intake can confirm the goal contract, must requirements, and default choices before implementation. Goal state is stored in `.RaymanCodingSkill/goals/*.json`; `goal run` advances the next active stage and attempts auxiliary AI during planning/summary when available. Success close requires impact evidence, validation evidence when verification commands exist, fresh context, clear asset retirement state, no pending/review/audit blockers, quality gate pass, release/deploy `customer_deploy` readiness when applicable, and explicit completion evidence for every must requirement by `req_id`. Each `req_id` evidence item must also include an existing current workspace path, a recorded successful validation command, or an existing evidence artifact; `req_1` alone is rejected. Non-success `goal close` writes a pending resume item so the next session can continue.

`rayman research` can be attached to a goal with `--goal-id`. Research output is advisory-only even when the scientist runs allowed experiments; unresolved research sessions with `conflict`, `blocked`, or `policy_violation` status are hard blockers for successful goal/session close.

`rayman session close --status success` fails when pending work, active goals, audit findings, asset blockers, unclean managed temp state, stale context, or review blockers remain. Use a non-success status with `-m/--message` when audit findings, remote/manual validation gaps, temporary asset cleanup, or other blockers remain; the command records pending work instead of treating the session as complete.

`rayman customer-deploy` stores customer release/deploy settings in the current workspace only, at `.RaymanCodingSkill/customer_deploy.yaml`. It records environment, build/test/deploy commands, artifacts, target alias, rollback command, notes, and credential references. Real secret values are rejected; use `--credential-env PROD_TOKEN` or `--credential-ref aliyun-prod` instead. Release/deploy goals automatically attach the sanitized customer deploy config to goal metadata, point `next_action` at missing required fields, and block `goal close --status success` until required deploy settings are ready.
