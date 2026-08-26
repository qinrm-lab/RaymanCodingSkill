# Shared Rayman workflow contract

This reference is client-neutral. Codex and Claude Code must apply it after
reading the repository [AGENTS.md](../AGENTS.md). Native entry files only add
host integration details.

## Claims and authority

- Treat files and fresh command output as authority; summaries and model
  confidence are not evidence.
- A build creates an artifact. Only the supported installer plus identity
  checks supports an installed claim.
- Unbound `rayman check` proves workspace health only. Use `check --goal`
  or `finish --goal` for a task claim.
- `check --profile release` proves strict workspace quality, not installation
  or source freshness.
- A release handoff requires an installed application, deployed canonical
  skill, repository audit, and `verify-release-contract.ps1
  -RequireSourceFresh`.
- `scripts/release-closeout.ps1` may reuse a prior audit only when the clean
  HEAD, canonical workspace, CLI/SKILL bytes, release scripts, and native tool
  paths and hashes exactly match its evidence binding. Reuse never substitutes
  for the current goal's mandatory `--authority --repeat 2` execution.

## Standard workflow

1. Confirm `workspace status`, then create a baseline-bound goal with
   `goal start` and run `prepare --goal <id>`. Prepare checks the current goal
   schema and reconciles the goal-start baseline delta against the effective
   plan both before and after context refresh. It fails on uncovered paths or
   a changing snapshot; it never auto-extends or treats Git-HEAD status as the
   goal delta.
2. Before changing two or more files, persist one aggregate
   `goal plan <id> <paths...> --check`. Extend it before touching a newly
   discovered path.
3. Use `map impact`, `map plan --check`, and `map quality --check` for
   planning. Their output is heuristic, never validation proof.
4. Split broad work into required `goal package` units. For a 12-or-more-path
   Goal, required packages must collectively bind every must requirement; an
   empty required package cannot satisfy the split. Record
   source-bound `goal progress` receipts at recovery points; they remain
   non-authoritative.
5. Run focused project tests, then record actual direct executions with
   `goal validate`. Declare the real changed paths.
6. A high-priority plan needs `goal review` bound to the final source
   fingerprint. This is a caller-authored review record, not proof of an
   independent reviewer identity; claim independence only from a separately
   verified external attestation.
7. Record a recognized final repository gate with
   `--authority --repeat 2`, complete packages, close the goal, and run
   `finish --goal <id>`.

Use `--must-proof KIND::TEXT` for atomic mandatory proof. Supported kinds are
`generic`, `test`, `repository_gate`, `source_fresh`, `installation`,
`documentation`, and `git_commit`. A typed requirement accepts only a
matching validation command; split compound delivery claims into separate
requirements.

Goal titles, requirements, and review names are caller-authored. Their hashes
prove exact local text and source binding, not equivalence to the host
conversation, user confirmation, or an independent reviewer identity. Before
making a user-task or independent-review claim, compare the Goal with the live
request and require a separately verified external attestation for any identity
claim.

For release transfer, start a separate
`goal handoff start --from-goal <implementation-id> --commit <exact-sha>`.
The source goal must already be success with current repeated authority, and
the worktree must be clean at that exact HEAD. The handoff contract binds the
source goal contract, authority receipt, commit, workspace fingerprint, and
typed installation, repository-audit, and source-fresh stages.

## Owner mode and boundaries

Continue safe agent-owned work until the goal is genuinely finished. Ask only
for a proven human or external boundary: credentials, MFA/CAPTCHA, legal or
payment acceptance, destructive or irreversible authority, conflicting
product direction, or an exhausted external service.

