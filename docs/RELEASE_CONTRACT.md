# Release and installed-CLI contract

## What identifies a release

The reported `rayman --version` value is necessary but insufficient. A valid release is this tuple:

1. The package version and MSRV in the root `Cargo.toml`. `crates/rayman/Cargo.toml` inherits those fields, so it cannot silently publish a different version.
2. A clean `cargo build --locked --release` artifact for the target platform: `target/release/rayman` on Unix-like platforms and `target/release/rayman.exe` on Windows.
3. The canonical repository `SKILL.md` and the SHA-256 recorded as `skill_sha256` in the target workspace's `.RaymanCodingSkill/workspace_skill.yaml`.
4. The public command surface `workspace` (including path-safe Git-aware `workspace inspect`), `context`, `goal` (including frontier states, monotonic plan extension, hierarchical packages, non-authoritative progress receipts, source-bound lane ledgers, and repeated-stable authority validation), workspace-health and goal-bound `check`, locked `prepare` plus `finish`, `map`, `assets`, `temp` (including probed pytest leases), read-only `state audit --check`, `checkpoint` (including activation-exempt recovery-only salvage and integrity-verifying `checkpoint verify`), `autosave`, and `doctor`; the v2.8 surface retains multilingual UTF-8/Unicode behavior and adds bounded long-task recovery under runtime label `rayman-cli-contract-v14`. Activation records both exact contract and version. Progress/lane/recovery-only records are explicitly non-authoritative and cannot satisfy final validation. This surface is proven by behavioral tests on every CI platform, not by scraping `--help` text in the release verifier.
5. For a tagged release, the exact Git tag `v<package-version>` on the release commit.

The Rust manifest and the command parser are the implementation sources of truth. There are deliberately three different claims:

- Unbound `rayman check --profile release` is a **workspace strict-quality** result. Goal-bound `check --goal <id>`/`finish --goal <id>` additionally proves that exact task's current receipt state. Neither proves an installed executable, PATH identity, or source freshness.
- `rayman doctor --check` proves the installed binary/PATH/workspace-SKILL **identity tuple**. It explicitly does not prove that the artifact was rebuilt from the current source.
- `scripts/verify-release-contract.ps1 -RequireSourceFresh` is the artifact-identity primitive used by handoff/CI. `-SkillPath` is mandatory. It verifies reported version, runtime contract label, and SHA-256 identity; it deliberately does not re-assert the command surface from help text or inspect installer source text. It rejects native-command shadows and known ambient compiler/linker/profile overrides, pins every Cargo/Git/rustc application it uses by path and hash, records clean `HEAD` and active `rustc -vV`, rebuilds in an isolated target, then terminally re-resolves/re-hashes every tool and supplied/deployed/PATH identity. User/parent Cargo config is intentionally not isolated. This proves current-source byte identity under the active rustc and active Cargo configuration context; it does not attest toolchain/config provenance or a hermetic repository-default build.

A matching version string, filename, copied binary, or workspace strict-quality result without the source-fresh check is not release evidence.

## Build and smoke-test a release artifact

Run this from the repository root in PowerShell 7+ (the script works on Windows, Linux, and macOS). It does not install an artifact. Clear ambient compiler/linker/profile/target override variables first; `-RequireSourceFresh` deliberately fails rather than inheriting them.

```powershell
cargo build --locked --release
$artifactDirectory = Join-Path $PWD 'target/release'
$artifactName = if ($IsWindows) { 'rayman.exe' } else { 'rayman' }
$artifact = Join-Path $artifactDirectory $artifactName
$env:PATH = "$artifactDirectory$([IO.Path]::PathSeparator)$env:PATH"

# doctor requires the workspace's recorded SKILL hash to be current.
$skillHash = (Get-FileHash ./SKILL.md -Algorithm SHA256).Hash.ToLowerInvariant()
# Put this exact value in .RaymanCodingSkill/workspace_skill.yaml as skill_sha256.

./scripts/verify-release-contract.ps1 `
  -CliPath $artifact `
  -ReferenceCliPath $artifact `
  -SkillPath ./SKILL.md `
  -RequirePath `
  -RequireSourceFresh
