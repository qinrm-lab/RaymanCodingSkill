# RaymanCodingSkill

A lean, owner-minded and evidence-first coding-agent helper. The small Rust
`rayman` CLI gives an agent (or you) the load-bearing basics for driving safe
local work to a stable finish. A separate, receipt-bound
`rayman-update-worker` binary is confined to the signed update transaction.
Together, the load-bearing surface is:

- **Context index** — a content-proven map of the workspace (files, kinds, symbols). `context refresh` hashes indexed content and preserves read failures as blockers; the cheap `context status` command remains a stat-only UI probe, while map and readiness conclusions re-check content hashes.
- **Explicit activation** — `.RaymanCodingSkill/` by itself is only runtime state. `workspace activate` writes a canonical-skill path/SHA256 plus the exact CLI contract/version; orphan state, skill drift, and stale CLIs are inactive. The six-field activation schema rejects duplicates, unknown fields, nesting, and malformed scalars.
- **Multilingual Unicode UI** — human-facing text supports Simplified Chinese and English through `--language auto|zh-CN|en` (or `RAYMAN_LANG`). Auto mode follows locale metadata and the Windows user locale, with a Chinese fail-safe when none exists. JSON output remains a locale-independent automation contract, and all captured CLI output must be valid UTF-8.
- **Project map** — a derived architecture view (modules, symbols, local dependencies, Cargo/pyproject packages, entrypoints, heuristic test candidates, impact hints). Rust modules/tests and Python imports plus pytest filename conventions are modeled; unsupported ecosystems remain advisory for missing-test conclusions.
- **Owner Mode** — the agent keeps working through locally knowable implementation, repair, risk checks, and re-audit. Frontier reports separate execution from consultation: a deferred question stays out of transient progress output, while a ready question becomes one deterministic, client-neutral workspace aggregate for the complete current response. Rendering creates no durable claim that the user saw or read it. Each native adapter owns its completion boundary; the Codex `Stop` adapter additionally compares the exact current `last_assistant_message`. Background continuation requires an explicitly authorized isolated mechanism, and an assistant cannot replace `finish --goal` with a reassuring final message.
- **Change plan** — capture the workspace's per-file SHA256 baseline at goal start, persist one immutable aggregate path set before mutation, and compare it with the real delta. A hash-chained extension may widen the plan only before new paths change; it cannot shrink scope or review priority. Missing baselines, split/post-hoc plans, unplanned files, and incomplete validation declarations are blocked.
- **Quality surface** — machine-readable maintainability findings from the project map; `map quality --check` fails only on error-level gaps. Multi-source Cargo and pyproject packages without indexed tests are blocking; unsupported ecosystems stay advisory. Strict/release always promote the built-in `large_file` and `high_fan_in` warnings. `.RaymanCodingSkill/quality.json` can only add blocking kinds or declare exact `(path, kind)` exemptions with a non-empty reason; it cannot clear those defaults.
- **Goal contract** — capture `must`/`should` requirements, a baseline-bound plan, hierarchical work packages, source-bound non-authoritative progress receipts, validation receipts that cover the real delta, repeated-stable authority proof, and fingerprint-bound review for high-priority changes. A pure zero-delta audit uses the authority-only `--workspace-snapshot` scope; it is rejected before the gate starts if the goal baseline has any real delta and never substitutes for `--changed` coverage. A lane ledger distinguishes scoped-writer lanes (which reject writes outside their allowlist) from advisory read-only and final-reviewer lanes (zero-write review brackets that reject every workspace change for their duration), while pending work records agent/human/external ownership.
- **Asset scan** — read-only report of obsolete-looking files and work-in-progress markers. It never deletes anything.
- **State audit** — a read-only inventory of allowed v2 state, retired entries, and recursive managed-temp metrics. `state audit --check` fails on retired state, audit errors, or traversal errors; it never deletes files.
- **Managed temp** — workspace-local scratch under `.RaymanCodingSkill/tmp/`, never system temp. Pytest leases pre-create and probe isolated basetemp/cache/TMP/pycache roots, publish exact argv/environment, and release only a manifest-owned lease. `temp status` reports recursive files, directories, bytes, and traversal errors rather than only the top-level entry count.
- **Windows identity bridge** — ordinary PowerShell remains inside Codex's preferred `elevated` sandbox. The optional broker delegates only installed operation IDs to the logged-on user through a least-privilege InteractiveToken task, with expiring requests, protected results, executor/hash binding, and no arbitrary command channel. Parallel writing chats use separate Git worktrees.
- **Trusted update boundary** — non-read-only skill use runs the throttled update poll by default. Windows has the reviewed fixed-source stable-release notification transport; non-Windows builds return the structured, zero-network `unsupported_platform` boundary and claim neither notification nor automatic apply. Installation remains independently opt-in and requires a compiled Ed25519 root, canonical signed manifest, monotonic anti-replay state, exact install receipt, all asset hashes, a versioned worker, and a journaled handle-relative rollback transaction. Tags and cached prompts never authorize code execution.

