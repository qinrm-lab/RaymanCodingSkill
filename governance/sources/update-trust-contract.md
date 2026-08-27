# Trusted update and installation source

Status: active
Review date: 2026-08-27
Valid until: 2027-02-27

## SRC-UPDATE-TRUST-CONTRACT

The trusted update floor is monotonic in installed version, highest observed
version, key epoch, sequence, manifest digest, and observation time. An exact
floor identity is only a classification result; it never authorizes recovery
without the separately bound active request and exact publication journal.
Clock rollback beyond the documented tolerance, a lower axis, a repeated
sequence for a newer version, or a same-version different manifest fails
closed.

Publication recovery is authorized only by the exact verified plan and its
fixed six-role journal. The journal schema, role order, destinations, backup
paths, old/new hashes, absence expectations, completion prefix, phase, next
role, and committed flag must all be consistent before recovery reads it as
authority. A committed journal additionally requires every destination to
retain the committed hash.

The source-fresh installer may upgrade to a different verified version. For an
already installed identical version, it may be idempotent only when the CLI,
worker, CLI contract, managed skill resources, install-manifest hash, source
class, and signed-release tuple are exactly equal and the live destination
bytes still match. A same-version difference is equivocation and must be
rejected before publication; a new installation ID or timestamp alone is not a
semantic tuple change and must not replace the existing receipt.