```

`-RequirePath` inspects PowerShell's effective command and rejects an Alias/Function/Cmdlet shadow. `-RequireSourceFresh` checks clean `HEAD`, active compiler consistency, isolated locked rebuild bytes, and final CLI/reference/skill/PATH hashes.

On any `-pc-windows-msvc` host the verifier forces `-C link-arg=/Brepro` for its isolated rebuild, because MSVC link timestamps and CodeView identifiers otherwise differ across target directories. `.cargo/config.toml` must therefore carry a `[target.<triple>]` `rustflags` entry with that same flag for **every** MSVC host the project supports — currently `x86_64-pc-windows-msvc` and `aarch64-pc-windows-msvc`. A missing entry does not degrade gracefully: the reference build would compile without `/Brepro` while the isolated rebuild compiles with it, so the two can never be byte-identical and `-RequireSourceFresh` fails permanently on that platform (which in turn makes `install-rayman.ps1`, where the check is a mandatory precondition, unable to succeed there at all). Adding a new Windows MSVC target means adding its config section in the same change. Cargo, Git, and rustc must resolve directly to applications whenever the verifier uses them; their paths and bytes are fixed for the run, but their supply-chain origin is not attested. The verifier rejects listed environment overrides (`RUSTFLAGS`, encoded flags, wrappers, target/profile and native compiler/linker env), but it neither isolates nor proves the origin of `CARGO_HOME/config.toml`, parent-directory Cargo config, rustup, Cargo, or linker binaries. Both compared builds inherit that active configuration context. CI creates an ignored workspace skill binding; a normal installation records the real one. None of these checks claims hermetic/toolchain provenance or model quality.

## Install or upgrade

The supported source-checkout procedure is transactional and deliberately refuses an old PowerShell shadow. The historical `.Rayman\rayman.ps1` profile function has a drive-root loop and must be removed with the exact-match migration tool; custom functions remain fail-closed:

```powershell
./scripts/repair-rayman-powershell-profile.ps1 -Check
./scripts/repair-rayman-powershell-profile.ps1 -Yes # only for the exact legacy shim
./scripts/install-rayman.ps1 -Yes -AddToUserPath
```

The installer builds and verifies before writing, reads `install-manifest.json`, validates the declared client deployment scopes, pins the verified hashes across every copy, and stages beside each destination. It updates only the managed CLI and the exact Codex global-skill resource set; `CLAUDE.md` remains a repository-only entrypoint and is never globally deployed by this installer. Unless `-SkipCodexStopHook` is explicit, it then idempotently merges the verified installed CLI as the Rayman user-level Codex `Stop` handler while preserving all unrelated hook entries. Codex requires the exact non-managed hook definition to be reviewed/trusted through `/hooks` and loaded by a restart; the installer never forges that trust state. It derives the recorded `cli_contract`/`cli_version` activation values from the built artifact's own `doctor` output; that binding is enforced by consumption rather than source inspection, because `rayman doctor --check` fails whenever the recorded contract or version does not match the installed binary. The complete staging lifecycle—including every manifest resource copy, hash, source recheck, original-to-backup move, and final replace—is one rollback domain.

That binding rewrite necessarily happens *before* the source-fresh gate, because the gate's own `doctor --check` reads it. It is therefore backed up first (including the fact that no binding existed) and restored on every abort below that point, so a run that stops at the clean-worktree check cannot leave the repository claiming a version that was never installed. Its directory is resolved with the same reparse-point-checked walk used for every other managed directory.

On Windows, `-AddToUserPath` prepends the managed directory inside the user PATH as part of that same transaction and verifies the projected future environment in real Windows order (`Machine PATH + proposed User PATH`); it refuses an older machine/user `rayman` that would win. The user PATH is read and written as a raw `HKCU\Environment` value: `[Environment]::GetEnvironmentVariable(...,'User')` expands `%VAR%` on read and its setter always writes `REG_SZ`, so using those accessors would freeze entries such as `%JAVA_HOME%\bin` into installation-time literals and downgrade `REG_EXPAND_SZ` to `REG_SZ` on success as well as on rollback. The existing value kind is preserved as-is (a value that was already `REG_SZ` stays `REG_SZ`; the installer does not retroactively repair one), unexpanded entries are carried through verbatim, and `WM_SETTINGCHANGE` is broadcast explicitly since the raw registry write bypasses the framework's own notification. If `-AddToUserPath` is omitted, the current PATH must already resolve the destination first. The switch fails on non-Windows instead of pretending to persist a shell-specific PATH.

A failure attempts every file, activation-binding, and user-PATH rollback and reports retained evidence if any restore fails. Verification is the commit point: later backup-cleanup failure retains the backup and cannot trigger destructive rollback of the committed install. `cargo install` is package smoke only because it does not deploy the manifest-bound Codex skill resources.

## Verify an existing installation before using it

Build a reference artifact from the desired commit first. Then verify that the effective command is an application rather than trusting its displayed version:

```powershell
$commands = @(Get-Command rayman -All)
if ($commands[0].CommandType -ne 'Application') { throw "rayman is shadowed by $($commands[0].CommandType)" }
$installed = $commands[0].Source
./scripts/verify-release-contract.ps1 `
  -CliPath $installed `
  -ReferenceCliPath $artifact `
  -SkillPath <path-to-deployed-canonical-SKILL.md> `
  -RequirePath `
  -RequireSourceFresh
```

