# Global Codex execution contract

This source records the user's 2026-09-08 approval in Codex task
01a07e7a-ad4e-7ae0-a0e4-c89c3858994a to implement a reusable, cross-project
execution mechanism. It is a design requirement, not a grant for an arbitrary
program to run as the desktop user.

## GLOBAL-REGISTRY

One protected installation serves multiple explicitly enrolled repositories.
Every registration binds a stable project ID, strong root/Git directory
identities, owner SID, fixed capabilities, and exact protected policy bytes.
Worktrees have distinct IDs and share a Git-common-directory commit lock.
Registration cannot be inferred from project text, a remote URL, or a request.
No global or cross-project registration is enabled by default.

## GLOBAL-PROTOCOL

Requests are bounded UTF-8 JSON with exact fields and duplicate-key rejection.
They bind project, worktree, registration hash, expected source identity,
operation, unique request ID and expiry. Unknown operations/fields, arbitrary
argv, root paths and program/script names are rejected before mutation.
Authorization is protected enrollment plus an exact allowed operation, not
caller-authored text. Scope widening and installation remain separate grants.

## GLOBAL-COMMIT

Commit requests bind exact sorted added/modified/deleted paths, before and
after raw bytes and Git modes, filtered Git blob IDs, HEAD and index identity.
Unlisted changes are preserved. Staged changes, unsafe path aliases, content
drift, non-ordinary files and concurrent ref/index writers fail closed.
Fixed Git plumbing cannot execute hooks, filters, helpers or network commands
as the desktop user. Ref/index publication is journaled and recoverable.

New Git metadata files retain the publisher's owner/group while preserving the
source access policy; publication must not demand assignment of a historical
owner. Seed identities are durable before metadata changes and lock claims.
Unjournaled seeds are retained and never adopted by name or hash alone; unique
staging attempts are bounded. Individual object seeds are journaled before rename.

Commit request bytes are retained before admission. Recovery is a separate,
freshly authorized request referring to the original intent and reviewed
candidate bytes; it never overwrites the original queue failure. A legacy
missing-request candidate requires no unattested hook, a matching complete Git
snapshot, exact selected/preserved paths, matching index/tree/object hashes and
the enrolled commit identity. Its new recovery verification is not an invented
original request. The own-worker-source commit prohibition also covers recovery.

## GLOBAL-STATE

Formal state has one desktop-user writer through typed operations. Reads,
ordinary source editing and tests stay in the sandbox. Projects, worktrees,
tasks and state kinds have distinct namespaces; no caller-selected filesystem
destination is accepted. CAS revisions reject lost updates. Request replay
returns the recorded outcome only when the request digest is identical.
Migration preserves a verified backup and all records; it never synthesizes
successful validation or copies stale evidence into new requirements.

## GLOBAL-INSTALL

Installation uses a protected, registered adapter and fixed destinations.
It never executes an unreviewed project script as the desktop user. Source,
artifact and tool identities are checked before reuse or continuation.
Local installation and complete release acceptance are separate claims.
The persistent least-privilege worker task must allow battery operation and
continue when AC power is disconnected. Installation validates both explicit
battery settings; missing, duplicate, malformed or stopping settings fail
closed. A healthy startup alone cannot prove this lifecycle contract.
Kernel maintenance pauses queue consumption and startup recovery, and clients
refuse new submissions. New code must be verified while requests remain paused
and recovery data is pinned. Once activation may permit new-format writes,
failure recovery moves forward; it cannot restore an older decoder over them.
Unchanged source alone is insufficient to reuse checks affected by tools,
environment or test scope. Progress distinguishes process liveness from
successful execution. Completion requires actual recorded outcomes.

## GLOBAL-ROLLOUT

Keep the Windows elevated sandbox. Prepare and test the installation package
before requesting a required host registration. Do not repair broad ACLs,
replace Codex databases or silently enroll other projects. Migration must
be explicit, reversible and independently verified. SSH hosts need their own
installation; this local service cannot claim to cover them.

## GLOBAL-WORKSPACE-ACL

Before installation and enrollment, inspect every trusted project's workspace
root and `.git` marker. A directory owned by a Codex sandbox principal must
grant the selected desktop owner only non-inherited `ChangePermissions` there
before Codex can publish its own deny ACE. Never recurse, reset the ACL, change
content rights, follow a reparse point, or treat a linked-worktree marker as a
directory. A foreign owner fails closed. Do not initialize repositories from
the sandbox; new Git metadata must be created by the selected qinrm owner.
