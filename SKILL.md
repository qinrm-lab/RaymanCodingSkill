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
- **Release / hand-off**: everything in Standard, plus resolve pending work and run `scripts/audit-repository.ps1 -CliPath <installed-application> -SkillPath <deployed-canonical-SKILL.md>`. That single lane includes the full root/evals suites, package/install smoke, strict/release self-dogfood, state/assets audit, and clean-source installed-release verification. `rayman check --profile release` alone remains only a workspace strict-quality claim.

`rayman check` defaults to `--profile standard`: it adds project-map freshness, error-level quality findings, validation relevance, and fail-closed goal checks to the base readiness snapshot. Use the explicit `check --profile quick` only for strong content freshness, asset findings, and pending work; it cannot support a delivery claim. Use `release` for strict policy. `rayman state audit --check` is a separate state-hygiene check; it is deliberately not run by `rayman check`.

## Core commands

- `rayman context refresh` — rebuild the index with content-hash proof; a file read error is recorded and cannot produce a ready index. Runtime state stays excluded, but the exact shared `.RaymanCodingSkill/quality.json` policy is indexed and participates in workspace fingerprints.
- `rayman context status` — cheap stat-only freshness UI probe; it does not prove content identity. Map and readiness commands use strong content-hash freshness; run `refresh` if any of them reports `stale`/`missing`.
- `rayman map summary|file <path>|symbol <name>|topology|impact <path>|plan <paths...> [--check]|quality [--profile standard|strict] [--check]` — build a project map from the current context index, then report modules, symbols, local dependencies, Cargo topology, test candidates, risks, and checks. Strict always blocks `large_file` and `high_fan_in`; policy can add but cannot replace defaults or raise the built-in missing-test threshold. An exemption must resolve now to one exact existing non-link regular file inside the workspace, one known kind, and a non-empty reason; directories/missing targets/globs/dot segments/symlink ancestors fail closed. Applied exceptions remain visible as `info` with provenance. **Symbol/test inference is Cargo/Rust-shaped**; outside Cargo, no-test-anchor heuristics stay advisory.
- `rayman goal start "<title>" --must "<req>" [--should "<req>"]` — capture the task as a contract.
- `rayman goal validate <id> --req <req_id> -m "<evidence>" --command "<command to execute>" [--changed <path>]` then `goal close <id>` — Rayman runs the command from the workspace root and records its zero exit code, output hashes, and before/after workspace fingerprints. Closing `success` requires at least one `must`, with every `must` done and carrying evidence; `check --profile standard` / `release` additionally require current receipts. `goal evidence --validated ...` is legacy attestation retained for migration; it cannot satisfy a current-schema standard/release success claim.
- `rayman goal pending add|list|resolve` — carry unfinished work across sessions; never report done while pending items remain.
- `rayman check --profile standard` — everything in `check`, plus project-map freshness, error-level quality findings, validation relevance for recorded `--changed` files, strict goal-state loading, closed-success evidence checks, and fail-closed active/partial/blocked goal handling.
- `rayman check --profile release` — workspace standard plus strict built-in quality and additive `.RaymanCodingSkill/quality.json` policy. Malformed/broad policy, unknown fields/kinds, duplicate entries, and blank exemption reasons are blockers. Its READY result never proves installed CLI identity or source freshness.
- `rayman assets` — read-only scan for obsolete-looking files and work-in-progress markers. It never deletes anything; deciding what to remove is yours.
- `rayman temp scratch <label> | status | cleanup` — put throwaway runtime files under `.RaymanCodingSkill/tmp/`, not system temp. `status` is read-only and recursively reports files, directories, bytes, and traversal errors; `cleanup` only removes that managed root.
- `rayman state audit [--check]` — read-only audit of allowed v2 state entries, retired entries, and recursive temp metrics. It never deletes files. `--check` exits nonzero when retired state, an audit error, or a traversal error needs review; obtain explicit approval before any migration or deletion.
- `rayman checkpoint save | list | status | verify [id|latest] | restore [id|latest] --yes` — cross-process locking protects save/restore/prune; failed or crashed staging is preserved for forensics and never auto-pruned. `verify` is read-only. Restore accepts only a verified complete manifest and uses durable same-directory staging plus atomic rename per file. The overlay is idempotently rerunnable but not an all-files transaction; it never deletes extra workspace files.
- `rayman autosave start | stop | status` — start-of-session, `start` saves a snapshot and registers a Windows scheduled task that auto-snapshots every N minutes (default 30); on completion or error call `stop` to attempt a final snapshot and unregister only after that snapshot succeeds. With auto-stop on (default), it self-stops only when at least one goal exists, every goal is `success`, and no pending work remains; `active`/`partial`/`blocked`, unreadable goal state, or no goals keep it running. On non-Windows platforms the scheduler is unsupported; use your system scheduler to call `rayman checkpoint save`. See `tools/README.md`.
- `rayman doctor [--check]` — report whether the running executable, the `rayman` application found on `PATH`, and the recorded `SKILL.md` hash form the same installed identity tuple. It does not prove source freshness. The handoff verifier requires explicit `-SkillPath`, rejects PowerShell shadows/known build-shaping env, compares an isolated locked rebuild, then terminally re-hashes every identity. It does not isolate or attest user/parent Cargo config; the claim is current active build-context byte identity, not hermetic provenance.

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
