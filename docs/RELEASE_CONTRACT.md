# Release and installed-CLI contract

## What identifies a release

The reported `rayman --version` value is necessary but insufficient. A valid release is this tuple:

1. The package version and MSRV in the root `Cargo.toml`. `crates/rayman/Cargo.toml` inherits those fields, so it cannot silently publish a different version.
2. Clean `cargo build --locked --release` artifacts for the target platform: the main `rayman` CLI and the separate `rayman-update-worker` (with `.exe` suffixes on Windows).
3. The canonical repository `SKILL.md` and the SHA-256 recorded as `skill_sha256` in the target workspace's `.RaymanCodingSkill/workspace_skill.yaml`.
4. The public command surface `workspace` (including path-safe Git-aware `workspace inspect`, trusted identity-only `workspace rebind`, and `workspace ensure-current`), activation-exempt `update status|check|configure|poll`, `context`, `goal` (including goal-bound write-ahead plan publication, capability-bound pending packages, deterministic client-neutral `rayman.human-boundary-aggregate.v1` rendering, frontier states, monotonic plan extension, hierarchical packages, non-authoritative progress receipts, source-bound lane ledgers, and repeated-stable authority validation), workspace-health and goal-bound `check`, snapshot-bound locked `prepare` plus `finish`, `map`, `assets`, `temp` (including probed pytest leases), read-only `state audit --check`, `checkpoint` (including activation-exempt recovery-only salvage and integrity-verifying `checkpoint verify`), `autosave`, and execution-context-aware `doctor`; the v2.11 surface retains multilingual UTF-8/Unicode and v2.8 long-task behavior under runtime label `rayman-cli-contract-v17`. Activation records both exact contract and version. Update notification, signed-release installation, source freshness, and workspace activation are distinct claims; see [UPDATE_CONTRACT.md](UPDATE_CONTRACT.md). Native completion observation is adapter-owned: a Codex Stop match is event-local, Claude Code does not execute that hook, and neither creates a durable delivery/read receipt. Progress/lane/recovery-only records are explicitly non-authoritative and cannot satisfy final validation. Behavioral tests cover this surface on the configured CI platforms rather than scraping `--help` text in the release verifier; only a successful run bound to the exact source supplies release evidence.
5. For a tagged release, the exact Git tag `v<package-version>` on the release commit.

The Rust manifest and the command parser are the implementation sources of truth. There are deliberately three different claims:

- Unbound `rayman check --profile release` is a **workspace strict-quality** result. Goal-bound `check --goal <id>`/`finish --goal <id>` additionally proves that exact task's current receipt state. Neither proves an installed executable, PATH identity, or source freshness.
- `rayman doctor --check` proves the installed binary/PATH/workspace-SKILL **identity tuple**. It explicitly does not prove that the artifact was rebuilt from the current source.
- `scripts/verify-release-contract.ps1 -RequireSourceFresh` is the artifact-identity primitive used by handoff/CI. `-WorkerPath` and `-ReferenceWorkerPath` bind the installed/candidate update worker alongside the main CLI; both binaries must match one isolated locked rebuild. Its default `-SkillResourceMode Deployed` treats `-SkillPath` as the mandatory global deployment root and verifies every manifest destination against its repository source. Explicit `-SkillResourceMode Source` is limited to pre-install artifacts, requires this checkout's exact `SKILL.md`, and binds only manifest source identities so repository adapter names cannot masquerade as deployment destinations. `-WorkspaceSkillPath` independently binds the skill reported by doctor and defaults to `-SkillPath` only for backward compatibility. It verifies reported version, runtime contract label, and SHA-256 identity; it deliberately does not re-assert the command surface from help text or inspect installer source text. It rejects native-command shadows and known ambient compiler/linker/profile overrides, pins every Cargo/Git/rustc application it uses by path and hash, records clean `HEAD` and active `rustc -vV`, rebuilds in an isolated target, then terminally re-resolves/re-hashes every tool and mode-selected source/deployed/workspace/PATH identity. User/parent Cargo config is intentionally not isolated. This proves current-source byte identity under the active rustc and active Cargo configuration context; it does not attest toolchain/config provenance or a hermetic repository-default build.

