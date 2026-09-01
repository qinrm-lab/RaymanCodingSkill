# Codex PowerShell identity broker

The broker removes manual copy/paste for the small class of Windows operations
whose evidence genuinely depends on the logged-on user. It is not a general
PowerShell remoting channel and does not replace the native Windows `elevated`
sandbox.

## Execution model

| Lane | Identity | Scope |
| --- | --- | --- |
| Normal | `CodexSandboxOnline` | Build, tests, read-only Git, workspace files, managed temp |
| Identity broker | configured logged-on user | Installed fixed operation IDs only |
| Human boundary | user | Credentials, MFA, destructive or irreversible authority |

The current repository keeps explicit `.git` mutation protection. Local Git
commits for this repository use `git_local_commit_v1`; they do not weaken that
protection or change the identity of normal commands. Every concurrent writing
task still needs its own worktree and branch. Finding another writer is a stop
condition: do not clean, restore, prune, stage, test, or commit that task's
worktree.

The broker must never bootstrap-commit its own uncommitted installer, worker or
standing-grant source. Implementation receives review and repeat-2 authority,
then an independently authorized host Git path creates the clean source commit.
Only that clean commit may be installed or activated. A later ordinary-user
attestation can prove the protected installed tuple, but installation evidence
never retroactively authorizes the source commit that produced it.

The installed task runs hidden with Task Scheduler `InteractiveToken` and
`LeastPrivilege`. It starts at user logon, keeps one worker instance, polls a
bounded request directory, and publishes a heartbeat every two seconds. It has
no administrator run level. The task XML, task DACL, target account/SID,
PowerShell path/hash, worker path/hash, install ID and capability manifest are
all protected receipt inputs.

Task Scheduler may omit schema-default `Enabled=true` and
`RunLevel=LeastPrivilege` elements, and may serialize a user as the exact SID.
The installer and client accept only those canonical equivalents. Additional
triggers, principals, actions, inherited/extra ACEs, stronger sandbox rights,
another user, `HighestAvailable`, or a different worker are rejected.

## Capability boundary

Request schema v3 admits exactly two operations:

- `identity_probe`: returns account, SID, profile, process/session IDs,
  PowerShell version and language mode. Its payload is empty.
- `git_local_commit_v1`: creates one local commit in the single installed
  repository registration described below.

The envelope has exact fields, canonical GUID filename binding, strict UTF-8
JSON with duplicate and case-colliding properties rejected, a bounded size and
a maximum two-minute lifetime. Unknown fields, expired/future requests, replay
drift, arbitrary operation names, command text, script paths, executable paths,
repository paths, refs, remotes, environment variables and argv are rejected
before an operation runs.

Adding an operation requires source changes, review, tests and a new protected
installation. Never add `Invoke-Expression`, `-Command`, an arbitrary program,
workspace script or caller-supplied argument capability; that would be Full
Access disguised as a broker.

## Normative `git_local_commit_v1` contract

The following rules are the semantic source for the feature's tests.

### BROKER-GIT-REGISTRATION — fixed installation authority

The protected capability manifest binds exactly:

- `E:\rayman\software\AI\RaymanCodingSkill` and its strong directory identity;
- the real `.git` directory and strong identity;
- `refs/heads/main` and SHA-1 object format;
- direct `C:\Program Files\Git\mingw64\bin\git.exe`, its strong identity,
  SHA-256 and valid Authenticode signer;
- `rayman <32691594@qq.com>` as both author and committer;
- exact local Git config, root `.gitattributes`, `info/exclude`, and the
  required absence of `info/attributes`;
- protected empty hooks and transaction directories;
- `authorization_mode=persistent_install_grant`, `local_commit_only=true`,
  `push_allowed=false`, `tracked_only=true`, `allow_untracked=false`,
  `allow_pre_staged=false`, and `confirmation_required=false`.

The registration rejects active hooks, external filters, unknown attribute
rules, config includes, fsmonitor/signing/maintenance execution, symlinks,
gitlinks/submodules, alternates, grafts, replace refs, shallow/sparse/split
state, linked worktrees and in-progress merge/rebase/cherry-pick/revert/bisect
state. Any later path, identity, hash, signer, config or attribute drift is
fail-closed and requires a reviewed reinstall or upgrade.

