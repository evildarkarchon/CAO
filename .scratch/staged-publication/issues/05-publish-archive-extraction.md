---
id: staged-publication-05
type: implementation
status: closed
blocked_by:
  - staged-publication-01
completion:
  commits:
    - bfea75744e54d261c415f7a2b2d12d8675f7cb06
---

# 05: Publish Archive Extraction Entries Through Temporary Ownership

## What to build

Archive extraction publishes each staged entry through Temporary Ownership with no-replace semantics, while Archive extraction continues to resolve game-path casing and protect Loose Asset precedence.

## Acceptance criteria

- [x] Extraction stages and validates entries before the merge, then supplies each final case-resolved destination to the one-use no-replace publication receipt. Archive-specific occupied-leaf checks and pinned destination parents remain in force through publication.
- [x] A Loose Asset created after preflight is never overwritten. Linked or changed destination parents are rejected before publication, and staged Archive bytes receive the shared explicit flush.
- [x] A failed later merge retains the existing attempt-level `PartialOrUnknown` mutation result and unsafe continuation. No per-entry Archive mutation evidence is added.
- [x] Focused tests verify no-replace collisions, case-correct targets, parent protection, and a deterministic release failure after an entry publishes. After the producer ends, recovery preserves the committed destination and removes only temporarily owned paths.
