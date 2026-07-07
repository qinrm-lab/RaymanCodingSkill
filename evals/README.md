# rayman-evals — agent A/B outcome eval

Measures whether the RaymanCodingSkill actually improves a coding agent's **output**, not just its process compliance. It runs a real LLM agent on the same coding tasks under two conditions and compares objective pass rates:

- **with_skill** — the agent's system prompt includes the repo's `SKILL.md` and it is told `rayman` is on PATH.
- **control** — neither.

The **only** difference between the two arms is the skill text + tool availability, so any pass-rate delta is attributable to the skill.

This is a standalone project — **not** part of the main `rayman` workspace (see `exclude = ["evals"]` in the root `Cargo.toml`), so its LLM/HTTP dependencies never touch the shipped tool.

## How it works

1. Each task under `tasks/<name>/` has a `fixture/` (a starting repo), a `prompt.md` (the instruction given to the agent), and a `grade.txt` (a hidden command run in the finished workspace — exit 0 = success). The agent never sees `grade.txt`.
2. For each task × condition × trial, the harness copies the fixture to a fresh workspace, runs a minimal tool-use agent loop (tools: `list_files`, `read_file`, `write_file`, `run`) against the chosen backend, then runs the hidden grade command.
3. Results aggregate into per-task and overall pass rates and the with-skill-minus-control **delta**, written to `.runs/report.md` and `.runs/report.json`.

Grading is objective: every shipped task grades with `cargo test` or `cargo clippy -- -D warnings`. The tasks range from one-line fixes to `large-repo-nav`, where the failing top-level test is caused by a bug buried several modules deep — the case where navigation/context tooling should earn its keep.

## Run it

Mock backend (free — proves the orchestration and grading end-to-end; the no-op agent scores 0% in both arms):

```
cargo run -- --backend mock
```

Anthropic backend (needs `ANTHROPIC_API_KEY`; **spends API credits**):

```
export ANTHROPIC_API_KEY=sk-ant-...      # PowerShell: $env:ANTHROPIC_API_KEY="sk-ant-..."
cargo run -- --backend anthropic --trials 3
```

### OpenAI-compatible backend (DeepSeek / local Ollama / any OpenAI-compatible endpoint)

Endpoints and models change often, so they live in a **gitignored config file** — edit the file, not the code. The secret stays in an env var.

1. Copy the example to your real config (gitignored):

   ```
   cp evals/backends.example.json evals/backends.json
   ```

2. Edit `evals/backends.json` — each top-level key is a `--backend` name; set its `base_url`, `model`, and `api_key_env` (the name of the env var that holds the key; omit it for a keyless local endpoint):

   ```json
   {
     "backends": {
       "deepseek": {
         "base_url": "https://api.deepseek.com/v1",
         "model": "deepseek-chat",
         "api_key_env": "DEEPSEEK_API_KEY"
       }
     }
   }
   ```

3. Set the secret in the env var you named:

   ```
   $env:DEEPSEEK_API_KEY="sk-..."      # bash: export DEEPSEEK_API_KEY=sk-...
   ```

4. Run it:

   ```
   cargo run -- --backend deepseek --trials 3
   ```

Config file path is `evals/backends.json` by default; override with `--backends <path>`. Each entry also accepts optional `wire` (`"chat"`, the default, for OpenAI chat/completions endpoints; `"responses"` for the OpenAI *Responses API*, which Codex-style relays stream over SSE), an inline `api_key` (takes priority over `api_key_env` — convenient for a private relay), and `max_tokens` (output cap; keep it low for small models). See `backends.example.json` for one entry per wire.

### Flags

`--backend <name>` (`mock` | `anthropic` | any name from `backends.json`), `--task <name>` (one task only, e.g. a cheap smoke), `--model <id>` (override the backend's default model), `--trials N`, `--max-steps N`, `--backends <path>`, `--runs-dir <dir>`.

A cheap real smoke: `cargo run -- --backend deepseek --task fix-failing-test` (2 agent runs).

## Adding a task

Create `tasks/<name>/` with:

- `fixture/` — the starting repo state.
- `prompt.md` — what the agent is asked to do (do not reveal the grader).
- `grade.txt` — one shell command; exit 0 means the task was completed correctly.

Keep fixtures small and dependency-free so grading is fast and offline.

## Caveats

- The `run` tool executes model-generated shell commands inside the per-trial workspace copy. That's inherent to any coding agent; run evals on a machine where that's acceptable.
- The harness re-hashes nothing and caches nothing between trials — each trial is an independent fresh workspace.
