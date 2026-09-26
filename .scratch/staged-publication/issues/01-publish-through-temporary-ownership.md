---
id: staged-publication-01
type: implementation
status: closed
blocked_by: []
completion:
  commits:
    - ae3c4216a2209709c4a1ccba91c18a02f423ea72
---

# 01: Publish Staged Files Through Temporary Ownership

## What to build

A producer can write and validate a durable staged file, then use a one-use Temporary Ownership receipt to publish it with an explicit replace or no-replace rule. The result tells the producer whether the destination became a Committed Mutation and whether Temporary Ownership was released. Existing producer calls may remain available during migration so each subsequent ticket can land with a working build.

## Acceptance criteria

- [x] Durable staging supplies a move-only receipt bound to its owning Temporary Ownership scope and canonical Mod Root. Asset staging binds the intended destination; Archive staging permits a final case-resolved destination after staging. A publication attempt consumes the receipt on success or failure.
- [x] Immediately before mutation, publication checks that the destination is absolute, inside the recorded Mod Root, outside reserved staging, reached through an ordinary unchanged parent, and on the staged file's physical volume. Asset publication rejects a destination different from the one staged. Linked or changed parents, root escape, reserved targets, and unsupported cross-volume publication fail without committing a destination.
- [x] Both policies explicitly flush staged bytes and use same-volume native publication without a copy fallback. No-replace refuses an occupied destination even when it appeared after preflight.
- [x] Publication reports `NotPublished`, `PublishedStillOwned`, or `PublishedAndReleased` with error detail. The published fact is retained if temporary-name removal or durable ownership release fails; any remaining temporary path stays owned, and no destination enters the ownership record.
- [x] Interface-level tests verify replace and no-replace destination bytes, occupied no-replace and unavailable staged-file failures, one-use behavior, and a deterministic post-publication release failure caused by an occupied ownership scratch name. Safety Cleanup and later recovery preserve committed destinations and remove only temporarily owned paths; the Mod Root lock remains held through cleanup.
- [x] Existing ownership record versions and non-durable registrations continue to work. No new public fault adapter or external publication adapter is introduced.