Operational runtime/task state is local under `.RaymanCodingSkill/` and normally gitignored; `.RaymanCodingSkill/quality.json` is the one exact shared-policy exception. Its bytes are indexed and included in workspace fingerprints, so changing policy makes context/validation evidence stale exactly like changing source. Checkpoint archives and update preferences are deliberately different: unless overridden, they live in a user-level data directory, not in the repository. Current files and command output are the source of truth; there is no cross-project memory or LLM call. The Windows network surface is the bounded official-release notification/download transport described in [docs/UPDATE_CONTRACT.md](docs/UPDATE_CONTRACT.md); unsupported platforms do not substitute another client.

> This is the v2 rewrite. It deliberately drops the previous framework's parallel LLM stack, HTTP API, research agents, and process-governance manifests in favor of the small load-bearing core above. See `SKILL.md` for the agent-facing usage contract.

## Codex and Claude Code compatibility

Client-neutral rules live in [AGENT_CONTRACT.md](AGENT_CONTRACT.md). The
repository [AGENTS.md](AGENTS.md) is the Codex workspace entrypoint,
[SKILL.md](SKILL.md) is the installed Codex adapter, and
[CLAUDE.md](CLAUDE.md) is the Claude Code entrypoint. Each adapter routes to the
shared contract and
[references/workflow-contract.md](references/workflow-contract.md) instead of
copying policy. [install-manifest.json](install-manifest.json) is the deployment
authority: it publishes `AGENT_CONTRACT.md` as the installed Codex skill's
`AGENTS.md`, without either workspace-local checkpoint block, while Claude Code
remains repository-only. Run
`pwsh ./scripts/check-agent-instructions.ps1 -SelfTest` to verify UTF-8,
markers, adapter ownership, managed-block isolation, the manifest mapping,
client-neutral pending semantics, deployment scopes, and fail-closed negative
fixtures; `scripts/check-repo.ps1` runs the same self-test.

## Windows PowerShell identity broker

Use a Codex-managed or permanent Git worktree for every concurrent writing
chat. Keep normal build, test, Git, and repository PowerShell commands in the
native `elevated` sandbox. Only a command whose evidence genuinely depends on
the logged-on Windows identity may use the optional fixed-capability broker:

```powershell
pwsh -NoProfile -File .\scripts\install-codex-powershell-broker.ps1 -SelfTest
pwsh -NoProfile -File .\scripts\install-codex-powershell-broker.ps1 `
  -Install -Yes -UserAccount "$env:USERDOMAIN\qinrm"

pwsh -NoProfile -File .\scripts\codex-powershell-broker.ps1 `
  -Operation identity_probe
```

The install requires one narrowly approved administrator run. It atomically
creates a protected ProgramData tree, binds the current `pwsh.exe`, worker,
task XML, and fresh `install_id`, and refuses unknown pre-existing state. The
persistent task itself uses `InteractiveToken` plus `LeastPrivilege`, runs
hidden, accepts only `identity_probe`, and writes results below a directory the
sandbox group can read but not modify. Requests are claimed through an
exclusive no-reparse handle and parsed from the exact hashed bytes. Adding a
capability is a reviewed source/install change; request JSON can never provide
command text, a script path, or argv. See
[docs/CODEX_POWERSHELL_BROKER.md](docs/CODEX_POWERSHELL_BROKER.md).

## Build

```
cargo build --release
```

The binary is written to `target/release/rayman` (`rayman.exe` on Windows). The minimum supported Rust version is **1.97.1**; the workspace declares the same exact `rust-version = "1.97.1"`, and the release verifier, repository audit, and MSRV CI lanes must remain aligned to it. Current code uses stable let-chains. Run it through Cargo while developing:

```
cargo run -p rayman -- context status
```

## Release identity and installed CLI

