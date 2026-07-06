# API

`rayman-api` provides the HTTP service with `axum`. Start it through the CLI:

```text
rayman api serve --host 127.0.0.1 --port 8000
```

## Public Endpoints

- `GET /`: service name, version, and status.
- `GET /health`: health check.
- `POST /mcp`: foreground MCP JSON-RPC endpoint when started with `rayman mcp serve --http`; accepts `initialize`, `tools/list`, `tools/call`, `resources/list`, and `resources/read`.

## Protected Endpoints

Protected endpoints require `RAYMAN_API_KEY` or `RAYMAN_API_TOKEN`. Clients may send either `X-API-Key` or `Authorization: Bearer <token>`.

CORS defaults to `http://127.0.0.1:8000` and `http://localhost:8000`. Override it with comma-separated `RAYMAN_API_CORS_ORIGINS`.

- `POST /api/generate`
- `POST /api/review`
- `POST /api/test`
- `GET /api/models`
- `GET /api/models/status`
- `POST /api/models/update`
- `GET /api/mcp/tools`
- `GET /api/mcp/resources`
- `GET /api/plugin/manifest`
- `GET /api/context`
- `GET /api/context/os`
- `POST /api/context/os`
- `GET /api/project`
- `POST /api/project/index`
- `POST /api/impact`
- `POST /api/regression/plan`
- `GET /api/assets`
- `POST /api/assets/scan`
- `POST /api/assets/cleanup`
- `POST /api/assets/retire`
- `POST /api/assets/exempt`
- `GET /api/evidence`
- `GET /api/stats`
- `POST /api/goals/clarify`
- `POST /api/goals`
- `GET /api/goals/{id}`
- `POST /api/goals/{id}/run`
- `POST /api/goals/{id}/close`
- `POST /api/research`
- `GET /api/research/{id}`
- `POST /api/research/{id}/run`
- `POST /api/research/{id}/reconcile`

## Request Fields

Generation accepts `prompt`, `language`, `model_type`, and `model_name`.

Review accepts `code`, `language`, `model_type`, `model_name`, `workspace_path`, and `reviewed_path`.

Test generation accepts `code`, `language`, `test_types`, `model_type`, and `model_name`.

Goal clarification accepts `goal`, optional `requirements`, `acceptance`, `verification`, and `assumptions`, and returns deterministic recommended defaults for hidden customer requirements without writing goal state. Goal start accepts `goal`, optional `workflow`, `requirements`, `acceptance`, `verification`, and `assumptions`; the created record includes `contract.clarification`. Goal run accepts optional `validation` (`passed` or `failed`) and `message`; failed validation moves the goal into `repair`. Without `validation`, goal run also accepts optional `until` (`next_step`, `blocked`, `summary`, or `complete`), `checkpoint_interval_minutes`, `max_repair_attempts`, and `resume`; the response keeps the goal record fields at top level and appends `long_run_report`. Goal close accepts `status` (`success`, `blocked`, `failed`, or `partial`), optional `message`, and `next_steps`; non-success close records pending recovery metadata with `blocker_kind`, `minimum_input`, `evidence_path`, `resume_command`, and `auto_resume_strategy`.

Research start accepts `question` and optional `goal_id`. Research run accepts an empty JSON object or optional `model_type`, `model_name`, `route_mode` (`manual` or `auto`), and `no_fallback`, then advances one multi-agent research round. When model override fields are provided, all parallel research roles use the same primary model route and persist route attempts per finding; explicit route configuration failures are returned as errors instead of silently downgrading to local synthesis. Research reconcile acknowledges non-critical research conflicts; critical policy violations remain blockers.

Impact and regression requests accept `paths`, an array of workspace-relative file paths.

Asset retirement requests use workspace-relative paths. Scan recomputes recorded obsolete-asset references and writes the workspace-local state back to `.RaymanCodingSkill/assets/retirement.json`; generated Context OS/Index files under `.RaymanCodingSkill/context` are state inputs, not current-behavior references. Cleanup accepts `apply`; with `apply=false` it returns a plan, and with `apply=true` it deletes only registered retirement-candidate files that still resolve inside the workspace and have no current references. Directory cleanup is blocked until a per-file manifest exists; cleanup never recursively deletes a directory. Retire accepts `path`, `replacement`, `reason`, `validation_command`, and optional `apply_delete`; deletion is limited to whole files inside the workspace after reference checks pass. Exempt accepts `path`, `reason`, and `expires_at` (`YYYY-MM-DD`).

Evidence requests accept optional query parameters `scope` (`workspace`, `goal`, `session`, or `research`), `goal_id`, and `include_advisory`. `GET /api/evidence` returns the same evidence report shape as `rayman evidence check --format json`.

