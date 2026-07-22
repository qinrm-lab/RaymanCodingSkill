# RaymanCodingSkill

A lean, owner-minded and evidence-first coding-agent helper. One small Rust binary, `rayman`, gives an agent (or you) the load-bearing basics for driving safe local work to a stable finish:

- **Context index** — a content-proven map of the workspace (files, kinds, symbols). `context refresh` hashes indexed content and preserves read failures as blockers; the cheap `context status` command remains a stat-only UI probe, while map and readiness conclusions re-check content hashes.
- **Explicit activation** — `.RaymanCodingSkill/` by itself is only runtime state. `workspace activate` writes a canonical-skill path/SHA256 plus the exact CLI contract/version; orphan state, skill drift, and stale CLIs are inactive. The six-field activation schema rejects duplicates, unknown fields, nesting, and malformed scalars.
- **Project map** — a derived architecture view (modules, symbols, local dependencies, Cargo/pyproject packages, entrypoints, heuristic test candidates, impact hints). Rust modules/tests and Python imports plus pytest filename conventions are modeled; unsupported ecosystems remain advisory for missing-test conclusions.
- **Owner Mode** — the agent keeps working through locally knowable implementation, repair, risk checks, and re-audit. Frontier reports separate execution from consultation: a deferred question stays out of transient progress output, a presented question pauses foreground execution, and background continuation requires an explicitly authorized isolated mechanism.
- **Change plan** — capture the workspace's per-file SHA256 baseline at goal start, persist one immutable aggregate path set before mutation, and compare it with the real delta. A hash-chained extension may widen the plan only before new paths change; it cannot shrink scope or review priority. Missing baselines, split/post-hoc plans, unplanned files, and incomplete validation declarations are blocked.
- **Quality surface** — machine-readable maintainability findings from the project map; `map quality --check` fails only on error-level gaps. Multi-source Cargo and pyproject packages without indexed tests are blocking; unsupported ecosystems stay advisory. Strict/release always promote the built-in `large_file` and `high_fan_in` warnings. `.RaymanCodingSkill/quality.json` can only add blocking kinds or declare exact `(path, kind)` exemptions with a non-empty reason; it cannot clear those defaults.
- **Goal contract** — capture `must`/`should` requirements, a baseline-bound plan, validation receipts that cover the real delta, repeated-stable authority proof, and fingerprint-bound review for high-priority changes. Pending work records agent/human/external ownership and carries across sessions.
- **Asset scan** — read-only report of obsolete-looking files and work-in-progress markers. It never deletes anything.
- **State audit** — a read-only inventory of allowed v2 state, retired entries, and recursive managed-temp metrics. `state audit --check` fails on retired state, audit errors, or traversal errors; it never deletes files.
- **Managed temp** — workspace-local scratch under `.RaymanCodingSkill/tmp/`, never system temp. `temp status` reports recursive files, directories, bytes, and traversal errors rather than only the top-level entry count.

Operational runtime/task state is local under `.RaymanCodingSkill/` and normally gitignored; `.RaymanCodingSkill/quality.json` is the one exact shared-policy exception. Its bytes are indexed and included in workspace fingerprints, so changing policy makes context/validation evidence stale exactly like changing source. Checkpoint archives are deliberately different: unless `--dir` is supplied they live in a user-level data directory, not in the repository. Current files and command output are the source of truth; there is no cross-project memory, no LLM calls, and no network surface.

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

`2.5.0` binds source-honest lifecycle-only replacement authority to `rayman-cli-contract-v9` and the exact running CLI version, so a v8 executable cannot interpret or report READY against the v9 proof contract. Keep these claims separate: `check --profile release` proves workspace strict-quality only; `doctor --check` proves installed binary/PATH/SKILL/activation identity only; release handoff additionally requires a locked fresh-source rebuild. The exact contract and release procedure live in [docs/RELEASE_CONTRACT.md](docs/RELEASE_CONTRACT.md).

The only supported source-checkout install/upgrade entry point is below. Open a PowerShell 7 (`pwsh`) session first—never Windows PowerShell. A historical profile shim that searches for `.Rayman\rayman.ps1` can loop forever at a drive root; inspect and migrate only that exact known shim before installation:

