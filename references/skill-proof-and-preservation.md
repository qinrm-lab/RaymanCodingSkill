# Extracted Skill Rules

Source: `SKILL.md`

## Closed-Loop Delivery

- A customer request is complete only when requirement intent, changed files, impact evidence, validation commands, validation results, docs/config sync, and final status agree with each other.
- Summaries, auxiliary AI output, and cached context never prove completion. The primary AI must verify against current files, command output, and goal/session/context state.
- Auxiliary AI never executes, edits, approves, or replaces validation; it can only provide advisory context for the primary AI.
- Codex host subagents standing authorization and closeout rules live in [Host Subagent Auto-Start Authorization](skill-subagent-auto-start-authorization.md). Read that file before deciding whether to spawn host subagents. In brief: suitable host subagents are authorized without per-use approval; in enabled workspaces that standing authorization has the same effect as an explicit `开启subagent` phrase; `rayman goal run` and `rayman goal resume` may emit `HOST_SUBAGENT_DISPATCH_REQUEST {json}`; `rayman subagent auto-start` provides host-ready spawn payloads; host subagents are main-model/strong-model speed lanes, not auxiliary AI; and every spawned lane must be closed with `rayman subagent record`, `rayman subagent result`, and `rayman subagent review`.
- Codex harness-style self-improvement must map harness terminology to documented Codex execution controls or verified current-session capabilities before implementation. Required evidence covers sandbox/approval boundaries, durable instruction surfaces (`AGENTS.md`, skills, hooks, MCP/rules), subagent inheritance plus Rayman subagent ledger review, and non-interactive approval/escalation failure handling.
- Research scientist agents may run only whitelisted argv experiments from `config/research_agents.yaml`; they must not edit files, approve validation, close goals, or claim completion.
- Unresolved research conflicts or policy violations block successful goal/session close until reconciled or recorded as non-success pending work.
- `goal close --status success` must include explicit `req_id` evidence for every must requirement, such as `req_1: crates/rayman-core/src/goal.rs updated and cargo test passed`.
- `goal close --status success` rejects `req_id`-only evidence. Each must requirement evidence must cite an existing current workspace path, a recorded successful validation command, or an existing evidence artifact.
- Repeated customer-reported failures must be captured as workspace-local quality incidents and generalized into reusable quality patterns; `goal close --status success` hard-blocks matched patterns until their regression evidence is present.

### Evidence-First Unknown Rule

- Current workspace files, successful command output, goal/session/context state, and existing evidence artifacts are the only proof sources for `success`, `satisfied`, or `verified` claims.
- Success/completion/verified/ready claims must carry current `evidence_refs`, explicit `search_effort`, and a cleared `counterexample_challenges` entry. The challenge must try to falsify the claim, record its result, and cite evidence refs that resolve in the current workspace; missing, stale, unresolved, advisory-only, or fabricated challenge metadata blocks success.
- Cached summaries, Context Index records, remembered conclusions, cross-project memory, auxiliary AI, research output, and model confidence are navigation or advisory only. They never prove completion by themselves.
- When evidence is missing, stale, conflicting, or advisory-only, the claim must be downgraded to `unknown`, `assumption`, `blocked`, or `advisory`; `unknown` is preferred over a plausible but unsupported answer.
- `confidence` is metadata only. Empty `evidence_refs` plus high confidence cannot produce `verified`.
- Evidence conflicts fail closed: a blocked, failed, stale, or unverified marker overrides a plausible path or summary until current evidence is refreshed.
- Public CLI/API responses that make implementation, validation, or completion claims must expose `evidence_status`, `claim_ledger`, `evidence_refs`, `search_effort`, `counterexample_challenges`, `unknowns`, `assumptions`, and `blockers`, and successful close/gate paths must reject unverified or unchallenged success claims.
### Repeated Value Centralization Rule

Repeated values that appear in multiple skill or program locations must be centralized as a named constant, config key, helper, template variable, or referenced rule section. If duplication is intentionally retained, record the reason and the checked scope so future changes do not require many manual edits.
- Agent workflow changes that touch agent behavior, LLM security, dependency policy, release evidence, provenance-required release handoff, or regression observability must run `rayman eval run`, `cargo deny check`, `rayman security audit`, and `rayman release evidence`; regression history must be present before success.
- Unresolved `rayman audit` failures, stale context, pending work, active goals, unclean managed temp state, or manual/remote validation gaps block `session close --status success`; fix them, record pending work, or close partial/blocked instead of reporting complete.
- Completed Rayman-managed temp runs and successful validation Cargo target caches are disposable task-end assets: run `rayman temp cleanup --completed` and, when `rayman temp status` reports retained Cargo target caches, `rayman temp cleanup --cargo-targets` before success. Stale managed temp runs require `rayman temp cleanup --stale`; failed runs require inspection plus `rayman temp cleanup --all-failed` or a non-success close. Unknown files under temp roots are never deleted automatically.
- Customer code projects must have complete project documentation before success. `rayman docs maintain` auto-completes missing README/setup/usage/architecture/configuration/testing coverage with Rayman-managed docs, without overwriting existing hand-written README content; `rayman docs maintain --check` and `rayman audit` block incomplete customer docs.
- If any part of the loop conflicts, mark the goal partial/blocked or continue repair; do not report success.
- New or changed public functionality claims in docs or skill rules must update `config/feature_coverage.yaml`, `docs/FEATURE_COVERAGE.md`, and focused test evidence before success.

## Paper Claim Audit Protocol

- Public docs, skill rules, YAML behavior claims, CLI/API claims, and close-gate claims must be backed by `config/feature_coverage.yaml`.
- A claim is sufficiently proven only when it has implementation anchors, semantic test anchors with `test_anchors[].proves`, and validation commands. A string anchor that merely finds a word in a file is not sufficient proof.
- Runtime consumption claims must name the Rust consumer. If a YAML key is not consumed by code, document it as governance/reference metadata or implement the consumption path and a negative test.
- `rayman coverage status --check` and `rayman audit` are the broad gates for paper-claim drift. Fix findings before success, or record explicit partial/blocked pending work.

## Feature Preservation Protocol

- Before deleting, replacing, hiding, or disabling a public command, API endpoint, config key, runtime behavior, generated asset, or skill rule, reread current files and run the relevant context/coverage/asset checks.
- Public surfaces must stay registered in `config/feature_coverage.yaml`, documented in public docs when user-facing, and tied to semantic parser/API/gate tests. Source-extracted CLI commands that are not registered fail coverage.
- Obsolete or replaced surfaces must go through asset retirement: replacement behavior, reference search, risk, and validation command. Unresolved retirement blockers stop `goal close --status success` and `rayman audit`.
- When context is too small or stale to prove a feature is unused, do not delete it from memory. Refresh/reread, add a focused test, or leave it pending instead of reporting success.
