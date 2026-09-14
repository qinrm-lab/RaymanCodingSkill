# CLI behavioral contract

This repository-owned document is the normative semantic source for the public
CLI. The Clap schema is authoritative for exact syntax/help generation; tests
verify this contract and do not define it. This internal contract is not a host
transcript or an independent-reviewer attestation.

Every command must preserve UTF-8/Unicode paths and dynamic values, emit
locale-independent JSON when requested, and return nonzero rather than convert
an unknown, stale, malformed, or incomplete state into success. Simplified
Chinese and English human output must describe the same machine state.

## Update

```text
rayman update status
rayman update check
rayman update poll
rayman update configure --auto-check [--interval-hours N] --yes
rayman update configure --no-auto-check --yes
rayman update configure --auto-install --yes
rayman update configure --no-auto-install --yes
```

All update commands are activation-exempt and can run outside a Rayman
workspace. `status` is offline and read-only. On Windows, `check` performs one
explicit fixed-source network read without writing state; a due `poll` performs
the same discovery, updates only the user-level cache, and can report a
stable-version notification. The polling preference is enabled by default on
every platform, but that scheduling bit is not evidence that a discovery
transport exists. Non-Windows builds expose the same stable JSON schema:
`check` returns `unsupported_platform` without a write, while a due `poll`
records only its attempt timestamp and returns `unsupported_platform`. Neither
path makes a network request, reports a notification, or produces a worker,
because no reviewed discovery transport or signed automatic-install asset set
exists there. Installation consent is a separate bit and defaults false. Linux
source-checkout installation remains supported.

A poll can return `worker_launch` only when the running program is the exact
supported installed CLI, the receipt-bound versioned worker and all deployed
resources still match, production trust is configured, and the candidate is
strictly newer. Execute the returned program/argv directly. Never pass it
through a shell or substitute a path. Worker success reports
`restart_required=true`; restart Codex before treating the new adapter as
loaded. See [UPDATE_CONTRACT.md](UPDATE_CONTRACT.md).

## Activation currency

```text
rayman workspace status
rayman workspace inspect [--probe-writes]
rayman workspace ensure-current
rayman workspace ensure-current --yes
rayman workspace activate --skill-file <canonical-SKILL.md> --yes
rayman workspace deactivate --yes
```

`ensure-current` without `--yes` is read-only. With `--yes`, it calls only the
existing identity-only `rebind` transaction and repeats its checks under the
activation lock. It cannot activate orphan state, enable a disabled contract,
change `skill_file`, migrate a canonical path, or repair malformed/wrong-skill
state. After an installed upgrade, a non-read-only invocation uses
`update poll` first and `ensure-current --yes` only after no restart is pending.
Its JSON report fixes
`migration_scope=current_workspace_activation_identity_only`,
`project_files_changed=false`, and `other_workspaces_scanned=false`. It never
installs Rayman's own `xtask` or rewrites a consumer project's Python or other
automation; that broader migration requires an explicit project-local goal.
Activation binds seven fields: the skill path/hash, delegated agent/workflow
bundle hash, and CLI contract/version. A legacy six-field contract is inactive;
`ensure-current --yes` may add only the missing bundle identity when all three
current resources exactly match the bundle embedded in the running CLI.
`workspace inspect` and `doctor` are read-only by default; workspace source
inspection fixes `GIT_OPTIONAL_LOCKS=0` and passes `--no-optional-locks` to
every Git child. Add
`--probe-writes` only when you need the transient managed-state and
activation-metadata staging capability probes before a write-capable task.

## Evidence boundaries

`check --profile release` is workspace strict quality, not an install or source
freshness claim. `doctor --check` proves the running CLI/PATH/workspace skill
and delegated contract-bundle identity. Goal requirements and review names are
caller-authored records; no CLI hash upgrades them to a host-transcript or
independent-reviewer attestation. `verify-release-contract.ps1 -RequireSourceFresh` additionally binds
the CLI and update worker to one locked isolated rebuild. A signed-release
worker receipt proves authenticated published bytes; it does not pretend those
bytes were rebuilt locally from the checkout.

## Context, map, goal, state, and host integration

- `context status` is a cheap stat-only observation. `context refresh` is the
  content-hashed authority used by map/readiness checks; stale or unreadable
  content fails closed.
- `map` reports deterministic project structure, path-safe dependency/impact
  hints, and conservative quality results. Its heuristics never become test or
  completion evidence.
- `goal`, `prepare`, `check --goal`, and `finish` preserve caller-authored
  requirements, immutable baselines/plans, current receipts, lifecycle history,
  and the task/workspace distinction defined by the shared workflow contract.
- `checkpoint`, `state`, `assets`, and `temp` keep recovery,
  read-only inspection, and deletion authority separate. A status or scan never
  silently becomes cleanup authority.
- `codex-hook`, `doctor`, and `workspace` preserve the host/identity boundaries:
  UI state, elevation labels, and caller-supplied identity names are not proof of
  installation, permissions, delivery, or user awareness.

`goal archive --quarantine-invalid-history` is an explicit, one-way migration
for completed history whose proof is invalid and has no trusted archive route.
A current record must have no unresolved unbound/goal-bound pending, open lane,
or incomplete required package. Its must requirements must already be done with
recorded evidence. Replacement histories retain their original transfer proof
instead of inventing direct validation receipts. The pending lock remains held
through publication. Only lifecycle fields change; original requirement,
validation, plan, authority and extension data are preserved. The resulting
history cannot return to current or supply completion/replacement authority.

The exact command families above may delegate to focused modules, but every
public exit status and JSON object remains one end-to-end CLI integration
contract. Retired commands must fail with an actionable migration rather than
silently emulating current behavior.

## Retired scheduled snapshots

`rayman autosave` is retired. Automatic agent recovery belongs to the separate
save-work-status skill; Rayman does not invoke or configure that skill.
Manual `checkpoint save`, listing, verification and restoration remain available.
Old snapshots and the custom-store hint in `autosave.json` remain readable;
restoring old metadata does not register or reactivate a scheduled task.

Before upgrading a workspace that still runs an old `RaymanCheckpoint-*` task,
use the old installed CLI's `rayman autosave stop --status success` and verify
that its task is absent. Keep its snapshot store. The new CLI rejects old
start/tick/stop/status calls without changing files or scheduled tasks.