```powershell
./scripts/repair-rayman-powershell-profile.ps1 -Check
./scripts/repair-rayman-powershell-profile.ps1 -Yes # only when -Check reports the exact legacy shim
./scripts/install-rayman.ps1 -Yes -AddToUserPath
```

The repair tool parses the profile, refuses custom `rayman` functions, preserves unrelated profile content, and uses a staged replacement with rollback. Do not run unbounded output-capturing probes such as `$output = rayman --version` while a profile shadow is unresolved; use `pwsh -NoProfile` and the direct application path for diagnosis.

It requires clean Git source, pins the preverified artifact/skill hashes, rechecks them before/after every copy, and replaces only the managed CLI and canonical skill through same-directory staging. On Windows, `-AddToUserPath` moves the managed bin directory to the front of the user segment inside the same transaction, then proves the exact future persistent ordering (`Machine PATH + proposed User PATH`) without an artificial process-only prepend; an older machine-level `rayman` therefore blocks installation. Without `-AddToUserPath`, the current effective PATH must already resolve the destination first, and non-Windows callers must configure PATH themselves because that switch fails explicitly. Any staging, backup, file, PATH-persistence, or verification failure attempts all file and PATH restorations and reports retained evidence. Verification is the commit point; later backup-cleanup failure only retains evidence and never deletes the committed install. Override destinations only for intentional managed roots. Manual copying and `cargo install` alone are not supported handoff procedures.

Do not assume that `rayman` on `PATH` is current. To inspect a built artifact without installing it, deliberately put it on `PATH` for this shell and verify it:

```powershell
cargo build --locked --release
$artifactDirectory = Join-Path $PWD 'target/release'
$env:PATH = "$artifactDirectory$([IO.Path]::PathSeparator)$env:PATH"
$artifact = Join-Path $artifactDirectory $(if ($IsWindows) { 'rayman.exe' } else { 'rayman' })
./scripts/verify-release-contract.ps1 -CliPath $artifact -ReferenceCliPath $artifact -SkillPath ./SKILL.md -RequirePath -RequireSourceFresh
```

For an installed executable, pass its resolved path as `-CliPath` and the newly built release artifact as `-ReferenceCliPath`. `-SkillPath` is mandatory. `-RequirePath` checks PowerShell's effective command and rejects an Alias/Function shadow before comparing bytes. Native Cargo/Git/rustc calls are resolved as real applications, pinned by path and hash, and rechecked before success. `-RequireSourceFresh` rejects known ambient build-shaping variables, records clean `HEAD` and active compiler identity, makes an isolated locked rebuild, then terminally re-resolves/re-hashes every CLI/reference/skill/PATH path. `-VerifyGitTag` always reads the exact tag from Git and, when GitHub context exists, cross-checks ref type/name/full ref/SHA rather than trusting environment variables. User-level or parent Cargo configuration (including `CARGO_HOME/config.toml`) is not isolated or provenance-checked and still participates in both builds. The result is byte identity for the current source + active rustc + active Cargo configuration context, not repository-default/hermetic toolchain provenance. Windows MSVC builds use `/Brepro`. The verifier never installs or replaces an executable.

## Commands

