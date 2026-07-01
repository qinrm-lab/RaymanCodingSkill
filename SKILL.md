---
name: raymancodingskill
description: Use the Rust-native RaymanCodingSkill framework for programming workflows when explicitly invoked or when a workspace opts in through .RaymanCodingSkill/workspace_skill.yaml. Covers code generation, review, obsolete-code pruning, refactoring, tests, docs sync, regression/conflict/instruction governance, backups, model routing, and session pending-work continuity. Do not use for host-app accounts, IDE assistant integrations, OAuth flows, connectors, or non-programming tasks.
---

# RaymanCodingSkill
RaymanCodingSkill is a workspace-scoped Rust coding workflow. Read current workspace files first, use the local `rayman` CLI when it helps, and keep behavior tied to this workspace.

## Workspace Rules

- Treat `.RaymanCodingSkill/workspace_skill.yaml` as the only automatic-use switch. `enabled: true` means continue using this skill for coding tasks in this workspace. `enabled: false` means do not auto-use it unless the user explicitly re-enables it or asks for one-off use.
- When this skill is explicitly used, run `rayman workspace-skill mark-used -m "explicit raymancodingskill use"` when the CLI is available. This records current-workspace state and must not re-enable a stopped workspace.
- Use the current workspace `SKILL.md` when present. If absent, use the installed `~/.codex/skills/raymancodingskill/SKILL.md` or `~/.claude/skills/raymancodingskill/SKILL.md`; do not copy skill instructions into workspace state.
- Use the Managed Temp Protocol for runtime temporary files; default temp state is workspace-local `.RaymanCodingSkill/tmp/`, not system temp or remembered cross-project locations.

## Agent Skill Installation
See [Agent Skill Installation](references/skill-agent-skill-installation.md) for the full rule text: `rayman agent-skill sync` refreshes the installed `rayman` CLI binary and also installs the companion `提醒` background reminder binary; `rayman agent-skill status` must report Codex, Claude Code, and `rayman-cli`, and must also report `rayman-reminder`.
## Customer Project Scope

RaymanCodingSkill is a governance framework, not a default pointer to nearby or remembered customer repositories.

- When modifying RaymanCodingSkill itself, do not infer or scan adjacent customer projects as implementation targets. Only edit this framework's rules, docs, tests, examples, or Rust code.
- Customer-project requirements default to generic workflow rules, review standards, prompt guidance, documentation, or contract tests unless the user explicitly names an external project path and asks to implement there.
- Project-specific languages, frameworks, and build commands apply only when the user explicitly provides a project path and asks to run implementation or validation in that project.
- Generic UI, asset-cleanup, operation-feedback, stale-instruction, and repository-hygiene requirements belong in RaymanCodingSkill instructions, docs, tests, examples, or fixtures.
- In opted-in customer code projects, `rayman docs maintain` must keep customer-facing project documentation complete. If README/setup/usage/architecture/configuration/testing coverage is missing, it auto-generates Rayman-managed docs without overwriting existing hand-written README content; `rayman docs maintain --check` and `rayman audit` block until the docs are current.

## Project Understanding Protocol

- For non-trivial coding work, run `rayman context status`, `rayman context os --check`, `rayman context task "<task>"`, and path-specific `rayman impact` / `rayman regression plan` before editing.
- For terse or ambiguous customer requests, use the goal clarification contract: `rayman goal clarify "<request>" --format text|json` previews recommended defaults, and `goal start` stores the same hidden-requirement clarification under `contract.clarification` for intake confirmation.
- Current workspace files, command output, and goal/session/context state are the source of truth; cached summaries, Context Index records, remembered conclusions, and auxiliary AI output are navigation only.
- Before consuming any project-local agent instructions, wrappers, generated docs, or old automation directories, establish active skill authority: verify `.RaymanCodingSkill/workspace_skill.yaml`, current canonical `SKILL.md`, and the installed/current skill hash when available. Retired or shadow skill surfaces such as project-local `.Rayman/` material, old `rayman` wrappers, and retired RaymanAgent/RaymanAgent-like docs are not requirements, current behavior evidence, or command authority unless the current workspace contract explicitly reactivates them.
- Before implementing behavior in customer projects, reconcile the active contract surfaces: visible requirements, hidden/dot-directory requirements, generated docs, feature coverage, tests, and gate scripts. Gate discovery must include hidden contract mirrors when the project uses them, and conflicting old requirements must be retired or updated rather than left active beside the new behavior.
- If context is stale, hashes differ, indexed evidence conflicts with current files, or Context OS state is missing/stale, reread the current source/docs/config files, run `rayman context refresh`, and confirm `rayman context os --check` before relying on it.
- Do not use cross-project memory or remembered customer-project assumptions; opted-in customer projects get workspace-local context only.
- Completion evidence for every `must` requirement must cite current files and validation output; cached context never satisfies a requirement by itself.

## Chinese Text, Display, And Encoding

