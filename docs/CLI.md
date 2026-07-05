# CLI

The canonical entry point is `rayman`. Shared documentation must use one command per line so examples work in both PowerShell and cmd.

## Build-Time Invocation

```text
cargo run -p rayman-cli --bin rayman -- session status
```

## Installed Invocation

```text
rayman session status
rayman agent-skill sync
rayman agent-skill status
rayman agent-skill install
rayman agent-skill update
rayman skill sync
rayman skill install
rayman skill update
rayman skill status
rayman install-tools rust,node
```

`rayman agent-skill sync` updates both host skill entries and the installed `rayman` CLI binary. It also installs the companion `提醒` background reminder binary next to `rayman`. The CLI binary source is the canonical release binary when it exists, falling back to the currently running binary only when release output is not present.

`rayman agent-skill install`, `rayman agent-skill update`, `rayman skill sync`, `rayman skill install`, and `rayman skill update` are accepted aliases for the same sync operation. `rayman skill status` is an accepted alias for `rayman agent-skill status`.

`rayman agent-skill status` reports `rayman-cli` as stale when the installed binary differs from the canonical release binary. It also reports `rayman-reminder` as stale when the installed reminder differs from the canonical release reminder binary. Treat stale installed binaries as execution blockers for commands or automatic flows added by the current RaymanCodingSkill source, including `rayman auxiliary` and task-stop reminders.

If a project-local wrapper, shell profile, or retired agent directory intercepts `rayman` or exposes an older command surface, treat that as an active-skill authority issue. Use the canonical RaymanCodingSkill release binary, run shells with profile loading disabled when needed, and record which retired or shadow surfaces were excluded before relying on command output. Project-local RaymanAgent-like material is not current command or requirements authority unless the active workspace contract explicitly reactivates it.

## Generation, Review, And Tests

```text
rayman generate "Create a parser" -l rust -o parser.rs
rayman review parser.rs -l rust
rayman review parser.rs -l rust --apply-prune --backup-comment "before obsolete-code pruning"
rayman test parser.rs -l rust -o parser_tests.rs
rayman refactor parser.rs "reduce duplication" -l rust -o parser_refactored.rs
rayman explain parser.rs -l rust --detail-level high
```

After a successful `generate` with `-o/--output`, RaymanCodingSkill automatically attempts to compile supported outputs. Rust single-file outputs are compiled with `rustc` into a sibling executable; Cargo target files use `cargo build`. Use `--no-auto-compile` when you only want the generated source.

## Model And Config
See [Model And Config](../references/cli-model-and-config.md) for the full rule text.
## Workspace And Session

```text
rayman workspace-skill status
rayman workspace-skill enable -m "this workspace uses raymancodingskill"
rayman workspace-skill disable -m "temporarily disable raymancodingskill here"
rayman workspace-skill mark-used -m "explicit raymancodingskill use"
rayman workspace-skill stop -m "stop using raymancodingskill here"
rayman session status
rayman session recover
rayman goal clarify "Support customer order export" --format text
rayman goal clarify "Support customer order export" --format json
rayman goal start "Ship the requested feature" --acceptance "tests pass" --verify "cargo test --all"
rayman goal run
rayman goal run --until blocked --checkpoint-interval 10 --max-repair-attempts 3
rayman goal resume --id <id> --until blocked
rayman goal status
rayman goal close --status success -m "req_1: crates/rayman-core/src/goal.rs updated and cargo test --all passed"
rayman research start "Investigate validation risk" --goal-id <id>
rayman research run --id <id>
rayman research reconcile --id <id>
rayman session add-pending "finish validation" -m "rerun broad gate"
rayman session complete <todo_id> -m "cargo test --all passed"
rayman session close --status blocked -m "waiting for credentials"
rayman context status
rayman context status --check
rayman context list
rayman context refresh
rayman context os --write
rayman context os --check
rayman context task "review the context kernel"
rayman context explain
rayman project detect
rayman project index
rayman customer-deploy status
rayman customer-deploy set --env prod --build "cargo build --release" --test "cargo test" --deploy "scripts/deploy.ps1" --credential-env PROD_TOKEN
rayman customer-deploy unset deploy_command
rayman customer-deploy validate
rayman impact --path crates/rayman-core/src/context.rs
rayman regression plan --path crates/rayman-core/src/context.rs
rayman regression run --profile auto
rayman regression run --profile parallel-full
rayman regression history --limit 5
rayman eval run --profile full
cargo deny check
rayman security audit
rayman release evidence --label local
rayman release evidence --label local --no-write --sbom sbom.json --attestation attest.json --signed --require-provenance
rayman gate status --check
rayman risk scan
rayman risk plan
rayman risk fix --safe-only
rayman risk fix --guarded
rayman risk verify
rayman risk learn
rayman evidence check --scope workspace --format json
rayman evidence check --scope goal --goal-id <id>
rayman evidence check --scope research --include-advisory
rayman subagent plan --task "审计 subagent 性能提速策略" --path crates/rayman-core/src/subagent.rs --read-only --max-lanes 4
rayman subagent auto-start --task "审计 subagent 性能提速策略" --path crates/rayman-core/src/subagent.rs --read-only --max-lanes 4
rayman subagent record --agent-id <host-agent-id> --goal-id <goal-id> --dispatch-request-id <request-id> --task "host subagent unavailable" --boundary "unavailable closeout" --read-only
rayman quality gate --goal-id <id>
rayman self status
rayman benchmark run --smoke
```

