# Behavioral ownership for maintenance

These repository-owned contracts split the former Goal, state/recovery and CLI
umbrella rules. A rule describes an independently removable behavior, not a file.
Bindings in test-traceability.json name the exact executable assertions and gate.
A compound test may bind more than one rule; retirement must preserve its surviving
assertions or remove the selector when its last valid obligation disappears.
Review date: 2026-09-14; review horizon: 2027-03-14.

## Current retirement decision

The global marker-catalog exemption is removed. Its old acceptance selector is
retired and replaced by a scoped-catalog regression that checks both scan routes.
The old selector owns no dedicated assets. No other test or asset is removed in
this phase: the reviewed behaviors remain supported, including legacy rejection
and checkpoint/broker migration safeguards. Removing an umbrella rule alone does
not authorize deleting its still-required tests.

## Contracts

### ACTIVATION-CONTRACT

Activation is explicit and bound to the current canonical CLI and skill contract; missing, orphan, duplicate, unknown or old identity fields fail closed even when called from a subdirectory.

### ACTIVATION-INSTALL-BIND

The hidden installer binding entrypoint requires confirmation, may create a canonical workspace activation, and idempotently refreshes eligible identity drift without changing the existing skill path.

### ACTIVATION-REBIND

Workspace identity rebind changes only an eligible existing contract with confirmation; it preserves unrelated bytes and paths, never activates orphan state, and never scans or rewrites sibling workspaces.

### ASSET-MARKERS

When scanning live or captured files, report obsolete-name candidates and real work markers without deleting files; only scanner catalog or test fixture syntax is exempt, and an incomplete walk is an error.

### ATOMIC-FILE

Atomic file IO verifies parent and temporary-file identities, preserves the previous complete target on failure, and distinguishes missing data from corruption or read-time drift.

### CHECKPOINT-ORPHAN

Checkpoint transaction recovery reclaims only provably empty or committed orphan state; backups or ambiguous incomplete transactions remain fail closed.

### CHECKPOINT-RECOVERY

Recovery-only checkpoint capture preserves partial evidence separately from the last complete snapshot; it never silently becomes the normal latest snapshot or reactivates a workspace.

### CHECKPOINT-RESTORE

Explicit checkpoint restore verifies snapshot identity and safe paths before publication, serializes managed state, and rolls back partial publication without overwriting unrelated state; unsafe or incompatible snapshots are refused.

### CHECKPOINT-RETENTION

Checkpoint save retains the latest complete snapshot and preserves prior snapshots by default; pruning requires explicit intent and only verified complete snapshot directories are eligible.

### CHECKPOINT-SAVE

Checkpoint capture binds its workspace and store, excludes its own snapshots, serializes concurrent saves, and surfaces storage errors without losing the last usable capture.

### CHECKPOINT-VERIFY

Explicit checkpoint verification reports the complete verified snapshot and file count without restoring it; state audit and temporary-storage inspection remain separate read-only observations.

### CLI-LOCALIZATION

Language selection preserves Unicode paths and stable JSON contracts while localizing user-facing text.

### CLI-MIGRATION

Retired command spellings fail with actionable migration guidance rather than silently invoking removed behavior.

### CODEX-STOP

The Codex Stop hook blocks unfinished active work and installs idempotently while preserving unrelated handlers; installation alone does not attest a host stop event.

### CONTEXT-READONLY

Context and map queries distinguish missing, ready and stale captures and preserve read-only state; map commands cannot report fresh structure from a missing or stale context.

### DOCTOR-IDENTITY

Doctor distinguishes installed identity, process profile and action-specific write probes; untrusted context requirements or earlier PATH wrappers fail without rewriting workspace identity.

### GIT-SNAPSHOT

Read-only Git inspection preserves exact porcelain path and XY state, reports lossy or truncated records, and leaves the real index bytes unchanged.

### GOAL-ADVISORY-IMPACT

Recording advisory changed-path evidence may describe an unknown path without writing project-map cache, but the claim remains distinct from an executed validation receipt.

### GOAL-ARCHIVE-PROOF

Archiving Goal success requires its original valid source, plan, review and receipt proof; historical fingerprint reconstruction cannot widen scope, repair forged current proof or erase the original workspace identity.