A matching version string, filename, copied binary, or workspace strict-quality result without the source-fresh check is not release evidence.

## Build and smoke-test a release artifact

Run this from the repository root in PowerShell 7+ (the script works on Windows, Linux, and macOS). It does not install an artifact. Clear ambient compiler/linker/profile/target override variables first; `-RequireSourceFresh` deliberately fails rather than inheriting them.

```powershell
cargo build --locked --release
$artifactDirectory = Join-Path $PWD 'target/release'
$artifactName = if ($IsWindows) { 'rayman.exe' } else { 'rayman' }
$workerName = if ($IsWindows) { 'rayman-update-worker.exe' } else { 'rayman-update-worker' }
$artifact = Join-Path $artifactDirectory $artifactName
$worker = Join-Path $artifactDirectory $workerName
$env:PATH = "$artifactDirectory$([IO.Path]::PathSeparator)$env:PATH"

# doctor requires the workspace's recorded SKILL hash to be current.
$skillHash = (Get-FileHash ./SKILL.md -Algorithm SHA256).Hash.ToLowerInvariant()
# Put this exact value in .RaymanCodingSkill/workspace_skill.yaml as skill_sha256.

./scripts/verify-release-contract.ps1 `
  -CliPath $artifact `
  -ReferenceCliPath $artifact `
  -WorkerPath $worker `
  -ReferenceWorkerPath $worker `
  -SkillPath ./SKILL.md `
  -WorkspaceSkillPath ./SKILL.md `
  -SkillResourceMode Source `
  -RequirePath `
  -RequireSourceFresh
