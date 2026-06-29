# YAML Configuration

RaymanCodingSkill keeps runtime and governance configuration in `config/*.yaml`. Runtime YAML loading, saving, parsing, and serialization go through the `rayman_core::yaml` adapter so parser behavior is centralized while the internal value type remains compatible with existing `serde_yaml::Value` state. A YAML key is a runtime switch only when Rust code explicitly reads it; otherwise it is governance/reference metadata for docs, audits, prompts, or future implementation planning.

## Main File

`config/default_config.yaml` contains:

- Runtime-consumed: referenced config files, `default_model.type/name`, `model_routing.enabled`, `model_routing.mode`, `model_routing.fallback_on_failure`, `model_routing.routes.<task>.manual/primary/fallback`, primary `models.<provider>` connection fields, and `runtime_temp.*`. Other `model_routing` keys are governance/reference unless Rust code explicitly reads them.
- Governance/reference unless a code path is added: `skills.*.enabled`, `backup_management.*`, `session_continuity.*`, `logging.*`, and `language_preference`.

`config/auxiliary_ai.yaml` contains the local advisory model settings. It keeps the AI-UBUNTU settings URL, preferred runtime port, async worker switch, fail-open advisory switch, required-when-available switch, skip-reason recording switch, task list, and auxiliary provider runtime settings separate from the main routing configuration. Edit auxiliary AI settings in this file only; `models.yaml` is reserved for the primary model catalog.

Auxiliary providers are declared in `auxiliary_ai.providers` as an ordered YAML list. Manual file order is runtime order: each auxiliary call reserves the next enabled provider through persistent round-robin state, then fails over through the rest of the ordered list when a provider fails, times out, or is not authorized for workspace data. Legacy single-provider fields `auxiliary_ai.provider` and `auxiliary_ai.model` are upgraded on load to a one-item `auxiliary_ai.providers` list and then removed from the file.

`auxiliary_ai.default_timeout` defaults to 120 seconds, and provider-level `models.<provider>.timeout` explicitly overrides it for that auxiliary provider. Proxy defaults to direct when `models.<provider>.proxy` is absent. If `proxy` is present it must declare `mode`: `direct` for explicit direct connection, `http` with `url: http://host:port`, or `env` to read `HTTP_PROXY`, `HTTPS_PROXY`, and `NO_PROXY`. The canonical `ai_ubuntu_8888` auxiliary provider must set `proxy.mode: env` so host proxy or tunnel configuration can reach the LAN router when direct `192.168.15.204:8888` routing is unavailable.

`fail_open: true` allows the primary operation to continue when auxiliary AI was attempted and failed. `required_when_available: true` means non-trivial programming tasks must try auxiliary AI when it is enabled, configured, and reachable. `record_skip_reason: true` means any available-but-unattempted auxiliary step must leave a reason and evidence in the user-visible report or persisted workflow/session state.

Auxiliary AI is advisory-only. It may receive workspace-derived prompts only when the configured provider is loopback or explicitly sets `allow_workspace_data: true`. Trusted local/LAN/remote helpers should set `trust_level` to `local_loopback`, `trusted_lan`, or `trusted_remote` as appropriate and keep `data_policy.advisory_only: true`; auxiliary output must never execute, edit, approve, replace validation, or overrule the primary AI.

Authenticated providers must use `api_key_env` for an environment variable name. Plaintext YAML `api_key` values are treated as LLM security blockers by `rayman security audit`; real secrets must stay outside repository files, reports, logs, screenshots, and uploaded artifacts.

`config/research_agents.yaml` contains multi-agent research and scientist experiment policy. `max_parallel_agents` limits concurrent role findings for `planner`, `scientist`, `critic`, `reflector`, `arbiter`, and `safety_monitor`; findings are merged in fixed role order even when work finishes out of order. The scientist role may run experiments only when `can_run_experiments: true`; `can_edit_files` and `can_close_goals` must remain false. `command_policy.allowed` is an argv whitelist, not shell text. Research commands run inside the workspace, reject shell operators, obey the runtime timeout cap, and diff-check protected source/docs/config files before and after execution. Default Scientist experiments exclude full-suite validation such as `cargo test --all`; those commands stay on the primary validation path. `require_workspace_cwd` must remain true; execution still rejects outside-workspace cwd and security audit blocks the unsafe setting.

## Model Catalog

`config/models.yaml` lists provider model metadata used by `rayman list-models`, API model catalog responses, provider `base_url` fallback, and `rayman check-models` route-reference validation. Every `default_model` and `model_routing.routes.*.{manual,primary,fallback}` reference must point to a provider/model entry in this catalog.

## Runtime Consumers

The Rust runtime currently consumes these YAML files directly:

- `default_config.yaml`: config file references, default model, `model_routing.enabled`, `model_routing.mode`, `model_routing.fallback_on_failure`, `model_routing.routes.<task>.manual/primary/fallback`, primary provider connection fields, and `runtime_temp.*`. `model_routing.switch_cooldown_seconds` is currently governance/reference metadata; it is not consumed by the route selector.
- `models.yaml`: model catalog/status metadata, provider base URL fallback, and `check-models` route-reference validation.
- `model_updates.yaml`: `auto_update.enabled`, `auto_update.interval_days`, `last_update`, and `update_sources` for status/check metadata and auxiliary settings refresh gating. It does not automatically mutate the model catalog.
- `auxiliary_ai.yaml`: auxiliary source settings, provider order, timeout/proxy settings, fail-open behavior, required-when-available behavior, skip-reason recording, task list, trust/workspace-data policy, and provider runtime settings.
- `research_agents.yaml`: research enablement, role policy, `max_parallel_agents`, scientist authority, argv whitelist, shell-operator rejection, cwd policy, runtime timeout cap, and protected diff checks. `allow_network` is loaded as policy metadata; it is not currently an execution-layer network blocker.

Other YAML files such as `prompts.yaml`, `skills.yaml`, `testing.yaml`, `conflicts.yaml`, and `performance.yaml` are governance/reference configuration unless a code path explicitly reads a value from them. `performance.yaml` may document the host-subagent speed strategy, but the executable dispatch surfaces are `rayman subagent auto-start` and `rayman subagent plan`, not YAML hard switches. Documentation must not describe those keys as runtime hard-fail switches without an implementation anchor and a negative test anchor in `config/feature_coverage.yaml`.

## Workspace State Compatibility

Rust code reads and writes existing workspace-local state files:

- `.RaymanCodingSkill/workspace_skill.yaml`
- `.RaymanCodingSkill/pending_work.json`
- `.RaymanCodingSkill/backups/*/index.json`
- `.RaymanCodingSkill/context/index.json`
- `.RaymanCodingSkill/project/index.json`
- `.RaymanCodingSkill/subagents/ledger.json`
- `.RaymanCodingSkill/auxiliary/provider_state.json`
- `.RaymanCodingSkill/auxiliary/tasks/*.json`
- `.RaymanCodingSkill/research/sessions/*.json`
- `.RaymanCodingSkill/tmp/runs/*/metadata.json`

Unknown YAML and JSON fields should be preserved where practical so existing workspaces do not need manual migration.

## Runtime Temp

`runtime_temp.root` defaults to `.RaymanCodingSkill/tmp`. Runtime work should use this workspace-local managed temp root instead of system temp or cross-project locations. `runtime_temp.retention_hours` controls stale-run cleanup eligibility, and `runtime_temp.preserve_failed` keeps failed runs until they are inspected or removed with `rayman temp cleanup --all-failed`. Successful validation caches created with an explicit `CARGO_TARGET_DIR` under the temp root are reported by `rayman temp status` when they look like Cargo target directories and are removed only with `rayman temp cleanup --cargo-targets`.

Atomic replacement temp files are still created next to the target file so rename stays on the same volume. These files use Rayman-managed unique names and are removed on failure where possible.

## Context Kernel

The Context Kernel is an aggregation layer over existing workspace-local files. It can write `.RaymanCodingSkill/context/index.json` when `rayman context refresh` is run. That file is a workspace-local navigation index with file hashes, project inputs, file inventory, symbols, manifests, entry points, verification commands, and task context hints.

`rayman project index` writes `.RaymanCodingSkill/project/index.json` with multi-language adapter output for Rust, JavaScript/TypeScript, Python, C#, and Go. Adapter output is copied into the Context Index so impact and regression planning can stay workspace-local and hash-backed.

Current files are authoritative. If the cached index and current file hashes differ, context reports the index as stale; agents must refresh or reread source files instead of treating cached summaries as facts. `rayman context refresh` also rewrites `.RaymanCodingSkill/context/state.json` and appends `.RaymanCodingSkill/context/events.jsonl`, the workspace-local Context OS state graph. `rayman context status --check` exits non-zero when the Context Index or Context OS state is stale, and `rayman gate status --check` treats stale context as a hard blocker alongside docs, coverage, temp, dependency policy, security, audit, and release-evidence checks. `rayman context task` remains read-only and never refreshes the index implicitly. The Context Kernel does not introduce a daemon, database, vector index, or cross-project memory.

The project-understanding protocol is documented in [Project understanding](PROJECT_UNDERSTANDING.md). Opted-in customer workspaces use workspace-local `.RaymanCodingSkill/` state and must not rely on cross-project memory or cached summaries as completion evidence.
