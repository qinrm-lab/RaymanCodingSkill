# rayman-evals — agent A/B outcome eval

`rayman-evals` measures objective task outcomes for the RaymanCodingSkill under two conditions:

- `with_skill`: the agent receives `SKILL.md` and can find the selected `rayman` binary on `PATH`.
- `control`: neither; PATH entries exposing a `rayman` command (including Windows `.cmd`/`.bat` wrappers) are removed from the tool child process.

Each task has a fixture, prompt, and grade command. Every task × condition × trial gets a fresh copy of the fixture, then the grade command decides pass/fail. `grade.txt` is hidden from the model prompt and file tools; it is **not** confidential from an unrestricted host shell.

## Safety boundary

The `run` tool executes a model-generated shell command directly as the current host user. It is **not a sandbox**: environment scrubbing, working-directory checks, timeouts, and symlink checks do not prevent a command from accessing the host filesystem or network through other means.

Therefore only the offline mock backend runs without an acknowledgement. Every real backend is rejected before config, credentials, or network use unless the operator explicitly passes:

```text
--unsafe-host-exec
```

Use real models only in a disposable VM, Windows Sandbox, container with appropriate host isolation, or another environment you deliberately accept as unsafe. This binary cannot attest that such isolation actually separates the agent from evaluator inputs, so every `unsandboxed_host_execution` report is deliberately marked **NON-COMPARATIVE**: it is useful for debugging, never for comparing arms or claiming an effect. The acknowledgement allows unsafe execution; it does not make the experiment valid.

File tools (`list_files`, `read_file`, `write_file`) reject visible symlink traversal and fixture copying rejects symlinks. This is a portable best-effort boundary, not a substitute for OS isolation; portable Rust has no race-free cross-platform `openat(O_NOFOLLOW)` equivalent.

Before a real-backend trial starts, the selected `rayman` binary must pass its own `doctor --check` with `PATH` pinned to that binary, and the selected `--skill` content must equal the workspace `SKILL.md`. Mock runs intentionally skip this release-metadata check so offline CI does not depend on ignored workspace state.

## Run

Mock backend (offline, free; proves orchestration, provenance, and grading):

```powershell
cargo run --manifest-path evals/Cargo.toml -- --backend mock --trials 2
```

Real Anthropic backend (spends credits and directly executes model shell commands on the host):

```powershell
$env:ANTHROPIC_API_KEY="..."
cargo run --manifest-path evals/Cargo.toml -- --backend anthropic --trials 2 --unsafe-host-exec
```

Use `--seed <u64>` to reproduce task/condition ordering. A generated seed is always recorded if omitted.

## OpenAI-compatible backends

Copy the example to a local, gitignored configuration:

```powershell
Copy-Item evals/backends.example.json evals/backends.json
```

The schema is strict: unknown top-level and backend fields are rejected. `base_url` must use HTTPS, except `http://localhost`, `127.0.0.1`, or `::1` for a local server. URLs with user-info, query, or fragments are rejected.

By default a backend can obtain a key only from an approved, dedicated API-key variable such as `OPENAI_API_KEY`, `DEEPSEEK_API_KEY`, or `OPENROUTER_API_KEY`. Arbitrary environment-variable names are rejected so a config cannot point a remote endpoint at a generic host secret. Keep keys out of JSON:

```json
{
  "backends": {
    "deepseek": {
      "base_url": "https://api.deepseek.com/v1",
      "model": "deepseek-chat",
      "api_key_env": "DEEPSEEK_API_KEY",
      "max_tokens": 8192
    }
  }
}
```

Then run:

```powershell
$env:DEEPSEEK_API_KEY="..."
cargo run --manifest-path evals/Cargo.toml -- --backend deepseek --trials 2 --unsafe-host-exec
```

`api_key` inline JSON values are blocked by default. They require the additional explicit acknowledgement `--unsafe-inline-api-key` as well as `--unsafe-host-exec`; prefer `api_key_env` instead. Keyless loopback endpoints are supported.

Supported `wire` values are `openai`/`chat` (the default, `/chat/completions`) and `responses` (`/responses`).

## Immutable artifacts and provenance

Each invocation creates a unique directory:

```text
evals/.runs/run-<timestamp>-p<pid>-<counter>/
  report.md
  report.json
  workspaces/<task>__<condition>__t<trial>/
```

Existing trial directories and run artifacts are never deleted or reused. `evals/.runs/latest.json` is a mutable JSON pointer plus summary, and `latest.md` is the matching human summary. They point to immutable reports rather than using a cross-platform symlink.

`report.json` records the run id, evaluator version, Git HEAD when available, backend, execution mode, OS/architecture, seed/order strategy, release-contract status, SHA-256 values for the selected skill and `rayman` binary, and prompt/grade/fixture/task hashes for every task. The evaluator rechecks those inputs before and after every trial. Any missing/read-error/mismatch stops later trials, records the drift, and marks the entire run non-comparative rather than presenting stale provenance as current.

## Reading results

The report shows both:

- **ITT rate**: `pass / planned`, where infrastructure errors remain in the denominator.
- **Evaluable rate**: `pass / (pass + fail)`, with errors reported separately.

It also reports both observed deltas. A run with fewer than two attempts per cell, missing observations, insufficient evaluable attempts, unequal error counts, or input drift is marked **INCONCLUSIVE** or **NON-COMPARATIVE** and explicitly forbids a causal claim. Even a balanced mock run is only eligible for descriptive comparison, not proof of causality; a real unsandboxed-host run is always non-comparative.

Condition order is counterbalanced deterministically: each task alternates which arm runs first; the seed chooses the task's initial arm. This and the order of each trial are stored in the report.

## Adding a task

Create `tasks/<name>/` with:

- `fixture/`: a small, self-contained starting repository with no symlinks.
- `prompt.md`: instruction shown to the agent.
- `grade.txt`: shell command where exit 0 means success. It is not sent through the prompt/file tools, but must not be treated as secret unless an OS-level sandbox separates the agent from `tasks/`.

Keep fixture builds offline and grades deterministic. A normal agent prompt does not include `grade.txt`; an unrestricted host-shell agent can still reach it, which is why real-host reports are fail-closed non-comparative.