```
rayman workspace status         # activation only: active/inactive/orphan/invalid
rayman workspace inspect        # activation plus Git HEAD/clean/dirty/untracked source state
rayman workspace activate --skill-file <canonical-SKILL.md> --yes
rayman workspace deactivate --yes
rayman context refresh          # rebuild index with content-hash proof; read failures block readiness
rayman context status           # cheap stat-only UI probe; not proof for map/check readiness
rayman map refresh              # rebuild the project map from the current index
rayman map summary              # project structure summary from the current index
rayman map file <path>          # symbols/dependencies/tests/risks for one file
rayman map symbol <name>        # find indexed symbols by name
rayman map topology             # Cargo package/path-dependency topology
rayman map impact <file>        # one file only; directories fail with a map-plan migration
rayman map plan <paths...> [--check] # multi-file change plan; --check blocks unscoped broad edits
rayman map quality [--profile standard|strict] [--check] # findings retain severity and report source roles
rayman check                    # workspace health only unless a goal is explicitly required
rayman check --goal <id>        # workspace_ready + task_ready bound to one exact goal
rayman check --require-current-goal # require exactly one current goal
rayman check --refresh-context  # refresh then check sequentially in the same process
rayman check --profile quick    # base snapshot only: context + assets + pending; not a delivery-evidence claim
rayman check --profile standard # explicit default workspace readiness gate
rayman check --profile release  # workspace strict-quality only; not installed-release/source-fresh proof
rayman prepare --goal <id>      # refresh context, verify current active goal, report source state
rayman finish --goal <id>       # require stable authority proof, refresh, then run the goal-bound gate
rayman assets                   # read-only obsolete-file + work-marker scan
rayman temp status              # recursive files/dirs/bytes plus traversal errors; read-only
rayman temp scratch <label> | cleanup
rayman state audit [--check]    # read-only v2/retired-state + recursive-temp audit; audit/traversal errors fail --check

rayman goal start "<title>" --must "<req>" [--should "<req>"]
rayman goal list | show <id>
rayman goal plan <id> <paths...> --check [--extend]
rayman goal review <id> --reviewer <name> -m "<review>"
rayman goal evidence <id> --req <req_id> -m "<legacy attestation>" --validated "<command claimed to have passed>"
rayman goal evidence <id> --req <req_id> -m "<legacy attestation>" --validated "<command claimed to have passed>" --changed <path>
rayman goal validate <id> --req <req_id> -m "<evidence>" --command "<command to execute>" [--changed <path>] [--authority --repeat 2]
rayman goal close <id> [--status success|partial|blocked] # only these three; standard READY requires closed success
rayman goal archive <id> --reason "<historical reason>"
rayman goal archive <id> --reason "<historical reason>" --migrate-receipt-policy receipt_integrity_v1
rayman goal archive <id> --reason "<legacy quarantine reason>" --quarantine-invalid-history
rayman goal authorize-replacement <replacement> --supersedes <old>... --authority-from <archived-success>
rayman goal supersede <id> --by <current gate-ready replacement>
rayman goal current [<id>]                                # list current goals; with an id, restore an archived/superseded goal to current
rayman goal frontier <id>       # legacy decision plus orthogonal execution / consultation
rayman goal pending add "<title>" -m "<detail>" [--goal <id>] [--owner agent|human|external] [solution-package fields] [--consultation-timing deferred|immediate] [background proof]
rayman goal pending list | resolve <id>

rayman checkpoint save | list | status                    # list exposes complete/partial/corrupt status
rayman checkpoint verify [id|latest]                      # read-only v3 manifest/path/per-file hash verification
rayman checkpoint restore [id|latest] --yes               # only a verified complete snapshot; journaled all-or-nothing
rayman autosave start | stop | status                     # scheduled auto-snapshots (see tools/README.md)
rayman doctor [--check]                                   # installed binary/PATH/workspace-SKILL identity only
```

Every command accepts `--format json` for machine-readable output.

`goal start` records a per-file SHA256 baseline. A baseline-less current v2 goal is never gate-ready; preserve completed history with `archive`, or replace unfinished work with a new baseline-bound goal and `supersede`. For multi-file work, `goal plan` must be written while the workspace still matches that baseline. `--extend` keeps the base receipt immutable and appends a cumulative hash-chained snapshot only when prior delta is already planned and newly added paths still equal the baseline. Paths/checks can only widen and review priority can only rise. Actual additions, edits, and deletions are recomputed from the baseline, so validation and close reject unplanned delta. A high-priority effective plan also needs `goal review` bound to the final source fingerprint.
`check` without a goal remains a workspace-health result and reports `workspace_ready`; it must not be presented as proof that a user task is complete. `check --goal <id>` additionally reports task evidence readiness. `finish --goal <id>` is stricter: it first requires an authority validation that ran the same project gate at least twice without changing the final workspace fingerprint, then refreshes and checks the exact closed goal. Every check also reports Git/source state when available.