```

`-RequirePath` inspects PowerShell's effective command and rejects an Alias/Function/Cmdlet shadow. `-RequireSourceFresh` checks clean `HEAD`, active compiler consistency, isolated locked rebuild bytes, and final CLI/reference/skill/PATH hashes.

On any `-pc-windows-msvc` host the verifier forces `-C link-arg=/Brepro` for its isolated rebuild, because MSVC link timestamps and CodeView identifiers otherwise differ across target directories. `.cargo/config.toml` must therefore carry a `[target.<triple>]` `rustflags` entry with that same flag for **every** MSVC host the project supports — currently `x86_64-pc-windows-msvc` and `aarch64-pc-windows-msvc`. A missing entry does not degrade gracefully: the reference build would compile without `/Brepro` while the isolated rebuild compiles with it, so the two can never be byte-identical and `-RequireSourceFresh` fails permanently on that platform (which in turn makes `install-rayman.ps1`, where the check is a mandatory precondition, unable to succeed there at all). Adding a new Windows MSVC target means adding its config section in the same change. Cargo, Git, and rustc must resolve directly to applications whenever the verifier uses them; their paths and bytes are fixed for the run, but their supply-chain origin is not attested. The verifier rejects listed environment overrides (`RUSTFLAGS`, encoded flags, wrappers, target/profile and native compiler/linker env), but it neither isolates nor proves the origin of `CARGO_HOME/config.toml`, parent-directory Cargo config, rustup, Cargo, or linker binaries. Both compared builds inherit that active configuration context. CI creates an ignored workspace skill binding; a normal installation records the real one. None of these checks claims hermetic/toolchain provenance or model quality.

## Install or upgrade

The supported source-checkout install procedure is transactional on Windows and Linux and deliberately refuses an old PowerShell shadow. macOS and other Unix hosts may still build and run the release verifier, but `install-rayman.ps1` fails before any installation write because those hosts do not provide the handle-bound publication primitive required by this installer contract. The historical `.Rayman\rayman.ps1` profile function has a drive-root loop and must be removed with the exact-match migration tool; custom functions remain fail-closed:

```powershell
./scripts/repair-rayman-powershell-profile.ps1 -Check
./scripts/repair-rayman-powershell-profile.ps1 -Yes # only for the exact legacy shim
./scripts/install-rayman.ps1 -Yes -AddToUserPath
```

The installer builds and verifies before writing, reads `install-manifest.json`, validates the declared client/update deployment scopes, pins the verified hashes across every copy, and stages beside each destination. It updates only the managed CLI, the versioned update worker, `%LOCALAPPDATA%\Rayman\install\receipt.json` (or the XDG equivalent), and the exact Codex global-skill resource set in one rollback domain. That set maps the publishable, client-neutral `AGENT_CONTRACT.md` source to installed `AGENTS.md`; repository `AGENTS.md` and `CLAUDE.md` remain client entrypoints and are never globally deployed by this installer. The receipt binds the installation ID, exact managed paths/hashes, version/contract, install-manifest identity, and source kind; a signed update adds its manifest digest/key epoch/sequence. The shared published source must contain neither checkpoint managed block nor checkout-specific absolute path. Candidate and installed `doctor` checks run only in a newly created isolated workspace outside the repository. Each CLI uses `workspace activate --skill-file ... --yes` only in that disposable workspace, while source-fresh proof remains bound to the real repository and exact artifact/worker/resource hashes. The installer therefore never prewrites or backs up the repository activation merely to make doctor pass. Doctor cleanup retains the terminal temporary-parent lease, verifies the owner marker and root identity, then no-replace renames the entire workspace tree to an exact reported retained leaf without traversing descendants. The installer performs no recursive or automatic cleanup of the retained tree and reports its exact path for explicit human review. The tree remains under the host temporary root, so it is not a durable archive and may still be removed by external host temp-retention policy. Marker, identity, or path-binding drift fails closed, preserves the original or observed tree, releases all leases, and suppresses any second isolation attempt.

The managed CLI, manifest resources, and optional Windows user-PATH update form the core rollback domain. Each destination transaction keeps a native lease on its terminal parent and performs only validated relative-leaf mutations beneath that handle. The prepared stage is created exclusively, never by a `Test-Path`/copy race. Every raw transition is recorded incrementally in a `PathBindingLedger`, including role, state, reason, expected presence, identity, and platform metadata; terminal reconciliation reopens every active leaf through the parent lease, and retained-evidence reporting identifies exact leaves and observed identities instead of a generic rescue or quarantine directory.

Windows file CAS binds an open handle's volume/file ID, content, security descriptor, and attributes, uses handle-bound no-replace rename, and deletes only the exact opened object via a one-byte (`UnmanagedType.U1`) `FILE_DISPOSITION_INFO`. Linux file CAS starts from a zeroed 256-byte `statx` buffer, rejects results missing any required type/mode/UID/GID/inode/size mask bit, and binds those values plus content and a stable digest of every fd-visible xattr name/value. Before changing a public destination, Linux preflights both relative `renameat2(RENAME_NOREPLACE)` and an open-fd hard link on the target filesystem; publication is the same no-replace hard-link operation. Linux deliberately never calls a path-based unlink as if it were inode-conditional: prepared stages, preflight evidence, superseded originals, rollback publications, and rollback or committed backups remain as exact relative retained leaves in the ledger. Some retained leaves are hard links to the live installed inode and must never be edited; after identity review, removing only the reported directory entry requires explicit cleanup authority. This retained storage is the explicit safety cost of the Linux contract.

The CI runtime boundary is exact: hosted Windows x64 and Ubuntu/Linux x64 execute `install-rayman.ps1 -SelfTest`. Linux ARM64 receives only the Rust cross-target `cargo check --target aarch64-unknown-linux-gnu`; it does not run the installer and is not ARM64 runtime-PASS evidence.

Only after every artifact, installed-file, PATH, and doctor check succeeds does the installed CLI run `workspace install-bind --skill-file <repository-SKILL.md> --yes` as the final core transaction step. Under the same activation lock used by activate, rebind, and deactivate, that Rust command creates a missing binding, leaves an already-current same-path binding byte-identical, or identity-rebinds an eligible same-path contract; disabled, malformed, unsafe, wrong-skill, untrusted, and path-change cases fail closed. Publication is atomic, write-verified, and compare-and-swap rolled back on failure. Every failure before this commit point attempts compare-and-swap-safe restoration of the files, resources, and PATH; once install-bind succeeds, the core installation and repository binding are committed and no later failure rolls them back.

Unless `-SkipCodexStopHook` is explicit, the installer then treats the Codex Stop guard as a separate post-commit additive integration. It runs the verified installed CLI's `codex-hook install --yes`, preserving unrelated hook entries, and re-verifies the exact managed command with `codex-hook status`. An install or status failure leaves the committed core installation intact, reports the standalone retry command, and must not report the hook as installed. A successful status still does not forge Codex trust: the exact non-managed hook definition must be reviewed through `/hooks` when prompted and loaded by a restart.

That repository-local rewrite is not a registry or migration of other workspaces. The installer never scans for or automatically changes their `.RaymanCodingSkill/workspace_skill.yaml` files. Since activation binds the exact canonical-skill hash and CLI contract/version, a successful upgrade can intentionally make an existing external workspace `invalid`. In each such workspace, first run `rayman workspace status`. `rayman workspace rebind --yes` accepts only an existing complete, `enabled: true` `raymancodingskill` binding whose only defects are identity drift and whose current skill bytes match the canonical `SKILL.md` embedded in the running CLI; under the shared activation lock it updates only the three identity scalar values while preserving all other contract bytes. Orphan, deactivated, malformed, wrong-skill, untrusted/missing-file, unsafe-path, and path-change cases fail closed and require review or an explicit `workspace activate --skill-file ... --yes`. Explicit Rayman use for a non-read-only task performs an eligible rebind and then resumes the original task; read-only work reports the command without rewriting state. The intent-blind Stop Hook never forces that write, but it still blocks an unfinished Owner Mode goal before allowing eligible drift to end.

On Windows, `-AddToUserPath` prepends the managed directory inside the user PATH as part of that same transaction and verifies the effective environment in real Windows order (`Machine PATH + proposed User PATH`); the captured machine value is re-read immediately before the core commit and any drift aborts the install, so an older machine/user `rayman` cannot pass that point-in-time verification. Both forward publication and rollback use a KTM/TxR registry compare-exchange on the non-predefined `HKCU\Environment` key: the transaction reads and checks the exact expected raw value and kind, stages the desired record, and commits only if no ordinary concurrent registry operation has forced Windows to roll the transaction back. Before replacing any managed file, the installer probes basic transaction creation, key open/read, and no-op commit capability; the real write/commit remains fail-closed at publication. A missing or failed TxR/KTM operation aborts `-AddToUserPath` without falling back to a non-atomic read/set/read sequence; callers may omit the switch and manage PATH externally. The user PATH is read and written as a raw `HKCU\Environment` value: `[Environment]::GetEnvironmentVariable(...,'User')` expands `%VAR%` on read and its setter always writes `REG_SZ`, so using those accessors would freeze entries such as `%JAVA_HOME%\bin` into installation-time literals and downgrade `REG_EXPAND_SZ` to `REG_SZ` on success as well as on rollback. The existing value kind is preserved as-is (a value that was already `REG_SZ` stays `REG_SZ`; the installer does not retroactively repair one), unexpanded entries are carried through verbatim, and `WM_SETTINGCHANGE` is broadcast explicitly since the raw registry write bypasses the framework's own notification. If `-AddToUserPath` is omitted, the current PATH must already resolve the destination first. The switch fails on non-Windows instead of pretending to persist a shell-specific PATH.

Before the core commit point, failed compare-and-swap restoration reports retained evidence instead of overwriting later state. After that point, backup-cleanup or Stop-hook failure cannot trigger destructive rollback of the committed core installation. `cargo install` is package smoke only because it does not deploy the manifest-bound Codex skill resources.

## Verify an existing installation before using it

Build a reference artifact from the desired commit first. Then verify that the effective command is an application rather than trusting its displayed version:

```powershell
$commands = @(Get-Command rayman -All)
if ($commands[0].CommandType -ne 'Application') { throw "rayman is shadowed by $($commands[0].CommandType)" }
$installed = $commands[0].Source
$installedRoot = Split-Path -Parent $installed
$installedWorker = Join-Path $installedRoot 'rayman-update-worker-<version>.exe'
./scripts/verify-release-contract.ps1 `
  -CliPath $installed `
  -ReferenceCliPath $artifact `
  -WorkerPath $installedWorker `
  -ReferenceWorkerPath $worker `
  -SkillPath <path-to-deployed-canonical-SKILL.md> `
  -WorkspaceSkillPath ./SKILL.md `
  -RequirePath `
  -RequireSourceFresh
