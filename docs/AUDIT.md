# Complete repository audit

Under RaymanCodingSkill Owner Mode, an unqualified full-repository audit is an audit-to-closure task: inspect current evidence, repair safe in-scope findings, rerun the authoritative gates, and stop only at a stable pass or a structured human/external boundary. `只审计` / `只报告` / `不要修改` explicitly selects the read-only variant.

During development, bind the final project gate to the task instead of running it as untracked prose evidence:

```powershell
rayman goal validate <id> --req <req-id> `
  -m "authority gate stable twice" `
  --command "pwsh -NoProfile -File scripts/check-repo.ps1" `
  --changed <path> --authority --repeat 2
rayman finish --goal <id>
```

When the goal baseline has no real delta, replace `--changed <path>` with
`--workspace-snapshot`. That scope requires `--authority`, accepts only a real
workspace-wide authority gate, and checks the goal delta before any list/gate
program starts. It fails closed on any added, removed, or modified indexed file;
it cannot be used to avoid declaring changed-path coverage.

An authority command is still a single direct executable plus argv. Inline/nested shells remain rejected. Authority is limited to a reviewed conventional repository gate (`check-repo`, `audit-repository`, or `verify-release-contract`), workspace-wide Cargo tests, or selector-free workspace pytest; an agent cannot promote an arbitrary focused command merely by adding `--authority`. Both runs must exit zero on the same workspace fingerprint; a normalizer, formatter, generator, or gate that changes indexed bytes writes no receipt. `finish` refuses a merely momentary PASS without that fixed-point proof.

For broad audits, use `goal package` plus `goal progress` to leave compact stage receipts and `goal lane` to prove read-only/final-reviewer lanes did not write or scoped writers stayed inside their allowlists. These are recovery and coordination records only: they never satisfy the authority line above. Python lanes should obtain a manifest-owned `temp pytest-lease`, apply its emitted basetemp/cache/environment values, probe it before reuse, and release that exact lease afterward; concurrent runs must not share pytest cache, basetemp, TMP, or pycache roots.

Run the complete audit from a PowerShell 7 (`pwsh`) session only, from a clean checkout after installing/upgrading through `scripts/install-rayman.ps1`:

```powershell
./scripts/audit-repository.ps1 `
  -CliPath (Get-Command rayman -CommandType Application).Source `
  -SkillPath "$HOME/.codex/skills/raymancodingskill/SKILL.md"