`rayman regression run --profile auto|quick|full|shared-parallel-full|parallel-full` executes repository regression gates and appends an immutable run record to `.RaymanCodingSkill/regression/history.jsonl`.

Non-success `rayman goal close` creates pending recovery metadata with explicit blocker ownership, minimum input, `.RaymanCodingSkill/goals/<id>.json` evidence path, resume command, and automatic resume strategy.

Host execution mode is outside the CLI's control. If the host is in Plan Mode or otherwise prevents edits, destructive actions, approvals, or long-running execution, the correct closeout is a blocked or resumable handoff with the current mode/capability, blocker owner, minimum input, evidence path, and resume command. Do not claim the CLI or an ordinary user message can exit that host mode.

For non-trivial goal plan stages, `rayman goal run` and `rayman goal resume` print `HOST_SUBAGENT_DISPATCH_REQUEST {json}` and stop with `host_subagent_dispatch_requested` when host-subagent lanes are recommended. The primary AI must call the host `spawn_agent` tool with the included lane payloads, or record an unavailable/failed closeout with `rayman subagent record --goal-id <id> --dispatch-request-id <request-id>` followed by `result` and `review`, before continuing the serial path. The standing authorization source is [Host Subagent Auto-Start Authorization](../references/skill-subagent-auto-start-authorization.md); in enabled workspaces it has the same effect as an explicit `开启subagent` phrase, so that phrase is not an extra precondition.

Use `rayman subagent plan --read-only` or `rayman subagent auto-start --read-only` for audit/review-only work. Read-only intent suppresses writable worker lanes, emits only read-only explorer lanes, and includes `--read-only` in the ledger record command templates.

After a top-level `rayman goal run`, `rayman goal resume`, `rayman goal close`, or `rayman session close` command ends, the CLI starts the companion `提醒` program in the background exactly once for that command, whether the command succeeds or returns an error. `subagent` / `host-subagent` commands do not arm this reminder, and child-agent contexts can suppress it with `RAYMAN_REMINDER_SCOPE=subagent`, `RAYMAN_AGENT_ROLE=subagent`, or `RAYMAN_SUBAGENT=1`. A later continued main-thread command can arm a fresh reminder. `提醒` has no visible window; it exits quietly if the user moves the mouse or presses a key, and plays a short beep pattern when the screen is black or no mouse/keyboard activity is detected for 10 seconds.

## Context Kernel
See [Context Kernel](../references/cli-context-kernel.md) for the full rule text.
## Project Intelligence
See [Project Intelligence](../references/cli-project-intelligence.md) for the full rule text.
## Asset Retirement

`rayman assets status` reports workspace-local obsolete asset retirement state from `.RaymanCodingSkill/assets/retirement.json`, including controller scope, ignored scan roots, blockers, candidates, cascade test candidates, retired assets still present, compatibility exemptions, cleanup plan with optional line ranges, detected references, required actions, and source policy.

`rayman assets scan` rereads current files, recomputes stale docs/config/tests/CLI/API references to recorded obsolete assets, registers tests that only prove retired declarations or retired assets as cascade retirement candidates when an isolated Rust test case can be found, registers orphan feature-coverage `test_anchors[].proves` as semantic cascade candidates, ignores non-current roots such as `.git`, `target`, build caches, and `.RaymanCodingSkill/tmp`, and writes the refreshed workspace-local retirement state.

