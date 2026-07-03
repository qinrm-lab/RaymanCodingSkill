# Quality Patterns

RaymanCodingSkill records repeated customer-reported failures as workspace-local quality memory. The state lives under `.RaymanCodingSkill/quality/` so one customer project does not contaminate another.

## State

- `.RaymanCodingSkill/quality/incidents/*.json` stores the source thread or note, symptom, root cause, fix, generalized behavior, and selected pattern.
- `.RaymanCodingSkill/quality/patterns.json` aggregates incidents into reusable patterns and tracks gate hits.
- Built-in Rayman templates are always available; workspace incidents add local history on top.

## Built-In Patterns

- `case_to_general_rule`: fixed screenshots, exact phrases, or one-off examples require a generic trigger plus at least 2 rewritten positive examples and 1 negative example.
- `context_relevance`: follow-up work must retrieve relevant historical/tool context, while independent questions need a stale-context pollution negative check.
- `project_understanding_freshness`: project understanding must use fresh workspace-local, hash-backed context; gate evidence must include context status/task, Context OS state graph freshness, current-file reread, stale-index handling, and impact/regression planning for touched paths.
- `managed_temp_freshness`: runtime temp failures require evidence from `rayman temp status` or `rayman temp doctor`, and cleanup evidence must show only managed Rayman entries were removed.
- `obsolete_asset_retirement`: review, refactor, and feature replacement work must include obsolete-asset inventory, replacement/current behavior evidence, docs/config/tests synchronization, `rayman assets status`, and `rayman audit` evidence before success. Obsolete assets default to retirement/deletion; tests and feature-coverage anchors that only prove retired declarations/assets cascade into retirement candidates and must be deleted, rewritten for current behavior, or explicitly exempted. Retained assets require an explicit compatibility or audit reason, expiry date, and exclusion from current-behavior context evidence.
- `audit_failure_delivery_gate`: failed audit or validation-gap evidence must include the audit output, triage for every finding, and either resolved audit evidence or a partial/blocked close with pending work. Findings cannot be dismissed as unrelated in a successful handoff.
- `repeated_value_centralization`: repeated literals, thresholds, paths, prompt fragments, and policy values must be inventoried and centralized as a named constant, config key, helper, template variable, or referenced rule section across skill and program surfaces. If duplication is intentionally retained, the close evidence must state the reason and checked scope.
- `agent_eval_security_provenance`: agent workflow changes involving behavior evals, LLM security, prompt-injection/red-team work, dependency policy, release evidence/provenance-required handoff, or regression observability require actual current `rayman eval run`, `cargo deny check`, `rayman security audit`, ready local `rayman release evidence`, and passed latest regression history state before success. Keyword-only close evidence is rejected.
- `codex_host_subagent_ledger`: Codex host subagent use requires `rayman subagent status` evidence, primary-agent review evidence, read-only/write-scope boundary evidence, and overlap/conflict disposition before success.
- `codex_harness_execution_contract`: Codex harness-style self-improvement must map undocumented harness language to documented Codex execution controls or verified current-session capabilities. Evidence must cover sandbox/approval boundaries, durable instruction surfaces (`AGENTS.md`, skills, hooks, MCP/rules), subagent inheritance plus Rayman subagent ledger review, and non-interactive approval/escalation failure handling.
- `active_skill_authority`: skill-driven work must prove the active workspace skill, canonical `SKILL.md`, installed/current skill hash, retired or shadow skill exclusion, canonical CLI or wrapper-bypass path, and current-behavior source decision. Retired RaymanAgent-like material, old `.Rayman/` wrappers, and stale generated agent docs cannot become requirements or command authority by proximity.
- `host_execution_mode_boundary`: Plan Mode, approval policy, sandbox, or other host capability limits must be treated as execution boundaries. Evidence must state the current mode/capability, avoid write/success claims while execution is unavailable, and leave a resumable handoff with blocker owner, minimum input, evidence path, and resume command.
- `delivery_gate_stratification`: project deliverable gates, Rayman broad readiness gates, and goal/session success-close gates prove different claims. Evidence must name the deliverable gate command, report Rayman meta/readiness disposition separately, classify unresolved blockers by layer, and align final status to the proven layer.
- `contract_surface_reconciliation`: implementation work must inventory and reconcile active contract surfaces before claiming behavior is implemented. Evidence must cover visible requirements, hidden/dot-directory requirements, generated docs, feature coverage, tests, gate-script discovery of hidden surfaces, and retirement or update of conflicting old requirements.
- `tool_loop_recovery`: empty responses, diagnostic-only failures, or irrelevant tool results require retry, supplemental lookup, or local synthesis evidence.
- `temporal_fact_evidence`: current facts and relative dates require absolute dates plus current source verification.
- `debug_release_delivery`: customer programs require both debug and release build evidence; a temporary target can prove a locked executable workaround but does not replace formal release verification.
- `evidence_first_unknown`: implementation, validation, research, auxiliary, CLI/API, and prompt claims must expose `evidence_status` plus a claim ledger. Unsupported claims are `unknown`, `assumption`, `blocked`, or `advisory`; high confidence, auxiliary output, cached summaries, memory, and research findings do not prove completion.

## Commands

```text
rayman quality incident add --source codex://threads/example --symptom "empty response" --root-cause "tool loop stopped"
rayman quality patterns
rayman quality gate --goal-id <id>
rayman subagent status
rayman subagent review --id <record-id> --verdict accepted -m "primary reviewed"
rayman assets status
rayman eval run --profile core
rayman eval run --profile full
cargo deny check
rayman security audit
rayman release evidence --label local
rayman regression history --limit 5
rayman regression plan --path crates/rayman-core/src/quality.rs
rayman evidence check --scope workspace --format json
```

`rayman quality gate --goal-id <id>` evaluates matched built-in and workspace patterns against the goal contract and evidence. The gate is a hard gate: when a historical or built-in pattern matches, `rayman goal close --status success` is blocked until the required regression evidence is present. Agent/eval/security/release/regression gates read current workspace state instead of accepting evidence phrases alone; formal release provenance remains a separate `--require-provenance` release-evidence mode.

`rayman regression plan` includes matched workspace quality patterns so old failures become regression checklist items before implementation starts. `rayman stats` prints incident counts, pattern counts, per-pattern incident counts, and gate hit counts.

See [Project understanding](PROJECT_UNDERSTANDING.md) for the command sequence behind `project_understanding_freshness`.