### GOAL-CLOSURE

Goal creation and success closure require a well-formed nonempty obligation set and current complete evidence; malformed IDs, corrupt stores, missing baselines and unfinished Goals cannot become task success.

### GOAL-CONSULTATION

Consultation requires a complete current solution package after agent-owned work is exhausted; rendering is deterministic and Goal scoped, and explicit operator pause remains non-completion without creating a delivery attestation.

### GOAL-HANDOFF

Release handoff starts only from a current successful source Goal with independent authority, clean exact HEAD and structured delivery stages; a malformed or retired source is refused.

### GOAL-LANES

A Goal lane enforces its declared read or write scope; read-only drift, writer scope escape, open lanes and mutations on retired goals cannot pass closure.

### GOAL-LEGACY

Legacy and archived Goal records preserve their schema and proof identity through explicit migration; history is never current success without the required current evidence, and retirement does not erase unfinished work.

### GOAL-PACKAGES

Required Goal packages are acyclic and cover their obligations; incomplete packages block success and progress receipts never substitute for executed validation.

### GOAL-PENDING-IDENTITY

Pending items have Goal-scoped capability identities; exact replay is idempotent, changed contracts are rejected, and explicit resolution removes only that live boundary.

### GOAL-PENDING-SCOPE

Pending readiness includes live Goal boundaries and unbound legacy work while retaining retired history; malformed owner contracts, missing Goals and corrupt stores are errors.

### GOAL-PLAN-PUBLICATION

Goal plan publication is write-ahead and identity bound; interrupted publication recovers its recorded chain, while shape changes, policy downgrades and post-hoc scope expansion are refused.

### GOAL-PLAN-SCOPE

Before a multi-file change the Goal owns one immutable aggregate plan; extensions are monotonic and precede newly scoped edits, and changes outside that plan block validation and readiness.

### GOAL-QUARANTINE

Invalid historical Goal evidence may be quarantined with its original identity preserved; quarantine never grants success authority or accepts a valid current goal as corrupt.

### GOAL-REPLACEMENT

Goal replacement transfers only exact compatible obligations from independently valid bound predecessor evidence; stale source, path substitution, missing transfers or reversed chronology are rejected.

### GOAL-REVIEW

High-priority changes require review bound to the final source fingerprint; a later source change invalidates that review until it is refreshed.

### GOAL-SCHEMA-MIGRATION

Explicit Goal schema migration preserves the versioned historical proof projection; real receipt records cannot take the pre-receipt route and legacy proof does not acquire current authority by writeback.

### GOAL-STORE

Goal queries accept only valid known IDs and ordinary readable store members; linked, corrupt, missing or wrongly typed members fail instead of disappearing from readiness.

### GOAL-TIME

Goal ledger events follow every bound predecessor time even when the clock moves backward; rehashed or reversed event times cannot legalize a later plan or forged authority.

### LEASE-CARGO

Self-hosted Cargo validation builds in one exclusive managed target separate from the running CLI; creation or release failure preserves substituted paths and prevents a receipt.

### LEASE-IDENTITY

Managed validation leases are exclusive, uniquely identified and manifest bound; tampering or namespace replacement prevents successful release and cannot authorize deleting a replacement or external path.

### LEASE-PYTEST

Each pytest process uses an exclusive manifest-bound lease with separate nested validation space; tampering, injected environment or replacement paths fail closed and lease paths never enter logical receipts.

### MAP-DEPENDENCIES

Project impact uses actual manifest-qualified dependency facts and package boundaries, including inherited and glob members; unknown directories or duplicate package names cannot produce misleading empty or cross-package test evidence.

### MAP-PLAN

Map planning requires indexed test anchors for supported broad package changes; unsupported package types remain explicitly heuristic rather than manufacturing a test requirement.

### MAP-QUALITY

Quality checks require valid explicit policy and available test evidence; unknown fields or warning kinds fail closed and configured blocking findings remain blocking.

### PENDING-MIGRATION

Legacy pending migration requires the exact old package hash and a current Goal-scoped capability contract; it preserves the old untrusted assertion, fails atomically on mismatch, and never attests that the rendered result was delivered.