The reference and installed executables must have the same SHA-256, and both must match an isolated locked rebuild from the clean current source. The supplied deployed skill must have the same SHA-256 as this repository's canonical `SKILL.md`. If any check fails, use `scripts/install-rayman.ps1`; do not repair the tuple by copying one file manually.

An intentionally different bootstrap wrapper is not the canonical skill and must not be passed as `-SkillPath`; first inspect where it points, then verify the canonical target. This prevents a wrapper's version prose from being mistaken for the workflow contract.

## Release tags

After every source-fresh local smoke test and the normal test suite pass, create an exact tag matching the manifest, for example `v2.4.0`. CI always runs the source-fresh verifier; tag-triggered builds additionally run `-VerifyGitTag`, read the exact tag from Git, and cross-check GitHub's ref type, ref name, full ref, and SHA against that checked-out `HEAD`. Forged or inconsistent `GITHUB_REF_*` values never substitute for repository evidence. Existing historical non-semver tags are not retroactively claimed as releases under this contract.

## One complete audit

The verifier above is one release primitive, not the test suite. The single full repository handoff lane is:

```powershell
./scripts/audit-repository.ps1 `
  -CliPath <installed-rayman-application> `
  -SkillPath <deployed-canonical-SKILL.md>
```

It includes root and evals fmt/Clippy/tests/dependency policy, `cargo package` and `cargo install` smoke, context refresh, strict quality, release readiness, state/assets checks, and this installed release contract. See [AUDIT.md](AUDIT.md).

For goal-bound handoff closure, `scripts/release-closeout.ps1` composes the
audit, source-fresh verification, and the final goal authority gate. Its
optional evidence reuse is content-addressed over the canonical workspace,
clean Git HEAD, installed CLI and deployed SKILL hashes, every release script,
and the exact native tool paths and hashes. Missing, malformed, dirty, or drifted
bindings rerun the audit. Even an exact reusable binding never replaces the
current goal validation: `pwsh -NoProfile -File scripts/check-repo.ps1` is still
executed with `--authority --repeat 2` on one unchanged fingerprint.
