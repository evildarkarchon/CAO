---
id: staged-publication-02
type: implementation
status: open
triage: ready-for-agent
blocked_by:
  - staged-publication-01
---

# 02: Publish Textures Through Temporary Ownership

## What to build

Native and convertible Textures use Temporary Ownership to replace their destinations after format-specific validation. A failed publication preserves usable original bytes; a published destination remains a Committed Mutation even when ownership release fails.

## Acceptance criteria

- [ ] Texture execution writes and validates staged output, then requests replace publication through the one-use receipt. Dry Run creates no staging, publication, or ownership release.
- [ ] An unpublished failure retains the original Asset and reports an Operation Failure with no Committed Mutation. A post-publication ownership-release failure retains the destination and reports its Committed Mutation without probing the filesystem to infer whether publication happened.
- [ ] Conversion removes its source only after destination publication and ownership release. Failed conversion retains usable source or destination bytes, and existing backup behavior remains producer-owned.
- [ ] Focused Texture tests retain the existing interruption and source-removal coverage and verify destination bytes, surviving source files, and mutation evidence after the deterministic release failure.
