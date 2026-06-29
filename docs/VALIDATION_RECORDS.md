# Feature Coverage Strict Validation Records

This file records current passed validation commands used by strict feature coverage entries.

updated_at: 2026-06-28T23:50:38.691762Z
status: passed

## close_success_requires_actual_evidence

record: close_success_requires_actual_evidence
status: passed
command: cargo test -p rayman-core goal::
command: cargo test -p rayman-core session::
command: cargo test -p rayman-core quality::

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
