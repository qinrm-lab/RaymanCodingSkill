# Feature Coverage Strict Validation Records

This file records current passed validation commands used by strict feature coverage entries.

updated_at: 2026-07-03T21:26:30Z
status: passed

## close_success_requires_actual_evidence

record: close_success_requires_actual_evidence
status: passed
command: cargo test -p rayman-core goal::
command: cargo test -p rayman-core session::
command: cargo test -p rayman-core quality::

## success_claim_counterexample_challenge_gate

record: success_claim_counterexample_challenge_gate
status: passed
command: cargo test -p rayman-core evidence::
command: cargo test -p rayman-core goal::
command: cargo test -p rayman-core session::
command: cargo test -p rayman-core skills::
command: cargo test -p rayman-api

## verification_supremacy_over_ai_judgment

record: verification_supremacy_over_ai_judgment
status: passed
command: cargo test -p rayman-core evidence::
command: cargo test -p rayman-core goal::
command: cargo test -p rayman-core quality::
command: cargo test -p rayman-core feature_coverage::
command: cargo test -p rayman-cli cli_parses_coverage_status
command: rayman coverage status --check

## codex_host_subagent_ledger_gate

record: codex_host_subagent_ledger_gate
status: passed
command: cargo test -p rayman-core subagent::
command: cargo test -p rayman-core session::
command: cargo test -p rayman-core goal::
command: cargo test -p rayman-core audit::
command: cargo test -p rayman-core gate::
command: cargo test -p rayman-cli cli_parses_subagent_commands

## codex_host_subagent_performance_contract

record: codex_host_subagent_performance_contract
status: passed
command: cargo test -p rayman-core subagent::
command: cargo test -p rayman-core goal::
command: cargo test -p rayman-core enabled_workspace_subagent_authorization_matches_explicit_phrase
command: cargo test -p rayman-cli cli_parses_subagent_commands
command: cargo test -p rayman-cli cli_goal_run_emits_host_subagent_dispatch_request
command: cargo test -p rayman-cli --test ui_contract cli_subagent_auto_start_emits_spawn_contract

## readiness_gate_aggregates_readiness_checks

record: readiness_gate_aggregates_readiness_checks
status: passed
command: cargo test -p rayman-core gate::
command: cargo test -p rayman-cli cli_parses_governance_commands

## paper_claim_audit_protocol

record: paper_claim_audit_protocol
status: passed
command: cargo test -p rayman-core feature_coverage::
command: cargo test -p rayman-cli cli_parses_coverage_status
command: rayman coverage status --check

## proactive_risk_governance

record: proactive_risk_governance
status: passed
command: cargo test -p rayman-core risk::
command: cargo test -p rayman-cli cli_parses_risk_commands
command: cargo test -p rayman-core gate::