### PREPARE-CAPTURE

Prepare verifies a current planned Goal against one source and state capture before enriching context; intervening Goal or workspace drift stops the operation and suggested commands preserve paths as argv data.

### READINESS-CAPTURE

Readiness distinguishes workspace health from a selected Goal and binds source, state, map and asset observations; finish performs two complete evaluations and rejects intervening mutation or inactive activation.

### STATE-AUDIT

State audit accepts known managed state with its correct type and rejects linked, unreadable, corrupt or unknown state; diagnostic commands do not repair it silently.

### STATE-CAPACITY

Managed temporary storage capacity thresholds produce operational warnings at the byte or entry boundary; they remain advisory and do not turn a healthy state audit into failure.

### STATE-DELETE

Managed tree deletion uses held no-follow identities; parent swaps, replacement entries or concurrent refill stop deletion and retain an orphan rather than authorizing a wider path.

### STATE-LOCK

Concurrent state operations hold a stable shared lock for the whole transaction; only actual OS lock contention is retried, and permission failures retain their structured cause.

### STATE-NAMESPACE

Held state directory and file handles retain strong identity across reads and detect namespace moves; short-lived probes release handles so they do not lock unrelated host directories.

### STATE-PATHS

Managed state path operations bind ordinary directory and file identities and use no-follow traversal; type errors, namespace substitution and concurrent refill fail closed rather than widening a read, write or delete target.

### STATE-PROBE

An action-specific state write probe reports actual capability without changing existing state; denied roots and planted links are reported distinctly and probe artifacts are removed.

### TEMP-SCRATCH

Managed scratch creation sanitizes labels without aliasing reserved or unsafe paths; status and cleanup stay within verified managed directories and return ordinary usable paths.

### UPDATE-PREFERENCE

Update status is read-only and activation exempt; configuration needs an exact confirmed selector, disabled polling performs no network request, unsupported platforms remain unsupported and corruption cannot become install consent.

### VALIDATION-AUTHORITY

Final authority requires repeated selector-free recognized repository execution on one unchanged captured workspace; narrowed Cargo scope, aliases, changed gates and workspace mutation are rejected.

### VALIDATION-COMMAND

Validation runs one direct classified program and argv with relevant scope and actual successful execution; probes, shell payloads, unsafe script paths and zero-test commands cannot create evidence.

### VALIDATION-GATE-DEPENDENCIES

Repository gate authority binds the reviewed gate and its complete source dependency closure from the baseline; added, removed, changed, ambiguous or external executable dependencies cannot self-authorize.

### VALIDATION-INSTALL

Typed requirements accept only the corresponding exact executed command and scope; installer previews, decoys, abbreviated self-test flags and unrelated receipts cannot claim installation or another typed proof.

### VALIDATION-LAUNCH-IDENTITY

Before PowerShell validation launches a canonical Windows script path, round-trip the launch spelling to the same file identity; canonicalization failure or substitution is rejected for drive and UNC paths.

### VALIDATION-PYTEST

Python test evidence requires a real selected test collection and positive matching execution summary with isolated process leases; arbitrary code hosts, collection-only modes, narrowing and isolation overrides cannot mint authority.

### VALIDATION-RECEIPT

A success receipt binds the same current workspace, command and declared impacts; failed execution, stale source, irrelevant scope or combining unrelated receipts cannot satisfy a requirement.

### VALIDATION-SCOPE

Ordinary changed-path validation retains its established impact scope encoding; zero-delta workspace-snapshot validation has a distinct hash domain and cannot be confused with ordinary change evidence.

### WORKSPACE-WALK

Workspace discovery honors tracked files and ignore rules while excluding managed state and build output; submodules, path case and no-follow failures must not silently remove eligible source from the capture.

## Scheduled snapshot retirement

The user selected the independent SaveStatus skill for automatic recovery.
Rayman scheduler creation, ticking, stopping and status reporting are removed,
together with tests whose sole requirement was that scheduler. Old spellings
remain rejection-only migration entries under CLI-MIGRATION. Manual snapshot
recovery keeps its legacy custom-store hint and stable autosave.lock contract;
it cannot register tasks or mutate SaveStatus configuration.