### BROKER-GIT-REQUEST — exact caller snapshot

The caller supplies only a one-line 1–200 character NFC commit message, the
expected 40-hex HEAD, the installed manifest hash, and a strictly sorted exact
list of tracked modifications/deletions. The sandbox-side caller treats the
registered `.git` directory as read-only: fixed `ls-files --stage` output must
be a complete stage-0 mode/OID bijection with fixed recursive `ls-tree HEAD`
output. It never runs `write-tree` against the live index, creates
`.git/index.lock`, stages files, writes objects, or mutates a ref; only the
logged-on-user worker performs the alternate-index transaction below. Each entry binds the prior index
mode/blob and the post-worktree raw SHA-256 plus the filtered Git blob OID that
the protected attributes would place in the index, or explicit null
after-hashes for a deletion. The filtered OID is derived from the exact held
worktree bytes by fixed `git -C <installed-repository> hash-object
--path=<validated-path> --stdin` without `-w`; the OS process still starts
suspended from the protected hooks directory before Git receives this fixed
repository context. Raw CRLF worktree bytes and normalized LF index bytes
therefore remain one verified change rather than a false mismatch. The worker
independently re-derives the branch,
HEAD, clean index tree, complete status, modes, blobs and file hashes while
holding the registered repository root, every modified-file ancestor and the
leaf itself against delete/rename. Each held object is opened as the namespace
entry itself and rejected when it is a reparse point, so a junction cannot
redirect a tracked read outside the registered repository. Missing, extra, untracked,
pre-staged, conflicted, renamed, type/mode-changing, unsafe or case-colliding
paths are rejected.

`-Yes` is an agent-side assertion that this already installed standing grant is
being used deliberately. It is not a new user confirmation token and must not
be copied into the untrusted request payload. The user does not need to approve
each commit again.

### BROKER-GIT-EXECUTION — no indirect arbitrary execution

Every Git argv is constructed by one internal `ValidateSet`/switch. The worker
uses only fixed read/plumbing commands, `git add -u -- :/` against a broker
alternate index, non-writing filtered-blob hashing, `git hash-object -t commit
-w --stdin`, and one
`git update-ref --no-deref --stdin` compare-and-swap for the registered direct
branch ref. The commit
message and commit object travel only on stdin. There is no dispatch for
commit, push, fetch, pull, remote mutation, tag, signing, checkout, reset,
restore, clean or `--no-verify`.

Git starts suspended from the protected empty hooks directory, with a fresh
environment that clears inherited Git/editor/pager/askpass/GCM state, disables
system/global config and system attributes, lazy fetch, replacement objects,
fsmonitor, signing, GC and maintenance. Before its first instruction it is
assigned to a Windows Job with `ActiveProcessLimit=1` and kill-on-close. A hook,
filter, fsmonitor, signer, editor, pager, credential helper or maintenance
child therefore cannot execute as the logged-on user. The repository must have
no active hook; v1 does not silently bypass a real installed hook.

### BROKER-GIT-TRANSACTION — exact local commit

The worker takes the protected transaction lock, writes a durable journal and
copies the live index to an alternate index. It stages only tracked changes,
then proves each alternate-index mode/blob equals the request and writes the
tree. Before ref mutation it writes the verified alternate bytes with
CreateNew to the standard `.git/index.lock`, so another conforming Git writer
cannot enter the live-index publication window. The worker constructs one
unsigned commit with exactly one parent, rejects a symbolic registered branch
ref, and uses no-deref `update-ref` with `old_head -> commit_oid` CAS. Only
after that CAS succeeds does it atomically replace the live index, requiring
the replacement backup to equal the pre-transaction index and preserving and
rechecking the live index owner/DACL.

Success requires the registered ref and HEAD to equal the commit, the commit
bytes/tree/parent/message/identity to equal the journal, the exact path set to
equal the request, the live index/worktree to be clean, and every remote and
other ref except the registered branch to remain byte-equivalent. The result
binds request/install/worker/PowerShell/Git/repository identities, parent,
commit, tree, paths/digest, message hash, before/after index hashes and the
single-process/no-network boundary. No remote operation is reachable.

### BROKER-GIT-RECOVERY — forward-only after ref mutation