Owner Mode never treats a plain pending string as permission to stop. `goal frontier` preserves legacy `decision` while adding `execution=continue_foreground|continue_background|paused_for_user|wait_external|complete` and `consultation=none|deferred|presented`. `continue_foreground + presented` is never produced: a presented question is a stable final foreground handoff, not commentary. `continue_background + presented` requires an immediate human consultation with a non-empty mechanism, `--background-authority-evidence`, and `--background-isolation-evidence`; partial or blank proof fails closed, and self-asserted booleans are not accepted. Human/external entries require attempts, evidence paths, minimum input, a recommendation, alternatives, risk, a resume command, and an auto-resume condition. `goal close --status blocked` still refuses agent-owned work and incomplete solution packages.


`related_tests` and change-plan checks are planning aids, not proof of real coverage. For a current-schema goal, use `goal validate`: it executes the command from the workspace root and stores its zero exit code, output hashes, before/after fingerprints, and declared changed paths. The current receipts must collectively cover the real delta. `--authority --repeat 2` accepts only a reviewed conventional repository gate (`check-repo`, `audit-repository`, or `verify-release-contract`), workspace-wide Cargo tests, or selector-free workspace pytest; it repeats that direct command on the exact same workspace identity and writes no receipt if any run fails or mutates indexed content. Pytest validation (`python -m pytest`, `pytest`, or `py.test`) parses positional directory/file/node selectors separately from option values, including pytest-xdist values such as `-n 4` and `--dist loadscope`, so parallel options stay workspace-wide while `pytest tests` is scoped and `file.py::test_name` covers its source file. It first runs an independent `--collect-only -q` proof, rejects zero tests and collect-only-as-execution, reads one terminal summary instead of arbitrary test output, requires `passed > 0`, and requires passed/skipped/xfailed/xpassed totals to match collection. A nonzero command writes no receipt. Pytest is recognized only when `-m pytest` sits in interpreter-option position: `python -c "<code>" -m pytest` is an arbitrary-code host, not a test run, and is rejected the same way `sh -c` is. `goal evidence --validated` remains legacy human attestation and cannot satisfy a current-schema standard/release claim.
Retired v1 spellings fail nonzero with migration guidance rather than silently changing meaning: `audit` maps to separate workspace/task/state commands, `workspace-skill` maps to `workspace`, and `context os`, `context task`, and `subagent` name their v2 replacements. They are diagnostic traps, not compatibility aliases.


Archiving or superseding a completed goal revalidates its receipt integrity at the fingerprint where that work actually passed; later source changes do not make historical proof current. Lifecycle proofs bind the receipt-policy version into their contract hash, so a later classifier upgrade cannot silently reinterpret valid history or be downgraded by editing JSON. Proofs written before policy versioning are checked with the exact legacy v1 integrity rules. A pre-policy-v2 current or archived success may be explicitly migrated with `--migrate-receipt-policy receipt_integrity_v1`, but only when it predates that rollout and every real v1 receipt still passes fingerprint, cwd, command/scope, contract, and output-hash checks. This option cannot repair missing receipts, post-rollout work, an incomplete requirement, or a goal whose immutable plan omitted real changes. A second, wider hatch exists for records that predate receipts entirely: `--migrate-unreceipted` archives a pre-rollout success whose requirements carry evidence and validations but no receipts at all, and the proof it writes is thereafter accepted without a receipt recheck. It is limited to goals whose `created_at` precedes the rollout, so no goal created by `goal start` today can reach it; use it only to file genuine pre-receipt history, and prefer `archive` without it whenever real receipts exist. Supersession requires a separate current, gate-ready replacement at the moment it is recorded; if that replacement is archived afterwards, its own lifecycle proof keeps the supersession valid.

The narrow zero-delta recovery is `goal authorize-replacement`. It does not treat a code gate as non-code and does not mint synthetic validation receipts. Instead, it requires a pristine replacement whose must texts are the exact normalized multiset from all named unfinished predecessors, then binds that contract to a non-migrated current-policy archived success with a direct repeated authority receipt at the same canonical workspace identity and source fingerprint. The persisted proof also binds each predecessor ID and immutable transfer contract, so unrelated goals cannot reuse it. Source drift, copied state, quarantine, legacy policy, indirect lifecycle authority, extra or missing musts, or any ordinary delta rejects the operation.

## State and checkpoint integrity

`rayman state audit` is a read-only hygiene report. It names the v2 state entries it accepts, any retired entries, and the recursive temp footprint. `--check` exits nonzero when retired state, an audit error, or a traversal error exists; it deliberately performs no migration or deletion. Review the report and obtain explicit approval before removing or migrating state.

