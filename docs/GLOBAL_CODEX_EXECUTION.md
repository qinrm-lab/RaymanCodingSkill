# Global Codex execution

Deployment is explicit. A source checkout alone does not attest an installed
worker, migrated projects or a functioning desktop hook.

## Ownership

The sandbox owns ordinary source editing, builds and tests. A protected,
least-privilege desktop-user worker owns formal workflow state and enrolled
local commit/install operations. Its client must never send an arbitrary
program, script, command string or destination path.

Repository policy remains authoritative. Enrollment is not automatic consent
for every future operation, and a trusted Codex project is not automatically
an enrolled desktop-user capability. Persistent grants apply only within
their recorded operation, target, identity and side-effect boundaries.

## Delivery sequence

1. Preflight actual capabilities before changing source.
2. Choose the version before the final source validation.
3. Freeze exact source, validate, then submit the exact commit request.
4. Install only through the enrolled protected adapter.
5. Verify the installed tuple and record the source-bound outcome.
6. Run or reuse only contract-valid release stages, then finish the task.

The existing repeat-2 release requirement remains in force until its
replacement contract and execution evidence have passed their own tests.

## Migration

Inventory the real state and installed broker manifests first. Back up and
verify each state store before changing its writer. Existing repositories and
worktrees are enrolled separately, with a common lock for shared Git storage.
Old workers are retired only after the replacement has passed real identity,
commit, state, interruption/recovery and installation checks. Backups are
retained. A failed migration leaves an explicit recoverable boundary.

## Implemented isolated components

The standalone `rayman-global-execution` crate provides a GUI worker and a
console client. Rayman forwards typed operations to that installed client;
it does not link the worker into its application workflow. Requests never
select arbitrary programs, scripts or privileged destination paths.

Protected enrollment, exact local commits with added/modified/deleted files,
raw source hashes, Git object hashes, ref/index recovery, queue fault isolation,
and leased Rayman Goal/pending/context writes have isolated execution tests.
Malformed, unreadable and conflicting queued packets are reported separately;
completed results remain immutable. Queue consumption checks the open file
handle through deletion so a replacement packet is preserved.

Commits retain the original request bytes before ledger admission. A separate
`recover-commit` request binds the original request ID and a reviewed candidate
SHA-256. Recovery updates only the original admitted effect after verifying its
publication, while preserving the original failed queue result. The client
preview performs the legacy candidate checks without creating a recovery
acceptance or publication journal.

For older missing-request records, recovery requires the original complete Git
change snapshot, exact candidate index/tree and object identities, preserved
unselected changes, and the enrolled author/committer. Unattested hooks are
refused. A new recovery acceptance explicitly records the fresh verification;
the missing old request is never fabricated. Unknown seed files are preserved.

Kernel upgrades use a maintenance marker to pause queued operations and startup
recovery. The paused worker remains observable but is not reported as ready for
requests. The owner upgrade verifies the new tuple and pinned data before
activation. After activation may have written a newer state format, recovery
keeps the new decoder and resumes forward instead of rolling binaries back.

Client result polling tolerates only Windows sharing/lock violations while a
published result's final handle is closing. It reads the same request result
within the original timeout and never resubmits an operation. Persistent locks
still time out; permissions, malformed data and identity failures remain errors.

## Linked worktrees

The desktop owner opts in a registered repository with `authorize-worktrees`,
binding an exact allowed worktree directory and its strong identity. The
`enroll-linked-worktree` request is carried through the existing queue under
that anchor. It verifies the actual Git worktree back-link and common directory,
creates a distinct registration, and does not inherit installation adapters.
Unknown, outside-root, replaced or ambiguously authorized worktrees are refused.

`bootstrap-worktree` previews by default. Its owner-context execution uses only
hash-pinned installed Rayman/SaveStatus product roles, regenerates local managed
paths through their normal frontends, and uses the native owner migration
operations. It does not copy the parent database. Existing routes must match
the new identity; interrupted checkpoint publication resumes its protected
journal. Normal editing, testing and subsequent frontend collection remain
sandbox work. The owner worker itself does not run product programs.

Codex's documented global SessionStart hook is the automatic entrypoint:
https://learn.chatgpt.com/docs/hooks . `install-worktree-hook` must run through
the matching installed client as the desktop owner. It preserves unrelated
handlers, backs up the old hook document and refuses conflicting owned entries.
Hook and global-skill upgrades recheck the reviewed bytes through a held file
identity, move that original object to a unique `.previous` sibling, and publish
without replacement. A concurrent destination is preserved. Publication failure
restores the original only when its name is still absent; otherwise both the
original sibling and candidate remain for explicit recovery. Preserve these
siblings after a crash and inspect their bytes before any manual restoration.
The host's hook review and a real new-worktree session remain separate acceptance
steps. Plan-mode events and ordinary checkout roots are no-ops; setup errors do
not instruct Codex to block opening the thread.