Journal phases are `accepted`, `index_prepared`, `objects_written`,
`index_staged`, `ref_updated`, `index_published`, and `verified`. Before
`ref_updated`, failure
may remove only verified transaction scratch when HEAD and the live index still
equal their preconditions; it never changes the worktree. Once the registered
ref equals the journaled commit, the worker never resets or rewinds it. It
continues forward by publishing the alternate index and re-running every
postcondition. A timeout or result-publication failure retains the request and
journal so the same request cannot create a second commit. Divergent HEAD,
concurrent index mutation, missing recovery material or failed postconditions
is `indeterminate_requires_recovery`; evidence is preserved and success is not
reported.

### BROKER-GIT-AUTHORIZATION — standing grant and residual risk

The standing grant is installation-scoped, repository-scoped, branch-scoped
and local-commit-only. It authorizes the agent to self-check and submit matching
commit requests without asking the user on every invocation. It does not grant
push, network access, Full Access, `unelevated`, arbitrary commands, parent ACL
changes or repository ACL resets.

The request queue is writable by the configured Codex sandbox capability.
Therefore any process that already holds that queue capability can request a
commit in this one registered repository. The queue does not cryptographically
identify one chat/task. Exact HEAD/content snapshots, the fixed repository and
branch, the single-process Git closure and postconditions limit the effect, but
they do not create task-level authentication. Adding more repositories requires
a separate reviewed installation change.

### BROKER-GIT-MUTATION-AUTHORITY — trusted root and single-use lifetime

The task-local trust root is the exact command delivered through the current
trusted user conversation, not a mutable workspace launcher. Preparation
publishes one nonce-specific authority manifest and no `.cmd`, outer or
elevated script. The user launches the fixed, hash-bound PowerShell 7 executable
as administrator with `-NoProfile -NoLogo` and then pastes the command. The
command rejects a host that lacks the `-NoProfile` process argument. It opens
the lexical runtime, installer and manifest entries with
`FILE_FLAG_OPEN_REPARSE_POINT`, verifies type/reparse attributes and hashes from
those same handles, holds every ancestor directory the same way, and denies
write/delete sharing until the installer returns. A bound-file leaf open
retries only Win32 sharing or lock violations (`32` / `33`) for a bounded
three-second window while the already-opened ancestor guards remain held. All
other errors fail immediately; exhaustion reports the logical label, exact
path, native error, attempt count and elapsed time.

The action-specific install/upgrade manifest has a 30-minute maximum lifetime and binds the active Goal and
global pending files, the unprivileged generator's non-authoritative local
`goal show`/`frontier` preflight, the exact human capability key/boundary,
review, repeat-2 authority, HEAD, an empty clean status, Git control inputs and all 13
planned source files by path, bytes, strong identity and DACL. The ignored
generator CLI is not an authority identity. The administrator process reopens
the held Goal/pending bytes and independently verifies the Goal lifecycle,
same-fingerprint review and repeat-2 runs, req1–req3 done, req4 open or done,
req5 open, the exact `install`/`upgrade` action and one exact pending
capability; it never executes a workspace-built Rayman binary. A
closed, superseded, changed, expired or duplicate-requirement Goal is rejected.
Once a durable recovery
journal exists, its launcher nonce is consumed in the protected install root;
success, rollback or process failure cannot make that nonce reusable.

### BROKER-GIT-INSTALLATION-SERIALIZATION — one writer and one object epoch

Every install, upgrade, uninstall and partial-uninstall recovery obtains the
same install-root-derived, protected-DACL cross-process mutex before reading or
changing installation state. `-Check` either obtains it for one consistent
snapshot or reports `transaction_in_progress` without reading a mixed epoch.
The access DACL is exact and never repaired in place. Its owner is accepted
only when it is SYSTEM, BUILTIN\Administrators, the configured interactive
user, CodexSandboxOffline or CodexSandboxOnline. Administrators is the normal
default owner for a full UAC administrator token and SYSTEM is the corresponding
service default; both already have the exact FullControl ACE, so accepting those
owners does not widen the DACL. CodexSandboxUsers and other broad groups are not
valid owners. A wrong owner, unprotected DACL or any ACE drift fails before the
mutex wait and before installation state is read or written. A readable
descriptor receives the distinct `owner_not_allowed`, `dacl_not_protected` or
`dacl_mismatch` diagnostic; an object or descriptor that cannot be opened with
the required read rights uses the earlier generic creation/open fail-closed
diagnostic.
The upgrade journal binds its transaction ID, mutex name, launcher nonce,
Goal/fingerprint, old receipt identity and every created staging identity.
Receipt, ready and journal decisions read bytes, DACL and strong identity from
one held handle; rollback, interrupted-staging and partial-recovery cleanup
marks exact file and empty-directory handles for deletion after proving the
complete owned child set. A stale transaction can
therefore neither delete nor adopt a successor journal or directory.

