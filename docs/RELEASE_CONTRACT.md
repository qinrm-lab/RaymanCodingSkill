# Release and installed-CLI contract

## What identifies a release

The reported `rayman --version` value is necessary but insufficient. A valid release is this tuple:

1. The package version and MSRV in the root `Cargo.toml`. `crates/rayman/Cargo.toml` inherits those fields, so it cannot silently publish a different version.
2. A clean `cargo build --locked --release` artifact for the target platform: `target/release/rayman` on Unix-like platforms and `target/release/rayman.exe` on Windows.
3. The canonical repository `SKILL.md` and the SHA-256 recorded as `skill_sha256` in the target workspace's `.RaymanCodingSkill/workspace_skill.yaml`.
4. The public command surface `workspace` (including path-safe Git-aware `workspace inspect`), `context`, `goal` (including receipt-producing `goal validate`), workspace-health and goal-bound `check`, locked `prepare` plus `finish`, `map`, `assets`, `temp`, read-only `state audit --check`, `checkpoint` (including integrity-verifying `checkpoint verify`), `autosave`, and `doctor`; the v2.2 workflow surface uses runtime report label `rayman-cli-contract-v6` and records both contract and version in activation. This surface is proven by the crate's behavioral tests (`crates/rayman/tests/cli.rs` plus in-crate unit tests) on every CI platform, not by scraping `--help` text in the release verifier.
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

`-RequirePath` inspects PowerShell's effective command and rejects an Alias/Function/Cmdlet shadow. `-RequireSourceFresh` checks clean `HEAD`, active compiler consistency, isolated locked rebuild bytes, and final CLI/reference/skill/PATH hashes. Cargo, Git, and rustc must resolve directly to applications whenever the verifier uses them; their paths and bytes are fixed for the run, but their supply-chain origin is not attested. The verifier rejects listed environment overrides (`RUSTFLAGS`, encoded flags, wrappers, target/profile and native compiler/linker env), but it neither isolates nor proves the origin of `CARGO_HOME/config.toml`, parent-directory Cargo config, rustup, Cargo, or linker binaries. Both compared builds inherit that active configuration context. CI creates an ignored workspace skill binding; a normal installation records the real one. None of these checks claims hermetic/toolchain provenance or model quality.

## Install or upgrade

The supported source-checkout procedure is transactional and deliberately refuses an old PowerShell shadow. The historical `.Rayman\rayman.ps1` profile function has a drive-root loop and must be removed with the exact-match migration tool; custom functions remain fail-closed:

```powershell
./scripts/repair-rayman-powershell-profile.ps1 -Check
./scripts/repair-rayman-powershell-profile.ps1 -Yes # only for the exact legacy shim
./scripts/install-rayman.ps1 -Yes -AddToUserPath
```

The installer builds and verifies before writing, pins the verified hashes across every copy, stages beside each destination, and updates only the managed CLI and canonical skill. It derives the recorded `cli_contract`/`cli_version` activation values from the built artifact's own `doctor` output; that binding is enforced by consumption rather than source inspection, because `rayman doctor --check` fails whenever the recorded contract or version does not match the installed binary. The complete staging lifecycle—including copy, hash, source recheck, original-to-backup move, and final replace—is one rollback domain. On Windows, `-AddToUserPath` prepends the managed directory inside the user PATH as part of that same transaction and verifies the projected future environment in real Windows order (`Machine PATH + proposed User PATH`); it refuses an older machine/user `rayman` that would win. If `-AddToUserPath` is omitted, the current PATH must already resolve the destination first. The switch fails on non-Windows instead of pretending to persist a shell-specific PATH. A failure attempts every file and user-PATH rollback and reports retained evidence if any restore fails. Verification is the commit point: later backup-cleanup failure retains the backup and cannot trigger destructive rollback of the committed install. `cargo install` is package smoke only because it does not deploy `SKILL.md`.

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

After every source-fresh local smoke test and the normal test suite pass, create an exact tag matching the manifest, for example `v2.2.0`. CI always runs the source-fresh verifier; tag-triggered builds additionally run `-VerifyGitTag`, read the exact tag from Git, and cross-check GitHub's ref type, ref name, full ref, and SHA against that checked-out `HEAD`. Forged or inconsistent `GITHUB_REF_*` values never substitute for repository evidence. Existing historical non-semver tags are not retroactively claimed as releases under this contract.

## One complete audit

The verifier above is one release primitive, not the test suite. The single full repository handoff lane is:

```powershell
./scripts/audit-repository.ps1 `
  -CliPath <installed-rayman-application> `
  -SkillPath <deployed-canonical-SKILL.md>
```

It includes root and evals fmt/Clippy/tests/dependency policy, `cargo package` and `cargo install` smoke, context refresh, strict quality, release readiness, state/assets checks, and this installed release contract. See [AUDIT.md](AUDIT.md).