Detached HEAD is supported without attaching a branch. Publication uses private
worktree HEAD/index paths and the existing journal. Only inert Codex
localEnvironmentConfigPath worktree metadata is accepted; arbitrary Git
worktree settings remain outside this adapter.

## Checkpoint adapter

An opt-in SaveStatus frontend keeps logical database and workspace identities,
executes application capture/validation in a sandbox SQLite working copy, and
publishes a typed row delta through the worker. The worker uses its fixed
schema, tenant keys and per-row previous hashes. It rejects schema changes,
triggers, cross-tenant writes, implicit workspace cascade deletion and duplicate
row mutations. Blob length and SHA-256 are checked after chunk transfer.

Rows and the transaction replay receipt commit together in SQLite. A request
whose result was lost may be retried using the same transaction ID and exact
delta after obtaining a new lease. A changed delta under an old ID is rejected.
Frontend success output is held until persistence; failed persistence retains
the working copies and a recovery manifest. Empty deltas do not create a new
backend database transaction. Rayman and SaveStatus renew held leases during
long commands and bound their client subprocess waits.

The current prototype routes explicitly marked local-only SaveStatus workspaces.
External mirrors and activation/rebind/uninstall migration are not enabled yet;
unsupported routes refuse before application execution. Existing unmarked
workspaces retain the original local behavior. Owner-only preparation, publication
and withdrawal commands now preserve a verified baseline and carry the latest
checkpoint rows back during withdrawal. Interrupted publication and withdrawal
are tested against real file identities. Production rollout is not installed.

`scripts/check-checkpoint-integration.ps1`, required by the Windows repository gate,
exercises real initialization, capture, verification and SQLite write-conflict
failure in a newly protected fixture. Set `RAYMAN_TEST_SAVE_SOURCE` to the clean
commit pinned in `governance/checkpoint-integration.json`. The gate builds that
frontend and this checkout's backend, uses their Cargo artifact paths and records
the frontend SHA-256. Missing or changed candidates fail; the suite is not
optional and has no pytest-only entrypoint. It never installs a production worker or changes Codex
configuration. Same-identity sandbox simulation does not prove qinrm deployment.

## Remaining deployment gates

Actual qinrm cross-identity acceptance, committed source, full repository gates
and official production installation remain required before calling this global
solution deployed. Large-history copy costs and queue retention limits still
need acceptance measurements.

The production installer validates exact clean Rayman and SaveStatus commits,
supports resuming only an identical retained backend, runs both official product
installers, updates Codex configuration and registrar trust, publishes the
global entrypoint, then enrolls trusted projects. The enrollment transaction
registers fixed Rayman and SaveStatus installation adapters before any project
ledger operation. With explicit state migration it safely rebinds Rayman,
upgrades each local SaveStatus runtime, resumes an existing checkpoint
transition or prepares and publishes a new one, and reports post-migration Git
status for every project. Existing completed routes are idempotent.

SaveStatus pause, continue, uninstall and notice acknowledgement still require
their separate owner file-control adapter after database routing. They fail
closed without changing checkpoint rows or workspace control files.

Bootstrap rollback verifies and disables its exact scheduled task, stops it,
and proves that the worker released its lifetime lock before removing the task
or relocating the backend. Once registrar trust refresh or global skill
publication has started, an error preserves the backend and Codex configuration
together for explicit recovery. Rolling back only the configuration would
invalidate the registrar's refreshed trust binding. Production installation
uses the same fixed endpoint as the global configuration.

## Fixed file installation adapter

Fixed application-storage frontends resolve only the owner-protected registration
and request an explicit `inspect_workspace` attestation. The desktop worker pins
and verifies the complete workspace ancestry before returning the exact registered
worktree/digest pair, then repeats its strict checks for every storage operation.
This supports sandbox threads whose workspace is readable but whose profile
ancestors cannot be opened by the sandbox token. Commit and installation source
inspection retain their existing client-side checks. No ACL expansion or
permission-error fallback is used; older workers reject the new inspection action.
The `app-state --no-wait --action release` cleanup path only queues an
unconfirmed release and performs no synchronous attestation. It returns no
workspace state and does not claim release success; the worker applies the same
strict checks when it eventually processes that packet.

