# Test provenance and retirement policy source

Status: active
Review date: 2026-08-27
Valid until: 2027-02-27

## SRC-TEST-PROVENANCE-POLICY

The complete first-party executable-test set must expose one machine-readable
chain from a semantic source through an atomic rule and an actually reachable
execution gate to every concrete test identity. File and selector hashes bind
bytes; they do not explain why a rule or expected outcome is correct. The
semantic source supplies that reason, while each source-rule association and
test binding carries its own review horizon.

`governance/first-party-test-inventory.json` is an exact, role-qualified
inventory. Rust identities distinguish root Cargo, the standalone eval crate,
eval fixture tests, and protected eval oracle tests. PowerShell identities
distinguish a public `-SelfTest` suite, its named cases, fixture scripts, and
protected oracle markers. A same-named fixture and oracle test are two
identities. Generated run directories, target output, Git state, and Rayman
state are outside the fixed roots and cannot widen the inventory.

Every active inventory test is named by at least one active, unexpired semantic
binding whose execution gate is compatible with that test role. Mutable
binding, gate, and asset relations do not live inside the immutable test node:
each semantic binding names exactly one test ID, while dedicated-asset ownership
is path-qualified. Source `cfg` describes where test code exists; it is not
execution evidence. In particular, protected eval fixture/oracle execution is
Windows `eval_runtime` only until the evaluator implements and tests another
descendant supervisor.
Discovered-but-unregistered tests, registered-but-missing tests, unknown test
dialects, unreachable PowerShell suites, ignored Cargo tests, and platform
matrix gaps fail closed. A broad phrase such as "the full suite covers this"
cannot create a semantic binding; each binding names one exact stable test ID and
includes its current selector digest.

Review renewal is a lifecycle transition rather than an in-place edit. An old
source, source-rule link, or semantic binding retires with an immutable
tombstone and a new reviewed ID takes over the same unchanged test selector.
Gate or dedicated asset replacement follows the same pattern. Therefore review
horizons can expire and be renewed without fabricating a test-code change or mutating a
stable test identity.

Retirement is a reverse-closure transaction. Retiring a declaration/source
retires its incident source-rule links. A rule with no remaining defining
source retires with every semantic binding that uses it. A test whose last
active binding disappears retires in the same revision and its executable
selector is deleted. Shared tests remain only while another separately
reviewed, active binding survives. A negative test for deleted behavior may
remain only through an active rule that still requires that behavior to be
rejected.

Dedicated test assets use path-qualified last-reference ownership. The last test retirement
must retire the asset tombstone and delete the actual file; a shared asset must
not be deleted while any active test still refers to it. Test, asset, source,
rule, gate, and link tombstones are immutable: active IDs cannot disappear,
be reused, be born retired, or be resurrected.

Selector discovery is structural. Rust scanning removes comments and
ordinary/raw strings before locating `#[test]` functions, and compiled Cargo
harness listing cross-checks the platform-active root/eval subset. PowerShell
uses its parser AST, proves every named case reachable through the public
`-SelfTest` call graph, and proves the suite is directly dispatched by
`check-repo.ps1`. Enumeration prunes generated directories before recursion
and rejects governed reparse entries.
Git predecessor discovery distinguishes a genuine first introduction from a
deleted/reintroduced path, a shallow checkout, and an unreadable history;
uncertain history never becomes generation 1.

This file is an internal repository policy adopted during the current
maintenance change. It is not an independent transcript receipt and must not
be presented as proof of the user's exact words.
