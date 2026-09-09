---
name: codex-global-execution
description: Use the installed Windows desktop-user backend for local commits, opted-in workflow state, and registered file installations in Codex projects. Use when an authorized commit or installation needs this backend, or sandbox and desktop-user ownership conflict. Ordinary editing and tests stay in the sandbox.
---

# Codex global execution

Use the protected client at
`C:\ProgramData\Rayman\CodexGlobalExecution\client.exe`. If
`RAYMAN_GLOBAL_EXECUTION_ROOT` is configured, use that root's client instead.
The native client verifies the installation and enrollment; a path, source
checkout, project trust setting, or copied JSON is not backend authorization.

Start with `client.exe --format json status --root <root>`. Distinguish worker
liveness, the current phase, pending installation recovery, and operation
success. Missing installation or enrollment is a setup boundary, not grounds
to launch an arbitrary command as the desktop user or to change ACLs.

## Local commits

Keep the workspace's own validation and review requirements. After the user
authorizes a local commit and the relevant checks pass, preview its exact
paths with:

```text
client.exe --format json commit --root <root> --workspace <workspace> --path <relative-file> --message <message>
```

Repeat `--path` for each selected file. The client binds raw file bytes,
filtered Git blob IDs, HEAD, index, policy, and sandbox-side hook results.
Submit the same intended scope with `--yes`; inspect the bound result and live
Git state. Do not normalize files merely to make hashes agree. Do not edit a
request packet to bypass a stale source, hook, signing, or filter rejection.
Git push is a separate user action and is not supported by this backend.

After a timeout, query `result --root <root> --request-id <id>` before retrying.
A failed or recovery-required effect is not a commit. The worker's own source
repository has a separate commit boundary; never evade it using a copied repo.

## Workflow state

Use the application's normal frontend after its owner-published route is
active. Rayman and SaveStatus keep application validation and collection in
the sandbox and send typed persistence operations to the owner. Do not hand
author Goal/test receipts, SQL, migration markers, or checkpoint row deltas.
Enrollment alone does not migrate a database. Preserve verified backups and
complete the application's supported preparation/publication steps first.
The supported rollout registers Rayman and SaveStatus adapters before the first
project operation, upgrades local SaveStatus runtimes, then publishes exact
Rayman/checkpoint routing markers. Inspect the reported post-migration Git
status; managed instruction updates may require the project's normal validation
and local commit.

## Installation

Use only an owner-registered adapter. Its fixed role-to-destination mapping
cannot be supplied or widened by an installation request. Build and validate
the payload in the sandbox, then use `install --root <root> --workspace
<workspace> --adapter-id <id> --version <version> --sources <json-map>` to
preview. The JSON map contains exact registered roles and workspace-relative
payload paths. `--yes` publishes files with the protected transaction kernel.

Use the protected client directly when replacing Rayman itself so a waiting
Rayman process does not lock its installed executable. The backend never runs
the installed application or a project script. File publication is followed
by the application's normal runtime, source-freshness, and host-integration
checks. `runtime_validation_required=true` is not installation completion.
Keep a failed transaction's result and backups; recovery cannot discard a
concurrent destination change.

## Authorization and identity

Reuse the user's explicit authorization when its operation, principal,
transport, scope and side effects match. Source changes or a new Codex thread
do not alone require the same approval again. This skill creates no grant for
new destinations, accounts, scripts, UAC, system security changes, publication,
or unrelated projects. Preserve the elevated Windows sandbox and the current
repository's policy. In particular, do not use a sandbox downgrade, blanket
ACL repair, alternate identity shell, or broker bypass to make a request pass.