Hook publication releases its own write handle before strict readback. A hook
already published when an older installer reported a sharing violation can be
verified through the unchanged/idempotent install path before continuing bootstrap.

The worker now has a file publication handler, separate from application
execution and release acceptance. The desktop owner registers a JSON adapter
specification with `adapter_id` and a `targets` map of role names to absolute
destinations. Targets must already have ordinary parents below that owner's
`AppData/Local` or `.codex/skills`; the worker's own installation is excluded.
A `{version}` token is supported only in the destination filename. Register
adapters before the project's first ledger operation. Later policy changes
need an explicit migration; they cannot silently reinterpret queued requests.

The client `install` command accepts the adapter ID, a strict numeric version,
and a JSON map of those exact roles to workspace-relative payload files. It
captures current destination hashes, source hashes, sizes and a canonical
bundle digest. A preview writes only the sandbox-side input packet; `--yes`
submits it to the owner. Roles publish in ordinal order, so package recipes
must put any activation receipt last, for example `99_receipt`.

The publication code is compiled into the worker from the official installer's
file transaction functions plus a fixed data-only entrypoint. No script path,
command string or executable supplied by the project is run by this handler.
The registered PowerShell executable is hash-checked and held against changes;
it runs in the existing one-process Job with a bounded wait. Raw payload bytes
remain distinct from the bundle's LF/CRLF-insensitive typed JSON digest.

Staged plans, prior-file backups, the in-flight request marker and terminal
outcomes are retained. A pending installation blocks other installations.
Startup makes one recovery attempt: unadmitted staging is cancelled, while an
admitted transaction recovers its original journal. Conflicting destinations
remain blocked. A successful file publication reports
`runtime_validation_required=true` and `project_code_executed=false`. If crash
recovery retains an initially absent file, the generic adapter preserves the
active intent and requires owner recovery; it never claims a complete rollback
or deletes a possibly concurrent file based only on matching bytes. Success is
not an installed/source-fresh/Goal-completion receipt. Rayman and SaveStatus
package recipes, registrar trust updates, and actual qinrm deployment still
need their dedicated acceptance before the global workflow is complete.

The worker rejects commits to its recorded installation-source Git repository
and linked worktrees. Its heartbeat runs independently of request processing;
`status` distinguishes recent liveness, current phase, completed request count
and service/request errors. Control JSON compares decoded content with strict
duplicate-key rejection, while source, database baseline and archive bytes keep
exact hashes. The configuration plan adds only the endpoint environment value
and the named-profile request-directory write entry, preserving unrelated TOML
and the elevated sandbox. The real config was inspected but not modified.

## Codex workspace ACL recovery

### Replacement disks and file-copy recovery

Copying a workspace to a replacement disk can preserve its bytes and path while
changing the volume and directory identities. A `registered workspace was
replaced` refusal must not be bypassed by deleting registrations or re-enrolling
the same path as a new project. Preserve the live files and protected history.

`rebind-workspace --root <backend> --workspace <exact-path>` produces a read-only
plan for one existing registration. After the registered desktop owner verifies
the copy, the installed client accepts `--expected-sha256 <plan-hash> --yes` only
with a fresh acknowledged maintenance heartbeat, an empty queue and no pending
installation. Changed preview bytes or physical identities require a fresh plan.
The protected physical binding preserves original project/worktree IDs, policy,
capabilities, registration bytes, ledger replay records and checkpoint rows.
Existing records are retained at unique siblings during a subsequent migration.
Maintenance stays active until the owner verifies the real application routes.

This command does not install a new kernel, move paths, refresh changed Git
configuration, broaden an allowed-worktree root, repair registrar trust or grant
new capabilities. A second physical replacement is rejected again. Older clients
do not understand these bindings; complete the verified kernel upgrade before
publication. Include the worker's recorded source repository in the explicit
migration so its logical source-commit prohibition continues to apply.

Codex protects a writable workspace's `.git` directory during sandbox setup.
If an older sandbox directly created that directory, its owner can be the
sandbox principal. A later Desktop refresh then cannot add the protection ACE,
and the thread fails before `AGENTS.md` is read. The rollout now inventories all
trusted project roots before installation and enrollment. A fixed preparation
adds only the qinrm account's non-inherited `ChangePermissions` right on an
affected workspace root or `.git` directory; it does not recurse, reset the ACL, change content
rights, or touch a linked-worktree `.git` file. The plan fails closed on a
foreign owner or reparse point. Direct `git init` from the sandbox is outside
the global capability; repository creation must run under the qinrm owner so a
new thread cannot inherit the same initialization failure.
