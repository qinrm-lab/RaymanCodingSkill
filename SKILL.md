---
name: raymancodingskill
description: Lean coding-workflow helper for a workspace that contains a .RaymanCodingSkill/ directory, or when the user explicitly invokes rayman. Provides workspace context indexing, a minimal goal contract with resumable pending work, read-only asset/state audits, and managed temp. Do not use for unrelated repos, host-app accounts, IDE assistant login, OAuth, or connectors.
---

# RaymanCodingSkill (lean)

A small, evidence-first coding helper. The `rayman` CLI keeps workspace-local state under `.RaymanCodingSkill/`; current files and command output are the source of truth, not cached summaries.

## When to use

Use in a workspace that has a `.RaymanCodingSkill/` directory, or when the user names `rayman`. Otherwise don't apply it.

## Task tiers — do the least that fits

- **Trivial** (typo, one-liner, doc tweak): just edit and run the project's own focused test. No `rayman` commands required.
- **Standard** (a feature or fix): `rayman context refresh` once up front (content hashes, not stat metadata), use `rayman map impact <path>` for non-trivial touched files, use `rayman map plan <paths...> --check` before broad or multi-file changes, review `rayman map quality --check` when touching broad architecture or multiple modules, implement, then, for an active current-schema goal, use `rayman goal validate <id> --req <req_id> -m "<evidence>" --command "<command to execute>" [--changed <path>]` so Rayman executes and receipts the validation. Run focused tests, then `rayman check --profile standard` before you call it done; if it reports a blocker, don't call the task done until it's resolved or the user has explicitly accepted the open item.
- **Release / hand-off**: everything in Standard, plus resolve pending work, run the project's full build+test, and use `rayman check --profile release` for workspace strict-quality. It is not an installed-release claim: handoff also requires `scripts/verify-release-contract.ps1 -RequireSourceFresh` against a clean checkout and deployed CLI/skill.

`rayman check` defaults to `--profile standard`: it adds project-map freshness, error-level quality findings, validation relevance, and fail-closed goal checks to the base readiness snapshot. Use the explicit `check --profile quick` only for strong content freshness, asset findings, and pending work; it cannot support a delivery claim. Use `release` for strict policy. `rayman state audit --check` is a separate state-hygiene check; it is deliberately not run by `rayman check`.

## Core commands

