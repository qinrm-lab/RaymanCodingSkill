# RaymanCodingSkill

A lean, evidence-first coding-agent helper. One small Rust binary, `rayman`, that gives an agent (or you) the load-bearing basics for working in a repository:

- **Context index** — a content-proven map of the workspace (files, kinds, symbols). `context refresh` hashes indexed content and preserves read failures as blockers; the cheap `context status` command remains a stat-only UI probe, while map and readiness conclusions re-check content hashes.
- **Project map** — a derived architecture view (modules, symbols, local dependencies, Cargo package topology, entrypoints, heuristic test candidates, impact hints). It refuses stale context instead of guessing. Symbol extraction and test-anchor detection are Cargo/Rust-shaped heuristics; on a workspace with no `Cargo.toml`, the map still builds (files, kinds, risks) but the "no test anchor" findings below are advisory, not blocking — see Quality surface.
- **Change plan** — aggregate multiple intended change paths into impacted files, package dependents, candidate tests, risks, and validation commands before broad edits. The "no candidate test anchor" blocker only fires as a hard `plan --check` failure inside a detected Cargo workspace; elsewhere it's a warning.
- **Quality surface** — machine-readable maintainability findings from the project map; `map quality --check` fails only on error-level gaps, and `multi_source_project_without_tests` is only ever error-level inside a detected Cargo workspace (outside one it's a non-blocking warning, since the underlying test-detection heuristics don't understand other languages). Warnings stay non-blocking unless you run `map quality --profile strict --check` (or `check --profile release`), which reads the strict policy from `.RaymanCodingSkill/quality.json`.
- **Goal contract** — capture a task as `must`/`should` requirements; closing as success is refused until every `must` has recorded evidence. Pending-work items carry across sessions.
- **Asset scan** — read-only report of obsolete-looking files and work-in-progress markers. It never deletes anything.
- **State audit** — a read-only inventory of allowed v2 state, retired entries, and recursive managed-temp metrics. `state audit --check` fails on retired state, audit errors, or traversal errors; it never deletes files.
- **Managed temp** — workspace-local scratch under `.RaymanCodingSkill/tmp/`, never system temp. `temp status` reports recursive files, directories, bytes, and traversal errors rather than only the top-level entry count.

Operational runtime/task state is local under `.RaymanCodingSkill/` and normally gitignored; `.RaymanCodingSkill/quality.json` is the intentional shared-policy exception. Checkpoint archives are deliberately different: unless `--dir` is supplied they live in a user-level data directory, not in the repository. Current files and command output are the source of truth; there is no cross-project memory, no LLM calls, and no network surface.

> This is the v2 rewrite. It deliberately drops the previous framework's parallel LLM stack, HTTP API, research agents, and process-governance manifests in favor of the small load-bearing core above. See `SKILL.md` for the agent-facing usage contract.

## Build

```
cargo build --release
```

The binary is written to `target/release/rayman` (`rayman.exe` on Windows). The minimum supported Rust version is **1.88.0** (the workspace declares `rust-version = "1.88"`; current code uses stable let-chains). Run it through Cargo while developing:

```
cargo run -p rayman -- context status
```

## Release identity and installed CLI

`2.1.0` is the first release identity after the stale `2.0.0` contract. A matching version string alone is not proof that an installed executable contains this CLI. Keep these claims separate: `check --profile release` proves workspace strict-quality only; `doctor --check` proves installed binary/PATH/SKILL identity only; release handoff additionally requires a locked fresh-source rebuild. The exact contract and release procedure live in [docs/RELEASE_CONTRACT.md](docs/RELEASE_CONTRACT.md).

Do not assume that `rayman` on `PATH` is current. From the checkout, build the reference artifact, deliberately put it on `PATH` for this shell, and verify it:

```powershell
cargo build --locked --release
$artifactDirectory = Join-Path $PWD 'target/release'
$env:PATH = "$artifactDirectory$([IO.Path]::PathSeparator)$env:PATH"
$artifact = Join-Path $artifactDirectory $(if ($IsWindows) { 'rayman.exe' } else { 'rayman' })
./scripts/verify-release-contract.ps1 -CliPath $artifact -ReferenceCliPath $artifact -SkillPath ./SKILL.md -RequirePath -RequireSourceFresh
```

For an installed executable, pass its resolved path as `-CliPath` and the newly built release artifact as `-ReferenceCliPath`. `-RequireSourceFresh` additionally records a clean Git `HEAD`, makes an isolated locked rebuild, then rechecks that the worktree and `HEAD` have not drifted before comparing all artifact SHA-256 digests. Windows MSVC builds use the repository's `/Brepro` Cargo setting so the comparison remains reproducible across target directories. The script checks the command surface, the installed-identity result from `rayman doctor --check`, and, when supplied, that the deployed canonical skill is byte-for-byte the repository `SKILL.md`. It never installs or replaces an executable.

## Commands

