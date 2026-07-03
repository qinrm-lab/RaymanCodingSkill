# Extracted Skill Rules

Source: `SKILL.md`

## Asset Retirement Protocol

- Treat obsolete assets as any repository content that keeps replaced behavior callable, documented, configured, tested, generated, packaged, routed, or presented to agents as current.
- Scope includes code, tests, docs, config, examples/fixtures, prompts/templates, scripts/tools, dependency manifests, CLI/API routes, migration notes, generated artifacts, caches, and managed temp/session state.
- Obsolete assets default to retirement and deletion from the current repository surface; git history or annotated backups are the preservation mechanism.
- Retiring a feature, declaration, command, API, or claim also retires tests whose only purpose was proving that retired surface. `rayman assets scan` registers isolated obsolete Rust test cases and matching feature-coverage `test_anchors` as cascade retirement candidates, and `rayman assets cleanup --apply` deletes those test cases plus orphan anchors before deleting the now-unreferenced retired asset. If the test case is already gone, cleanup may delete the pure orphan feature-coverage anchor by itself. If a stale test reference cannot be isolated to one test case or mixes current and retired proofs, cleanup reports `manual_test_case_prune_required` and gates remain blocked until the test is deleted, rewritten for current behavior, or explicitly exempted.
- Retained obsolete assets require `compatibility_exempt` state with an explicit retention reason and `expires_at` date, and they cannot satisfy current-behavior evidence.
- `.RaymanCodingSkill/assets/retirement.json` stores hash-backed retirement records in the current workspace root. User workspaces and the RaymanCodingSkill repository each use their own local state file; neither controller writes retirement state into the other workspace.
- `rayman assets status` reports candidates, cascade candidates, references, exemptions, cleanup plan, controller scope, ignored roots, and blockers. `rayman assets scan` recomputes stale docs/config/tests/CLI/API references, adds cascade test retirement candidates, registers orphan feature-coverage `test_anchors[].proves` as semantic cascade candidates, ignores non-current roots such as `.git`, `target`, build caches, `.RaymanCodingSkill/tmp`, and generated `.RaymanCodingSkill/context` state, and writes the refreshed state.
- `rayman assets cleanup --apply` deletes only registered `retirement_candidate` files that still resolve inside the workspace and have no current references, plus registered isolated cascade test-case line ranges and their orphan feature-coverage anchors. Pure orphan feature-coverage anchors may be removed even when the referenced obsolete test case is already absent. It does not delete unknown files, user data, active `raymancodingskill` / `rayman.exe` assets, or directories without an explicit per-file manifest.
- `rayman assets retire --path <path> --replacement <text> --reason <text> --validation-command <cmd> --apply-delete` may delete only a whole file inside the workspace after reference checks pass.
- `rayman gate status --check`, `rayman goal close --status success`, and `rayman audit` fail while any retirement candidate, expired exemption, retired-present asset, or stale reference remains.
- Before removal, record current-file evidence: asset path, stale behavior, replacement/current behavior, direct references or callers, deletion reason, risk, and validation command.
- Default to reporting. Only explicit customer approval or `review --apply-prune` may write obsolete-asset pruning changes.
- Pruning may remove only review-identified obsolete assets. Do not delete unrelated content, active compatibility paths, user data, or unknown files.
- After retiring an asset, synchronize docs/config/tests/examples/prompts and verify no old entrypoint, stale reference, or dead test remains; run focused tests plus `rayman audit`.