Before waiting, record one complete solution package with `goal pending add`.
Every public human/external boundary is goal-bound and carries stable
`--capability-key` and `--boundary-class` identities; replaying the same
package is idempotent, while changing its contract under the same key fails.
Inspect `goal frontier`: `consultation=ready` only authorizes asking. Run `goal
pending render --current` and emit that exact client-neutral workspace aggregate
as the whole final response. Rendering itself creates no completion observation.
The active client's native adapter owns its completion boundary. In Codex, only
the current Codex Stop event may compare its exact `last_assistant_message` with
a freshly rendered aggregate; before allowing the stop it must re-list every
current goal and pending package and recheck the workspace fingerprint. Claude
Code must not execute or emulate that Codex hook. Any native observation is
transient and never becomes a persisted or reusable receipt. Never claim that
it proves delivery, visibility, reading, or user awareness. Background
continuation additionally needs explicit authority, mechanism, and
workspace-isolation evidence. Never report done while pending work remains for
a current goal. Goal-bound pending attached to an archived or superseded goal
remains visible as audit history but does not re-enter current readiness;
unbound legacy pending remains a global blocker.

Commit, push, publish, install, deploy, account changes, payment, deletion, and
overwriting user-managed state still require user authority.

## State, checkpoints, and concurrency

- Operational state lives under `.RaymanCodingSkill/`; the tracked
  `quality.json` policy is the exception.
- Installation never scans for or automatically rewrites activation contracts
  in other workspaces. An exact-identity upgrade may therefore leave a prior
  binding invalid until that workspace explicitly runs `workspace rebind
  --yes`.
- A non-read-only explicit Rayman invocation begins with activation-exempt
  `rayman --format json update poll`. The polling preference is enabled by
  default and writes only the user-level update cache. Windows has the reviewed
  stable-version discovery transport; non-Windows due polls return the
  zero-network `unsupported_platform` boundary and claim no notification
  capability. Read-only work uses `update status` and performs no poll/cache
  write. A tag, release API result, prompt, or cached observation is untrusted
  notification data and can never authorize a download, worker, or installation.
- Automatic installation has an independent `auto_install=false` default and
  requires `update configure --auto-install --yes`. Legacy auto-check state is
  always migrated with install consent false. Trusted apply additionally
  requires the exact supported-install receipt, managed CLI and versioned
  worker identities, a canonical manifest signed by the compiled Ed25519 root,
  monotonic key/sequence/version state, and every fixed-role asset size/hash.
  Missing, expired, replayed, equivocated, malformed, or unverifiable input is
  fail closed with no installation write.
- The versioned update worker is a direct program plus argv returned by poll;
  never execute it through a shell string or accept a remote path/URL. It
  holds the installation-scoped OS mutex, stages under a creation-bound
  no-reparse transaction root, and publishes the full CLI/worker/skill/receipt
  tuple through the installer-equivalent handle-relative CAS and durable
  journal. It does not change PATH, hooks, or any workspace activation. A
  successful apply reports `restart_required=true`; the old loaded adapter
  cannot claim the new runtime current. After restart, only the current
  workspace may run eligible `workspace ensure-current --yes`.
- Rebind is a narrow identity migration: it accepts only an existing complete
  bundle binding or legacy six-field binding whose defects are limited to
  skill/bundle hash, CLI contract, or CLI version drift and whose current skill,
  agent contract, and workflow contract match the canonical bundle embedded in
  the running CLI. It updates only those identity scalar values under the same
  state lock used by activate/deactivate, preserving `skill_file`, comments,
  field order, quoting, and line endings except for inserting the missing
  `bundle_sha256` during the one-way legacy migration.
  It refuses orphan, deactivated, malformed, wrong-skill, untrusted/missing-file,
  unsafe-path, and path-change cases. New activation or a canonical-path change
  uses `workspace activate --skill-file ... --yes`.
- Post-install `workspace ensure-current --yes` reuses only that rebind
  transaction for the current workspace. Its machine report fixes
  `migration_scope=current_workspace_activation_identity_only`,
  `project_files_changed=false`, and `other_workspaces_scanned=false`. It never
  installs this repository's `xtask` or rewrites Python/other automation in a
  consumer project; such a project migration needs a separate explicit goal.
- When the user explicitly invokes Rayman for a non-read-only task, perform an
  eligible rebind and continue the original task. A read-only request does not
  authorize the state write and receives the recovery command instead.
- The Stop Hook cannot infer whether a request was read-only and never writes
  the activation contract. It sends safely rebindable drift through normal
  goal/frontier checks: no-goal and legitimately paused/complete work may end,
  but unfinished goals and structurally invalid activation remain fail-closed.