`2.11.2` binds the notification, signed-update, independent worker, install-receipt, and safe `ensure-current` surfaces to `rayman-cli-contract-v17` and the exact running CLI version. It retains the v2.10 goal/Stop/profile behavior and the identity-only rebind boundary. Keep these claims separate: `check --profile release` proves workspace strict-quality only; `doctor --check` proves installed binary/PATH/SKILL/activation identity and any explicitly required execution-context match; the versioned update worker is separately receipt- and signature-bound; release handoff additionally requires locked fresh-source rebuilds of both binaries. The exact release and update procedures live in [docs/RELEASE_CONTRACT.md](docs/RELEASE_CONTRACT.md) and [docs/UPDATE_CONTRACT.md](docs/UPDATE_CONTRACT.md).

Signed update metadata has a 30-day fail-closed validity window. Tag CI requires
a nearly full window before publication, and the weekly freshness guard fails
with an actionable new-patch release instruction when fewer than 14 days
remain. It never refreshes an existing tag or weakens expiry automatically.

The only supported source-checkout install/upgrade entry point is below. The installer supports Windows and Linux; macOS and other Unix hosts fail closed before any installation write because they do not expose the required handle-bound publication primitive. Building and `verify-release-contract.ps1` remain cross-platform. Open a PowerShell 7 (`pwsh`) session first—never Windows PowerShell. A historical profile shim that searches for `.Rayman\rayman.ps1` can loop forever at a drive root; inspect and migrate only that exact known shim before installation:

```powershell
./scripts/repair-rayman-powershell-profile.ps1 -Check
./scripts/repair-rayman-powershell-profile.ps1 -Yes # only when -Check reports the exact legacy shim
./scripts/install-rayman.ps1 -Yes -AddToUserPath
```

The repair tool parses the profile, refuses custom `rayman` functions, preserves unrelated profile content, and uses a staged replacement with rollback. Do not run unbounded output-capturing probes such as `$output = rayman --version` while a profile shadow is unresolved; use `pwsh -NoProfile` and the direct application path for diagnosis.

It requires clean Git source, pins the preverified CLI, versioned update worker, install receipt, and every manifest resource hash, rechecks them before and after every copy, and replaces only that managed tuple through same-directory staging. The global set maps the publishable `AGENT_CONTRACT.md` to installed `AGENTS.md`; repository `AGENTS.md`, its Codex checkpoint block, and `CLAUDE.md` are intentionally absent. Candidate and installed `doctor` checks run only in a newly created isolated workspace outside the repository, and each CLI uses `workspace activate --skill-file ... --yes` only in that disposable workspace. The source-fresh proof remains bound to the real repository and exact hashes of both binaries and all resources; doctor never requires a provisional repository binding. Doctor cleanup holds the terminal temporary-parent lease, verifies the owner marker and root identity, and no-replace renames the entire workspace tree to a reported retained leaf without traversing descendants. The installer itself never recursively deletes or automatically reclaims that retained tree; it reports the exact path for explicit human review and cleanup. Because the tree remains under the host temporary root, it is review evidence rather than a durable archive and remains subject to external host temp-retention policy. Marker, identity, or path-binding drift fails closed, preserves the original or observed tree, releases the leases, and does not retry the isolation.

The managed CLI, exact skill resources, and optional Windows user-PATH update form the core transaction. `-AddToUserPath` moves the managed bin directory to the front of the user segment and verifies the effective `Machine PATH + proposed User PATH` ordering without an artificial process-only prepend; the machine-PATH snapshot must remain byte-stable through final core verification, so an older machine-level `rayman` blocks installation at that point. Forward PATH publication and rollback use the same KTM/TxR registry compare-exchange: the exact raw value and kind are checked through a transacted `HKCU\Environment` handle, the desired value is staged in that transaction, and an ordinary concurrent registry operation before commit rolls the transaction back instead of being overwritten. Basic TxR transaction creation, key open/read, and no-op commit capability is probed before any managed file replacement; the real write/commit remains fail-closed at publication, and an unavailable or failed transaction never degrades to a read/set/read sequence. Without it, the current effective PATH must already resolve the destination first, and Linux callers must configure PATH themselves. Every file mutation holds a native lease on the terminal destination parent and operates only on validated relative leaf names beneath that lease. Staging is an exclusive native create. Every raw transition is added to an incremental `PathBindingLedger` with its role, state, expected presence, identity, metadata, and reason; terminal reconciliation reopens active leaves through the held parent, and failure or committed cleanup reports every retained leaf and its observed evidence precisely.