```
rayman context refresh          # rebuild index with content-hash proof; read failures block readiness
rayman context status           # cheap stat-only UI probe; not proof for map/check readiness
rayman map refresh              # rebuild the project map from the current index
rayman map summary              # project structure summary from the current index
rayman map file <path>          # symbols/dependencies/tests/risks for one file
rayman map symbol <name>        # find indexed symbols by name
rayman map topology             # Cargo package/path-dependency topology
rayman map impact <path>        # dependent files, heuristic test candidates, suggested checks
rayman map plan <paths...> [--check] # multi-file change plan; --check blocks unscoped broad edits
rayman map quality [--profile standard|strict] [--check] # maintainability findings; --check blocks
                                # on errors; only --profile strict reads .RaymanCodingSkill/quality.json
rayman check                    # default standard: context + project map/quality + validation relevance + goal evidence
rayman check --profile quick    # base snapshot only: context + assets + pending; not a delivery-evidence claim
rayman check --profile standard # explicit default; same standard readiness gate as `rayman check`
rayman check --profile release  # workspace strict-quality only; not an installed-release/source-fresh claim
rayman assets                   # read-only obsolete-file + work-marker scan
rayman temp status              # recursive files/dirs/bytes plus traversal errors; read-only
rayman temp scratch <label> | cleanup
rayman state audit [--check]    # read-only v2/retired-state + recursive-temp audit; audit/traversal errors fail --check

rayman goal start "<title>" --must "<req>" [--should "<req>"]
rayman goal list | show <id>
rayman goal evidence <id> --req <req_id> -m "<legacy attestation>" --validated "<command claimed to have passed>"
rayman goal evidence <id> --req <req_id> -m "<legacy attestation>" --validated "<command claimed to have passed>" --changed <path>
rayman goal validate <id> --req <req_id> -m "<evidence>" --command "<command to execute>" [--changed <path>]
rayman goal close <id> [--status success|partial|blocked] # only these three; standard READY requires closed success
rayman goal pending add "<title>" -m "<detail>" | list | resolve <id>

rayman checkpoint save | list | status                    # list exposes complete/partial/corrupt status
rayman checkpoint verify [id|latest]                      # read-only v2 manifest/path/per-file hash verification
rayman checkpoint restore [id|latest] --yes               # only a verified complete snapshot; overlays matching files
rayman autosave start | stop | status                     # scheduled auto-snapshots (see tools/README.md)
rayman doctor [--check]                                   # installed binary/PATH/workspace-SKILL identity only
```

Every command accepts `--format json` for machine-readable output.

`related_tests` and change-plan checks are heuristic planning aids, not proof of real coverage. For a current-schema goal, use `goal validate`: it executes the command from the workspace root and stores its zero exit code, output hashes, and before/after workspace fingerprints in a receipt. A nonzero command writes no receipt. Closing `success` requires at least one `must`, with every `must` done and carrying evidence; an evidence-only closure is still not a standard/release delivery claim. `check --profile standard` / `release` additionally require a successful receipt bound to the current workspace. `goal evidence --validated` is retained only as a legacy human attestation and cannot satisfy a current-schema standard/release success claim. A receipt whose after-fingerprint is no longer current is also insufficient.

## State and checkpoint integrity

`rayman state audit` is a read-only hygiene report. It names the v2 state entries it accepts, any retired entries, and the recursive temp footprint. `--check` exits nonzero when retired state, an audit error, or a traversal error exists; it deliberately performs no migration or deletion. Review the report and obtain explicit approval before removing or migrating state.

Checkpoint manifests are v2 integrity records. `checkpoint list` labels every snapshot `complete`, `partial`, or `corrupt`; `checkpoint status` selects only the newest verified `complete` snapshot. A failed save leaves a `partial` forensic snapshot and returns an error instead of replacing or rotating the latest good snapshot. `checkpoint verify` re-checks its schema, paths, file count, sizes, and SHA-256 values without writing the workspace. Restore rejects partial/corrupt snapshots and validates source and destination paths before its first overlay write.

Cargo topology includes direct path dependencies and workspace-inherited path dependencies from `[workspace.dependencies]` plus `{ workspace = true }`, including common dotted TOML forms. Impact recommendations use `cargo test -p <name>` only for unique workspace member packages; excluded fixtures, non-workspace nested packages, and duplicate package names use `cargo test --manifest-path <path>` instead. `map plan --check` treats package-level checks as broad-change anchors only when the package has an indexed test target.

Optional strict quality policy lives at `.RaymanCodingSkill/quality.json` (the one file under `.RaymanCodingSkill/` that is **not** gitignored, so the policy can be committed and shared):

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
cargo fmt --manifest-path evals/Cargo.toml --all --check
cargo clippy --manifest-path evals/Cargo.toml --locked --all-targets --all-features -- -D warnings
cargo test --manifest-path evals/Cargo.toml --locked
cargo deny --manifest-path evals/Cargo.toml check --config evals/deny.toml
```

CI (`.github/workflows/ci.yml`) runs root fmt/clippy/tests on Linux **and** Windows, verifies the declared Rust 1.88.0 MSRV on Linux, and builds plus smoke-tests a release artifact on both platforms. The handoff smoke fails closed unless a clean checkout's isolated locked rebuild is byte-identical to the artifact and `PATH` command, the canonical skill binding is current, and `rayman doctor --check` reports installed identity; tag builds also require `v<crate-version>`. `evals/` runs fmt/clippy/unit tests plus an offline mock CLI smoke on Linux and Windows, and cargo-deny on Linux. The smoke first proves a real backend is rejected without `--unsafe-host-exec`, then verifies the mock report's immutable-run pointer, seed, execution mode, and trial count. It never supplies a real-model credential, runs a real remote backend, or claims that a mock result measures model quality.

## License

MIT — see [LICENSE](LICENSE).