```

The complete-audit parameter set has no skip switches. Its contract parameters cannot be weakened: MSRV and the coverage tool are fixed at `1.97.1` and `0.8.7`, while the CLI line threshold may stay at 75 or be raised. It fails rather than silently substituting another toolchain or omitting a lane. At startup its structured `bootstrap` phase resolves the effective `cargo`, `cargo-deny`, `rustup`, `git`, and `rustc` commands, rejects Function/Alias/Cmdlet shadows, captures each Application path and SHA-256, invokes the captured paths throughout, and re-resolves/re-hashes them before success. During environment preflight it installs `llvm-tools-preview` for the explicitly named MSRV toolchain, resolves the declared MSRV `cargo` and `rustc` through exact `rustup which ... --toolchain 1.97.1` paths, verifies both versions, and derives the matching `llvm-cov` and `llvm-profdata` from that exact rustc's `--print target-libdir` sibling `bin`; all four applications are path/SHA-256 bound and terminally rechecked. The MSRV lane invokes that exact Cargo with `RUSTC` and `CARGO_BUILD_RUSTC` fixed to the exact MSRV compiler, temporarily removes Rust compiler wrappers, and uses a fresh `.RaymanCodingSkill/tmp` `CARGO_TARGET_DIR`; it restores every prior environment value and safely removes the isolated target after success or failure. This prevents an earlier stable build or a system Rust installation that precedes rustup on PATH from contaminating or replacing MSRV evidence. Windows filesystem canonicalization uses `ProviderPath`, so valid `\\?\` extended paths are compared as filesystem paths instead of being corrupted by a PowerShell provider-qualified prefix. A missing application emits `bootstrap:start` followed by `bootstrap:fail` before the original exception is rethrown. `-SelfTest` and `-DependencyPolicyOnly` intentionally do not require the real complete-audit rustup/MSRV toolset; the self-test uses injected exact-path fixtures to prove binding, command shape, failure cleanup, and environment restoration. The declared MSRV and its `llvm-tools-preview` component must be available through rustup for a complete audit. Every audit creates a fresh writable managed-temp advisory root and seeds it from the existing cargo-deny database when one is available, injects that exact path into temporary root/evals configs, runs both dependency-policy checks, and deletes the copy; the committed configs and user cache are never rewritten. With `CARGO_NET_OFFLINE=true`, fetching is disabled but the advisories check still runs against the isolated database; if no seed exists, it fails closed. Every audit also installs the exact pinned `cargo-llvm-cov` version into a fresh managed-temp root and verifies that exact Application path/version. The coverage lane then uses the exact MSRV Cargo/rustc and matching LLVM tools, a fresh isolated target, explicit `CARGO`, `RUSTC`, `LLVM_COV`, and `LLVM_PROFDATA` bindings, and complete environment restoration; a system Rust outside rustup or an arbitrary PATH tool can neither satisfy nor contaminate coverage evidence. The managed coverage executable and isolated target are rechecked/removed after success or failure. The threshold was set from a current MSRV-bound measured shipped-CLI total of 80.71% lines.

Artifact reproducibility and advisory seeding are separate inputs. When the active `CARGO_HOME` must remain fixed to reproduce installed bytes but its `advisory-dbs` tree is unavailable to the execution identity, pass `-CargoDenyDatabaseSeedPath <readable-directory>`. The explicit seed must already exist as an ordinary, fully readable directory tree; it is copied into managed temp and bound to both cargo-deny configs. A missing path, file, symlink, reparse point, or unreadable descendant fails closed, and the option never skips dependency policy or rewrites either Cargo home.
Every lane emits `RAYMAN_AUDIT_PHASE` JSON records with `start|pass|fail`
status. A failure record names the current phase before the original exception
is rethrown, so CI and host agents can report live progress without parsing
free-form command logs.

The script-level negative guards can be exercised without running the expensive audit lanes. This also runs the release-closeout, installer CAS/path, release-verifier, and exact-match legacy PowerShell profile migration self-tests:

```powershell
./scripts/audit-repository.ps1 -SelfTest
```


`scripts/release-closeout.ps1 -SelfTest` separately proves that cached audit
evidence is rejected across binding drift and that the closeout command always
retains `--authority --repeat 2`. The normal closeout accepts reuse only for an
exact clean HEAD/CLI/SKILL/script/tool binding.
The focused dependency-policy regression lane exercises the same isolated root/evals advisory state without claiming a complete audit:

```powershell
./scripts/audit-repository.ps1 -DependencyPolicyOnly
```

It covers, in order:

1. Root workspace fmt, Clippy with warnings denied, tests, and cargo-deny.
2. Declared MSRV release build and all root tests under that exact rustup toolchain.
3. A real `cargo llvm-cov` line threshold for the shipped root CLI workspace, compiled with the exact isolated MSRV Cargo/rustc and matching `llvm-tools-preview`. This threshold does not claim coverage for the standalone eval harness.
4. Standalone `evals` fmt, Clippy, tests, cargo-deny, real-backend host-exec rejection, third-party-grade rejection, and offline mock report/grade provenance.
5. `cargo package` plus an isolated managed-temp `cargo install` smoke.
6. A locked release build followed by current-artifact context refresh, strict quality, release readiness, the `state audit --check` gate plus a report-only `assets` scan, and isolated standard checkpoint save + `checkpoint verify`; recovery-only salvage is negative-tested but never accepted as release evidence.
7. Installed CLI/reference artifact/deployed skill/effective PATH identity plus a clean isolated source-fresh rebuild and terminal identity re-hash.

Use focused Cargo/rayman commands during development; only this command supports the complete local claim. CI mirrors these mandatory lanes across platform jobs and additionally performs the scheduled weekly advisory refresh declared in `.github/workflows/ci.yml`.