```

The reference and installed executables must have the same SHA-256, and both must match an isolated locked rebuild from the clean current source. The supplied deployed skill and doctor-reported workspace skill must each have the same SHA-256 as this repository's canonical `SKILL.md`; manifest resources are resolved only relative to the deployed root. If any check fails, use `scripts/install-rayman.ps1`; do not repair the tuple by copying one file manually.

An intentionally different bootstrap wrapper is not the canonical skill and must not be passed as `-SkillPath`; first inspect where it points, then verify the canonical target. This prevents a wrapper's version prose from being mistaken for the workflow contract.

## Release tags

After every source-fresh local smoke test and the normal test suite pass, create an exact tag matching the manifest, for example `v2.11.2`. CI always runs the source-fresh verifier; tag-triggered builds additionally run `-VerifyGitTag`, read the exact tag from Git, and cross-check GitHub's ref type, ref name, full ref, and SHA against that checked-out `HEAD`. Only the protected `rayman-release` job receives `contents: write`; it emits and signs the canonical fixed-role update manifest, verifies that signature with the production public key compiled into the worker, then publishes the exact asset set. An absent or mismatched production public key, protected signing secret, or environment approval fails instead of publishing unsigned or client-unusable assets. Forged or inconsistent `GITHUB_REF_*` values never substitute for repository evidence. Existing historical non-semver tags are not retroactively claimed as releases under this contract.

Signed metadata remains valid for 30 days. The release job requires at least 29
days after signature verification, while the weekly freshness guard uses the
current protected-source verifier—never code from the release being
inspected—and fails below 14 remaining days. That alert requires a new signed
patch release; it never authorizes rewriting assets for the existing tag or
relaxing manifest expiry.

## One complete audit

The verifier above is one release primitive, not the test suite. The single full repository handoff lane is:

```powershell
./scripts/audit-repository.ps1 `
  -CliPath <installed-rayman-application> `
  -SkillPath <deployed-canonical-SKILL.md>
