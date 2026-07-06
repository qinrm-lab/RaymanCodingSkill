# RaymanCodingSkill

RaymanCodingSkill is a Rust-native coding workflow framework for local agent work. It provides one canonical command, `rayman`, for code generation, review, tests, workflow reports, multi-agent research, Codex host-subagent auto-start contracts, controller-driven goal dispatch requests, ledger review, documentation governance, model routing, trace/eval/replay, MCP/plugin integration metadata, control-plane status, backups, workspace activation, session continuity, and Chinese/CJK-safe text handling across prompts, docs, CLI output, and generated artifacts.

## Document Index

- [Quickstart](QUICKSTART.md): build, install, configure, and run the first commands.
- [Docs index](docs/README.md): layered navigation by task, subsystem, and validation gate.
- [CLI](docs/CLI.md): every public command and shell-neutral usage pattern.
- [API](docs/API.md): Rust HTTP service endpoints and authentication.
- [Model routing](docs/MODEL_ROUTING.md): provider configuration and fallback behavior.
- [Project understanding](docs/PROJECT_UNDERSTANDING.md): fresh hash-backed context, stale-index handling, and no cross-project memory.
- [Workflows](docs/GOAL_WORKFLOWS.md): goal contracts, execution reports, and quality gates.
- [Configuration](docs/YAML_CONFIG.md): YAML files, state files, and compatibility rules.
- [Testing](docs/TESTING.md): validation commands and repository audit gates.

## Rust Workspace

- `crates/rayman-core`: configuration, model routing, state, backups, workflow contracts, skills, lossless documentation splitting, instruction lifecycle, and audit gates.
- `crates/rayman-api`: `axum` HTTP API with the same request and response field names as the previous public API.
- `crates/rayman-cli`: `rayman` binary built with `clap`.

## Build

```text
cargo build --release
```

The release binary is written to `target/release/rayman.exe` on Windows and `target/release/rayman` on Unix-like systems.

## Common Commands

```text
rayman session status
rayman workspace-skill status
rayman context status
rayman context status --check
rayman context refresh
rayman context os --write
rayman context os --check
rayman context semantic status --check
rayman context task "review the context kernel"
rayman trace status
rayman trace replay
rayman temp status
rayman temp doctor
rayman project detect
rayman impact --path crates/rayman-core/src/context.rs
rayman regression plan --path crates/rayman-core/src/context.rs
rayman regression run --profile auto
rayman regression run --profile parallel-full
rayman regression history --limit 5
rayman gate status --check
rayman coverage status --check
cargo deny check
rayman eval run --profile full
rayman eval dataset run --dataset .RaymanCodingSkill/evals/dataset.json --grader contains
rayman security audit
rayman release evidence --label local --no-write
rayman models status --check
rayman control status --format json
rayman auxiliary target
rayman research start "Investigate validation risk"
rayman research run --id <id>
rayman research status --id <id>
rayman goal run --until blocked
rayman subagent auto-start --task "Review independent validation lanes" --path crates/rayman-core/src/subagent.rs --read-only
rayman subagent dispatch --task "Review independent validation lanes" --path crates/rayman-core/src/subagent.rs --read-only
rayman subagent dispatch --task "Implement isolated lane" --path crates/rayman-core --create-worktree
rayman subagent reconcile
rayman subagent record --goal-id <goal-id> --dispatch-request-id <request-id> --agent-id <agent-id> --task "..."
rayman subagent status
rayman mcp schema
rayman mcp serve --stdio
rayman mcp serve --http
rayman plugin export
rayman workflow status
rayman self status
rayman benchmark run --smoke
rayman agent-skill status
rayman agent-skill sync
rayman route-models --task code_review
rayman generate "Create a small parser" -l rust -o parser.rs
rayman review crates/rayman-core/src/config.rs -l rust
rayman api serve --host 127.0.0.1 --port 8000
rayman audit
```

All canonical commands are single-line commands that work the same way in PowerShell and cmd. Do not use shell-specific continuations, chained command snippets, heredocs, or shell-only environment assignment in shared docs. `rayman agent-skill sync` keeps the installed CLI binary aligned with the canonical release binary as well as updating Codex and Claude Code skill entries.

## Configuration

Runtime configuration stays in `config/*.yaml`. API keys are read from `.env` or the process environment using names declared in `config/default_config.yaml`.

State files remain workspace-local and compatible with existing data:

- `.RaymanCodingSkill/workspace_skill.yaml`
- `.RaymanCodingSkill/pending_work.json`
- `.RaymanCodingSkill/backups/*/index.json`
- `.RaymanCodingSkill/context/index.json`
- `.RaymanCodingSkill/context/state.json`
- `.RaymanCodingSkill/context/events.jsonl`
- `.RaymanCodingSkill/context/semantic/index.jsonl`
- `.RaymanCodingSkill/traces/events.jsonl`
- `.RaymanCodingSkill/evals/reports/*.json`
- `.RaymanCodingSkill/subagents/ledger.json`
- `.RaymanCodingSkill/subagents/dispatches/*.json`
- `.RaymanCodingSkill/models/catalog_cache.json`
- `.RaymanCodingSkill/integrations/plugin-manifest.json`
- `.RaymanCodingSkill/workflows/*.json`
- `.RaymanCodingSkill/research/sessions/*.json`
- `.RaymanCodingSkill/tmp/runs/*/metadata.json`
- `.RaymanCodingSkill/tmp/<cargo-target-cache>/.rustc_info.json`

Runtime temporary work uses the Managed Temp Protocol: `runtime_temp.root` defaults to workspace-local `.RaymanCodingSkill/tmp`, `rayman temp status` reports managed runs and recognized retained `CARGO_TARGET_DIR` caches, and `rayman temp cleanup --completed`, `--stale`, `--all-failed`, or `--cargo-targets` removes only Rayman-recognized entries. Completed managed temp runs and successful Cargo target caches must be cleaned before task/session success; failed runs or failed validation caches require inspection before cleanup. Same-directory atomic temp files are still used when replacing target files.

## Context Kernel And Context OS

RaymanCodingSkill provides a Context Kernel for framework-level context aggregation. It reads workspace activation, pending work, project input files, backup indexes, review blockers, audit findings, asset retirement state, and the workspace-local Context Index from local files.

The Context Index uses current files as the source of truth and stores only workspace-local navigation data: project inputs, file inventory, symbols, manifests, entry points, verification commands, task-relevant evidence, and multi-language project intelligence. Built-in adapters cover Rust, JavaScript/TypeScript, Python, C#, and Go. Cached index records include file hashes and are marked stale when current files differ.

The stronger Context OS form is also workspace-local: `rayman context os --write` derives `.RaymanCodingSkill/context/state.json` and appends `.RaymanCodingSkill/context/events.jsonl` from the current Context Index, goal/session state, asset retirement state, audit findings, and pending work. `rayman context status --check`, `rayman gate status --check`, goal success, and session success fail when this state graph is missing or stale. Optional semantic context is a hash-bound navigation index at `.RaymanCodingSkill/context/semantic/index.jsonl`; stale semantic hits are blockers/navigation signals, not completion evidence. It does not add a daemon, database, vector index service, or cross-project memory.

For project understanding, use the layered protocol in [Project understanding](docs/PROJECT_UNDERSTANDING.md): context index first, task-scoped context second, then reread exact current source files before implementation or review.

## Closed-Loop Delivery

Customer requests are closed only when the goal contract, must requirements, impact evidence, changed files, validation output, docs/config sync, context freshness, and final status agree. Goal success requires explicit `req_id` evidence for every must requirement, for example `req_1: implemented requested behavior and cargo test --all passed`. Release/deploy goals also require a ready `.RaymanCodingSkill/customer_deploy.yaml` before success. Auxiliary AI output and cached summaries are advisory only; they cannot satisfy requirements or replace primary validation against current files and command output.

Research agents add an auditable multi-agent scientist loop. The scientist can request and run only whitelist argv experiments from `config/research_agents.yaml`; it cannot edit files, close goals, approve validation, or claim completion. Experiments must run inside the workspace, and attempts to disable the cwd boundary are rejected. Research sessions persist hypotheses, experiments, reflections, findings, and conflicts. Unresolved research conflicts or policy violations block successful goal/session close.

Trace, eval, and replay state are local JSON/JSONL contracts. `rayman trace record`, `rayman trace status`, `rayman trace replay`, `rayman eval dataset run`, and the fail-closed `trace_eval` gate provide repeatable process evidence, while `rayman workflow learn`, `rayman workflow promote`, and `rayman workflow status` promote only patterns backed by replay, eval, and gate evidence. MCP serve and plugin export commands expose the same evidence-first control plane to external hosts without changing the authority model.

## Validation

```text
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all
cargo deny check
rayman context refresh
rayman context os --check
rayman gate status --check
rayman coverage status --check
cargo run -p rayman-cli --bin rayman -- audit
rayman regression run --profile auto
rayman regression run --profile parallel-full
rayman eval run --profile full
rayman models status --check
rayman control status --format json
rayman security audit
rayman release evidence --label local --no-write
```
