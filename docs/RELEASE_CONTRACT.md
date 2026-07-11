# Release and installed-CLI contract

## What identifies a release

The reported `rayman --version` value is necessary but insufficient. A valid release is this tuple:

1. The package version and MSRV in the root `Cargo.toml`. `crates/rayman/Cargo.toml` inherits those fields, so it cannot silently publish a different version.
2. A clean `cargo build --locked --release` artifact for the target platform: `target/release/rayman` on Unix-like platforms and `target/release/rayman.exe` on Windows.
3. The canonical repository `SKILL.md` and the SHA-256 recorded as `skill_sha256` in the target workspace's `.RaymanCodingSkill/workspace_skill.yaml`.
4. The public command surface `context`, `goal` (including receipt-producing `goal validate`), `check`, `map`, `assets`, `temp`, read-only `state audit --check`, `checkpoint` (including integrity-verifying `checkpoint verify`), `autosave`, and `doctor`; the runtime report label is `rayman-cli-contract-v5`.
5. For a tagged release, the exact Git tag `v<package-version>` on the release commit.

The Rust manifest and the command parser are the implementation sources of truth. There are deliberately two different claims:

- `rayman check --profile release` is a **workspace strict-quality** result. It does not prove an installed executable, PATH identity, or source freshness.
- `rayman doctor --check` proves the installed binary/PATH/workspace-SKILL **identity tuple**. It explicitly does not prove that the artifact was rebuilt from the current source.
- `scripts/verify-release-contract.ps1 -RequireSourceFresh` is the handoff/CI claim. It records a clean Git `HEAD`, rebuilds `rayman` from the locked current source in an isolated temporary target directory, then requires the same clean `HEAD` again and byte identity with the supplied artifact. On Windows MSVC, the repository Cargo config uses `/Brepro` so PE timestamps and CodeView identifiers remain reproducible across those isolated target directories.

A matching version string, filename, copied binary, or workspace strict-quality result without the source-fresh check is not release evidence.

## Build and smoke-test a release artifact

Run this from the repository root in PowerShell 7+ (the script works on Windows, Linux, and macOS). It does not install an artifact. `-RequireSourceFresh` performs an additional isolated temporary build and removes that verified temporary target afterward.

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

`-RequirePath` proves that `Get-Command rayman` resolves to byte-identical executable content. `-RequireSourceFresh` requires a clean Git worktree, records `HEAD`, rebuilds from the locked current source in an isolated target, then rechecks both clean status and the same `HEAD` before accepting its hash. It compares that fresh artifact's SHA-256 with `-CliPath` and `-ReferenceCliPath`. The repository applies `/Brepro` to Windows MSVC builds, and the verifier forces the same flag for its isolated build even when an ambient `RUSTFLAGS` would override Cargo config; an artifact built without it fails closed rather than producing a spurious release claim. The script also checks the version, lockfile, MSRV declaration, every listed top-level command, `goal validate`, `checkpoint verify`, `state audit --check`, the installed-identity output of `doctor --check`, and the supplied skill hash. `doctor` works from an ordinary managed workspace: it verifies the running binary, the first PATH command selected by the platform, and that workspace's recorded `SKILL.md`; it intentionally does not infer a source artifact from `<workspace>/target/release`. CI creates a temporary workspace hash binding before it runs this same smoke test; a normal installation must record the real workspace binding instead. The verifier checks `state audit --check` as a command surface (via its help), not as an instruction to mutate or clean a caller's workspace; its detailed operational contract is in `README.md`, `SKILL.md`, and `tools/README.md`.

## Verify an existing installation before using it

Build a reference artifact from the desired commit first. Then resolve the installed program rather than trusting its displayed version:

```powershell
$installed = (Get-Command rayman -CommandType Application).Source
./scripts/verify-release-contract.ps1 `
  -CliPath $installed `
  -ReferenceCliPath $artifact `
  -SkillPath <path-to-deployed-canonical-SKILL.md> `
  -RequirePath `
  -RequireSourceFresh
```

The reference and installed executables must have the same SHA-256, and both must match an isolated locked rebuild from the clean current source. The supplied deployed skill must have the same SHA-256 as this repository's canonical `SKILL.md`. If any check fails, do not use the PATH installation for release handoff; rebuild and perform the ordinary installation procedure outside this script, update the workspace `skill_sha256`, and rerun both this verifier and `rayman doctor --check`.

An intentionally different bootstrap wrapper is not the canonical skill and must not be passed as `-SkillPath`; first inspect where it points, then verify the canonical target. This prevents a wrapper's version prose from being mistaken for the workflow contract.

## Release tags

After every source-fresh local smoke test and the normal test suite pass, create an exact tag matching the manifest, for example `v2.1.0`. CI always runs the source-fresh verifier; tag-triggered builds additionally run `-VerifyGitTag` and reject any other tag. Existing historical non-semver tags are not retroactively claimed as releases under this contract.