`rayman assets cleanup --apply` deletes only retirement-candidate files already registered in `.RaymanCodingSkill/assets/retirement.json`, only when they still resolve inside the current workspace and have no current references. It also deletes registered isolated obsolete Rust test-case line ranges plus matching orphan feature-coverage anchors before recomputing references and deleting the now-unreferenced retired asset; if the test case is already absent, it may delete the pure orphan feature-coverage anchor by itself. It does not delete unknown files, active `raymancodingskill` or `rayman.exe` assets, arbitrary user data, or unisolated mixed test files. Directory cleanup is blocked until an explicit per-file manifest is available; there is no bare recursive delete path.

`rayman assets retire --path <path> --replacement <text> --reason <text> --validation-command <cmd> --apply-delete` records a hash-backed retirement candidate and deletes only a whole file inside the workspace after reference checks pass. Without `--apply-delete`, it records the candidate and blocks success until deletion or exemption.

`rayman assets exempt --path <path> --reason <text> --expires-at <YYYY-MM-DD>` records a temporary compatibility or audit exemption. Exempt assets remain non-current evidence and expired exemptions block `goal close --status success` and `rayman audit`.

## Managed Temp

`rayman temp status` reports workspace-local managed temp runs under `.RaymanCodingSkill/tmp`, including active, completed, failed, stale, and foreign run entries. It also reports recognized retained `CARGO_TARGET_DIR` caches directly under `.RaymanCodingSkill/tmp` when they carry Cargo target markers such as `.rustc_info.json` or `CACHEDIR.TAG` plus `debug/` or `release/`. It prints next actions such as cleanup or manual inspection.

`rayman temp doctor` checks that the temp root is inside the workspace, can be created, can write and same-directory rename a probe file, and is not at a risky Windows path length.

`rayman temp cleanup --completed` removes completed Rayman-managed temp runs before task success. `rayman temp cleanup --stale` removes expired Rayman-managed temp runs. `rayman temp cleanup --all-failed` removes failed Rayman-managed runs after inspection. `rayman temp cleanup --cargo-targets` removes recognized retained Cargo target caches after successful validation. `goal close --status success`, `session close --status success`, and `rayman audit` block while active, completed, stale, failed, foreign run entries, or retained Cargo target caches still need cleanup or inspection. Cleanup never removes unknown entries without Rayman metadata or Cargo target markers. Stable release binaries such as `target/release/rayman.exe` are not cache cleanup targets.

## Backups

```text
rayman backup create crates/rayman-core/src/config.rs -m "before config change"
rayman backup list
rayman backup restore bkp_20260518T120000000000Z_abcd1234
rayman backup cleanup --stale
```

Restore recreates backed-up empty directories and creates a safety backup for existing files before overwriting them.

## API And Audit

```text
rayman api serve --host 127.0.0.1 --port 8000
rayman docs maintain --prompt "explain the current project boundaries" --model-output model-notes.txt --output docs/project-docs.html
rayman docs maintain --check
rayman docs maintain --apply-prune
rayman docs compact-skill-rules --dry-run
rayman docs compact-skill-rules --root E:\rayman\software\AI\RaymanCodingSkill
rayman gate status --check
rayman coverage status --check
rayman coverage status --strict
rayman coverage status --format markdown --check --output docs/FEATURE_COVERAGE.md
rayman doctor shell
rayman instruction audit
rayman audit
```

`rayman docs maintain` generates or checks a layered HTML developer document from current code, existing docs, configuration, prompts, and optional developer-facing model output. In customer code projects, it also checks README/setup/usage/architecture/configuration/testing documentation coverage. Missing coverage is auto-completed into Rayman-managed docs such as `docs/PROJECT_GUIDE.md`; if `README.md` is absent it creates a minimal README that links to that guide. Existing hand-written README content is not overwritten. The model output is not treated as decorative wording; it is preserved as escaped developer understanding material that can be converted into the generated development document. The generated document covers project purpose, functional boundaries, CLI usage, developer architecture, prompt/model routing context, auxiliary AI usage value, implementation-validation correction contribution totals, customer documentation completeness, and obsolete-asset cleanup state.

Auxiliary AI usage value in the generated report covers advisory and analysis tasks such as planning, workflow summaries, and research. Implementation-validation correction contribution uses the same conservative project-total metric as `rayman stats`: a contribution is counted only when auxiliary AI participated in implementation validation and the final validator output actually corrected the primary model result. If no such validation round has occurred, the report says there is no implementation-validation correction sample instead of presenting a bare `0/0`.

