# Codex PowerShell identity broker

The broker removes the manual copy/paste step for the small class of Windows
operations whose evidence genuinely depends on the logged-on user. It does not
replace the Codex sandbox and is not a general PowerShell remoting channel.

## Execution model

| Lane | Identity | Scope |
| --- | --- | --- |
| Normal | `CodexSandboxOnline` | Build, test, Git, workspace files, managed temp |
| Identity broker | configured logged-on user | Installed fixed operation IDs only |
| Human boundary | user | Credentials, MFA, destructive or irreversible authority |

Every concurrent writing chat uses its own Git worktree and branch. A shared
local checkout has one writer. Finding another writer is a stop condition: do
not clean, restore, prune, test, stage, or commit that task's worktree.

The installed task runs hidden with Task Scheduler `InteractiveToken` and
`LeastPrivilege`. It starts at user logon, keeps one worker instance, polls a
bounded request directory, and publishes a heartbeat every two seconds. It has
no administrator run level. The installer requests a protected task owner/DACL
with generic-all for `SYSTEM`, Administrators, and the configured user, and
generic-read for the sandbox group. Task Scheduler materializes those generic
rights as exact `FA`/`FR` masks and does not echo the requested `D:P` flag.
The installer and client therefore validate the actual descriptor structurally:
exact owner/group, exactly four non-inherited allow ACEs, fixed SID order and
fixed `FA`/`FR` masks, with no inherited ACEs or extra rights. This permits
readback of the exact task XML and DACL without granting sandbox modification.
The administrator installer validates the live registered task through Task
Scheduler COM and rejects auto-inherited control flags there.
`CodexSandboxOnline` cannot open or enumerate the scheduler root
folder on this host even when the fixed task itself grants generic-read, so the
client independently reads only the fixed protected storage file at
`C:\Windows\System32\Tasks\Rayman-CodexPowerShellBroker`. It rejects a nested
name, missing/reparse/empty/oversized file, XML drift, owner/group drift, any
inherited or extra ACE, and any rights drift. The file-system projection may
carry protected `D:PAI` control flags while retaining exactly four explicit
ACEs; only that projection is accepted. A fresh receipt-bound heartbeat and
result then prove that Task Scheduler actually started the self-validating
worker for the current `install_id`.
Task Scheduler may omit `LogonTrigger/Enabled` and `Settings/Enabled` when
their schema-default value is `true`, and may omit `Principal/RunLevel` when
its default is `LeastPrivilege`. Installer and client readback accept either
the corresponding omission or one exact canonical value. They reject explicit
`false`, `HighestAvailable`, non-canonical values, duplicate or
foreign-namespace default elements. All other required fields must occur
exactly once. The task must have exactly one `LogonTrigger`, one `Author`
principal, and one `Author` `Exec` action; additional triggers, principals, or
actions are rejected.
Task Scheduler may also serialize a trigger or principal `UserId` as the
account SID instead of the submitted `DOMAIN\user` name. Installer and client
accept the exact submitted name or resolve the readback value and require its
canonical SID to equal the protected receipt's target `user_sid`. An
unresolvable value or any different SID remains a hard failure.

Task existence is a three-state query: present, positively confirmed absent,
or failed. Only a Task Scheduler file/path-not-found result followed by a
successful hidden-task enumeration with no matching root task is treated as
absent. Connection, access, COM, enumeration, task-path, and XML-read errors
fail closed. Fresh-install rollback and uninstall remove protected broker files
only after task removal or prior absence has been positively confirmed; if
that confirmation fails, the files are preserved and success is not reported.
The installer status view likewise reports task/install state as unknown rather
than `false` when a protected query fails.

## Capability boundary

Version 1 installs exactly one operation:

- `identity_probe`: returns account, SID, user profile, process/session IDs,
  PowerShell version, and language mode.

The request schema has exact fields, a canonical GUID filename binding, a
maximum 16 KiB size, strict UTF-8/JSON parsing, a maximum two-minute lifetime,
and an empty payload. Unknown fields, expired requests, replayed IDs, arbitrary
operation names, command text, script paths, and argv are rejected before any
operation runs.

Adding another operation requires a source change, review, tests, and a new
installation. Never add an `Invoke-Expression`, `-Command`, arbitrary program,
workspace script, or caller-supplied argument capability: that would be Full
Access disguised as a broker.

## Filesystem and evidence boundary

Default locations:

- installed versions and receipt:
  `C:\ProgramData\Rayman\CodexPowerShellBroker`;
- request queue:
  `C:\ProgramData\Rayman\CodexPowerShellBroker\requests`;
- result and heartbeat files:
  `C:\ProgramData\Rayman\CodexPowerShellBroker\results`.

The installed root, `versions`, immutable version directory, result directory,
worker, lock, and receipt are created with protected owner/DACL descriptors in
their creation operations. `SYSTEM`, Administrators, and the configured user
have full control; `CodexSandboxUsers` has read/execute only. Their ACLs remain
exact. The installer refuses to adopt a pre-created namespace or a
stale/different install tuple.

The request directory is the single exception because it is the deliberately
untrusted inbox named as an exact writable path in the Codex permission
profile. The installer first gives the sandbox group create-file access on the
directory object and makes request children inherit `Modify`. Native Windows
`elevated` sandbox setup may then materialize that writable-root grant as one
explicit inheritable `Modify` ACE for the sandbox group plus one or more
unresolved, foreign-domain capability SIDs with the same exact rights.
Installer and client accept only those two structural forms: the owner and
three full-control ACEs remain exact, the DACL stays protected and canonical,
and every additional SID must be unresolved, outside the configured user's
account domain, and limited to explicit inheritable `Modify`. Resolvable,
broad, same-domain, deny, inherited, or stronger ACEs are rejected. No such
tolerance applies to the result directory or executable/receipt state.

