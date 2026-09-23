---
id: run-evidence-07
type: implementation
status: closed
blocked_by:
  - run-evidence-06
completion:
  commits:
    - 6a21819ba6ea852797f5a09f32320b4bc499bbf0
---

# 07: Render focused Run Evidence in the GUI

**What to build:** Move GUI terminal presentation to the focused read-only Run Evidence views so users continue to see complete failures, cancellation, collisions, and committed mutations without the adapter learning or depending on evidence storage layout.

- [x] GUI terminal details render Run Failures, Operation Failures, Safety Cleanup failures, Archive Collisions, and mutation summaries through focused read-only views.
- [x] Cancelled and failed runs continue to show completed attempts and committed mutations retained before termination.
- [x] Ordered Run Event handling, stale-event rejection, and terminal classification presentation remain unchanged.
- [x] The GUI observes the same immutable terminal result instance committed to the Run Handle and terminal Run Event.
- [x] GUI integration tests no longer construct or inspect mutable evidence containers.
- [x] The affected GUI suites and profile-triplet builds pass.
