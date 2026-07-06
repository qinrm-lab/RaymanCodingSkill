# Extracted Skill Rules

Source: `docs/CLI.md`

## Model And Config

```text
rayman list-models
rayman check-models
rayman update-models --force
rayman route-models --task code_review
rayman route-models --task code_generation --route-mode auto
rayman models refresh --dry-run
rayman models refresh --apply
rayman models status --check
rayman auxiliary advise --task planning -m "Summarize implementation risks for this task"
rayman auxiliary advise --task workflow_summary -m "Check final changes and validation evidence"
rayman auxiliary target
rayman auxiliary status
rayman auxiliary reconcile
rayman research start "Why did validation fail?" --goal-id <id>
rayman research run --id <id>
rayman research run --id <id> --model-type openai --model-name gpt-4o --route-mode auto
rayman research status --id <id>
rayman research reconcile --id <id>
rayman research report --id <id>
rayman subagent plan --task "审计 subagent 性能提速策略" --path crates/rayman-core/src/subagent.rs --max-lanes 4
rayman subagent auto-start --task "审计 subagent 性能提速策略" --path crates/rayman-core/src/subagent.rs --max-lanes 4
rayman subagent dispatch --task "审计 subagent 性能提速策略" --path crates/rayman-core/src/subagent.rs --max-lanes 4
rayman subagent dispatch --task "implement isolated lane" --path crates/rayman-core --create-worktree
rayman subagent reconcile
rayman subagent record --agent-id <id> --task "review API routes" --boundary "read-only route review" --read-only
rayman subagent record --agent-id <id> --task "edit docs" --boundary "docs only" --write-path docs
rayman subagent result --id <record-id> --status completed -m "changed docs" --changed-path docs/CLI.md --evidence "cargo test -p rayman-cli"
rayman subagent review --id <record-id> --verdict accepted -m "primary reviewed" --overlap-resolution "no overlap"
rayman subagent status
rayman quality incident add --source codex://threads/example --symptom "empty response" --root-cause "tool loop stopped"
rayman quality patterns
rayman quality gate --goal-id <id>
rayman stats
rayman config show
rayman config get default_model.type
rayman config set default_model.type openai
```

When overriding a model, provide both `--model-type` and `--model-name`; partial overrides are rejected to avoid invalid provider/model pairs.

`rayman models refresh --dry-run` and `rayman models refresh --apply` build the local model catalog snapshot at `.RaymanCodingSkill/models/catalog_cache.json` with hashes for route config files. `rayman models status --check` blocks when automatic/default routes point to unknown or deprecated catalog entries, when the local cache is missing, malformed, or stale after route-config changes, and `rayman gate status --check` includes the same governance check. The catalog cache is governance metadata; current files, validation output, and provider documentation remain authoritative for route changes.

`rayman auxiliary advise` is the lightweight bridge for customer projects where Codex or Claude Code performs the edits directly. It calls the configured auxiliary AI synchronously, records usage stats in the current workspace, and prints the advisory without asking the primary model to generate code. Auxiliary output is advisory-only: it can strengthen the primary AI context, but it cannot execute, edit, approve, replace validation, or overrule the primary AI. Opted-in customer workspaces without local `config/default_config.yaml` use the canonical RaymanCodingSkill config from `.RaymanCodingSkill/workspace_skill.yaml`, while stats remain workspace-local.

`rayman auxiliary target` prints provider order, the next round-robin provider, timeout, proxy mode, destination, trust level, and workspace-data policy without making a model request. Non-loopback auxiliary targets require explicit `allow_workspace_data: true`; otherwise advisory calls are skipped with `skipped_external_auxiliary_not_authorized`.

`rayman auxiliary status` prints queued/running/succeeded/failed/reconciled/conflict task state from `.RaymanCodingSkill/auxiliary/tasks/`. `rayman auxiliary reconcile` processes completed worker conclusions, creates pending work for conflicts, and leaves successful conclusions as `reconciled_ok`. The hidden `rayman auxiliary worker --task-id <id>` entry is reserved for background worker processes.

`rayman quality incident add` records a repeated customer-reported failure in `.RaymanCodingSkill/quality/incidents/` and aggregates it into `.RaymanCodingSkill/quality/patterns.json`. `rayman quality patterns` prints built-in and workspace patterns. `rayman quality gate --goal-id <id>` checks matched quality patterns and hard-blocks missing regression evidence.

