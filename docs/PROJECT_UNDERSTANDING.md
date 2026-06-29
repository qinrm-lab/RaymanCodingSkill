# Project Understanding

RaymanCodingSkill handles long-context project understanding through fresh, workspace-local, hash-backed context. It must not rely on long-lived model memory, cross-project assumptions, cached summaries, or auxiliary AI output as facts.

## Protocol

Use this sequence for non-trivial implementation, review, refactor, or documentation-sync work:

```text
rayman context status
rayman context os --check
rayman context task "<task>"
rayman assets status
rayman project detect
rayman impact --path <changed-or-reviewed-file>
rayman regression plan --path <changed-or-reviewed-file>
```

The layered flow is:

1. Read the workspace Context Index as a navigation layer.
2. Check the workspace Context OS state graph and event log. Missing or stale state means `.RaymanCodingSkill/context/state.json` no longer matches current index, goal/session, asset, audit, or pending-work evidence.
3. Use task-scoped context to identify likely files, symbols, entry points, and verification commands.
4. Check asset retirement state; obsolete, retired, or compatibility-exempt assets are not current-behavior evidence.
5. Reread the referenced current source, docs, configs, manifests, and command output from disk before editing or review.
6. Refresh stale context when hashes differ, files are added, files are removed, or indexed evidence conflicts with current files.
7. Record completion evidence from current files and validation output for every `must` requirement.

## Stale Context

`rayman context status` reports stale or missing Context Index state. When it reports stale context:

```text
rayman context refresh
rayman context os --write
rayman context task "<task>"
```

Then reread the files listed under changed, missing, or new file details. Cached context is still only a map; it never replaces current source files, tests, command output, or goal/session state.

## Context OS State Graph

`rayman context os --write` derives `.RaymanCodingSkill/context/state.json` and appends `.RaymanCodingSkill/context/events.jsonl` from the current workspace root. The state graph connects current files, Context Index freshness, task context, asset retirement, goal/session pending work, audit findings, and the derived Context OS snapshot. `rayman context os --check`, `rayman context status --check`, `rayman gate status --check`, `goal close --status success`, and `session close --status success` fail when the state graph is missing or stale. This is the supported stronger Context OS form; it remains local files, not a daemon, database, vector index, or cross-project memory.

## Obsolete Assets

Asset retirement state stays workspace-local under `.RaymanCodingSkill/assets/retirement.json`. Normal task context excludes `retirement_candidate`, `retired`, and `compatibility_exempt` paths from current evidence; cleanup tasks must use the dedicated `asset_retirement` report. Retained obsolete assets need an explicit compatibility or audit reason plus an expiry date. Unresolved candidates, expired exemptions, retired assets still present, or stale references block success and audit.

## Customer Workspaces

Opted-in customer projects use the same protocol through `.RaymanCodingSkill/workspace_skill.yaml`. They get their own workspace-local Context OS state under `.RaymanCodingSkill/context/` and never reuse the RaymanCodingSkill repository state. They do not need a daemon, database, vector index, or cross-project memory.

## Completion Evidence

Goal success requires evidence that agrees with the current workspace:

- context status/task checked;
- Context OS state graph checked or refreshed;
- asset retirement blockers handled or confirmed clear;
- current source/docs/config files reread;
- stale index handled or confirmed ready;
- impact and regression planning recorded for touched paths;
- validation commands run and results recorded.
