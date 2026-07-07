# RaymanCodingSkill

A lean, evidence-first coding-agent helper. One small Rust binary, `rayman`, that gives an agent (or you) the load-bearing basics for working in a repository:

- **Context index** — a fingerprint-cached map of the workspace (files, kinds, symbols). Refresh reuses unchanged files and only re-hashes what changed.
- **Goal contract** — capture a task as `must`/`should` requirements; closing as success is refused until every `must` has recorded evidence. Pending-work items carry across sessions.
- **Asset scan** — read-only report of obsolete-looking files and `TODO`/`FIXME`/`未完成` markers. It never deletes anything.
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
rayman check                    # one aggregated readiness gate (context + assets + pending)
rayman assets                   # read-only obsolete-file + TODO/未完成 scan
rayman temp status | scratch <label> | cleanup

rayman goal start "<title>" --must "<req>" [--should "<req>"]
rayman goal list | show <id>
rayman goal evidence <id> --req <req_id> -m "<file + validation that passed>"
rayman goal close <id> [--status success|partial|blocked]
rayman goal pending add "<title>" -m "<detail>" | list | resolve <id>
```

Every command accepts `--format json` for machine-readable output.

## Validate

```
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test --all
cargo deny check
```

CI (`.github/workflows/ci.yml`) runs the same on Linux and Windows.

## License

MIT — see [LICENSE](LICENSE).
