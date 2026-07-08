---
name: raymancodingskill
description: Lean coding-workflow helper for a workspace that contains a .RaymanCodingSkill/ directory, or when the user explicitly invokes rayman. Provides workspace context indexing, a minimal goal contract with resumable pending work, a read-only obsolete-asset/TODO scan, and managed temp. Do not use for unrelated repos, host-app accounts, IDE assistant login, OAuth, or connectors.
---

# RaymanCodingSkill (lean)

A small, evidence-first coding helper. The `rayman` CLI keeps workspace-local state under `.RaymanCodingSkill/`; current files and command output are the source of truth, not cached summaries.

## When to use

Use in a workspace that has a `.RaymanCodingSkill/` directory, or when the user names `rayman`. Otherwise don't apply it.

## Task tiers — do the least that fits

- **Trivial** (typo, one-liner, doc tweak): just edit and run the project's own focused test. No `rayman` commands required.
- **Standard** (a feature or fix): `rayman context refresh` once up front, use `rayman map impact <path>` for non-trivial touched files, implement, run focused tests, then `rayman check` before you call it done; if it reports a blocker, don't call the task done until it's resolved or the user has explicitly accepted the open item.
- **Release / hand-off**: everything in Standard, plus resolve pending work and run the project's full build+test.

`rayman check` is the single readiness gate: it reports context freshness, obsolete-asset/TODO findings, and open pending items, and exits non-zero when there is a hard blocker.

## Core commands

- `rayman context refresh` — rebuild the index; unchanged files are reused, only changed files are re-hashed.
- `rayman context status` — cheap freshness check; run `refresh` if it says `stale`/`missing`.
- `rayman map summary|file <path>|symbol <name>|impact <path>` — build a project map from the current context index, then report modules, symbols, local dependencies, entrypoints, heuristic test candidates, risks, and suggested checks. It refuses stale/missing context.
- `rayman goal start "<title>" --must "<req>" [--should "<req>"]` — capture the task as a contract.
- `rayman goal evidence <id> --req <req_id> -m "<file + validation>"` then `goal close <id>` — success is refused until every `must` requirement has evidence.
- `rayman goal pending add|list|resolve` — carry unfinished work across sessions; never report done while pending items remain.
- `rayman assets` — read-only scan for obsolete-looking files and TODO/FIXME/未完成 markers. It never deletes anything; deciding what to remove is yours.
- `rayman temp scratch <label> | status | cleanup` — put throwaway runtime files under `.RaymanCodingSkill/tmp/`, not system temp.
- `rayman checkpoint save | list | status | restore` — snapshot the whole working tree (gitignore-aware) plus task state to a user-level store, keeping the newest few; for crash recovery and handing off between AI assistants. `restore` needs `--yes`.
- `rayman autosave start | stop | status` — start-of-session, `start` saves a snapshot and registers a Windows scheduled task that auto-snapshots every N minutes (default 30); on completion or error call `stop` to save a final snapshot and unregister. With auto-stop on (default), it self-stops once all goals are closed and no pending work remains. See `tools/README.md`.

## Evidence and honesty

- A `must` requirement is satisfied only by a current file path plus a validation command that actually ran and passed. Model confidence is not evidence.
- If something is unverified, blocked, or skipped, say so plainly and record it with `goal pending add`. Prefer "unknown" over a plausible but unchecked claim.
- Don't limit yourself to the literal ask: if you notice a real, unrelated problem in files you're reading or touching (a bug, a security issue, clearly broken behavior), fix it if it's small and low-risk. If it's out of scope, risky, or you're unsure, don't silently skip it — record it with `goal pending add` and tell the user what you found, with a couple of concrete options and a recommended one. `rayman check` won't pass while that item is open.

## Degradation ladder — when `rayman` is missing

1. On PATH → use the commands above.
2. Built but not on PATH → call the binary by its `target/release` path, or `cargo run -p rayman --`.
3. Not built / unavailable → do the work manually: read current files, run the project's own tests, and record unfinished items in your reply. Do not claim gate-verified success, and do not go build the tool as a side quest unless asked.

## Boundaries

Programming workflows only. No host-app accounts, IDE assistant login, OAuth, or connector management. State stays workspace-local; no cross-project memory.