Windows CAS binds volume/file ID, content, security descriptor, and attributes to open handles, uses handle-bound no-replace rename, and deletes only an exact opened object through a one-byte (`UnmanagedType.U1`) `FILE_DISPOSITION_INFO`. Linux CAS zeroes the full 256-byte `statx` buffer, requires the complete type/mode/UID/GID/inode/size result mask, and additionally binds content plus the complete fd-visible xattr name/value set. Before any public destination mutation, Linux proves `renameat2(RENAME_NOREPLACE)` on the prepared stage and proves open-fd hard-link publication on the target filesystem; actual publication is likewise no-replace. Because Linux has no conditional unlink-by-inode primitive, prepared stages, preflight evidence, superseded originals, rollback publications, and rollback or committed backups remain as exact relative retained leaves recorded in the ledger. Some are hard links to the live installed inode: do not edit them, and remove only the reported directory entry after explicit identity review and cleanup authority. This retained storage is the deliberate safety cost of strong Linux CAS.

After every other core check succeeds, the installed CLI runs `workspace install-bind --skill-file <repository-SKILL.md> --yes` as the final core transaction step. That Rust command commits the repository binding under the shared activation lock, atomically creates or identity-rebinds only an eligible same-path contract, and performs compare-and-swap-safe rollback on a failed publication. Before this commit point, failures attempt compare-and-swap-safe file, resource, and PATH restoration and report retained evidence; after it succeeds, the core installation is never rolled back, and backup cleanup is warning-only. Override destinations only for intentional managed roots. Manual copying and `cargo install` alone are not supported handoff procedures.

Unless `-SkipCodexStopHook` is explicit, the installer next performs the Stop guard as a separate post-commit additive integration: it idempotently runs `codex-hook install --yes`, preserves unrelated handlers, and then verifies the installed command with `codex-hook status`. Hook installation or status failure preserves the committed CLI, skill resources, PATH, and repository binding, reports the separate retry command, and never claims that the hook is installed. A verified hook still requires one explicit `/hooks` trust review when Codex presents a new or changed non-managed hook, followed by a restart.

Installation never scans for or rewrites activation contracts in other workspaces. Because each activation binds the exact canonical-skill bytes and running CLI identity, a successful upgrade can intentionally leave a previously activated workspace `invalid` until it is explicitly rebound there. Run `rayman workspace status` in each workspace. `rayman workspace rebind --yes` is eligible only for an existing, complete, `enabled: true` `raymancodingskill` binding whose only defects are `skill_sha256`, `cli_contract`, or `cli_version` identity drift and whose current skill bytes match the canonical `SKILL.md` embedded in the running CLI; it updates only those identity scalar values under the shared activation lock while preserving `skill_file`, comments, ordering, quoting, and line endings. It refuses orphan, deactivated, malformed, wrong-skill, untrusted/missing-file, unsafe-path, and path-change cases. Use `workspace activate --skill-file ... --yes` for a new activation or an intentional canonical-path change. When Rayman is explicitly invoked for a non-read-only task, perform an eligible rebind and continue that original task; a read-only request remains read-only and reports the recovery command instead. The Stop Hook cannot infer intent, so safely rebindable drift is allowed to end without forcing a state write only after normal goal/frontier checks; an unfinished goal or structurally invalid activation still blocks.

Do not assume that `rayman` on `PATH` is current. To inspect a built artifact without installing it, deliberately put it on `PATH` for this shell and verify it:

```powershell
cargo build --locked --release
$artifactDirectory = Join-Path $PWD 'target/release'
$env:PATH = "$artifactDirectory$([IO.Path]::PathSeparator)$env:PATH"
$artifact = Join-Path $artifactDirectory $(if ($IsWindows) { 'rayman.exe' } else { 'rayman' })
$worker = Join-Path $artifactDirectory $(if ($IsWindows) { 'rayman-update-worker.exe' } else { 'rayman-update-worker' })
./scripts/verify-release-contract.ps1 -CliPath $artifact -ReferenceCliPath $artifact -WorkerPath $worker -ReferenceWorkerPath $worker -SkillPath ./SKILL.md -WorkspaceSkillPath ./SKILL.md -SkillResourceMode Source -RequirePath -RequireSourceFresh
```