### BROKER-GIT-DIAGNOSTIC-BOUNDARY — advisory output only

No mutable fixed diagnostic file participates in installation or retry
authority. Preflight and installer errors stay on the one administrator
PowerShell 7 console and contain only phase/error facts. A heartbeat timeout
also reports the last heartbeat validation error and a read-only Task Scheduler
COM snapshot containing registration, state, running-instance count and
`LastTaskResult`; it never uses WMI/CIM. Missing or stale console output never
changes the independently verified receipt, recovery journal, task, heartbeat
or final ready decision.

## Filesystem and evidence boundary

Default locations are below
`C:\ProgramData\Rayman\CodexPowerShellBroker`: immutable versions, receipt,
capability manifest, final ready marker, empty hooks, transactions, consumed
upgrade nonces, results and the request queue. During schema switching it also
contains one protected `upgrade-recovery-v1.json`; its current schema binds the
transaction and exact created objects while the filename remains compatible
with the outstanding legacy journal. Successful commit or proven rollback
exact-deletes the journal. Upgrade holds the protected result-root directory
against delete/rename for the whole transaction. A journal-authorized recovery
recreates a missing result root with the exact reviewed owner/DACL only after
receipt/task/ready classification is known; invalid ready or unknown receipt
state performs zero recovery writes and preserves all evidence.
The installed root and every protected child have exact owner/DACL descriptors:
SYSTEM, Administrators and the target user have full control; the sandbox has
read/execute only. The request directory is the deliberate exception: it allows
bounded request creation but not DACL modification or protected result/worker
mutation. Native sandbox capability ACEs are accepted only in the exact
restricted structural form documented and tested by the installer.

Each heartbeat/result binds install ID, worker and PowerShell hashes, executor
account/SID, request ID/hash and timestamps. Git results additionally bind the
registered manifest/repository/Git and verified commit transaction. A task
registration or administrator installer exit alone is not success; the client
also checks task storage, ACLs, fresh heartbeat, protected hashes and negative
write probes.

## Validate source without system changes

```powershell
pwsh -NoProfile -File .\scripts\codex-powershell-broker.ps1 -SelfTest
pwsh -NoProfile -File .\scripts\install-codex-powershell-broker.ps1 -SelfTest
```

The broker self-test uses an isolated temporary Git repository. It covers a
read-only client snapshot while `.git` file creation is denied, a real
CRLF-to-filtered-index commit, exact output, absent `info/attributes`,
direct-ref enforcement, arbitrary-field rejection, duplicate JSON keys,
untracked and pre-staged refusal, held-ancestor rename denial, junction-backed
tracked-file refusal, standard `index.lock` staging, failure before
ref mutation, forward recovery after ref mutation, replay immutability, and a
required external clean filter blocked by the single-process Job.

The installer self-test covers ACL/task XML, fixed runtime and repository
registration, manifest/ready bindings, the installation-wide cross-process
mutex, simulated elevated Administrators and SYSTEM default owners, rejection
of broad owners and DACL drift, consistent busy `-Check`, TTL/nonce/Goal replay
refusal, exact-handle journal deletion, strict durable journal parsing,
Task Scheduler COM registration/start, explicit-false confirmation rejection,
ordinary-user existing-tuple attestation, action-bound install/upgrade commands,
owned-tree exact deletion and concurrent replacement refusal,
rollback-failure
classification, missing result-root reconstruction, held result-root deletion
refusal, receipt-replace denial and bounded publication, post-create cleanup
and atomic-file write failure cleanup. Eight PowerShell 7 producer children run
the real switch coordinator and terminate their own process immediately after
each stage from `staged` through `ready_published`; fresh recovery children then
prove exact rollback or committed-forward behavior. Five more fresh children
prove unknown receipt plus malformed, wrong-ACL, directory and reparse ready
states remain indeterminate with zero recovery writes and all evidence retained.