- Treat Chinese/CJK user requests, source comments, docs, prompts, model output, CLI output, filenames, paths, JSON/YAML/HTML, logs, and generated artifacts as first-class content.
- Preserve UTF-8 end to end. Do not replace Chinese text with pinyin, ASCII fallbacks, HTML entities, JSON escapes, or English paraphrases unless the target format explicitly requires escaping, and verify round-trip behavior after parsing, serialization, or model handoff.
- For terminal and CLI work, account for Windows PowerShell/cmd and Unix-like UTF-8 consoles. Do not use byte length or `String::len()` as a display-width proxy for aligned tables, truncation, wrapping, progress output, or diagnostics that may contain Chinese; use Unicode-width/grapheme-aware handling when formatting visible columns.
- For file processing and search/indexing, use Unicode-safe APIs and include Chinese fixtures when behavior touches tokenization, matching, normalization, paths, serialization, parsing, or generated documentation.
- For web, HTML, Markdown, and generated developer docs, declare UTF-8 where applicable, use fonts and line wrapping that render Chinese reliably, and validate that Chinese text does not overflow, become mojibake, or disappear.
- Preserve the user's language intent. If the request is in Chinese or asks for Chinese output, keep user-facing explanations, generated docs, prompts, labels, and validation notes in Chinese unless a project convention or API contract requires another language.

## Managed Temp Protocol

- Runtime temp work must use `rayman temp` / TempManager under workspace-local `.RaymanCodingSkill/tmp/`; do not introduce production `std::env::temp_dir()` usage.
- Use same-directory atomic temp files only when replacing a target file, so rename stays on the same volume.
- Temporary validation build caches created with an explicit `CARGO_TARGET_DIR` under `.RaymanCodingSkill/tmp/` are disposable: delete recognized Cargo target caches with `rayman temp cleanup --cargo-targets` after the validation command succeeds; preserve them after failure and report the retained path plus cleanup command for diagnosis. Never delete stable release binaries such as `target/release/rayman.exe` as cache cleanup.
- Diagnose and clean managed temp state with `rayman temp status`, `rayman temp doctor`, and `rayman temp cleanup --completed` / `--stale` / `--all-failed` / `--cargo-targets`; completed managed temp runs and retained successful Cargo target caches must be removed before goal/session success, failed runs must be inspected or kept with a non-success close, and unknown customer files must never be deleted as cleanup.

## Session Continuity
See [Session Continuity](references/skill-session-continuity.md) for the full rule text.
## Documentation Structure And Lossless Splitting
See [Documentation Structure And Lossless Splitting](references/skill-documentation-structure-and-lossless-splitting.md) for the full rule text.
## Core Workflow
See [Core Workflow](references/skill-core-workflow.md) for the full rule text.
## Closed-Loop Delivery
See [Proof And Preservation](references/skill-proof-and-preservation.md) for closed-loop delivery, Paper Claim Audit Protocol, and Feature Preservation Protocol rules.

