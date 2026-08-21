# Trusted update contract

Rayman separates release discovery, installation consent, authenticated
content, and workspace activation. None of those layers may substitute for
another.

## User behavior

An explicit non-read-only skill invocation runs:

```text
rayman --format json update poll
```

Notification checks are enabled by default and throttled to 24 hours. They use
the compiled `qinrm-lab/RaymanCodingSkill` GitHub Releases endpoint and update
only `%LOCALAPPDATA%\Rayman\update\update.json` on Windows (the XDG user-data
equivalent elsewhere). `update status` is offline and read-only; `update check`
is an explicit network read that does not write cache. A read-only coding
request does not run poll.

Automatic installation is disabled independently:

```powershell
rayman update configure --auto-install --yes
rayman update configure --no-auto-install --yes
rayman update configure --no-auto-check --yes
rayman update configure --auto-check --interval-hours 24 --yes
```

Legacy check preferences always migrate with `auto_install=false`. Disabling
checks does not silently grant or revoke installation consent.

When a due poll finds a newer stable version, it normally emits only a release
notification. With explicit installation consent and a valid supported-install
receipt, it may return one structured `worker_launch` containing a fixed local
program and argv. The skill executes those values directly, never as a shell
string. A successful worker returns `restart_required=true`; restart Codex
before claiming the new global skill is loaded. The next invocation may run
`workspace ensure-current --yes`, which reuses the existing identity-only
rebind transaction and cannot activate an orphan, disabled, wrong-skill,
path-changed, malformed, missing, or untrusted workspace.

## Discovery is not authority

The Releases tag list accepts only public, non-draft, non-prerelease
`vMAJOR.MINOR.PATCH` tags from the fixed HTTPS endpoint. It disables redirects,
uses short WinHTTP timeouts, and caps the response at 1 MiB. Those controls make
notification bounded; they do not authenticate installable content. A tag,
cached observation, GitHub asset identifier, browser URL, or a SHA-256 computed
from the downloaded file is never installation authority.

## Signed manifest

The only install authority is a canonical `rayman.update.manifest.v1` JSON
document plus its 64-byte detached Ed25519 signature. The signature covers the
domain-separated bytes:

```text
RaymanCodingSkill update manifest v1\0 || canonical_manifest_bytes
```

The client rejects BOMs, whitespace/key-order variants, duplicate or unknown
fields, unknown key IDs/epochs, zero sequences, invalid or expired validity
windows, tag/version/contract/platform mismatches, noncanonical commit or hash
fields, missing/extra/duplicate roles, and asset aliases. The fixed Windows
x86_64 manifest contains exactly:

- `rayman-windows-x86_64.exe`
- `rayman-update-worker-windows-x86_64.exe`
- `raymancodingskill-SKILL.md`
- `raymancodingskill-AGENTS.md`
- `raymancodingskill-workflow-contract.md`
- `install-rayman.ps1`

Each role binds its exact name, size, and SHA-256. The manifest contains no URL
or installation destination. The client constructs only the official GitHub
Release path, permits at most one HTTPS redirect, checks the final host against
the compiled GitHub release-CDN allowlist, and still requires the signed size
and hash.

The production public key is compiled into both binaries. An unprovisioned,
malformed, unknown, test, or all-zero key disables trusted apply; there is no
unsigned fallback. Key rotation requires a new client released under the
currently trusted root and a monotonic key epoch. Production private keys must
never enter the repository, a test fixture, a release asset, logs, or local
update state.

## Receipt, replay floor, and worker

The supported source installer creates
`%LOCALAPPDATA%\Rayman\install\receipt.json` in the same rollback domain as the
CLI, versioned worker, and three deployed skill resources. It binds the
installation ID, absolute managed destinations, exact hashes, version,
contract, install-manifest hash, source type, and (for signed releases) the
manifest digest/key epoch/sequence. A Cargo target binary, renamed copy,
hard-link alias, PATH shadow, changed resource, missing receipt, or different
destination cannot request automatic apply.

The worker re-reads consent and the complete receipt tuple after acquiring the
installation-scoped Windows mutex. It rejects versions at or below the
installed version, lower key epochs/sequences, lower previously seen versions,
clock rollback, and same-version manifest equivocation. Exact manifest replay
is allowed only to recover its existing transaction.

Immediately before the first transaction-directory mutation the worker writes
`active.json`, binding the request, prior receipt/version, managed CLI path,
and prior versioned-worker path/hash. A later poll gives this recovery record
priority over notification and returns the exact prior worker even if the user
has since disabled new automatic installs: recovery is authority to finish or
roll back only that existing transaction, not to choose another release. If no
journal was ever written, the staged-only directory is removed by held-handle
cleanup and the old generation remains. Once a journal exists, the exact signed
bundle and captured plan must reverify before recovery runs.

Downloaded files are created exclusively below a held no-reparse transaction
root and revalidated by legacy and 128-bit Windows file identity. Publication
uses the same handle-relative, no-replace CAS primitives as the supported
installer. The write-ahead journal records the old/new hashes and deterministic
backup names before each mutation. A caught failure rolls records back in
reverse. On restart, an incomplete journal can restore only receipt-bound old
bytes; an unexpected replacement is retained and blocks instead of being
overwritten or deleted. Skill resources publish first, the new versioned worker
next, the CLI after supporting files, and the new receipt last. An isolated
workspace activation plus `doctor --check` must pass before the journal commit
marker. Committed-backup cleanup is warning-only.

Committed transaction evidence is retained rather than deleted in the same
worker run. This closes the crash window between the installer commit marker
and `active.json` becoming `committed`: a restarted worker can prove the exact
new generation from the retained journal. Removing older committed transaction
evidence requires a later explicit retention policy.

The updater never changes user PATH, Codex hooks, another workspace, or the
current workspace activation.

## Release production

Tag CI first completes the normal format, Clippy, test, MSRV, dependency,
installer, package, coverage, and source-fresh release-contract jobs. The
protected `rayman-release` environment then builds the two Windows binaries,
copies the fixed resource set, derives canonical manifest and signing payload
bytes through `rayman-update-worker create-manifest`, signs with OpenSSL using
the environment secret `RAYMAN_UPDATE_SIGNING_KEY_PEM_B64`, verifies the
signature both with the key derived from the secret and with the production
public key compiled into the release worker, deletes the temporary private key
and signing payload, and publishes only the fixed assets for the exact Git tag.

Repository code cannot prove GitHub environment reviewers, tag/release
protection, or private-key custody. Those are external release prerequisites.
Until a production public key and matching protected private-key secret are
configured and a signed release is published, only notification is available;
claiming production automatic update would be false.