- `state audit --check` is read-only. Structural failure remains separate from
  non-blocking capacity and crash-recovery warnings. Do not delete state to
  make it pass.
- `checkpoint save` is lossless by default. Use explicit prune/retention
  policy for deletion. `salvage-save` is recovery-only and never completion
  evidence.
- Restore is journaled and all-or-nothing; it never deletes extra workspace
  files.
- `goal lane` coordinates scoped-writer, advisory read-only, and
  final-reviewer work. Read-only and final-reviewer lanes are zero-write
  brackets that reject any workspace change. In a shared worktree, do not keep
  one open while the main agent or another writer mutates source; close it
  before mutation or use an isolated checkout. An open lane blocks closing the
  goal as `success`; to keep the goal deliverable, discharge a drifted lane by
  reverting the drift or by `supersede`. A non-success close (`partial` /
  `blocked`) followed by `archive` retires the goal as history with the lane
  violation intact — the archived record never feeds readiness or authority,
  but the drift itself is not reverted by retiring it. Lane records never
  replace validation.
- Host-level chat concurrency is stricter than goal lanes: one physical Git
  worktree has one writer. Before mutation, inventory active tasks and current
  worktrees when concurrent work is plausible. A second writer must use a
  dedicated worktree on a separate branch; a task that finds another writer
  stops without cleanup, restore, pruning, tests, staging, or commit. A task
  may not delete another task's worktree merely because it is uncommitted or
  absent from its own goal state.
- Give concurrent pytest runs independent managed leases and traversable temp
  roots.

## Sandbox and permission boundaries

- Elevation transport and OS identity are different capabilities. Read the
  `workspace inspect` / `doctor` execution-context probe before repeating an
  ACL or profile-bound action. `principal_fingerprint` proves only the SID,
  not token permissions: a principal-bound retry needs new SID evidence, a
  profile-bound retry needs new profile evidence, and an ACL-bound retry needs
  an action-specific permission probe. Elevated PowerShell, COM, Terminal, or
  Task Scheduler labels alone prove none of those changes. Optional diagnostic
  comparisons use `RAYMAN_REQUIRED_SID`, `RAYMAN_REQUIRED_PRINCIPAL`, and
  `RAYMAN_REQUIRED_PROFILE`; their provenance is untrusted process environment
  and they never authorize a privileged action.

- A logged-on-user identity broker is a narrow delegated capability, not an
  elevation shortcut. Keep ordinary PowerShell in the `elevated` sandbox. A
  broker may run only a protected, installed operation identifier; its request
  schema, expiry, worker hash, executor SID, result path, and ACL must be
  verified. It must reject arbitrary command text, script paths, dynamic argv,
  administrator run level, secrets, and destructive actions. Installation or
  removal needs explicit user authority and a real InteractiveToken loopback.

Restricted host sandboxes (for example the Codex Windows restricted token)
can deny writes even inside the workspace. Known boundaries: state lock files
under `.RaymanCodingSkill/`, `.git` writes, spawning `git`, the user-profile
checkpoint root, and user-profile toolchain caches such as the Cargo advisory
database all surface as ACL / `os error 5` denials. Treat them as environment
boundaries, not lock contention and not source defects.

- Probe before long write-capable work with `rayman doctor --probe-writes` or
  `rayman workspace inspect --probe-writes`. Without that flag both commands
  are read-only and report the probes as not run. When an opted-in probe reports
  a permission denial, run known
  state-writing commands (goal/checkpoint/autosave transactions, git
  stage/commit, installers, repository gates) with escalated host permission
  from the first attempt instead of probe-fail-retry loops.
- The opted-in `state_write` probe proves only ordinary create/write/remove capability
  under `.RaymanCodingSkill/tmp`; it is not evidence that activation metadata
  can be preserved. With `--probe-writes`, `doctor` and `workspace inspect` report a
  separate `activation_metadata` probe that stages the real activation
  owner/group/ACL and platform attributes, verifies them through the held
  handle, cleans the stage, and rechecks that `workspace_skill.yaml` did not
  change. Only `activation_metadata.ready=true` is action-specific evidence
  for retrying an owner-preserving activation write in the current execution
  context. The result is diagnostic, not cached authority: `activate`,
  `rebind`, and `install-bind` still revalidate inside their own transaction.
