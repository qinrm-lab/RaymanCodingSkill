# Goal Workflows

Goal workflows turn a request into a contract, atomic operations, artifacts, and an execution report.

Goal intake expands the customer's terse request into a deterministic clarification record before implementation. `rayman goal clarify "<request>" --format text|json` previews the same recommended defaults that `goal start` stores under `contract.clarification`: default choices, inferred requirements, acceptance criteria, verification suggestions, out-of-scope items, and customer-confirmation questions. These defaults are assumptions for customer review, not proof of completion.

Substantial goals include an impact stage before implementation. The impact stage uses project adapters and test selection to record affected modules, likely tests, public API risk, and broad gates. A goal cannot be closed as success unless required impact and validation evidence are present.

Goal success is a closed-loop contract, not a status label. Every `must` requirement must be explicitly mapped to completion evidence by its `req_id`, and that evidence must agree with current files, impact output, validation commands, context freshness, pending work, review blockers, quality gate results, and audit findings. `goal run` can advance to summary or a workflow can complete an operation, but that only creates `pending_evidence`; success requires final evidence such as `req_1: implemented X and verified with cargo test`. A path evidence item must resolve to an existing file inside the workspace; validation-command evidence must match a successful validate step already recorded on the goal. Auxiliary AI output, research findings, confidence, memory, and cached summaries are advisory context only; they cannot satisfy requirements or replace primary validation.

Evidence-first unknown handling is mandatory for goal and workflow reports. Unsupported implementation or validation claims must be represented as `unknown`, `assumption`, `blocked`, or `advisory` in `evidence_status` and `claim_ledger`; `workflow completed`, high confidence, or auxiliary/research agreement is not completion evidence.

Repeated customer-reported failures must be added as quality incidents and generalized into workspace-local quality patterns. When a goal matches a built-in or historical quality pattern, `goal close --status success` is a hard gate and fails until the required regression evidence is present.

Large implementation tasks should use `rayman goal start`, `rayman goal run`, and `rayman goal close` so final status passes the same hard gates as the work. If `rayman audit`, context freshness, pending work, active goals, remote UI checks, or manual validation remain unresolved, close partial/blocked and record pending work instead of reporting complete.

Long-running work can use `rayman goal run --until blocked|summary|complete --checkpoint-interval <minutes> --max-repair-attempts <n>` to advance stage by stage until a safe stop point. The runner writes `metadata.long_run.checkpoints`, keeps advancing through non-human executable inspect/implement/validate/repair/doc-sync paths, stops at summary when req_id evidence is still required, and blocks only for user-owned input, unavailable external dependencies, or hard validation/release gates that remain after local repair. Blocked/partial goal close records `blocker_kind`, `minimum_input`, `evidence_path`, `resume_command`, and `auto_resume_strategy` in pending-work metadata. `rayman goal resume --id <id>` reopens recoverable goals without claiming success, and `rayman session recover` reports the next pending item or recoverable goal with the exact resume command.

For project understanding work, goals must follow [Project understanding](PROJECT_UNDERSTANDING.md): context status/task evidence, Context OS state graph freshness, current-file reread evidence, stale-index handling, and impact/regression evidence for touched paths.

## Workflows

- `standard_development`: generate code and run implementation validation.
- `feature_update`: review or update existing behavior.
- `documentation_update`: synchronize documentation intent and implementation notes.