For an installed executable, pass its resolved path as `-CliPath` and the newly built release artifact as `-ReferenceCliPath`. `-SkillResourceMode Deployed` is the default: `-SkillPath` must be the globally deployed canonical skill, defines the deployment root, and makes every manifest destination match and remain bound to its repository source. `-SkillResourceMode Source` is only for a pre-install artifact, requires `-SkillPath` to resolve to this checkout's exact `SKILL.md`, and binds manifest sources without treating repository entrypoint names such as `AGENTS.md` as deployed destinations. `-WorkspaceSkillPath` independently names the skill that `doctor` must report and defaults to `-SkillPath` only for backward compatibility. Both roots and all mode-selected resources are independently re-resolved and re-hashed at the terminal check. `-RequirePath` checks PowerShell's effective command and rejects an Alias/Function shadow before comparing bytes; in Source mode it does not make an installed-release claim. Native Cargo/Git/rustc calls are resolved as real applications, pinned by path and hash, and rechecked before success. `-RequireSourceFresh` rejects known ambient build-shaping variables, records clean `HEAD` and active compiler identity, makes an isolated locked rebuild, then terminally re-resolves/re-hashes every CLI/reference/skill/PATH path. `-VerifyGitTag` always reads the exact tag from Git and, when GitHub context exists, cross-checks ref type/name/full ref/SHA rather than trusting environment variables. User-level or parent Cargo configuration (including `CARGO_HOME/config.toml`) is not isolated or provenance-checked and still participates in both builds. The result is byte identity for the current source + active rustc + active Cargo configuration context, not repository-default/hermetic toolchain provenance. Windows MSVC builds use `/Brepro`. The verifier never installs or replaces an executable.

## Commands

```text
rayman --language zh-CN context status # force Simplified Chinese UI
rayman --language en context status    # force English UI
rayman workspace status         # activation only: active/inactive/orphan/invalid
rayman workspace inspect        # activation plus Git HEAD/clean/dirty/untracked source state
rayman workspace activate --skill-file <canonical-SKILL.md> --yes
rayman workspace rebind --yes   # refresh an existing eligible identity-drifted binding only
rayman workspace ensure-current [--yes] # current activation identity only; never injects xtask or rewrites project automation
rayman workspace deactivate --yes
rayman update status            # offline/read-only user update preference and cache
rayman update check             # Windows fixed-source read; non-Windows reports unsupported_platform
rayman update poll              # due poll: Windows notification; non-Windows unsupported_platform
rayman update configure --auto-install --yes # independent verified-install opt-in
rayman codex-hook status                       # inspect user-level Stop guard installation
rayman codex-hook install --yes                # merge managed handler; preserves other hooks
rayman codex-hook uninstall --yes              # remove only Rayman managed handler
rayman codex-hook stop                         # Codex host entrypoint; reads Stop JSON from stdin
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
rayman prepare --goal <id>      # refresh context plus compact goal counts and latest verified standard checkpoint
rayman finish --goal <id>       # require stable authority proof, refresh, then run the goal-bound gate
rayman assets                   # read-only obsolete-file + work-marker scan
rayman temp status              # recursive files/dirs/bytes plus traversal errors; read-only
rayman temp scratch <label> | cleanup
rayman temp pytest-lease <label> | pytest-probe <id> | pytest-release <id>
rayman state audit [--check]    # read-only v2/retired-state + recursive-temp audit; audit/traversal errors fail --check

rayman goal start "<title>" --must "<req>" [--should "<req>"]
rayman goal list | show <id> | summary <id>
rayman goal plan <id> <paths...> --check [--extend]
rayman goal review <id> --reviewer <name> -m "<review>"
rayman goal package add <id> <package> "<title>" [--parent <package>] [--req <req_id>] [--optional]
rayman goal progress <id> --package <package> -m "<stage evidence>" --command "<direct command>"
rayman goal package complete <id> <package> --progress <progress_id>
rayman goal lane open <id> <lane> --mode advisory-read-only|writer|final-reviewer [--allow <path>]
rayman goal lane close <id> <lane>
rayman goal evidence <id> --req <req_id> -m "<legacy attestation>" --validated "<command claimed to have passed>"
rayman goal evidence <id> --req <req_id> -m "<legacy attestation>" --validated "<command claimed to have passed>" --changed <path>
rayman goal validate <id> --req <req_id> -m "<evidence>" --command "<command to execute>" [--changed <path>] [--authority --repeat 2]
rayman goal close <id> [--status success|partial|blocked] # only these three; standard READY requires closed success
rayman goal archive <id> --reason "<historical reason>"
rayman goal archive <id> --reason "<historical reason>" --migrate-receipt-policy receipt_integrity_v1
rayman goal archive <id> --reason "<legacy quarantine reason>" --quarantine-invalid-history
# One-way and non-authoritative: only an invalid archived success, or a complete current legacy success after every trusted archive/migration path fails.
rayman goal authorize-replacement <replacement> --supersedes <old>... --authority-from <archived-success> --command "<exact archived authority command>" --repeat 2
# Only for an archived command whose sole source-bound input is this exact flag:
rayman goal authorize-replacement <replacement> --supersedes <old>... --authority-from <archived-success> --command "<exact archived authority command>" --maintenance-cycle-rebind .check-repo-output/<current-cycle>-maintenance-review-cycle.json --repeat 2
rayman goal supersede <id> --by <current gate-ready replacement>
rayman goal current [<id>]                                # list current goals; with an id, restore an archived/superseded goal to current
rayman goal frontier <id>       # legacy decision plus orthogonal execution / consultation
rayman goal pending add "<title>" -m "<detail>" [--goal <id>] [--owner agent|human|external] [solution-package fields] [--consultation-timing deferred|immediate] [background proof]
rayman goal pending list | render --goal <id> | render --current | migrate <id> | resolve <id>

rayman checkpoint save | list | status                    # list exposes complete/partial/corrupt status
rayman checkpoint salvage-save                            # activation-exempt recovery-only; never default latest/evidence
rayman checkpoint verify [id|latest]                      # read-only v3 manifest/path/per-file hash verification
rayman checkpoint restore [id|latest] --yes               # only a verified complete snapshot; journaled all-or-nothing
rayman checkpoint restore <recovery-id> --yes --allow-recovery-only # also requires repaired active contract
rayman autosave start | stop | status                     # scheduled auto-snapshots (see tools/README.md)
rayman doctor [--check]                                   # installed binary/PATH/workspace-SKILL identity only
```

