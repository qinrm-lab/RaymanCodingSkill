# Source bytes and evidence identities

Repository-owned source text uses UTF-8 without a BOM and LF. Empty files and
missing final newlines are preserved. `.editorconfig`, `.gitattributes`, and
`governance/source-bytes-policy.json` describe the same boundary. The policy
explicitly classifies text and binary extensions/names; unknown types fail.
Exact binary fixtures may use `binary_paths` with matching binary Git
attributes. Exact CRLF fixtures require a `crlf_paths` entry, matching Git
attributes, and a literal EditorConfig section with `end_of_line = crlf` after
the UTF-8/LF defaults. Exception paths cannot contain glob metacharacters.
Git index text still uses LF for such a fixture. Invalid UTF-8 is never treated
as an implicit binary exception.

## Check before hashing

```powershell
pwsh -NoProfile -File scripts/source-bytes.ps1
pwsh -NoProfile -File scripts/source-bytes.ps1 -Json
```

The checker reads raw worktree bytes and raw index blobs through Git's binary
pipes. It includes tracked files and unignored new files, reports both views,
and refuses unmerged or case-colliding entries, gitlinks, symlinks/reparse entries, noncanonical
encoding and conflicting attributes. It does not execute clean filters. A
clean `git diff` does not substitute for this check: Git may normalize CRLF
while a worktree SHA-256 still changes. The fast gate, complete audit and
three-platform primary CI gate run this preflight before expensive validation.

## Preview repairs without changing source

```powershell
pwsh -NoProfile -File scripts/source-bytes.ps1 -Mode Plan
pwsh -NoProfile -File scripts/source-bytes.ps1 -Mode Plan -Paths scripts/example.ps1 -CandidateDirectory <new-absolute-managed-temp-directory>
```

Plans show old/new SHA-256 values. Candidate generation removes an initial
UTF-8 BOM and converts CRLF to LF; it preserves final-newline presence and
refuses repeated BOMs, bare CR, NUL, or invalid UTF-8. It never modifies source or the index,
including partially staged files. Binary and declared CRLF fixtures are not
rewritten. Review candidates and publish them through the normal source-edit
workflow; recheck worktree/index and regenerate current evidence afterwards.
The candidate directory must be new. Use managed temp, not a source directory.

## Generate reviewed governance manifests

```powershell
pwsh -NoProfile -File scripts/update-test-traceability.ps1 -DraftInventoryPath <inventory-draft.json> -DraftManifestPath <traceability-draft.json> -OutputDirectory <new-absolute-managed-temp-directory>
pwsh -NoProfile -File scripts/update-test-traceability.ps1 -DraftInventoryPath <inventory-draft.json> -DraftManifestPath <traceability-draft.json> -OutputDirectory <another-new-absolute-managed-temp-directory> -Publish -Yes
pwsh -NoProfile -File scripts/update-test-traceability.ps1 -Recover -Yes
pwsh -NoProfile -File scripts/update-test-traceability.ps1 -Summary
```

Draft authors still supply reviewed rules, successor IDs, retirement links and
review horizons. Generation computes only derived selector/source/gate/asset,
inventory and semantic hashes, and binds revisions to committed predecessors.
It uses the existing traceability checker and projection function, validates
immutable history and all active relationships, emits LF bytes, then re-reads
those bytes to verify their hashes. It cannot silently renew a changed immutable
ID. `-ListInventory` remains a discovery report, not an authorized manifest.

Generation alone writes a new candidate directory. Explicit publication accepts
only those validated candidate bytes and the original worktree hashes for the
two fixed governance files. A workspace-scoped lock and durable preimage journal
protect two-file publication; replacement/readback failure restores the old
tuple. After an interrupted process, `-Recover -Yes` restores only files whose
bytes still match the journal's before/after identities. Unknown drift retains
the journal and fails closed. Neither generation nor publication rewrites
historical hashes, retired entries, signatures, installation files or user
configuration. Summary is a non-authoritative view of the complete ledger.

## Commit integration

Run the same check immediately before creating a commit request. An optional
ordinary Git adapter is provided in `.githooks/pre-commit`; enabling it is a
separate local configuration choice and must preserve existing hooks. On Unix
the adapter must be executable. Protected `git_local_commit_v1` deliberately
disables Git hooks: do not weaken that capability or ask its privileged worker
to execute mutable repository scripts. Its caller runs the preflight; existing
worktree SHA-256 and filtered index-blob checks remain authoritative for commit
identity. CI executes the source gate even if an ordinary local hook is absent.

## Hash contract

Raw SHA-256 remains exact for source fingerprints, validation receipts, trust,
checkpoints and installed artifacts. Git blob OIDs identify Git's index
representation and remain a separate field. A normalized hash is diagnostic
only: the quality provider reader can explain a CRLF/BOM-only mismatch while
still rejecting it. Source generation canonicalizes bytes before hashing;
validation, installation, recovery and signature verification never normalize
bytes into acceptance.