```

It includes root and evals fmt/Clippy/tests/dependency policy, the exact
source-bound Rust gate `cargo run --locked --manifest-path xtask/Cargo.toml --
repository-gate`, `cargo package` and `cargo install` smoke, context refresh,
strict quality, release readiness, the `state audit --check` gate plus a
report-only `assets` scan, and this installed release contract. The convenient
`cargo xtask` alias is never release authority. See [AUDIT.md](AUDIT.md).

The default audit validates already installed host tooling and does not run `rustup component add` or `cargo install cargo-llvm-cov`. A missing fixed MSRV LLVM component or fixed `cargo-llvm-cov` Application fails closed during preflight. `audit-repository.ps1 -PrepareAuditTools` is a separate explicit provisioning command: it requires no CLI/SKILL paths, persists and verifies only those two pinned tools, then exits without running an audit lane. `release-closeout.ps1` never forwards or implicitly grants provisioning authority; prepare first, then invoke the normal audit or closeout separately.

For goal-bound handoff closure, `scripts/release-closeout.ps1` composes the
audit's internal source-fresh verification and the final goal authority gate. A
non-reused closeout consumes the audit's `installed_release_identity` proof and
then recomputes the exact release binding; it does not perform a second
source-fresh rebuild. Its
optional evidence reuse is content-addressed over the canonical workspace,
clean Git HEAD, installed CLI and deployed SKILL hashes, every release script,
and the exact native tool paths, versions, and hashes (including cargo-deny,
the current PowerShell host independently of PATH, the preinstalled coverage
Application, exact MSRV Cargo/rustc, and matching LLVM tools), plus the
advisory-database tree and `CARGO_NET_OFFLINE` state. Reuse is accepted only
when offline mode is effective; an online advisory run always reruns. The full
binding is recomputed immediately before a reused result is announced and
again after authority and optional finish. Missing, malformed, dirty, or drifted
bindings fail closed. Even an exact reusable binding never replaces the
current goal validation: the canonical
`cargo run --locked --manifest-path xtask/Cargo.toml -- repository-gate` is
still executed with `--authority --repeat 2` on one unchanged fingerprint. The
gate delegates the reviewed transition script while binding the complete
baseline/current `xtask/` and `scripts/` dependency union.
