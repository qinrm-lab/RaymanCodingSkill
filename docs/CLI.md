# CLI reference notes

The Clap schema and behavioral tests are authoritative. This page records the
public update/activation ordering that is easy to misuse from help text alone.

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
rayman workspace inspect
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

## Evidence boundaries

`check --profile release` is workspace strict quality, not an install or source
freshness claim. `doctor --check` proves the running CLI/PATH/workspace-skill
identity. `verify-release-contract.ps1 -RequireSourceFresh` additionally binds
the CLI and update worker to one locked isolated rebuild. A signed-release
worker receipt proves authenticated published bytes; it does not pretend those
bytes were rebuilt locally from the checkout.
