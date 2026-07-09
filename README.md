# RaymanCodingSkill

A lean, evidence-first coding-agent helper. One small Rust binary, `rayman`, that gives an agent (or you) the load-bearing basics for working in a repository:

- **Context index** — a fingerprint-cached map of the workspace (files, kinds, symbols). Refresh reuses unchanged files and only re-hashes what changed.
- **Project map** — a derived architecture view (modules, symbols, local dependencies, Cargo package topology, entrypoints, heuristic test candidates, impact hints). It refuses stale context instead of guessing.
- **Change plan** — aggregate multiple intended change paths into impacted files, package dependents, candidate tests, risks, and validation commands before broad edits.
- **Quality surface** — machine-readable maintainability findings from the project map; `map quality --check` fails only on error-level gaps and keeps warnings reviewable unless a strict quality policy explicitly promotes them.
- **Goal contract** — capture a task as `must`/`should` requirements; closing as success is refused until every `must` has recorded evidence. Pending-work items carry across sessions.
- **Asset scan** — read-only report of obsolete-looking files and work-in-progress markers. It never deletes anything.
- **Managed temp** — workspace-local scratch under `.RaymanCodingSkill/tmp/`, never system temp.

All state is workspace-local under `.RaymanCodingSkill/` (gitignored). Current files and command output are the source of truth; there is no cross-project memory, no LLM calls, and no network surface.

> This is the v2 rewrite. It deliberately drops the previous framework's parallel LLM stack, HTTP API, research agents, and process-governance manifests in favor of the small load-bearing core above. See `SKILL.md` for the agent-facing usage contract.

## Build

```
cargo build --release
```

The binary is written to `target/release/rayman` (`rayman.exe` on Windows). Put it on your `PATH`, or run it through Cargo while developing:

```
cargo run -p rayman -- context status
```

## Commands

```
rayman context refresh          # build/update the index (reuses unchanged files)
rayman context status           # cheap freshness check (stat-only; no rebuild)
rayman map summary              # project structure summary from the current index
rayman map file <path>          # symbols/dependencies/tests/risks for one file
rayman map symbol <name>        # find indexed symbols by name
rayman map topology             # Cargo package/path-dependency topology
rayman map impact <path>        # dependent files, heuristic test candidates, suggested checks
rayman map plan <paths...> [--check] # multi-file change plan; --check blocks unscoped broad edits
rayman map quality [--check]    # maintainability quality findings; --check blocks on errors
rayman check                    # one aggregated readiness gate (context + assets + pending)
rayman check --profile standard # plus project map, quality errors, validation relevance, and goal evidence
rayman check --profile release  # standard plus strict quality policy from .RaymanCodingSkill/quality.json
rayman assets                   # read-only obsolete-file + work-marker scan
rayman temp status | scratch <label> | cleanup

rayman goal start "<title>" --must "<req>" [--should "<req>"]
rayman goal list | show <id>
rayman goal evidence <id> --req <req_id> -m "<file + validation that passed>" --validated "<command that passed>"
rayman goal evidence <id> --req <req_id> -m "<evidence>" --validated "<command that passed>" --changed <path>
rayman goal close <id> [--status success|partial|blocked] # standard READY requires closed success
rayman goal pending add "<title>" -m "<detail>" | list | resolve <id>
```

Every command accepts `--format json` for machine-readable output.

`related_tests` and change-plan checks are heuristic planning aids, not proof of real coverage. `--validated` must record commands that actually ran and passed; `check --profile standard` rejects source-change evidence that only records unrelated text such as "docs reviewed".

Cargo topology includes direct path dependencies and workspace-inherited path dependencies from `[workspace.dependencies]` plus `{ workspace = true }`, including common dotted TOML forms. Impact recommendations use `cargo test -p <name>` only for unique workspace member packages; excluded fixtures, non-workspace nested packages, and duplicate package names use `cargo test --manifest-path <path>` instead. `map plan --check` treats package-level checks as broad-change anchors only when the package has an indexed test target.

Optional strict quality policy lives at `.RaymanCodingSkill/quality.json`:

```
{
  "multi_source_no_test_min_sources": 3,
  "block_warning_kinds": ["public_api_without_test_evidence"]
}
```

The default standard profile ignores this file and keeps warnings non-blocking; strict/release profiles fail closed if the file is malformed, contains unknown fields, or names an unknown `block_warning_kinds` entry.

## Validate

```
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test --all
cargo deny check
cargo deny --manifest-path evals\Cargo.toml check --config evals\deny.toml
```

CI (`.github/workflows/ci.yml`) runs the same on Linux and Windows.

## License

MIT — see [LICENSE](LICENSE).
