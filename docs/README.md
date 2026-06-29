# Documentation Index

Start here when looking for RaymanCodingSkill behavior. The docs are layered from operational entry points to subsystem details.

## First Read

- [Quickstart](../QUICKSTART.md): build, configure, and run first commands.
- [CLI](CLI.md): command surface and shell-neutral examples.
- [Testing](TESTING.md): validation and audit gates.
- [Feature Coverage](FEATURE_COVERAGE.md): machine-checkable feature-to-test evidence matrix.

## Core Topics

- [API](API.md): HTTP service, authentication, request and response shapes.
- [Model routing](MODEL_ROUTING.md): provider selection, manual mode, auto mode, and fallback.
- [Project understanding](PROJECT_UNDERSTANDING.md): fresh hash-backed context and stale-memory prevention.
- [Goal workflows](GOAL_WORKFLOWS.md): goal contracts, execution reports, and quality gates.
- [Quality patterns](QUALITY.md): repeated-failure incidents, pattern aggregation, and hard success gates.
- [YAML configuration](YAML_CONFIG.md): configuration files and state compatibility.

## Task Index

- Build the binary: `cargo build --release`
- Check pending work: `rayman session status`
- Inspect auxiliary provider order: `rayman auxiliary target`
- Inspect auxiliary async tasks: `rayman auxiliary status`
- Start a research session: `rayman research start "Investigate validation risk"`
- Advance a research session: `rayman research run --id <id>`
- Mark workspace use: `rayman workspace-skill mark-used -m "explicit raymancodingskill use"`
- Inspect context: `rayman context status`
- Refresh context index: `rayman context refresh`
- Refresh/check Context OS state graph: `rayman context os --write` / `rayman context os --check`
- Retrieve task context: `rayman context task "review the context kernel"`
- Read project-understanding protocol: `docs/PROJECT_UNDERSTANDING.md`
- Inspect managed temp state: `rayman temp status`
- Diagnose managed temp problems: `rayman temp doctor`
- Clean completed managed temp runs before task close: `rayman temp cleanup --completed`
- Clean stale managed temp runs: `rayman temp cleanup --stale`
- Clean successful validation Cargo target caches: `rayman temp cleanup --cargo-targets`
- Detect project languages: `rayman project detect`
- Analyze impact: `rayman impact --path crates/rayman-core/src/context.rs`
- Plan regression tests: `rayman regression plan --path crates/rayman-core/src/context.rs`
- Inspect quality patterns: `rayman quality patterns`
- Run quality gate: `rayman quality gate --goal-id <id>`
- Maintain HTML developer docs and auto-complete customer project docs: `rayman docs maintain --prompt "explain current functionality" --output docs/project-docs.html`
- Check docs drift, customer docs completeness, and obsolete assets: `rayman docs maintain --check`
- Check release binary: `rayman self status`
- Run smoke benchmark: `rayman benchmark run --smoke`
- Sync host skill entries and installed CLI: `rayman agent-skill sync`
- Run the API: `rayman api serve --host 127.0.0.1 --port 8000`
- Audit repository rules: `rayman audit`