Every command accepts `--format json` for machine-readable output.

Text mode is UTF-8 and locale-aware. `--language auto` checks `RAYMAN_LANG`, `LC_ALL`, `LC_MESSAGES`, `LANG`, then the Windows user locale; explicit `--language` wins. Chinese, English, Unicode workspace paths, and safe Unicode scratch labels round-trip without lossy decoding. JSON keys, enum/status values, and schema are identical in every language.

`goal start` records a per-file SHA256 baseline. A baseline-less current v2 goal is never gate-ready; preserve completed history with `archive`, or replace unfinished work with a new baseline-bound goal and `supersede`. Work that was simply abandoned — most often a goal opened before the code existed, whose baseline no longer resembles the tree — is retired by stating the real outcome and then filing it: `goal close <id> --status partial` (or `blocked`) followed by `goal archive <id> --reason "<why>"`. Archiving asserts nothing about completion, and every consumer of an archived record additionally requires success, so a retired partial can never stand in for evidence. An `active` goal still cannot be archived: closing it first is what makes the outcome an honest record rather than a disappearance. For multi-file work, `goal plan` must be written while the workspace still matches that baseline. `--extend` keeps the base receipt immutable and appends a cumulative hash-chained snapshot only when prior delta is already planned and newly added paths still equal the baseline. Paths/checks can only widen and review priority can only rise. Actual additions, edits, and deletions are recomputed from the baseline, so validation and close reject unplanned delta. A high-priority effective plan also needs `goal review` bound to the final source fingerprint.
Large plans emit scale warnings when they lack staged work packages or progress receipts. Required packages can only complete from a same-package progress receipt bound to the source snapshot and direct command output, but progress is always `authoritative=false` and never substitutes `goal validate`. `goal summary` omits the full baseline and reports compact counts. Lane close recomputes the opening-baseline delta: advisory/final-reviewer lanes reject every write, while writer lanes reject paths outside their exact allowlist; lane evidence is coordination-only. A read-only/final-reviewer lane is thus a zero-write bracket over the whole workspace, not a reviewer running concurrently with writers to the same tree; once it observes drift it cannot be closed cleanly, so recover by reverting the drift or by discharging the goal with `supersede`/`archive`.
`check` without a goal remains a workspace-health result and reports `workspace_ready`; it must not be presented as proof that a user task is complete. `check --goal <id>` additionally reports task evidence readiness. `finish --goal <id>` is stricter: it first requires an authority validation that ran the same project gate at least twice without changing the final workspace fingerprint, then refreshes and checks the exact closed goal. Every check also reports Git/source state when available.