The trusted-command test starts a fake installer while the command holds its
runtime, installer and authority-manifest handles. It rejects a missing
`-NoProfile` host, a symlink inserted at the leaf-open boundary and a junction
ancestor. Independent PowerShell 7 processes receive real Windows sharing
violations when replacing installer or manifest, and replacement succeeds after
return; wrong hashes and Windows PowerShell 5 are rejected before execution.
The native pinned-open tests additionally hold a real file with
`FileShare.None`: a delayed release must succeed through the bounded retry,
while a persistent hold must fail with path, `win32=32`, attempts and elapsed
time rather than an anonymous `OpenPinnedFile` exception.
Extra-frame tests cover upgrade,
fresh-install rollback, uninstall and partial-uninstall action paths without a
production `GetNewClosure()`. Installer self-test state is rooted only in the
process temporary directory, never repository `.RaymanCodingSkill/tmp`. Main,
crash-parent and both crash/recovery child modes derive their case root from the
same compact `b-<token>` constructor and independently require the deepest
hash-versioned worker path at or below 240 characters. Native held opens thus
remain valid inside Rayman-managed validation leases without changing the fixed
production install root. The
Windows-only Rust execution test copies the complete governed source set into a
new clean Git repository with no `.RaymanCodingSkill`, runs both self-tests and
then runs them in one PowerShell runspace, so either a hidden live-workspace
dependency or an isolated-process scope accident fails. Non-Windows runners
retain static contract coverage but do not claim WindowsIdentity/Task Scheduler
runtime execution.

## Install, upgrade and use

Fresh installation and upgrade require one narrowly approved administrator
process whose real identity is the target user. They are allowed only after an
independent host commit leaves the source clean while the same-fingerprint
review and repeat-2 authority remain current, and the Goal is at its exact
human UAC boundary. Press **Win+R**, paste
`"C:\Program Files\PowerShell\7\pwsh.exe" -NoProfile -NoLogo`, then press
**Ctrl+Shift+Enter** and approve UAC. Paste the generated command only in that
profile-free administrator process. Windows PowerShell, a profile-loaded shell,
a mutable `.cmd`, an outer wrapper and a separate elevated script are not
authority surfaces.

After focused tests, traceability, frozen review, current-fingerprint Goal
requirements, repeated authority and the independent clean source commit all
agree, prepare the ignored task-local launcher bundle without starting UAC.
Use `-PrepareInstallLauncher` only when the fixed install root is absent and
`-PrepareUpgradeLauncher` only for an existing protected tuple:

```powershell
pwsh -NoProfile -File .\scripts\install-codex-powershell-broker.ps1 `
  -PrepareInstallLauncher -Yes `
  -ExpectedGoalId 'goal_0123456789' `
  -ExpectedSourceFingerprint ('0' * 64)

pwsh -NoProfile -File .\scripts\install-codex-powershell-broker.ps1 `
  -PrepareUpgradeLauncher -Yes `
  -ExpectedGoalId 'goal_0123456789' `
  -ExpectedSourceFingerprint ('0' * 64)
```

The Goal ID and fingerprint above are placeholders: callers must supply the
fresh final values after publishing the exact current pending package. The
generator accepts no output path, command, PowerShell path or arbitrary argv.
Its local CLI preflight requires the expected Goal/frontier tuple, but the
administrator's held-byte parser—not that ignored CLI—requires req1/req2/req3
done, req4 open or done, req5 open, the exact action-specific capability
key/boundary, the latest review and both repeated authority runs on one
fingerprint. It requires an empty Git status and binds each held object plus
current HEAD,
installer, worker, signed Git, Git control inputs and fixed PowerShell 7. Empty
status is not trusted by itself: the administrator retains the complete Git
input epoch before querying any source, and every one of the 13 source paths must have one
normal `H` index entry at stage 0; its mode/OID must equal the exact HEAD tree
blob and the Git blob OID computed locally from the same retained worktree
bytes. `assume-unchanged`, `skip-worktree`, sparse/unmerged entries, index/HEAD
drift and status-hidden bytes fail closed in both generator and administrator
verification. The
Rayman executable is used only by the unprivileged generator and is not placed
in or executed from the administrator manifest. The generator writes one nonce-named
authority manifest and returns a command plus hashes;
`mutable_launcher_file_published=false`. Any Goal/source/Git-input change or the 30-minute expiry
requires a completely new manifest and command.

