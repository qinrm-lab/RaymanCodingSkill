# Extracted Skill Rules

Source: `SKILL.md`

## Asset Retirement Protocol

- Treat obsolete assets as any repository content that keeps replaced behavior callable, documented, configured, tested, generated, packaged, routed, or presented to agents as current.
- Scope includes code, tests, docs, config, examples/fixtures, prompts/templates, scripts/tools, dependency manifests, CLI/API routes, migration notes, generated artifacts, caches, and managed temp/session state.
- Obsolete assets default to retirement and deletion from the current repository surface; git history or annotated backups are the preservation mechanism.
- Retained obsolete assets require `compatibility_exempt` state with an explicit retention reason and `expires_at` date, and they cannot satisfy current-behavior evidence.
- `.RaymanCodingSkill/assets/retirement.json` stores hash-backed retirement records in the current workspace root. User workspaces and the RaymanCodingSkill repository each use their own local state file; neither controller writes retirement state into the other workspace.
- `rayman assets status` reports candidates, references, exemptions, cleanup plan, controller scope, ignored roots, and blockers. `rayman assets scan` recomputes stale docs/config/tests/CLI/API references, ignores non-current roots such as `.git`, `target`, build caches, `.RaymanCodingSkill/tmp`, and generated `.RaymanCodingSkill/context` state, and writes the refreshed state.
- `rayman assets cleanup --apply` deletes only registered `retirement_candidate` files that still resolve inside the workspace and have no current references. It does not delete unknown files, user data, active `raymancodingskill` / `rayman.exe` assets, or directories without an explicit per-file manifest.
- `rayman assets retire --path <path> --replacement <text> --reason <text> --validation-command <cmd> --apply-delete` may delete only a whole file inside the workspace after reference checks pass.
- `rayman gate status --check`, `rayman goal close --status success`, and `rayman audit` fail while any retirement candidate, expired exemption, retired-present asset, or stale reference remains.
- Before removal, record current-file evidence: asset path, stale behavior, replacement/current behavior, direct references or callers, deletion reason, risk, and validation command.
- Default to reporting. Only explicit customer approval or `review --apply-prune` may write obsolete-asset pruning changes.
- Pruning may remove only review-identified obsolete assets. Do not delete unrelated content, active compatibility paths, user data, or unknown files.
- After retiring an asset, synchronize docs/config/tests/examples/prompts and verify no old entrypoint, stale reference, or dead test remains; run focused tests plus `rayman audit`.
