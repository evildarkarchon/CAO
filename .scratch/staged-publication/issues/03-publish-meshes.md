---
id: staged-publication-03
type: implementation
status: closed
blocked_by:
  - staged-publication-01
completion:
  commits:
    - 3b65f9387139e5dfb2ddc5d8aa7d561bce5fa647
---

# 03: Publish Meshes Through Temporary Ownership

## What to build

Mesh optimization and Mesh Reference Maintenance publish validated replacements through Temporary Ownership. Failed publication leaves the prior Mesh usable, while release failure retains the committed replacement in Run Evidence.

## Acceptance criteria

- [x] Changed Meshes use the one-use replace publication receipt after producer validation. Dry Run and unchanged Meshes do not publish.
- [x] A failure before publication preserves the original Mesh and reports no Committed Mutation. A failure after publication retains the replacement and records a Committed Mutation with the appropriate Operation Failure.
- [x] Focused Mesh tests cover optimization and Mesh Reference Maintenance, failed output, destination bytes, Safety Cleanup, and the deterministic post-publication ownership-release failure.
