# Context, project-map, asset, toolchain, and strict-quality policy

This repository-owned document is the normative source for the context/quality
test family. Content indexing hashes every included byte and records read
failures. Cargo and pyproject discovery, topology, symbol lookup, dependency
impact, test anchors, and tool application resolution must be deterministic,
path-contained, and conservative when topology or ecosystem support is
incomplete. Impact and quality output is heuristic guidance, never validation
authority.

The read-only asset scan reports obsolete filename patterns and authored work
markers without deleting them; strings used as test fixtures are not themselves
cleanup authority. Toolchain/application discovery distinguishes a missing,
ambiguous, shadowed, or non-application command instead of treating a basename
as identity.

## Strict profile

`standard` reports maintainability warnings but does not promote them, and it
does not read the workspace policy file at all: promotions, exemptions, and
threshold overrides apply to `strict`/`release` only. `strict` and `release` always promote the built-in `large_file` and `high_fan_in` kinds. A workspace policy at `.RaymanCodingSkill/quality.json` is additive: `block_warning_kinds` can promote more known warning kinds, but an empty or partial list cannot remove built-in defaults.

The policy file is the one indexed file under `.RaymanCodingSkill/`. Its content hash participates in context freshness and workspace fingerprints; after editing it, run `rayman context refresh` and regenerate validation evidence.

An exemption requires an exact path, kind, reason and positive reviewed metric limit to apply:

```json
{
  "path": "generated/schema.rs",
  "kind": "large_file",
  "reason": "Exact generated file; schema snapshot and package tests validate it.",
  "reviewed_max": 2400
}
```

`path` must resolve now to one exact existing ordinary file inside the workspace. Absolute/missing/directory paths, symlink or reparse ancestors, backslashes, dot segments, and glob metacharacters are rejected. `kind` must be known and `reason` non-blank. Duplicate kinds or duplicate `(path, kind)` entries are rejected. An applied exemption keeps the finding visible as `info`; it does not hide it or authorize future files. `reviewed_max` bounds physical lines for `large_file`, incoming dependency edges for `high_fan_in`, and indexed public symbols for `public_api_without_test_evidence`. The measurement uses the same current project map as the finding. At the limit the exemption applies; above it the normal warning/promotion policy applies. Missing, null or zero limits grant no exemption, so older reason-only files remain readable but must be reviewed before they can waive findings. Unknown metrics or missing module evidence never grant an exemption. Do not automatically increase a limit when a file grows; review and reduce the growth first.

`multi_source_no_test_min_sources` is tightening-only. Values below the built-in default lower the threshold; values above it are capped at the default and cannot switch off the missing-tests error.

JSON quality output makes policy origin auditable:

- `strict_default_block_warning_kinds` lists built-in strict defaults.
- `configured_block_warning_kinds` lists workspace additions.
- A promoted or exempted finding carries `blocking_policy_source` (`strict_default` or `workspace_config`).
- An applied exact exemption also carries `exemption_reason` and is sorted with informational findings.

Never add a directory/glob escape hatch to the engine. A repository-specific intentional fixture or shared hub must use one exact entry with a reviewable reason, and strict CI must still run the applicable broad tests.

## CLI integration suite

`crates/rayman/tests/cli.rs` owns the shared process/fixture helpers. Its
`cli_cases/` children group navigation, project maps, readiness, goal evidence,
validation processes, lifecycle, pending boundaries, workspace activation, host
integration, recovery and updates. They compile into the same Cargo `cli` test
target; run `cargo test --locked -p rayman --test cli` for the complete suite.
Splitting files must preserve test bodies, platform conditions and runtime test
count, and retire/rebind exact inventory paths without losing history.

## Script validation ownership

The complete repository gate directly runs source-byte and traceability
publication self-tests, traceability mutation self-tests and the live runtime
inventory. The release audit owns its corresponding direct checks. Rust audit
tests verify orchestration and unique behavior instead of launching those same
script suites a second time. A Cargo-only pass is not a complete repository
gate: use `scripts/check-repo.ps1` for the script and runtime inventory checks.