- Escalation cannot fix every host defect. A broken host patch tool or an
  over-long command line fails identically when escalated: split over-long
  command lines, record the boundary once in the goal environment notes, and
  stop retrying the broken tool.
- Separate host misconfiguration from a broken tool. A Codex `unelevated`
  Windows sandbox refuses whole classes of profile — split writable roots,
  split filesystem reads, deny-read — so `apply_patch` fails before reading
  the target. That is fixed by `[windows] sandbox = "elevated"`, not by a
  workaround; surface it to the user as a one-line host fix and keep working
  through the fallback meanwhile.
- `Access is denied.` from `apply_patch.bat` is a second, unrelated failure.
  Codex generates that shim as a direct absolute-path invocation of its own
  executable; when the running Codex is the MSIX/Store build the target sits
  under `C:\Program Files\WindowsApps\`, which Windows refuses to start by
  path regardless of the caller — an ordinary interactive user is denied
  too, so this is neither an ACL to repair nor a sandbox effect, and
  escalation cannot help. Diagnose it by reading the newest
  `~/.codex/tmp/arg0/*/apply_patch.bat`: a `WindowsApps` target fails every
  call for the whole session, a `%LOCALAPPDATA%\OpenAI\Codex\bin\...` target
  works. The only fix is to run Codex from the non-MSIX install; report that
  and fall back to `git apply` meanwhile.
- A third `apply_patch` signature exists and neither fix above applies to it:
  `windows sandbox failed: helper_unknown_error: setup refresh had errors`,
  observed on a host already set to `sandbox = "elevated"`. Treat any
  `apply_patch` failure that names the sandbox helper as a host boundary,
  report it once, and move to the fallback rather than re-attempting.
- Apply patches from a file, never from stdin or a shell here-string: host
  quoting mangles them into `corrupt patch`. Write UTF-8 (no BOM) with LF
  endings to managed temp, then `git apply --whitespace=nowarn <file>`.
  Re-read the target immediately before generating a hunk, and prefer a
  whole-file rewrite over hand-computed `@@` line counts.
- "Managed temp" means the directory `rayman temp scratch <label>` prints —
  nothing else is guaranteed writable. A restricted sandbox denies ordinary
  scratch locations such as `C:\tmp` or `%TEMP%` with
  `Access to the path '...' is denied`, which reads like a permission bug but
  is only the wrong directory.
- The `git apply` fallback does not exist in a workspace that is not a Git
  worktree, and rayman supports such workspaces. There, re-read the target,
  rewrite the **whole file** through the host's file-write tool, and re-read it
  again to confirm. Do not splice a multi-line region with regex or
  exact-string replacement: a near-miss silently produces syntactically broken
  source that only the compiler catches, and the failed build then has to be
  separated from the real work.
- Gates fail closed on environment permission boundaries. Never stitch a
  partial pass; rerun the entire gate with sufficient permission.
- `rayman checkpoint save` and autosave default to a user-profile root. In a
  workspace-only sandbox pass `--dir` under the workspace or escalate. Adding
  that root to the host's writable roots fixes it once for every session.
- An explicit operator pause ("stop now, I am shutting down") overrides Owner
  Mode without becoming success or an external blocker. Record one immediate,
  goal-bound human solution package with `boundary_class=operator_pause`, a
  stable capability key, the user's pause request as the attempt/evidence, the
  minimum input `resume`, the exact resume command, and the condition under
  which work may resume. Render the current aggregate and allow the Stop guard
  to end the turn. Remaining agent work stays unfinished and resumes only on a
  later user instruction.

## Validation integrity

`goal validate` runs one program plus argv from the workspace root. Shell
control operators, nested shell hosts, nonzero exits, source mutation, stale
fingerprints, irrelevant scopes, and zero-test receipts fail closed. Pytest
requires independent collect proof and a matching terminal summary. A pure
zero-delta audit uses the authority-only `--workspace-snapshot` scope: the goal
baseline delta must be empty before any validation program starts, and any real
change still requires ordinary `--changed` coverage.

Every Rayman-managed physical pytest process is isolated automatically. The
recognized `pytest`, `python -m pytest`, and `py -3.x -m pytest` forms each
create and probe a fresh manifest-owned lease before spawn; collect, every run,
and every authority repeat use different leases. Rayman inserts `--basetemp`
and `-o cache_dir=...` before the user's `--`, then injects a final `-o
addopts=` so repository configuration cannot narrow or disable the run. It
clears inherited `PYTEST_ADDOPTS` and sets `TEMP`, `TMP`, `TMPDIR`, and
`PYTHONPYCACHEPREFIX` to lease-owned paths. A user command that overrides
`--basetemp`, `cache_dir`, or `addopts` before the separator is rejected before
execution, including an override hidden in a valid short-option cluster.
Pre-separator `@argsfile` expansion and Python `-E`, `-I`, or
`-X pycache_prefix=...` are also rejected because they can bypass inspection or
the managed pycache environment. Lease release is attempted after success,
nonzero exit, collection failure, and spawn failure; a release failure fails
validation and prevents a receipt. On Windows, release reads the manifest from
the same held leaf handle that authorizes deletion and requires the full
manifest to match the lease created for that physical process; a missing,
replaced, or mismatched lease fails closed. Receipts and invocation hashes
retain the user's logical command and never persist the effective argv,
environment, randomized lease id, or lease paths.

On native Windows, every other Rayman-managed validation child also receives a
fresh manifest-owned host-temp lease, including test listing, every repeat,
goal progress, repository gates, and the internal `cargo metadata` used before
self-hosted Cargo isolation. `TEMP`, `TMP`, and `TMPDIR` all name the probed
leaf for that one physical process. Pytest does not create a redundant generic
lease: its existing richer lease is relocated to the same external host root
and remains the sole owner of its temp, basetemp, cache, and pycache paths. The
external root contains two marker-free sibling trees, `v/<id>` for generic
validation children and `p/<id>` for external pytest; these compact physical
aliases are manifest-owned implementation details, not an operator-facing
path contract. It never creates a `.RaymanCodingSkill` directory. A configured
root nested below a pre-existing
`.RaymanCodingSkill` or `.git` marker is rejected before any lease directory is
created, so temporary descendants cannot mistake the cleanup authority root for
a workspace.
Inside each generic Windows validation lease, compact `t` and `n` directories
are separate siblings bound by the same manifest and held leaf identity. The
physical child receives `TEMP`/`TMP`/`TMPDIR=<lease>/t` and
`RAYMAN_VALIDATION_TEMP_ROOT=<lease>/n`. A test may therefore
create a temporary workspace below its process temp and run Rayman recursively
without making the cleanup-authority root an ancestor of that workspace. Each
nested Rayman process repeats the same layout one level deeper; the outer lease
still owns and releases the whole tree. External managed pytest uses compact
`b`, `c`, `t`, `y`, and `n` directories for basetemp, cache, process temp,
pycache, and nested validation respectively. The operator-only Windows
workspace-local pytest lease retains its compatibility layout
`basetemp`, `cache`, `temp`, `pycache`, and `nested-validation`; it does not
publish its marker-contained nested-validation directory as validation authority.
Release completes before output parsing or receipt creation; success, nonzero
exit, spawn failure, panic unwind, manifest mismatch, or cleanup failure can
therefore never turn an unverified or live lease into evidence.
Every Windows validation-owned lease leaf is created exclusively and its
no-follow directory handle remains open from creation through child exit.
Release compares both legacy and 128-bit strong identity from that held object
with the current parent-handle-opened leaf before reading the manifest or
deleting anything. A child that renames the original leaf and clones its
manifest into a same-named replacement therefore leaves both objects
fail-closed and cannot mint a receipt. The same creation-identity token also
protects the self-hosted Cargo target lease.

The durable local-Codex default uses two sibling directories below the dedicated
`E:\codex-sandbox` data root: `TEMP`, `TMP`, and `TMPDIR` use
`E:\codex-sandbox\temp`, while `RAYMAN_VALIDATION_TEMP_ROOT` uses
`E:\codex-sandbox\rayman-validation`. Neither may be `E:\` or another volume
root. Keeping ordinary process temp separate from the Rayman lease root allows a
temporary workspace to remain disjoint from the cleanup authority that validates
it. The user-level `~/.codex/config.toml` may set these values for all newly
started local Codex projects. The active permission profile must independently
grant write access to each managed leaf, either directly or through its dedicated
non-volume parent; shell environment values do not grant filesystem permission.
Use
`scripts/configure-codex-validation-temp.ps1 -Check|-Yes` to inspect or apply
this contract. The script must preserve unrelated TOML bytes and structure,
refuse unknown or incompatible permission layouts, back up and roll back a
concurrent or invalid write, and never enable Full access, reset ACLs, or grant
the whole E drive. A changed user-level configuration takes effect only after
Codex is fully restarted. Rayman still resolves, probes, strongly revalidates,
and releases every physical-process lease at runtime; it rejects an empty,
relative, reparse-backed, volume-root, workspace-overlapping, or inaccessible
configured root instead of silently falling back. Non-Windows validation keeps
its inherited process environment unchanged.

On Windows, validation also protects a self-hosted Rayman CLI from Cargo's
running-image lock. The effective target comes from a non-empty
`CARGO_TARGET_DIR`, or from locked/offline `cargo metadata` so workspace
configuration and an explicit `--manifest-path` cannot silently move it. The
running image is matched by canonical path and strong Windows file identity,
including an NTFS hard-link alias. When that image is a direct artifact under
the effective target, one outer validation operation creates and probes a
unique manifest-owned Cargo target at `.RaymanCodingSkill/tmp/c/<id>/t`.
Its root, target child, and internal lease label stay deliberately concise so
Cargo's deeper build-script and package outputs retain enough path budget for
MSVC tools that are not fully long-path compatible.
Independent test listing and every authority repeat receive that same absolute
target through their child-process environment, so the first build is cached
without letting a concurrent validation relink its running CLI. The lease is
revalidated before each spawn and released before any receipt is written;
execution or cleanup failure therefore cannot mint evidence. Non-Windows and
non-self-hosted executions preserve the inherited target exactly and create no
lease. A direct Cargo command whose `--target-dir` overlaps the running image,
or whose `--config` can make the effective target ambiguous, fails before
spawn because those command-line settings can override environment isolation.
Receipts continue to bind only the user's logical command: the physical target,
lease id, and injected environment are never persisted.
Cleanup opens the Windows workspace root once, opens every later component
relative to its held parent with `OBJ_DONT_REPARSE`, and enumerates directories
from their handles. Enumeration IDs are cross-checked with both legacy and
128-bit handle identities; the lease manifest is read through the same held
leaf handle that authorizes the transaction. Files and empty directories are
removed only with `FileDispositionInfoEx` on their verified handles. Namespace
renames are deliberately allowed and detected by re-opening every name from its
parent handle, so an ancestor/leaf replacement, junction/reparse entry,
irregular entry, malformed enumeration, or concurrent refill leaves a
fail-closed orphan and prevents evidence instead of widening the deletion
target.

Authority accepts only a reviewed repository gate, the exact source-bound Rust
entrypoint `cargo run --locked --manifest-path xtask/Cargo.toml --
repository-gate`, selector-free workspace Cargo tests, or selector-free
workspace pytest, repeated on one unchanged fingerprint. The convenient
`cargo xtask` alias is never authority: aliases are configuration-overridable.
The exact xtask gate binds root Cargo configuration plus the baseline/current
union of its complete source tree and every delegated repository script, so an
added, deleted, or changed gate dependency cannot validate itself. Current must
receipts must collectively declare the real baseline delta. Evidence-only text,
progress receipts, and advisory review cannot substitute for authority.

If the CLI is missing, prefer PATH, then a built release binary, then
`cargo run -p rayman --`. If none is available, work manually and report the
missing gate plainly; do not claim Rayman-verified completion.
