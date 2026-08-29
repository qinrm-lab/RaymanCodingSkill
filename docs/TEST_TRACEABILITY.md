# First-party test semantic lifecycle

The repository gate governs the complete first-party executable-test set with
two manifests:

- `governance/first-party-test-inventory.json` contains every exact,
  role-qualified executable test identity and dedicated test asset.
- `governance/test-traceability.json` contains semantic sources, atomic rules,
  reachable gates, reviewed links, and immutable lifecycle history.

The scope is `repository_first_party_executable_tests`. There is no legacy
first-party escape hatch: a newly discovered test is unregistered and fails the
gate until it receives a real semantic binding, or is deleted as an orphan.

## Byte source versus semantic source

A **byte source** answers “which exact bytes were used?” Examples are a path
plus SHA-256, a selected test-function digest, a task-tree digest, or the exact
inventory bytes. It detects replacement and drift. It does not establish why
an assertion is correct; one writer can consistently hash a wrong test.

A **semantic source** answers “why does this rule and expected result exist?”
It may be an internal contract or policy, a reviewed synthetic case, an
incident snapshot, or a versioned external standard. Each active source has a
stable ID, exact bytes, a kind, and `valid_until`.

Both are required:

```text
semantic source bytes
  -> reviewed source-rule link
  -> atomic rule
  -> reachable execution gate
  -> reviewed semantic binding with one exact role-qualified test ID
```

The binding has independent `reviewed_at` and `valid_until` fields. Its digest
covers the source/link review, rule, gate bytes and target matrix, test
selector bytes, cfg, and any dedicated asset bytes. Changing any of them
requires retiring the old binding and creating a newly reviewed binding.
Each binding owns one mutable test/gate relation; immutable inventory test
nodes do not embed binding IDs, gate IDs, or asset IDs. This permits a review
or gate to be renewed without pretending that unchanged test code changed.

## Exact inventory identities

Rust identities distinguish root Cargo, standalone eval Cargo, eval fixture,
and protected eval oracle roles. PowerShell identities distinguish public
`-SelfTest` suites, named cases, fixture scripts, and protected oracle markers.
Path, role, and selector are part of identity, so a fixture and oracle test
with the same function name never collide.

Rust source discovery removes comments and ordinary/raw strings before
recognizing `#[test]` functions. Runtime mode compiles the root and eval test
harnesses, executes `--list --format terse`, rejects ignored tests, and compares
the platform-active selector multiset with the static inventory. A duplicate
Cargo leaf selector fails closed until a full runtime identity can be proven.
PowerShell uses the parser AST, proves each named case reachable from the
public `-SelfTest` call graph, and verifies that every public suite is directly
invoked by `scripts/check-repo.ps1 -SelfTest`.

The fixed roots prune `.git`, `.RaymanCodingSkill`, every nested `target`, and
generated `evals/.runs*` trees before recursion. Governed reparse entries,
unknown test dialects, and scope widening fail closed.

Source `cfg` and gate execution targets are separate claims. Root/eval Cargo
and PowerShell suites must cover every platform selected by their `cfg`.
Protected eval fixture/oracle tests currently require exactly Windows plus the
`eval_runtime` route because the evaluator has no non-Windows descendant
supervisor; Linux/macOS compilation cannot be reported as oracle execution.

## Orphan and expiry rules

A test is blocking orphan state when any of these holds:

- discovery finds it but the active inventory does not;
- the inventory registers it but the selector no longer exists;
- it has no active semantic binding;
- every binding is expired or references a retired source, rule, or gate;
- its gate is missing, unreachable, ignored, or lacks a required platform;
- a retired selector remains executable without an explicit reviewed
  replacement;
- a last-reference dedicated asset remains after its tests retire.

The gate never auto-deletes source. It emits a deterministic failure and the
repository change must either establish a legitimate reviewed chain or remove
the orphan selector and its last-reference assets.

## Declaration deletion cascade

The checker compares the current generation with exact Git predecessor bytes
and computes the reverse closure:

1. A retired declaration/source retires every incident source-rule link.
2. A rule with no remaining active defining source retires.
3. Every semantic binding using that rule retires.
4. A test whose last active binding disappears retires in the same revision,
   and its executable selector is removed.
5. A dedicated asset retires and is deleted when its final active test
   reference disappears.

Review renewal uses the same successor transaction: retire the old source,
source-rule link, and binding, create new reviewed IDs, and point the new
binding at the unchanged stable test ID. Gate and path-qualified asset
successors work the same way. The test itself retires only when its last active
semantic binding disappears or its executable selector bytes change.

A shared test is not deleted when another separately reviewed, unexpired
binding remains. Conversely, adding an unrelated edge cannot launder an old
test: a new binding has a new stable ID, review horizon, and digest. A negative
test for removed behavior may remain only when an active rule still requires
that behavior to be rejected.

`reviewed_at` and `valid_until` prove only the recorded review horizon for that
repository-owned relation. They do not identify an independent reviewer or
constitute an external attestation; any such identity claim needs separate
evidence outside this manifest.

The tombstone stays forever even though executable code and last-reference
files are removed. Active IDs cannot disappear, be born retired, be changed in
place, be reused, or be resurrected. A legitimate test-code update retires the
old test ID and names a reviewed replacement.

## History and commands

Each manifest revision binds predecessor schema, generation, and SHA-256.
Delete/reintroduction, shallow history, unreadable Git state, and a missing
reachable predecessor fail closed; they cannot restart lifecycle history at
generation 1.

Run the focused contracts with:

```powershell
pwsh -NoProfile -File scripts/check-test-traceability.ps1 -SelfTest
pwsh -NoProfile -File scripts/check-test-traceability.ps1
pwsh -NoProfile -File scripts/check-test-traceability.ps1 -RuntimeInventory
```

`scripts/check-repo.ps1` and the full repository audit run runtime inventory.
The standalone checker default remains non-recursive so its Rust integration
test cannot launch Cargo from inside an already running Cargo test process.