`model_type` and `model_name` must be provided together or omitted together. Review requests return `409 Conflict` when pending work or unfinished code markers block the review gate.

Model update status reads `config/model_updates.yaml`. `POST /api/models/update` checks metadata only when automatic updates are enabled or `force=true`; it does not mutate the model catalog without a configured updater.

MCP and plugin metadata endpoints expose integration descriptors. `POST /mcp` is the foreground MCP HTTP JSON-RPC endpoint used by `rayman mcp serve --http`; it is read-only and binds to loopback by default. `GET /api/mcp/tools` returns Rayman tool descriptors for goal, context, gate, evidence, risk, subagent, model, and control-plane status. `GET /api/mcp/resources` returns JSON resource descriptors such as `rayman://context`, `rayman://gate`, `rayman://evidence`, `rayman://subagents`, and `rayman://models`. `GET /api/plugin/manifest` returns the same plugin manifest shape as `rayman plugin export`. These endpoints do not create a daemon, database, or external authority; current files, validation commands, and Rayman evidence remain authoritative.

Context reads workspace activation, pending work, project input files, backup indexes, review blockers, audit findings, asset retirement state, the workspace-local Context Index, and the workspace-local Context OS state graph from local files. It does not create a daemon, database, vector index, or cross-project memory.

The Context Index records project inputs, file inventory, symbols, manifests, entry points, verification commands, asset retirement state, and task-relevant evidence with file hashes. Cached summaries are navigation only: when hashes differ, context reports the index as stale and callers must reread current source files before using the indexed evidence for implementation or review. Recorded obsolete, retired, or compatibility-exempt assets are excluded from current-behavior task evidence and appear only in `asset_retirement`.

`GET /api/context/os` returns the current comparison between `.RaymanCodingSkill/context/state.json` and current workspace facts. `POST /api/context/os` rewrites `.RaymanCodingSkill/context/state.json` and appends `.RaymanCodingSkill/context/events.jsonl`. The state graph is derived from Context Index freshness, task context, goal/session pending work, audit findings, and asset retirement state.

Context responses include the [Project understanding](PROJECT_UNDERSTANDING.md) protocol through additive `source_policy`, `understanding_protocol`, and `required_actions` fields. These fields are guidance, not proof of completion.

When auxiliary AI is enabled in `config/auxiliary_ai.yaml`, model-backed operations queue asynchronous auxiliary reconciliation by default and continue the primary model route without waiting. `rayman auxiliary advise` remains the synchronous advisory path. Auxiliary errors are diagnostics only and do not change the protected endpoint success criteria.

Fail-open applies after an auxiliary attempt fails. If auxiliary AI was available but not attempted, the response or persisted workflow/session record must include a skip reason and evidence, such as disabled config, task exclusion, endpoint unavailability, timeout/error details, or an explicit user constraint.

Research agents are governed by `config/research_agents.yaml`. Role findings run in parallel up to `max_parallel_agents` and are persisted in deterministic `planner`, `scientist`, `critic`, `reflector`, `arbiter`, and `safety_monitor` order. Scientist experiments may run only direct argv whitelist commands, must run inside the workspace, reject shell operators, respect the runtime timeout cap, and diff-check protected source/docs/config files before and after execution. Full-suite validation such as `cargo test --all` stays on the primary validation path instead of the default Scientist whitelist. The workspace cwd boundary is mandatory; disabling `require_workspace_cwd` is rejected by execution and security audit. Research output is advisory-only and cannot satisfy completion evidence by itself. Unresolved research conflicts or policy violations block successful goal/session close.

Auxiliary usage stats keep `attempt_count` as the backward-compatible count of recorded auxiliary steps, including queued and skipped records. Real auxiliary calls are reported separately as `call_count` (`success_count + failed_count`), async queue records are reported as `queued_count`, and successful main AI completions are reported as `main_ai_count`. Display ratios use `auxiliary success / auxiliary calls / main AI calls`, with the percentage calculated as auxiliary success divided by `call_count`, so skipped or queued-only records do not dilute the call success rate. Usage stats are the visible value signal for advisory and analysis tasks such as planning, workflow summaries, and research. Auxiliary contribution stats are narrower: they count only implementation-validation rounds where auxiliary AI succeeded and the validator actually corrected the primary model output (`status=fixed`, non-empty `fixes_applied`, or changed final code). Contribution records retain a bounded `events` history with `counted`, `reason`, and evidence strings so API consumers can distinguish a real auxiliary correction from mere participation. When no implementation-validation contribution round has occurred, the contribution state file may be absent and consumers should present that as no implementation-validation correction sample, not as zero auxiliary value. Generation responses include the current request round and project totals. `GET /api/stats` returns usage totals from `.RaymanCodingSkill/auxiliary_usage.json`, provider summaries in `usage_stats.by_provider`, contribution totals and recent events from `.RaymanCodingSkill/auxiliary_contributions.json`, and quality stats from `.RaymanCodingSkill/quality/`.