- `rayman context refresh` — rebuild the index with content-hash proof; a file read error is recorded and cannot produce a ready index.
- `rayman context status` — cheap stat-only freshness UI probe; it does not prove content identity. Map and readiness commands use strong content-hash freshness; run `refresh` if any of them reports `stale`/`missing`.
- `rayman map summary|file <path>|symbol <name>|topology|impact <path>|plan <paths...> [--check]|quality [--profile standard|strict] [--check]` — build a project map from the current context index, then report modules, symbols, local dependencies, Cargo package topology, entrypoints, heuristic test candidates, risks, maintainability quality findings, and suggested checks. It understands direct Cargo path dependencies and workspace-inherited path dependencies; non-workspace nested packages use `cargo test --manifest-path` recommendations instead of invalid workspace package selectors. It refuses stale/missing context; `plan --check` exits non-zero on unscoped broad changes such as multiple source files without a candidate test or indexed package test anchor; `quality --check` exits non-zero only on error-level quality gaps. Only `--profile strict` (and `check --profile release`) reads `.RaymanCodingSkill/quality.json`; the default standard profile never applies strict policy, so a promoted warning kind blocks nothing without `--profile strict`. **Symbol extraction and test-anchor detection are Cargo/Rust-shaped heuristics** (verified against a real 60k-line multi-project C# workspace: near-zero symbols/dependencies extracted). Outside a Cargo workspace (no `Cargo.toml` found anywhere in the index) the "no test anchor" finding in `plan --check` and `quality`'s `multi_source_project_without_tests` both stay non-blocking warnings instead of hard errors — treat their advice as informational, not proof of missing test coverage, on non-Rust projects.
- `rayman goal start "<title>" --must "<req>" [--should "<req>"]` — capture the task as a contract.
- `rayman goal validate <id> --req <req_id> -m "<evidence>" --command "<command to execute>" [--changed <path>]` then `goal close <id>` — Rayman runs the command from the workspace root and records its zero exit code, output hashes, and before/after workspace fingerprints. Closing `success` requires at least one `must`, with every `must` done and carrying evidence; `check --profile standard` / `release` additionally require current receipts. `goal evidence --validated ...` is legacy attestation retained for migration; it cannot satisfy a current-schema standard/release success claim.
- `rayman goal pending add|list|resolve` — carry unfinished work across sessions; never report done while pending items remain.
- `rayman check --profile standard` — everything in `check`, plus project-map freshness, error-level quality findings, validation relevance for recorded `--changed` files, strict goal-state loading, closed-success evidence checks, and fail-closed active/partial/blocked goal handling.
- `rayman check --profile release` — workspace standard plus optional strict quality policy from `.RaymanCodingSkill/quality.json`; malformed strict policy, unknown fields, and unknown blocking warning kinds are blockers. Its READY result never proves installed CLI identity or source freshness.
- `rayman assets` — read-only scan for obsolete-looking files and work-in-progress markers. It never deletes anything; deciding what to remove is yours.
- `rayman temp scratch <label> | status | cleanup` — put throwaway runtime files under `.RaymanCodingSkill/tmp/`, not system temp. `status` is read-only and recursively reports files, directories, bytes, and traversal errors; `cleanup` only removes that managed root.
- `rayman state audit [--check]` — read-only audit of allowed v2 state entries, retired entries, and recursive temp metrics. It never deletes files. `--check` exits nonzero when retired state, an audit error, or a traversal error needs review; obtain explicit approval before any migration or deletion.
- `rayman checkpoint save | list | status | verify [id|latest] | restore [id|latest] --yes` — snapshot the gitignore-aware working tree plus the v2 allowlisted task state to a user-level store. `list` marks every snapshot `complete`, `partial`, or `corrupt`; `status` selects only the newest verified complete snapshot. A failed save preserves a partial forensic snapshot and fails instead of replacing the latest good one. `verify` rechecks the v2 manifest, safe paths, sizes, and SHA-256 values without writing the workspace; `restore` refuses partial/corrupt snapshots and needs `--yes`.
- `rayman autosave start | stop | status` — start-of-session, `start` saves a snapshot and registers a Windows scheduled task that auto-snapshots every N minutes (default 30); on completion or error call `stop` to attempt a final snapshot and unregister only after that snapshot succeeds. With auto-stop on (default), it self-stops only when at least one goal exists, every goal is `success`, and no pending work remains; `active`/`partial`/`blocked`, unreadable goal state, or no goals keep it running. On non-Windows platforms the scheduler is unsupported; use your system scheduler to call `rayman checkpoint save`. See `tools/README.md`.
- `rayman doctor [--check]` — report whether the running executable, the `rayman` executable resolved from `PATH`, and the `SKILL.md` hash recorded by the current workspace form the same installed identity tuple. `--check` exits non-zero on identity mismatch; it does not prove source freshness. Release handoff/CI must run `scripts/verify-release-contract.ps1 -RequireSourceFresh`, which requires a clean checkout and compares an isolated locked rebuild; see `docs/RELEASE_CONTRACT.md`.

## Evidence and honesty

- For a current-schema **standard/release success claim**, every `must` needs a successful `goal validate` receipt bound to the current workspace fingerprint. `goal close` itself accepts evidence-only completion, but that closure is not gate-ready. Model confidence and a merely typed `--validated` string are not evidence.
- Project-map `related_tests`, `impact`, and `plan` output are planning heuristics, not coverage proof. Do not claim a source change is validated from `docs reviewed` or another unrelated command; standard profile checks validation relevance.
- If something is unverified, blocked, or skipped, say so plainly and record it with `goal pending add`. Prefer "unknown" over a plausible but unchecked claim.
- Don't limit yourself to the literal ask: if you notice a real, unrelated problem in files you're reading or touching (a bug, a security issue, clearly broken behavior), fix it if it's small and low-risk. If it's out of scope, risky, or you're unsure, don't silently skip it — record it with `goal pending add` and tell the user what you found, with a couple of concrete options and a recommended one. `rayman check` won't pass while that item is open.

## Degradation ladder — when `rayman` is missing

1. On PATH → use the commands above.
2. Built but not on PATH → call the binary by its `target/release` path, or `cargo run -p rayman --`.
3. Not built / unavailable → do the work manually: read current files, run the project's own tests, and record unfinished items in your reply. Do not claim gate-verified success, and do not go build the tool as a side quest unless asked.

## Boundaries

Programming workflows only. No host-app accounts, IDE assistant login, OAuth, or connector management. Operational task state stays workspace-local; checkpoint archives use a user-level store by default (or an explicit `--dir`), and there is no cross-project memory.