For non-trivial programming work, goal workflows should separate advisory channels from host-subagent speed lanes. Advisory channels include auxiliary AI, Rayman research roles, and local auxiliary models; they provide evidence-aware suggestions only. Codex host subagents are separate main-model or other strong-model child agents used to speed development when the host exposes them and the work is necessary, independent, and parallelizable. During `rayman goal run` and `rayman goal resume`, the controller automatically emits `HOST_SUBAGENT_DISPATCH_REQUEST {json}` at the plan stage when lanes are recommended, and it stops the long run with `host_subagent_dispatch_requested` until the primary AI calls the host `spawn_agent` tool or records an unavailable/failed closeout. Use `rayman subagent auto-start --task "<task>" --path <path>` when the primary agent needs host-tool-ready spawn requests, or `rayman subagent plan --task "<task>" --path <path>` when it needs the same deterministic dispatch rubric, independent lane split, skip reason, record-command template, and auto-start contract. Add `--read-only` for audit/review-only work so auto-start emits read-only explorer lanes and suppresses writable worker lanes. Codex host subagents are not `ai_ubuntu_8888` auxiliary AI. Host subagent use is recorded in the Codex host subagent ledger: `rayman subagent record --goal-id <id> --dispatch-request-id <id>` captures task/boundary/read-only or write scope and binds it to a controller request, `rayman subagent result` captures result evidence and changed paths, and `rayman subagent review` captures the primary agent's disposition. The standing authorization source is [Host Subagent Auto-Start Authorization](../references/skill-subagent-auto-start-authorization.md); in enabled workspaces it has the same effect as an explicit `开启subagent` phrase, so that phrase is not an extra precondition. Unreviewed records, unclosed dispatch requests, unresolved conflicts, parse errors, or overlapping subagent ledger entries block goal/session success, repository audit, and readiness gate. Auxiliary failure is fail-open: the workflow may continue through the primary route after recording the failed attempt. Availability without an attempt is not fail-open for a required advisory channel; the execution report must include the skip reason and evidence.

For Codex harness or Codex execution-envelope self-improvement work, goal evidence must map the request to documented Codex controls or current-session capabilities. Required evidence covers sandbox/approval boundaries, durable instruction surfaces (`AGENTS.md`, skills, hooks, MCP/rules), subagent inheritance with Rayman ledger review, and non-interactive approval/escalation failure handling. Treat unpublished harness terms as terminology to map, not as independent authority.

For exploratory, ambiguous, or failure-analysis work, a goal may attach a research session with `rayman research start "<question>" --goal-id <id>`. Research agents produce hypotheses, whitelist experiments, reflection rounds, and conflict records. Scientist experiments may run only commands allowed by `config/research_agents.yaml`; they cannot edit files, close goals, approve validation, or satisfy must requirements by themselves. Any research session in `conflict`, `blocked`, or `policy_violation` status blocks `goal close --status success` until reconciled or recorded as non-success pending work.

## Commands

```text
rayman generate "Create a CLI parser" -l rust --workflow standard_development --goal-report report.json
rayman review crates/rayman-cli/src/main.rs -l rust --workflow feature_update
rayman quality incident add --source codex://threads/example --symptom "stale fact answer" --root-cause "missing temporal evidence gate"
rayman quality gate --goal-id <id>
rayman research start "Investigate failing validation" --goal-id <id>
rayman research run --id <id>
rayman research reconcile --id <id>
rayman subagent plan --task "审计 subagent 性能提速策略" --path crates/rayman-core/src/subagent.rs --path docs/GOAL_WORKFLOWS.md --read-only --max-lanes 4
rayman subagent auto-start --task "审计 subagent 性能提速策略" --path crates/rayman-core/src/subagent.rs --path docs/GOAL_WORKFLOWS.md --read-only --max-lanes 4
rayman subagent record --agent-id <id> --task "review docs" --boundary "read-only docs review" --read-only
rayman subagent result --id <record-id> --status completed -m "review complete"
rayman subagent review --id <record-id> --verdict accepted -m "primary reviewed"
rayman goal clarify "Support customer order export" --format text
rayman goal run --until blocked --checkpoint-interval 10 --max-repair-attempts 3
rayman goal resume --id <id> --until blocked
rayman goal close --status success -m "req_1: implemented requested behavior; req_2: validation passed"
rayman session recover
rayman docs compact-skill-rules --dry-run
```

## Documentation Size Rules

For skill rule Markdown, files above 20,000 characters must be split losslessly into linked `references/` files until the source is below 12,000 characters and at least 20% smaller. The main `SKILL.md` has a 100-line target and only triggers cleanup/audit above 125 lines; when line-triggered cleanup runs, it must return the main file to the 100-line target without deleting, summarizing, or paraphrasing rules, commands, acceptance criteria, or edge cases.