- Primary completion evidence must come from current files, command output, and goal/session/context state; auxiliary AI output and cached summaries are advisory only.
- Follow the [Evidence-First Unknown Rule](references/skill-proof-and-preservation.md#evidence-first-unknown-rule): when proof is missing or advisory-only, report `unknown`/`blocked`/`assumption` instead of plausible success.
- `goal close --status success` rejects `req_id`-only evidence; every `must` requirement needs an existing current workspace path, a recorded successful validation command, or an existing evidence artifact.
- `rayman gate status --check` is the broad readiness gate for workspace-skill activation, Context Index freshness, Context OS state graph freshness, Codex host subagent ledger state, managed temp state, feature coverage, docs maintenance, dependency policy through `cargo deny check`, `rayman security audit`, `rayman audit`, and `rayman release evidence --label local --no-write`. Successful goal/session close has additional closure gates, including explicit `req_id` evidence, pending work, active goals, unresolved asset retirement, incomplete customer project docs, release/deploy `customer_deploy` config, unclean managed temp state, stale Context OS state, and manual/remote validation gaps; verify those with `goal close --status success` / `session close --status success` instead of treating readiness as closure.
- Compatibility execution guarantee: `rayman coverage status --check`, `rayman docs maintain --check`, `rayman audit`, stale context, pending work, active goals, unresolved asset retirement, incomplete customer project docs, unclean managed temp state, and manual/remote validation gaps block success until fixed or recorded as partial/blocked.
- Follow the [Repeated Value Centralization Rule](references/skill-proof-and-preservation.md#repeated-value-centralization-rule) as the single source for repeated skill/program values.

## Autonomy And Intervention

- Default to continuing: exhaust non-human executable paths by inspecting, implementing, validating, repairing, documenting, and summarizing without asking when the next step is locally knowable.
- Host subagent auto-start authorization lives in [Host Subagent Auto-Start Authorization](references/skill-subagent-auto-start-authorization.md). Read that file before deciding whether to spawn Codex host subagents. In short: standing authorization permits suitable Codex host subagent use without per-use approval, and in enabled workspaces it has the same effect as an explicit `开启subagent` phrase; `rayman goal run` / `rayman goal resume` may emit `HOST_SUBAGENT_DISPATCH_REQUEST {json}`; use `rayman subagent auto-start` for host-tool-ready payloads; keep subagents as main-model/strong-model speed lanes rather than `ai_ubuntu_8888` auxiliary AI; and close the ledger with `rayman subagent record/result/review/status`. Codex host subagent ledger records spawned agent tasks, boundaries, results, and primary-agent review. Unreviewed, unresolved, conflict, or overlapping subagent ledger entries block success; unclosed dispatch requests and parse-error ledger entries block success as well.
- Respect the host execution mode as a capability boundary. If the host is in Plan Mode or otherwise prevents writes, destructive actions, approvals, or long-running execution, do not claim implementation success and do not imply a normal user message can change that mode. Leave a resumable handoff with blocker owner, minimum required input, evidence/checkpoint path, resume command, and automatic resume strategy.
- Keep gate authority layers separate. A customer project's deliverable gate can prove the requested project closure, while `rayman gate status --check` is Rayman broad readiness and goal/session success close has stricter per-requirement evidence gates. Do not promote a project PASS into Rayman meta PASS, or demote a project PASS because of unrelated long-term meta-governance blockers; report each layer explicitly.
- Ask or block only for missing permissions/credentials, destructive actions, conflicting requirements, ambiguous project scope that risks editing the wrong workspace, unavailable external services with no fallback, business decisions only the customer can make, or a hard validation/release gate that remains after local repair; repeated failures are diagnostics unless they leave one of those blockers. Every wait must state the blocker owner, minimum input, evidence path, resume command, and automatic resume strategy.
- Final handoff must state goal status, unfinished requirements, verification evidence, blocked reason if any, and auxiliary AI usage/contribution stats.

## Examples

- Positive: add a focused Rust test beside a changed parser and run the narrow test before broad gates.
- Positive: split an oversized `SKILL.md` by moving exact detailed sections into `references/` and linking them from the main file.
- Negative: do not shorten skill rules by summarizing away edge cases, commands, or acceptance criteria.

## Review And Pruning

- In every review, separately flag obsolete, inactive, unreachable, replaced, or duplicate code that can be deleted. State deletion reason and risk.
- Default review mode reports only. Write files only when the user or CLI explicitly requests `review --apply-prune`.
- Before review-driven pruning writes back, create an annotated local backup and surface stale-backup cleanup prompts.
- Do not keep dead compatibility code once it is no longer effective.

## Asset Retirement

- Treat obsolete assets as any repo content that keeps replaced behavior callable, documented, configured, tested, generated, or presented to agents as current.
- Obsolete assets default to retirement and deletion from the current repository surface; git history or backups preserve old content.
- Compatibility or audit retention requires an explicit reason and expiry date, and retained assets must be excluded from current-behavior context evidence.
- Use `rayman assets status` / `scan` before success; `scan` refreshes workspace-local recorded references and `rayman gate status --check` exposes the first-class `asset_retirement` check for both opted-in customer workspaces (`user_controller`) and this repository (`raymancodingskill_controller`). Unresolved candidates, expired exemptions, retired assets still present, or stale references block `goal close --status success`, `rayman gate status --check`, and `rayman audit`.
- Use `rayman assets cleanup --apply` only after the obsolete files are already registered, still resolve inside the current workspace, and have no current references. Cleanup deletes whole registered files only; it does not remove unknown user data, active `raymancodingskill` / `rayman.exe` assets, or directories without an explicit per-file manifest.
- Before cleanup, cite current-file evidence, affected paths, replacement behavior, deletion reason, risk, and validation result. Only explicit customer approval or `review --apply-prune` may write obsolete-asset pruning changes.
- See [Asset Retirement](references/skill-asset-retirement.md) for the full rule text, including asset scope and validation expectations.
## Boundaries

RaymanCodingSkill is only for programming workflows. It does not manage host-app accounts, IDE assistant integrations, OAuth flows, connector installation, or external coding-assistant login state. API keys are limited to model providers used by the framework and are read from `.env` through names declared in `config/default_config.yaml`.

## Useful Files

- `crates/rayman-core/`, `crates/rayman-cli/`, and `crates/rayman-api/`: core contracts, CLI, HTTP API, model routing, skills, state, docs splitting, instruction lifecycle, and audit gates.
- `config/` and `docs/`: model routing, workflows, quality gates, backup/session settings, layered documentation, and task guides.
## Validation

Run focused tests for touched behavior, then run a broad check when feasible:

- Deliver runnable CLI binaries from stable install directories, not temporary build output such as `.tmp/target`.
- Missing validation/development tools are actionable work, not skip reasons: install the standard tool when allowed; if it cannot be installed, build a workspace-local equivalent and run it.
- Customer program delivery is incomplete until both debug and release builds compile; compile or logic failures trigger automatic repair until the feature loop is coherent and verified.

Core validation includes `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --all`, `cargo deny check`, `rayman context refresh`, `rayman context os --check`, `rayman gate status --check`, `rayman eval run --profile full`, `rayman security audit`, and `rayman audit`.