## Response Fields

Generation returns `code`, `language`, `model`, additive `evidence_status`, `claim_ledger`, `unknowns`, `assumptions`, `blockers`, `auxiliary_ai`, `implementation_validation`, `edge_cases`, `logic_simulation`, `potential_bugs`, `fixes_applied`, and `validation_summary`.

Review returns `review`, `issues`, `suggestions`, `score`, `structured_fields_available`, additive `evidence_status`, `claim_ledger`, `unknowns`, `assumptions`, `blockers`, and `auxiliary_ai`.

Test generation returns `test_code`, `test_count`, `test_types`, additive `evidence_status`, `claim_ledger`, `unknowns`, `assumptions`, `blockers`, and `auxiliary_ai`.

Goal clarification returns `goal_summary`, `default_choices`, `inferred_requirements`, `acceptance_criteria`, `verification_suggestions`, `out_of_scope`, and `customer_questions`.

`auxiliary_ai` includes `queued_task_id`, `selected_provider`, `provider_attempts`, `async_status`, and `reconciliation_status` when auxiliary AI is involved. Async worker tasks persist under `.RaymanCodingSkill/auxiliary/tasks/*.json`; reconciliation conflicts create pending work and block successful goal close until repaired. Malformed auxiliary task JSON is a success blocker for `rayman audit`, `rayman gate status --check`, goal close, and session close instead of being silently skipped.

Goal and research responses append `evidence_status`, `claim_ledger`, `unknowns`, `assumptions`, and `blockers` without removing the persisted record fields. `verified` requires current workspace path evidence, a recorded successful validation command, current goal/session/context state, or an existing evidence artifact. Success/completion/verified claim ledger entries must preserve current `evidence_refs`, `search_effort`, and cleared `counterexample_challenges`; missing or unresolved challenge metadata keeps the claim unverified. Auxiliary AI, cached summaries, memory, research output, and confidence remain advisory only.

Research responses return the persisted session with `hypotheses`, `experiments`, `reflections`, `findings`, `conflicts`, `autonomy_policy`, `status`, and `current_stage`. Finding records include `role`, `status`, `model_ref`, `execution_mode`, `duration_ms`, `route_attempts`, `error`, prompt/response hashes, summary, evidence status, evidence refs, confidence, and risk level. Experiment records include `command.argv`, exit status, stdout/stderr tails, duration, policy violation, and changed-file evidence.

Context returns `workspace_path`, `generated_at`, `status`, `counts`, `records`, `guidance`, `source_policy`, `understanding_protocol`, `required_actions`, `context_os`, and `asset_retirement`. Context records include `context_index`, `context_os`, `file_inventory`, `symbol_index`, `dependency_map`, `task_context`, and `asset_retirement` when the workspace can be scanned.

Project returns multi-language adapter reports for Rust, JavaScript/TypeScript, Python, C#, and Go when those projects are detected, plus additive `asset_retirement`. Impact returns changed files, affected modules, affected public API, likely tests, broad gates, docs/config risk, confidence, evidence, and `asset_retirement`. Regression plan returns `risk_level`, `risk_reasons`, `recommended_focus`, minimal tests, language gates, broad gates, checklist, matched `quality_patterns`, `asset_retirement`, and the underlying impact report.

Asset retirement responses return `workspace_path`, `generated_at`, `state_path`, `controller_scope`, `ignored_roots`, `blockers`, `candidates`, `cascade_candidates`, `retired_present`, `exemptions`, `cleanup_plan`, `detected_references`, `records`, `required_actions`, and `source_policy`. Cleanup plan entries may include `line` and `line_end` when an isolated obsolete Rust test case cascades from a retired declaration or asset.

Stats returns `goals` with active/completed/blocked/failed/partial totals, `research_agents` with session/experiment/conflict/policy-violation totals, `quality` with incident and pattern hit counts, `auxiliary_ai.usage_stats.project_total` with `attempt_count`, `call_count`, `success_count`, `failed_count`, `skipped_count`, `queued_count`, `main_ai_count`, `auxiliary_success_rate`, and `auxiliary_call_success_rate`, `auxiliary_ai.usage_stats.by_provider` with the same counters per provider, plus `auxiliary_ai.contribution_stats.project_total` with `production_count`, `contribution_count`, and `contribution_percentage`. `auxiliary_ai.contribution_stats.events` contains recent implementation-validation contribution events with task, counted flag, correction reason, and evidence.