The returned administrator command pins the fixed PowerShell runtime, installer
and action-specific manifest before invoking the installer. The installer then
holds every source/Git/Goal/pending input and uses only the held worker bytes.
Direct fresh `-Install` and direct `-Upgrade` lack that authority and fail
closed. The install nonce is recorded in the new protected root before Task
publication.

Against an already installed exact schema-v3 tuple, ordinary non-administrator
`-Install -Yes` is deliberately different: it performs no installation write.
It validates the held client source, protected receipt/ready/manifest and then
runs the fixed `identity_probe`; the worker proves its Task XML/DACL, account,
SID, runtime and source binding before returning `already_current=true`. This is
the supported typed req4 proof. `-Yes:$false` is rejected at the production
entry for install, upgrade, uninstall, partial-uninstall recovery and both
launcher preparations.

An exact protected schema-v2 `identity_probe` installation upgrades only
through the generated upgrade command.

An already committed schema-v3 installation uses the same one-shot authority
for a current-schema worker refresh. It preserves the installation ID, exact
repository capability manifest, hooks root and transaction root, acquires the
existing transaction lock, and stages only the new hash-named worker version.
Its durable journal binds a unique transaction ID plus the exact old/new
receipt, Task XML and ready-marker bytes and identities. The switch publishes
the new receipt and Task, proves a fresh heartbeat, releases the transaction
guard, and compare-and-swaps the old ready marker to the new worker binding as
the final commit point.

Current-schema crash recovery commits forward only when the new receipt, Task,
ready marker and heartbeat all agree. Before that commit point it restores the
exact prior receipt, ready marker and Task, restarts and re-proves the prior
heartbeat, and deletes only the journal-owned new worker version. Unknown
receipt, Task, ready, ACL or identity state remains
`indeterminate_requires_recovery` with zero recovery mutation. There is no
uninstall gap and no recreation of the standing repository grant.

Before publishing a nonce manifest, the unprivileged generator opens any
existing protected Git transaction lock and refuses an active transaction; the
elevated transaction rechecks the same condition because generation is not a
lease. Install/upgrade then obtains the installation mutex, validates the single-use
authority and recovers any exact prior journal. It verifies the v2
receipt/task/ACL/heartbeat and, while v2 is still running, performs an initial
DELETE+READ_ATTRIBUTES/share-all receipt probe; failure ends with zero new
transaction writes. It then stages protected v3 assets, durably publishes the
transaction/identity-bound journal and consumes the launcher nonce before
stopping anything. Only after that recovery point exists does it stop the Task
Scheduler instance and—when that API leaves the action process alive—pin and
stop the heartbeat PID whose start time, session, owner SID, executable and
complete command line match the protected receipt. A second bounded receipt
probe closes the post-stop publication window. It then sequentially publishes the new receipt/task, starts the worker and
waits up to 60 seconds for a fresh v3 heartbeat while holding the exclusive Git transaction
guard. The worker cannot commit until the installer releases that guard and
atomically writes the protected ready marker as the final capability step.
Worker-lock contention is a nonzero startup failure, never a silent successful
exit. If heartbeat startup times out, the installer includes Task Scheduler
state, running instances, `LastTaskResult`, and the last rejected heartbeat
reason before entering the existing rollback state machine.
Task registration and start use the fixed Task Scheduler COM service and root
folder directly. No `WINDIR`-derived executable, `schtasks.exe`, shell command,
or caller-selected program is executed in the administrator phase.