Checkpoint manifests are v3 integrity records; v2 manifests are rejected rather than reinterpreted. Saves take a cross-process workspace/store lock; a failed or crashed save keeps its staging/partial directory for forensics and automatic pruning never deletes staging; partial snapshots are kept for forensics but rotate at a fixed cap so a repeatedly failing autosave cannot grow them without bound. `checkpoint list` labels complete/partial/corrupt snapshots and `checkpoint status` selects only the newest verified complete one. `checkpoint verify` is read-only. Restore validates the complete manifest first, then durably replaces each destination file through same-directory staging (`flush`/`fsync` + atomic rename). It is a journaled all-or-nothing transaction: every source is staged and verified and every existing destination is backed up before the first file is published, and a failure during publication restores the backups in reverse order. A crash leaves a recorded transaction that the next `save`/`restore` resolves; it never deletes extra workspace files.

Cargo topology includes direct path dependencies and workspace-inherited path dependencies from `[workspace.dependencies]` plus `{ workspace = true }`, including common dotted TOML forms. Impact recommendations use `cargo test -p <name>` only for unique workspace member packages; excluded fixtures, non-workspace nested packages, and duplicate package names use `cargo test --manifest-path <path>` instead. Pyproject packages use `python -m pytest` (scoped to the package root when nested), and Python impact resolves local `import`/`from ... import ...` edges from the nearest nested pyproject root (including its `src/` layout) plus `test_*.py`/`*_test.py` naming. Mixed Cargo/Python workspaces choose checks by the changed file's package type. `map plan --check` treats package-level checks as broad-change anchors only when the package has an indexed test target.

Optional strict quality policy lives at `.RaymanCodingSkill/quality.json` (the one file under `.RaymanCodingSkill/` that is **not** gitignored, so the policy can be committed and shared):

```
{
  "multi_source_no_test_min_sources": 3,
  "block_warning_kinds": ["public_api_without_test_evidence"],
  "exemptions": [
    {
      "path": "generated/schema.rs",
      "kind": "large_file",
      "reason": "Exact generated file; schema snapshot and package tests validate it."
    }
  ]
}
```

The standard profile ignores this file and keeps ordinary warnings non-blocking. Strict/release merge `block_warning_kinds` into their built-in defaults; an empty or partial array cannot weaken them. `multi_source_no_test_min_sources` may only lower the built-in threshold (tighten policy); a larger value is capped at the default. Exemption paths must be exact existing non-link regular files inside the workspace—no directories, missing preauthorization, symlink/reparse ancestors, `..`, backslashes, or glob syntax—and demote only the exact finding to visible `info`. JSON reports built-in/configured sources and the exemption reason. Malformed policy, unknown fields/kinds, duplicate entries, broad targets, or blank reasons fail closed.

## Validate

The single complete repository audit is intentionally stricter than a development test run. It requires an already installed CLI and deployed canonical skill, runs both Rust projects, package/install smoke, strict/release self-dogfood, state/assets checks, and the clean-source installed-release contract:

```powershell
./scripts/audit-repository.ps1 `
  -CliPath (Get-Command rayman -CommandType Application).Source `
  -SkillPath "$HOME/.codex/skills/raymancodingskill/SKILL.md"
```

For focused development before installation, the underlying commands remain:

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

CI (`.github/workflows/ci.yml`) grants only `contents: read` and pins every third-party Action to an immutable commit. It runs root fmt/clippy/tests on Linux and Windows, verifies Rust 1.88.0, self-dogfoods context refresh + strict quality + release readiness, and verifies clean-source release identity on both platforms without a conflicting global `RUSTFLAGS`. Separate jobs exercise `cargo package`, `cargo install`, and a real 75% line threshold for the shipped root CLI workspace (not the standalone eval harness; current measured total is 79.86%). Dependency policy runs on every change, with an additional scheduled weekly advisory refresh. `evals/` runs fmt/clippy/unit tests, both host-exec rejection guards, and an offline mock provenance smoke on Linux and Windows; no real-model credential or quality claim is involved.

## License

MIT — see [LICENSE](LICENSE).
