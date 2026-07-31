# Strict quality policy

`standard` reports maintainability warnings but does not promote them, and it
does not read the workspace policy file at all: promotions, exemptions, and
threshold overrides apply to `strict`/`release` only. `strict` and `release` always promote the built-in `large_file` and `high_fan_in` kinds. A workspace policy at `.RaymanCodingSkill/quality.json` is additive: `block_warning_kinds` can promote more known warning kinds, but an empty or partial list cannot remove built-in defaults.

The policy file is the one indexed file under `.RaymanCodingSkill/`. Its content hash participates in context freshness and workspace fingerprints; after editing it, run `rayman context refresh` and regenerate validation evidence.

An exemption has three mandatory fields:

```json
{
  "path": "generated/schema.rs",
  "kind": "large_file",
  "reason": "Exact generated file; schema snapshot and package tests validate it."
}
```

`path` must resolve now to one exact existing ordinary file inside the workspace. Absolute/missing/directory paths, symlink or reparse ancestors, backslashes, dot segments, and glob metacharacters are rejected. `kind` must be known and `reason` non-blank. Duplicate kinds or duplicate `(path, kind)` entries are rejected. An exemption keeps the finding visible as `info`; it does not hide it or authorize future files.

`multi_source_no_test_min_sources` is tightening-only. Values below the built-in default lower the threshold; values above it are capped at the default and cannot switch off the missing-tests error.

JSON quality output makes policy origin auditable:

- `strict_default_block_warning_kinds` lists built-in strict defaults.
- `configured_block_warning_kinds` lists workspace additions.
- A promoted or exempted finding carries `blocking_policy_source` (`strict_default` or `workspace_config`).
- An applied exact exemption also carries `exemption_reason` and is sorted with informational findings.

Never add a directory/glob escape hatch to the engine. A repository-specific intentional fixture or shared hub must use one exact entry with a reviewable reason, and strict CI must still run the applicable broad tests.