Switch actions that are not defined to return a value have their output
discarded; heartbeat and commit actions must each return exactly one object, so
an accidental PowerShell pipeline object cannot corrupt phase/result handling.
Because the trusted command invokes the installer as a child script in the same
PowerShell 7 process, all production actions deliberately retain the installer
call stack instead of using `GetNewClosure()` dynamic modules. Static coverage
rejects that method anywhere before `Invoke-SelfTest`; extended-path,
shared-runspace and extra-frame behavior tests exercise upgrade, install
rollback, uninstall and partial recovery.
Before replacing the fixed receipt, the installer proves DELETE/share access
against the exact file, waits only for a bounded sharing-violation window, and
retries only access/sharing failures. Rollback skips the write when the durable
journal already matches the live receipt bytes.
Failures before that marker enter the tested rollback state machine: when the
old receipt/task/heartbeat and empty staging are all re-proven, only newly
created staging is removed; otherwise rollback reports that it could not be
proven, preserves evidence, and leaves the ready marker absent so the v3 worker
still cannot commit. If the process terminates after the durable journal is
published, the next upgrade accepts only the journaled v2/v3 receipt pair,
exact old task XML and a current task from that pair. A valid ready marker plus
v3 task/heartbeat is completed forward; otherwise the exact v2 receipt, task
and heartbeat are restored before staging is removed. A held or nonempty Git
transaction directory makes recovery indeterminate and preserves the journal.
Legacy recovery keeps the exclusive transaction guard while deriving its
cleanup hash, strong file identity, owner and DACL from that same held stream;
it never attempts a second path open against its own lock. The guard is released
only after every other staging object is bound and immediately before exact
handle-checked cleanup.
Unknown receipt bytes or any present-but-invalid ready object—including
malformed bytes, wrong ACL, directory and reparse—become
`indeterminate_requires_recovery` before nonce/result/task mutation. Once the
receipt/task/ready tuple is known recoverable, interrupted recovery creates a
missing results root if required and holds its guard across committed-forward
validation or rollback and staging cleanup. A normal upgrade holds its own
result-root guard across preflight, staging, task/receipt switching and
rollback. `-Check` separately reports transaction activity, journal state,
recovery requirement, receipt/ready states and identity/Git/operational
readiness; a heartbeat cannot hide recovery debt.
An earlier interruption that predates the durable journal still accepts only
one complete extra staged hash-version beside the formal v2 receipt worker,
plus exact protected directories/files, empty hooks, an unlocked zero-byte
transaction lock, canonical manifest and current repository/Git identities.
Partial, multiple, foreign, reparse-backed, ACL-drifted or content-drifted
state is preserved and refused. Upgrade never resets an existing ACL or
creates an uninstall gap.

Client usage:

```powershell
pwsh -NoProfile -File .\scripts\codex-powershell-broker.ps1 `
  -Operation identity_probe

pwsh -NoProfile -File .\scripts\codex-powershell-broker.ps1 `
  -Operation git_local_commit_v1 `
  -CommitMessage 'fix: describe the exact local change' `
  -Yes `
  -TimeoutSeconds 120
```

Windows UAC may still require the user to physically click **Yes** on the
security desktop. A verified OpenAI sandbox setup prompt and this exact broker
upgrade/install remain separate: neither authorizes another publisher,
executable, repository or capability.

## Uninstall

Uninstall remains separate and destructive. It requires exact receipt-bound
arguments plus `Yes.IsPresent`; explicit `-Yes:$false` is not confirmation. It
stops only the registered worker/task, waits for the exact lock, then snapshots
the complete root before deleting the Task. Every descendant must be a bounded,
non-reparse object owned by the receipt user. Files bind held bytes/hash/strong
identity/owner/DACL; directories bind strong identity/owner/DACL. The root is
held against rename, and every descendant directory keeps its own DELETE-capable
no-reparse handle with delete sharing denied from snapshot through bottom-up
deletion. Files are reopened only while that entire ancestor chain remains
held; directories, including the root, are deleted through their original held
handles. A concurrent leaf replacement, ancestor rename/refill or junction
substitution fails closed and preserves the observed tree without crossing into
an external sentinel. This includes
broker-owned result history and is not represented as per-child rollback
cleanup. It refuses a pending upgrade recovery
journal and a v3 transaction directory containing a held lock, commit recovery
journal or unknown entry. It accepts the protected v2 or v3
receipt, never resets a parent ACL and never touches another task, skill,
worktree, credential or WindowsApps path.

```powershell
pwsh -NoProfile -File .\scripts\install-codex-powershell-broker.ps1 `
  -Uninstall -Yes -UserAccount 'QIN5521\qinrm'
```

Interrupted legacy uninstall recovery still accepts only a positively absent
task plus the exact protected root containing one unlocked zero-byte
`worker.lock`; it is not a generic force-delete facility.
