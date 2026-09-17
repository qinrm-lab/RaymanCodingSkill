# Continuity integration

Rayman Continuity owns conversation recovery, original user-request bindings,
cross-session progress and bounded handoff in enrolled repositories. Rayman's
context/map commands remain source-navigation providers; its Goals, validation
receipts and release gates remain technical authority. SaveStatus remains
independent disaster recovery. None of these capabilities grants another one's
permissions, proves host delivery, or replaces an original user instruction.

## Runtime evidence and source inputs

The repository-root `.rayman-evidence/events.jsonl`, `objects/` and `write.lock`
are live plugin state. Hooks can append evidence after any tool invocation.
The exact root-anchored ignore rules keep these runtime paths out of Git's
untracked source list and Rayman's source index/Goal fingerprint. They do not
delete, relocate or disable the evidence store. Do not put project source,
validation programs or policy in these paths.

`config.json` and `RECOVER.md` remain visible source inputs. Unexpected files
elsewhere in `.rayman-evidence/`, nested directories with that name, and normal
project changes are not hidden by these rules. Do not force-add the live ledger:
Git ignore rules do not remove already tracked files, and Rayman deliberately
rescues tracked files into its source snapshot. If runtime files are already
tracked, preserve their history and arrange an explicitly authorized index
migration before claiming stable gates.

Plugin health and original-evidence checks remain separate from the source gate.
A stable source fingerprint says nothing about the integrity or completeness of
the ignored live ledger. Use the trusted installed plugin's doctor, recover,
source inspection and claim trace operations before accepting recovered claims.
Missing originals, stale claims or missing runtime are explicit recovery gaps.

## Technical Goal binding

When a technical Goal is created for a Continuity task, pass all six
`--external-task-*` fields together: coordinator system, task id, requirement
revision, exact source reference, source-event SHA-256, and exact requirement
text SHA-256. Rayman stores this as a synthetic, immutable Goal requirement so
existing validation and lifecycle contracts bind it without changing historical
unbound Goal hashes. The binding is provenance only; it is not authorization,
completion, or host identity. Partial or malformed bindings fail closed.

## Moving or cloning the repository

Git clone alone does not carry the ignored ledger and objects. Before moving to
another computer, stop writers and copy the complete `.rayman-evidence/` store
with the repository using an authorized backup/transfer route. Retain config,
ledger and every referenced object as one coherent set; a copied recovery notice
or config alone is not recoverable history. Review evidence for sensitive data
before sharing it. No automatic remote export or cross-computer acceptance is
claimed here.

On the destination, read `RECOVER.md`, install the plugin only from the trusted
personal plugin route if missing, satisfy host hook trust, and run doctor and
recover against the copied store. Retrieve originals and revalidate source and
claims. If a fresh clone has only config/notice, report missing history instead
of silently manufacturing a new ledger under the previous repository identity.

## Source checkout versus installed CLI

Changes to `AGENT_CONTRACT.md`, `SKILL.md` or the workflow contract must keep
their packaged canonical copies byte-identical. These bytes are embedded when
the CLI is built. A development CLI built from the current source can therefore
activate a checkout that an older installed CLI rejects, even when both print
the same version. Rebinding with the old CLI cannot repair that mismatch.

Use the freshly built, source-matching CLI explicitly for local validation;
do not describe it as installed. A deployed migration requires a reviewed new
release version, the supported installer, installed identity, repository audit
and source-fresh verification. Preserve the existing install until that release
is authorized and verified. Do not weaken activation checks to make it appear
current. Retire old recovery entrypoints only after replacement coverage and
consumer acceptance; retain source navigation and professional gates.
