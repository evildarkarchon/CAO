---
id: staged-publication-06
type: implementation
status: closed
blocked_by:
  - staged-publication-01
completion:
  commits:
    - 19aa60e537fb4024bbc293b174bf7948ada52a52
---

# 06: Publish Output Archives and Staged Loading Plugins

## What to build

Archive Finalization publishes planned output Archives and their staged loading plugins through Temporary Ownership. Occupied destinations remain intact, and completed publication remains visible in the attempt even if release or later cleanup fails.

## Acceptance criteria

- [x] Planned output Archives and staged loading plugins use one-use no-replace publication. A competing destination is preserved, and an exact existing dummy is reused without replacing it.
- [x] Publication and ownership release precede producer-specific source cleanup. A committed Archive or plugin survives later release failure, cancellation, or source-cleanup failure, with accurate Committed Mutation and Operation Failure evidence.
- [x] Archive Finalization retains its output planning, capacity checks, and source/backup handling. Focused tests verify output-name collisions, plugin reuse and collision, committed bytes after release failure, cancellation, and source retention.