The managed form can delete the disposable request queue itself, but the
read-only protected parent prevents a sandbox process from replacing it. Queue
loss is therefore a fail-closed availability error, not a route to results or
arbitrary execution. The worker opens each request with no reparse following
and exclusive sharing, then hashes and parses the same single byte buffer while
holding that handle. It never moves a sandbox-owned file and then reopens it by
name.

Each result binds:

- request ID, operation, exact request SHA-256, and timestamps;
- the fresh, canonical `install_id` for this installation;
- worker SHA-256 from the protected install receipt;
- the exact current `pwsh.exe` path and SHA-256 selected from the installer
  process, never a multi-candidate PATH lookup;
- executor account and SID;
- status, exit code, output, and error.

The client validates exact protected ACLs/owners everywhere except the
structurally bounded request-root form above, a current-install heartbeat,
installed worker and PowerShell hashes, expected user account/SID, request
hash, operation, result envelope, and output identity. It also performs fixed
negative probes: request-root `WRITE_DAC`, result-file creation, and
worker/receipt write-open must be denied. A successful task registration or
administrator installer exit alone is not the sandbox-side verification claim;
keep these as two separate Goal requirements.

## Validate source without system changes

```powershell
pwsh -NoProfile -File .\scripts\codex-powershell-broker.ps1 -SelfTest
pwsh -NoProfile -File .\scripts\install-codex-powershell-broker.ps1 -SelfTest
```

The broker self-test proves both request-root ACL forms, rejects broad,
same-domain, or stronger extra ACEs, and covers a valid identity request,
unknown-operation rejection without sentinel creation, expiry rejection, and
replay immutability. The installer self-test independently checks the same ACL
contract, parses both scripts, checks the least-privilege task XML, rejects
`-Command`/Full Access/`unelevated`, and exercises atomic file publication
under managed temp.

## Install and verify

Installation changes ProgramData ACLs and registers a persistent logon task, so
run it only after explicit user authorization in one administrator-approved
PowerShell 7 process whose real identity is the target user:

```powershell
pwsh -NoProfile -File .\scripts\install-codex-powershell-broker.ps1 `
  -Install -Yes `
  -UserAccount 'QIN5521\qinrm' `
  -SandboxGroup 'QIN5521\CodexSandboxUsers'
```

Check installed state from the same authorized context:

```powershell
pwsh -NoProfile -File .\scripts\install-codex-powershell-broker.ps1 -Check
```

From Codex, run the current repository client. It refuses to proceed unless its
own bytes equal the protected installed worker:

```powershell
pwsh -NoProfile -File .\scripts\codex-powershell-broker.ps1 `
  -Operation identity_probe
```

The command succeeds only when the envelope and output both report
`QIN5521\qinrm` and its exact SID, the heartbeat belongs to the current
`install_id`, every protected ACL matches, and the fixed negative probes are
denied without leaving a file. Do not loosen an ACL to make verification pass.

Re-running `-Install -Yes` is read-only/idempotent only for the exact current
user, group, worker, PowerShell runtime, task XML, receipt, heartbeat, exact
protected ACLs, and one accepted request-root ACL form. A source or runtime
change requires explicit uninstall followed by a fresh install; the installer
never takes ownership of an unknown existing tree.

## Uninstall

Uninstall is intentionally separate and destructive. It requires explicit
authority, the same administrator-approved target-user identity, and arguments
that exactly match the protected receipt:

```powershell
pwsh -NoProfile -File .\scripts\install-codex-powershell-broker.ps1 `
  -Uninstall -Yes `
  -UserAccount 'QIN5521\qinrm'
```

It verifies the receipt-bound XML, stops every exact task instance, confirms
the task deletion, waits until `worker.lock` can be opened exclusively, and only
then removes the exact protected install root (the request directory is inside
it). The worker also watches its already validated fixed Task Scheduler storage
path and exits when that task disappears.

If Task Scheduler reports no running instance while the action process remains
alive, the installer pins that process before task removal using a fresh
receipt-bound heartbeat. Only after the task is positively absent may it stop
the held process, and only when PID/start time, session, owner SID, exact
`pwsh.exe`, and complete fixed worker command line all match the protected
receipt. PID reuse, stale heartbeat, command drift, or an unverifiable owner is
fatal and leaves the install root untouched. This fallback lets an interrupted
uninstall whose task is already absent safely rerun the same `-Uninstall -Yes`
command; it is not a general process killer.

Any stop/delete/lock-timeout failure is fatal and preserves the files; the
installer never resets parent ACLs or touches another task, skill, worktree,
credential, or WindowsApps path.

An interrupted older uninstall can be recovered only when the task is
positively absent, the receipt is absent, and the protected install root
contains exactly one unlocked zero-byte `worker.lock` with the expected root
and file ACLs:

```powershell
pwsh -NoProfile -File .\scripts\install-codex-powershell-broker.ps1 `
  -RecoverPartialUninstall -Yes `
  -UserAccount 'QIN5521\qinrm' `
  -SandboxGroup 'QIN5521\CodexSandboxUsers'
```

Any additional entry, nonzero/locked file, task, receipt, reparse point, or ACL
drift is refused; this recovery mode is not a generic force-delete facility.
