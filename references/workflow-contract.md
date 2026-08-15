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
4. Split broad work into required `goal package` units. Record
   source-bound `goal progress` receipts at recovery points; they remain
   non-authoritative.
5. Run focused project tests, then record actual direct executions with
   `goal validate`. Declare the real changed paths.
6. A high-priority plan needs `goal review` bound to the final source
   fingerprint.
7. Record a recognized final repository gate with
   `--authority --repeat 2`, complete packages, close the goal, and run
   `finish --goal <id>`.

Use `--must-proof KIND::TEXT` for atomic mandatory proof. Supported kinds are
`generic`, `test`, `repository_gate`, `source_fresh`, `installation`,
`documentation`, and `git_commit`. A typed requirement accepts only a
matching validation command; split compound delivery claims into separate
requirements.

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
- Rebind is a narrow identity migration: it accepts only an existing complete,
  enabled `raymancodingskill` binding whose defects are limited to skill hash,
  CLI contract, or CLI version drift and whose current skill bytes match the
  canonical `SKILL.md` embedded in the running CLI. It updates only those
  identity scalar values under the same state lock used by activate/deactivate,
  preserving `skill_file`, comments, field order, quoting, and line endings.
  It refuses orphan, deactivated, malformed, wrong-skill, untrusted/missing-file,
  unsafe-path, and path-change cases. New activation or a canonical-path change
  uses `workspace activate --skill-file ... --yes`.
- When the user explicitly invokes Rayman for a non-read-only task, perform an
  eligible rebind and continue the original task. A read-only request does not
  authorize the state write and receives the recovery command instead.
- The Stop Hook cannot infer whether a request was read-only and never writes
  the activation contract. It sends safely rebindable drift through normal
  goal/frontier checks: no-goal and legitimately paused/complete work may end,
  but unfinished goals and structurally invalid activation remain fail-closed.
- `state audit --check` is read-only. Do not delete state to make it pass.
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

Restricted host sandboxes (for example the Codex Windows restricted token)
can deny writes even inside the workspace. Known boundaries: state lock files
under `.RaymanCodingSkill/`, `.git` writes, spawning `git`, the user-profile
checkpoint root, and user-profile toolchain caches such as the Cargo advisory
database all surface as ACL / `os error 5` denials. Treat them as environment
boundaries, not lock contention and not source defects.

- Probe before long work: `rayman doctor` and `rayman workspace inspect`
  report a state-write probe. When it reports a permission denial, run known
  state-writing commands (goal/checkpoint/autosave transactions, git
  stage/commit, installers, repository gates) with escalated host permission
  from the first attempt instead of probe-fail-retry loops.
- The `state_write` probe proves only ordinary create/write/remove capability
  under `.RaymanCodingSkill/tmp`; it is not evidence that activation metadata
  can be preserved. `doctor` and `workspace inspect` therefore report a
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
- An operator pause ("stop now, I am shutting down") has no representation in
  the goal contract: `paused_for_user` is reachable only from a recorded
  human/external blocker. With the Stop guard active and agent-owned work
  remaining, a bare pause request is therefore refused. Record the remaining
  work as pending, hand back a resume command, and say plainly that the guard,
  not the request, is what is still open.

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
validation and prevents a receipt. Receipts and invocation hashes retain the
user's logical command and never persist the effective argv, environment,
randomized lease id, or lease paths.

Authority accepts only a reviewed repository gate, selector-free workspace
Cargo tests, or selector-free workspace pytest, repeated on one unchanged
fingerprint. Current must receipts must collectively declare the real baseline
delta. Evidence-only text, progress receipts, and advisory review cannot
substitute for authority.

If the CLI is missing, prefer PATH, then a built release binary, then
`cargo run -p rayman --`. If none is available, work manually and report the
missing gate plainly; do not claim Rayman-verified completion.