Owner Mode never treats a plain pending string as permission to stop. `goal frontier` preserves legacy `decision` while adding `execution=continue_foreground|continue_background|paused_for_user|wait_external|complete` and `consultation=none|deferred|ready`. `ready` only authorizes generating `goal pending render --current`; the `rayman.human-boundary-aggregate.v1` payload is client-neutral and the whole deterministic workspace aggregate must be the current final response. Rendering alone is neither persisted nor evidence of delivery, visibility, reading, or user awareness. The native client adapter owns any completion observation: Codex re-lists all current goals and pending packages, rechecks the workspace fingerprint, and compares the exact `last_assistant_message`, while Claude Code does not execute or emulate that Codex hook. `continue_background + ready` additionally requires an immediate human consultation with a non-empty mechanism, `--background-authority-evidence`, and `--background-isolation-evidence`; partial or blank proof fails closed, and self-asserted booleans are not accepted. Human/external entries require attempts, evidence paths, minimum input, a recommendation, alternatives, risk, a resume command, and an auto-resume condition. `goal close --status blocked` still refuses agent-owned work and incomplete solution packages. The Codex Stop guard allows an inactive workspace or an activated goal-less trivial task, but fails closed on invalid activation, corrupt goal/pending state, current foreground work, a partial aggregate, a stale event, or forged success evidence.


`related_tests` and change-plan checks are planning aids, not proof of real coverage. For a current-schema goal, use `goal validate`: it executes the command from the workspace root and stores its zero exit code, output hashes, before/after fingerprints, and declared changed paths. The current receipts must collectively cover the real delta. `--authority --repeat 2` accepts only a reviewed conventional repository gate (`check-repo`, `audit-repository`, or `verify-release-contract`), the exact Rust source-gate command `cargo run --locked --manifest-path xtask/Cargo.toml -- repository-gate`, workspace-wide Cargo tests, or selector-free workspace pytest; it repeats that direct command on the exact same workspace identity and writes no receipt if any run fails or mutates indexed content. The xtask gate binds the root Cargo manifest/lock/config, the complete `xtask/` tree, and every repository `scripts/` helper from the baseline/current union; the convenient `cargo xtask` alias is never authority because Cargo configuration or `CARGO_ALIAS_XTASK` can replace it. Pytest validation (`python -m pytest`, `pytest`, or `py.test`) parses positional directory/file/node selectors separately from option values, including pytest-xdist values such as `-n 4` and `--dist loadscope`, so parallel options stay workspace-wide while `pytest tests` is scoped and `file.py::test_name` covers its source file. It first runs an independent `--collect-only -q` proof, rejects zero tests and collect-only-as-execution, reads one terminal summary instead of arbitrary test output, requires `passed > 0`, and requires passed/skipped/xfailed/xpassed totals to match collection. A nonzero command writes no receipt. Pytest is recognized only when `-m pytest` sits in interpreter-option position: `python -c "<code>" -m pytest` is an arbitrary-code host, not a test run, and is rejected the same way `sh -c` is. `goal evidence --validated` remains legacy human attestation and cannot satisfy a current-schema standard/release claim.
Retired v1 spellings fail nonzero with migration guidance rather than silently changing meaning: `audit` maps to separate workspace/task/state commands, `workspace-skill` maps to `workspace`, and `context os`, `context task`, and `subagent` name their v2 replacements. They are diagnostic traps, not compatibility aliases.


Archiving or superseding a completed goal revalidates its receipt integrity at the fingerprint where that work actually passed; later source changes do not make historical proof current. Lifecycle proofs bind the receipt-policy version into their contract hash, so a later classifier upgrade cannot silently reinterpret valid history or be downgraded by editing JSON. Proofs written before policy versioning are checked with the exact legacy v1 integrity rules. A pre-policy-v2 current or archived success may be explicitly migrated with `--migrate-receipt-policy receipt_integrity_v1`, but only when it predates that rollout and every real v1 receipt still passes fingerprint, cwd, command/scope, contract, and output-hash checks. This option cannot repair missing receipts, post-rollout work, an incomplete requirement, or a goal whose immutable plan omitted real changes. A second, wider hatch exists for records that predate receipts entirely: `--migrate-unreceipted` archives a pre-rollout success whose requirements carry evidence and validations but no receipts at all, and the proof it writes is thereafter accepted without a receipt recheck. It is limited to goals whose `created_at` precedes the rollout, so no goal created by `goal start` today can reach it; use it only to file genuine pre-receipt history, and prefer `archive` without it whenever real receipts exist. `--quarantine-invalid-history` is the final non-authoritative fallback: it preserves an invalid archived success, or atomically retires a complete current legacy success only when retirement resolves the lifecycle boundary and ordinary current/v1/unreceipted archive paths all fail. It preserves the requirement and validation ledger, cannot serve as replacement authority, and cannot be restored to current. Supersession requires a separate current, gate-ready replacement at the moment it is recorded; if that replacement is archived afterwards, its own lifecycle proof keeps the supersession valid.