Use `--prompt` or `--prompt-file` to record the task or project-understanding prompt that shaped the document. Use `--model-output <file>` when another model produced developer notes that should be included in the generated document. Use `--check` in gates to fail when the HTML document is stale, customer project docs are incomplete/stale, or obsolete asset blockers exist. Use `--apply-prune` to delete only stale Rayman-generated HTML files that carry the generated-doc marker; it does not delete arbitrary user HTML or source files.

`rayman gate status [--check] [--format text|json]` aggregates workspace-skill activation, Context Index freshness, Context OS state graph freshness, obsolete asset retirement, Codex host subagent ledger state, managed temp state, strict feature coverage, docs maintenance, dependency policy, LLM security audit, repository audit, evidence claims, proactive risk ledger, and `rayman release evidence --no-write`. The asset retirement check runs against the current workspace root: opted-in customer projects report `user_controller`, while this RaymanCodingSkill repository reports `raymancodingskill_controller`; both use their own local `.RaymanCodingSkill/assets/retirement.json` state. Release evidence validates configured `customer_deploy.artifact_paths` first, otherwise Cargo bin release artifacts discovered from `cargo metadata`, and falls back to the Rayman CLI binary only when no customer artifact source is available. Text output is optimized for humans; `--check --format text` emits progress lines while long checks run; JSON output exposes every check and required action. With `--check`, any readiness blocker exits non-zero. A non-Git release provenance warning does not block local development unless `--require-provenance` is also provided. Goal/session success still has additional closure gates such as `req_id` evidence, pending work, active goals, manual/remote gaps, unresolved asset retirement, stale Context OS state, and release/deploy `customer_deploy` readiness.

`rayman risk scan` builds a current-workspace risk ledger under `.RaymanCodingSkill/risk/ledger.jsonl` from context freshness, Context OS, managed temp, asset retirement, feature coverage, docs maintenance, dependency/security, repository audit, evidence claims, auxiliary task state, and host-subagent ledger state. `rayman risk plan` groups findings into `safe_auto`, `guarded_auto`, and `human_required`; `risk fix --safe-only` applies only deterministic maintenance fixes such as context refresh, Context OS write, managed-temp cleanup, and docs maintenance; `risk fix --guarded` records guarded items without editing uncertain code surfaces. `rayman risk verify` fails while unresolved high or critical findings remain, and `rayman risk learn` writes learned risk categories to `.RaymanCodingSkill/risk/learned-patterns.json`. The readiness gate includes a read-only risk check and blocks unresolved high/critical risk.

For customer projects, keep the project deliverable gate and Rayman broad readiness gate separate in reports. A project gate such as a requirements gate can prove the requested product closure, while `rayman gate status --check` can still expose framework-governance debt such as feature coverage, docs, provenance, or regression history. Report both layers instead of converting one result into the other.

`rayman evidence check [--scope workspace|goal|session|research] [--goal-id <id>] [--include-advisory] [--format text|json]` reports evidence-backed claim status. JSON output includes `evidence_status`, `claim_ledger`, `unknowns`, `assumptions`, `blockers`, and `required_actions`. Workspace scope reports current file-presence evidence separately from completion claims; file presence proves that an artifact exists, not that a feature is complete. Completion and success claims require a current workspace path, recorded successful validation command, current goal/session/context state, or an existing evidence artifact plus the success-claim counterexample/search metadata required by the proof rules. Auxiliary AI, cached summaries, memory, research output, and confidence remain advisory.

`rayman coverage status` checks `config/feature_coverage.yaml`, verifies documentation anchors, implementation anchors, test anchors, public CLI commands, public API endpoints, and UI contract markers, and prints a compact text summary by default. Use `--format json` for the full machine report. Use `--strict` to require entries marked `strict_validation: true` to carry current passed `validation_records` with `updated_at`, `evidence_path`, exact command text, passed status, and configured `evidence_contains` text. Use `--check` in gates; it enables strict validation. Canonical `docs/FEATURE_COVERAGE.md` output uses strict validation by default, so `--format markdown --output docs/FEATURE_COVERAGE.md` refreshes the human-readable matrix with the same strict semantics that `--check` compares.

`rayman doctor shell` diagnoses host shell noise that can corrupt or stall CLI output, especially PowerShell profile stderr or startup loops. It probes PowerShell with and without profile loading and recommends using `target\release\rayman.exe` or a NoProfile shell when profile output pollutes command results. Routine commands keep stats footers off by default; add global `--show-stats` when project auxiliary-AI usage and implementation-correction totals are needed beside a command result.