`rayman stats` prints persisted auxiliary AI usage totals as `auxiliary success / auxiliary calls / main AI calls`, with the percentage calculated as auxiliary success divided by real auxiliary calls (`call_count`). It presents those totals as auxiliary AI usage value, then prints implementation-validation correction contribution totals, the newest contribution-evidence events, per-task and per-provider usage summaries, provider-attempt counts, average duration, failure-kind counts, skip-reason counts, optional estimated cost, and quality incident/pattern hit counts. Usage keeps `attempt_count` as the backward-compatible count of recorded auxiliary steps, while `call_count` counts successful or failed auxiliary executions, `queued_count` counts async enqueue records, skipped records remain separate, and main AI calls count successful primary model completions. Planning, workflow-summary, and research tasks are advisory/analysis value and remain visible in usage stats; they are not counted as implementation correction contribution. A contribution is counted only when auxiliary AI participated in implementation validation and the final code was actually corrected; each retained event records whether it counted, the reason, and bounded evidence strings such as validator changes. If no implementation-validation correction round has occurred, CLI output reports that there is no implementation-validation correction sample instead of showing a bare `0/0`.

Successful CLI commands print the project-total auxiliary contribution ratio at the end of each interaction. Commands that already print detailed auxiliary AI status reuse that detailed output instead of adding a duplicate footer.

When auxiliary AI is enabled, configured, and available for a non-trivial programming task, CLI workflows must attempt it. A failed auxiliary attempt is fail-open and may downgrade to the primary route after printing status and reason. If auxiliary AI was available but not attempted, CLI output or the generated report must include a skip reason and evidence.

`rayman research` is the multi-agent research ledger and autonomous scientist experiment loop. A research session stores hypotheses, role findings, whitelist experiment commands, command output tails, reflection rounds, and conflicts under `.RaymanCodingSkill/research/sessions/`. `rayman research run` executes `planner`, `scientist`, `critic`, `reflector`, `arbiter`, and `safety_monitor` role findings in parallel up to `research_agents.max_parallel_agents`, then persists findings in fixed role order. Research roles use the primary model route for advisory output; `--model-type`, `--model-name`, `--route-mode`, and `--no-fallback` apply to all roles in the run. The scientist agent may request and run only argv-form commands allowed by `config/research_agents.yaml`; it cannot edit files, approve validation, close goals, or replace primary verification. Experiments run inside the workspace, reject shell operators, clamp requested timeouts to the runtime cap, and diff-check protected source/docs/config files before and after execution. Default Scientist experiments exclude full-suite validation such as `cargo test --all`; those commands stay on the primary validation path. `require_workspace_cwd` must stay enabled; execution and security audit reject attempts to disable the cwd boundary. Any research conflict or policy violation blocks `goal close --status success` and `session close --status success` until reconciled or recorded as blocked work.

`rayman subagent` is the Codex host subagent planner and ledger for host-managed parallel agents. `rayman subagent plan` converts a task plus optional touched paths into recommended explorer/worker/validation lanes, skip rationale, main-agent duties, an auto-start contract, host `spawn_agent` request payloads, and `rayman subagent record` command templates. `rayman subagent auto-start` is the explicit agent-facing entry for the same host-tool-ready output. `rayman subagent dispatch` writes a durable dispatch record under `.RaymanCodingSkill/subagents/dispatches/` for host request, local worktree lane, and non-overlapping scope coordination. `--create-worktree` creates a real detached git worktree under `.RaymanCodingSkill/worktrees/<dispatch-id>` for writable lanes; without it, dispatch records `declared_not_created`. `rayman subagent reconcile` blocks unclosed dispatches, missing reviews, conflicts, out-of-scope writes, overlapping scope, and failed/unavailable requested worktrees. Its auto-start contract reports `authorization_mode=standing_workspace_authorization`, `per_use_prompt_required=false`, and `explicit_subagent_phrase_required=false`, so in enabled workspaces the standing authorization has the same effect as an explicit `开启subagent` phrase. It treats host subagents as main-model or strong-model child agents for speed, not auxiliary AI. `rayman subagent record` creates a bounded host-subagent ledger entry and requires either `--read-only` or at least one `--write-path`; `rayman host-subagent` is an alias for the same command group. `rayman subagent result` records completion, failure, or conflict with result evidence and changed paths, and rejects changed paths outside the declared write scope. `rayman subagent review` records the primary agent's disposition and optional `--overlap-resolution`. `rayman subagent status` reports Codex host subagent ledger blockers, including parse-error blockers for damaged ledger JSON. Unreviewed, unresolved, conflict, parse-error, open dispatch, requested-worktree failure, or overlapping subagent ledger entries block `goal close --status success`, `session close --status success`, `rayman audit`, and `rayman gate status --check`.