The narrow lifecycle-only recovery is `goal authorize-replacement`. It does not treat a code gate as non-code and does not mint synthetic validation receipts. It requires a goal-state-pristine replacement whose musts are the exact normalized multiset of (proof kind, text) pairs from all named unfinished predecessors — a plain must never discharges a `--must-proof` obligation of the same text, while an unset kind and an explicit `generic` are one key — requires every current source-delta path to be covered by those predecessors' immutable plans, and requires `--command` to match a direct repeated authority command from a non-migrated current-policy archived success. Rayman reruns that gate at least twice on one unchanged current source fingerprint, then binds the run receipts, current source delta, canonical workspace identity, predecessor IDs, plans, and immutable transfer contracts into the replacement proof. The only source-bound exception is `--maintenance-cycle-rebind`: the archived command must contain exactly one `-MaintenanceOrchestrationCycle` value, Rayman derives the effective argv internally, rejects absolute/traversal/link/reparse paths, and hashes the current cycle before and after every run while preserving every other archived token. An unplanned delta, stale-only receipt, arbitrary command or flag substitution, unstable or failing execution, copied state, quarantine, legacy policy, indirect authority, extra, missing, or differently-typed musts, unsafe cycle path, or any later source drift rejects the proof.

## State and checkpoint integrity

`rayman state audit` is a read-only hygiene report. It names the v2 state entries it accepts, any retired entries, and the recursive temp footprint. `--check` exits nonzero when retired state, an audit error, or a traversal error exists; it deliberately performs no migration or deletion. Review the report and obtain explicit approval before removing or migrating state.

Checkpoint manifests are v3 integrity records with activation provenance and `standard|recovery_only` purpose; v2 manifests are rejected rather than reinterpreted. `salvage-save` remains available when activation is invalid, but its independently rotated snapshot never becomes default `latest` or completion evidence. Restoring it requires an explicit ID, `--allow-recovery-only`, and a repaired active contract. Saves take a cross-process workspace/store lock; failed/crashed staging is preserved, and standard, recovery-only, and partial rotation pools cannot consume each other. `checkpoint verify` is read-only. Restore validates the complete manifest first, then durably replaces each destination file through same-directory staging (`flush`/`fsync` + atomic rename). It is a journaled all-or-nothing transaction: every source is staged and verified and every existing destination is backed up before the first file is published, and a failure during publication restores the backups in reverse order. A crash leaves a recorded transaction that the next `save`/`restore` resolves; it never deletes extra workspace files.

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

The single complete repository audit is intentionally stricter than a development test run. It requires an already installed CLI and deployed canonical skill, runs both Rust projects, package/install smoke, strict/release self-dogfood, the `state audit --check` gate plus a report-only `assets` scan, and the clean-source installed-release contract:

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

CI (`.github/workflows/ci.yml`) grants only `contents: read` and pins every third-party Action to an immutable commit. It runs root fmt/clippy/tests on Linux and Windows, installs the Linux `acl` capability so named-ACL regressions cannot capability-skip, and executes `install-rayman.ps1 -SelfTest` as an installer runtime test on hosted Windows x64 and Ubuntu/Linux x64. The Linux ARM64 leg is only a Rust cross-target `cargo check`; it does not execute the installer and must never be reported as an ARM64 runtime PASS. CI also verifies Rust 1.97.1, self-dogfoods context refresh + strict quality + release readiness, and verifies clean-source release identity on both runtime platforms without a conflicting global `RUSTFLAGS`. Separate jobs exercise `cargo package`, `cargo install`, and enforce a real 75% line threshold for the shipped root CLI workspace rather than the standalone eval harness. Dependency policy runs on every change, with an additional scheduled weekly advisory refresh. `evals/` runs fmt/clippy/unit tests, both host-exec rejection guards, and an offline mock provenance smoke on Linux and Windows; no real-model credential or quality claim is involved.

## License

MIT — see [LICENSE](LICENSE).
