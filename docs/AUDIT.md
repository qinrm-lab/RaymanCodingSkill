# Complete repository audit

Under RaymanCodingSkill Owner Mode, an unqualified full-repository audit is an audit-to-closure task: inspect current evidence, repair safe in-scope findings, rerun the authoritative gates, and stop only at a stable pass or a structured human/external boundary. `只审计` / `只报告` / `不要修改` explicitly selects the read-only variant.

During development, bind the final project gate to the task instead of running it as untracked prose evidence:

```powershell
rayman goal validate <id> --req <req-id> `
  -m "authority gate stable twice" `
  --command "pwsh -NoProfile -File <workspace-gate.ps1>" `
  --changed <path> --authority --repeat 2
rayman finish --goal <id>
```

An authority command is still a single direct executable plus argv. Inline/nested shells remain rejected. Authority is limited to a reviewed conventional repository gate (`check-repo`, `audit-repository`, or `verify-release-contract`), workspace-wide Cargo tests, or selector-free workspace pytest; an agent cannot promote an arbitrary focused command merely by adding `--authority`. Both runs must exit zero on the same workspace fingerprint; a normalizer, formatter, generator, or gate that changes indexed bytes writes no receipt. `finish` refuses a merely momentary PASS without that fixed-point proof.

Run the complete audit from a PowerShell 7 (`pwsh`) session only, from a clean checkout after installing/upgrading through `scripts/install-rayman.ps1`:

```powershell
./scripts/audit-repository.ps1 `
  -CliPath (Get-Command rayman -CommandType Application).Source `
  -SkillPath "$HOME/.codex/skills/raymancodingskill/SKILL.md"
```

The complete-audit parameter set has no skip switches. Its contract parameters cannot be weakened: MSRV and the coverage tool are fixed at `1.88.0` and `0.8.7`, while the CLI line threshold may stay at 75 or be raised. It fails rather than silently substituting another toolchain or omitting a lane. At startup it resolves the effective `cargo`, `cargo-deny`, `rustup`, `git`, and `rustc` commands, rejects Function/Alias/Cmdlet shadows, captures each Application path and SHA-256, invokes the captured paths throughout, and re-resolves/re-hashes them before success. The declared MSRV must already be available through rustup. Every audit creates a fresh writable managed-temp advisory root and seeds it from the existing cargo-deny database when one is available, injects that exact path into temporary root/evals configs, runs both dependency-policy checks, and deletes the copy; the committed configs and user cache are never rewritten. With `CARGO_NET_OFFLINE=true`, fetching is disabled but the advisories check still runs against the isolated database; if no seed exists, it fails closed. Every audit also installs the exact pinned `cargo-llvm-cov` version into a fresh managed-temp root, verifies that exact Application path and version, invokes it directly, re-hashes it, and removes the root; an arbitrary PATH copy is never accepted as coverage evidence. The threshold was set from a current measured shipped-CLI total of 79.86% lines.

The script-level negative guards can be exercised without running the expensive audit lanes. This also runs the release-verifier self-test and the exact-match legacy PowerShell profile migration self-test:

```powershell
./scripts/audit-repository.ps1 -SelfTest
```

The focused dependency-policy regression lane exercises the same isolated root/evals advisory state without claiming a complete audit:

```powershell
./scripts/audit-repository.ps1 -DependencyPolicyOnly
```

It covers, in order:

1. Root workspace fmt, Clippy with warnings denied, tests, and cargo-deny.
2. Declared MSRV release build and all root tests under that exact rustup toolchain.
3. A real `cargo llvm-cov` line threshold for the shipped root CLI workspace. This threshold does not claim coverage for the standalone eval harness.
4. Standalone `evals` fmt, Clippy, tests, cargo-deny, real-backend host-exec rejection, third-party-grade rejection, and offline mock report/grade provenance.
5. `cargo package` plus an isolated managed-temp `cargo install` smoke.
6. A locked release build followed by current-artifact context refresh, strict quality, release readiness, state/assets checks, and isolated checkpoint save + `checkpoint verify`.
7. Installed CLI/reference artifact/deployed skill/effective PATH identity plus a clean isolated source-fresh rebuild and terminal identity re-hash.

Use focused Cargo/rayman commands during development; only this command supports the complete local claim. CI mirrors these mandatory lanes across platform jobs and additionally performs the scheduled weekly advisory refresh declared in `.github/workflows/ci.yml`.
