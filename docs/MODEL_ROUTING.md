# Model Routing

Model routing is configured in `config/default_config.yaml`.

## Manual Mode

Manual mode uses each task's `manual` model reference.

```text
rayman route-models --task code_review --route-mode manual
```

## Auto Mode

Auto mode uses `primary` first and then tries each `fallback` model when the request fails.

```text
rayman route-models --task code_generation --route-mode auto
```

## Provider Configuration

Each provider declares authentication and an optional base URL under `models`. Authenticated providers must use `api_key_env` to name a local environment variable; plaintext YAML `api_key` values are blocked by `rayman security audit` and must not be stored in repository configuration.

OpenAI-compatible third-party providers use `adapter: "openai_compatible"` and are called through the same Rust HTTP client path as OpenAI.

Local models use the configured local base URL and do not require an API key.

## Auxiliary AI Advisory

`config/auxiliary_ai.yaml` defines a fail-open advisory channel. `rayman auxiliary advise` remains synchronous and returns concise context, risks, and validation checks directly to the caller. Model-backed operations use the async worker by default: the primary route continues without waiting, a reconciliation task is queued under `.RaymanCodingSkill/auxiliary/tasks/`, and the worker records whether the primary result is correct or needs repair.

Auxiliary AI is not an executor. It may strengthen the primary AI's context, but it must not write files, approve completion, replace tests or validation, or overrule the primary AI. Use `rayman auxiliary target` to inspect the configured target and workspace-data policy without sending a prompt.

The default auxiliary provider is `ai_ubuntu_8888`, an OpenAI-compatible AI-UBUNTU router endpoint at the current `source.preferred_base_url` in `config/auxiliary_ai.yaml`, serving model `auto`. It sets `auth_required: false`, `proxy.mode: env`, and a 120-second timeout, so no API key environment variable is required and slower local advisory responses can still participate through the host proxy or tunnel when direct LAN routing is unavailable. Legacy `auxiliary_ai.provider` and `auxiliary_ai.model` fields are upgraded to the ordered `auxiliary_ai.providers` list on load.

The ordered `auxiliary_ai.providers` list is the provider order. RaymanCodingSkill stores the round-robin cursor in `.RaymanCodingSkill/auxiliary/provider_state.json`, selects the next enabled provider for each auxiliary call, and then tries subsequent providers in YAML order on failure, timeout, or workspace-data authorization skip.

Auxiliary provider proxy defaults to direct when no `proxy` is configured. If a provider has `proxy`, `proxy.mode` is mandatory. `proxy.mode: direct` disables environment proxy use for that call, `proxy.mode: http` requires `url: http://host:port`, and `proxy.mode: env` explicitly reads `HTTP_PROXY`, `HTTPS_PROXY`, and `NO_PROXY`.

The source settings live at `http://ai-ubuntu.local:8001/`. `rayman update-models --force` and `POST /api/models/update?force=true` refresh `config/auxiliary_ai.yaml` from that settings page while preserving the preferred runtime port `8888`; the settings fetch also uses the 120-second auxiliary timeout window.

Non-loopback auxiliary targets are treated as untrusted for workspace-derived prompts unless their provider config explicitly sets `allow_workspace_data: true`. Trusted helpers should set `trust_level` to `trusted_lan` for LAN services or `trusted_remote` for explicitly trusted internet services, and keep `data_policy.advisory_only: true`. Unauthorized non-loopback targets are skipped with `skipped_external_auxiliary_not_authorized` and a recorded reason.

Auxiliary failures are recorded in `last_auxiliary_attempt` but do not fail `generate`, `review`, `test`, `refactor`, `explain`, obsolete-code pruning, or workflow execution. CLI commands print an `辅助AI` status line, and API responses include an `auxiliary_ai` field with `queued_task_id`, `selected_provider`, `provider_attempts`, `async_status`, and `reconciliation_status`. Fail-open means the primary operation may continue after an auxiliary error; it does not permit silently skipping auxiliary collaboration when the auxiliary model was available.

Worker reconciliation writes structured conclusions with `primary_correct`, `correction_required`, `risk_level`, `rationale`, `suggested_fix`, and `tests`. A conflict creates pending work and prevents successful goal close until the primary AI repairs the issue and the conflict is resolved.

With `required_when_available: true` and `record_skip_reason: true`, any available-but-unattempted auxiliary step must record a skip reason and supporting evidence in the CLI/API report, workflow report, or session/pending-work record. Acceptable evidence includes disabled config, unavailable endpoint, task not listed in `tasks`, timeout/error details, or an explicit user constraint.

Generation and implementation-validation flows also record conservative auxiliary correction contribution stats. A contribution is counted only when the auxiliary implementation-validation attempt succeeds and the final validator output actually corrects the primary model result (`status=fixed`, non-empty `fixes_applied`, or changed final code). Usage stats keep recorded auxiliary steps (`attempt_count`) separate from real auxiliary executions (`call_count`) and async enqueue records (`queued_count`), so CLI output shows success ratios against actual auxiliary calls. Planning, workflow-summary, and research tasks are advisory or analysis value and remain visible in usage stats, not correction contribution stats. CLI output shows both the current round and project-total ratios when contribution samples exist, and says there is no implementation-validation correction sample when none exist. Persisted contribution totals live in `.RaymanCodingSkill/auxiliary_contributions.json` after the first contribution event and are exposed by `rayman stats` and `GET /api/stats`.
