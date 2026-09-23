---
id: staged-publication-02
type: implementation
status: closed
blocked_by:
  - staged-publication-01
completion:
  commits:
    - 261e1cb1e436e83b63eb9c51056e6bcc43e7646f
---

# 02: Publish Textures Through Temporary Ownership

## What to build

Native and convertible Textures use Temporary Ownership to replace their destinations after format-specific validation. A failed publication preserves usable original bytes; a published destination remains a Committed Mutation even when ownership release fails.

## Acceptance criteria

- [x] Texture execution writes and validates staged output, then requests replace publication through the one-use receipt. Dry Run creates no staging, publication, or ownership release.
- [x] An unpublished failure retains the original Asset and reports an Operation Failure with no Committed Mutation. A post-publication ownership-release failure retains the destination and reports its Committed Mutation without probing the filesystem to infer whether publication happened.
- [x] Conversion removes its source only after destination publication and ownership release. Failed conversion retains usable source or destination bytes, and existing backup behavior remains producer-owned.
- [x] Focused Texture tests retain the existing interruption and source-removal coverage and verify destination bytes, surviving source files, and mutation evidence after the deterministic release failure.
