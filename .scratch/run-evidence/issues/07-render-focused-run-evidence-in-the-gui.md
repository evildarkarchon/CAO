# 07: Render focused Run Evidence in the GUI

**What to build:** Move GUI terminal presentation to the focused read-only Run Evidence views so users continue to see complete failures, cancellation, collisions, and committed mutations without the adapter learning or depending on evidence storage layout.

**Blocked by:** 06: Classify sealed evidence and construct passive terminal results.

**Status:** ready-for-agent

- [ ] GUI terminal details render Run Failures, Operation Failures, Safety Cleanup failures, Archive Collisions, and mutation summaries through focused read-only views.
- [ ] Cancelled and failed runs continue to show completed attempts and committed mutations retained before termination.
- [ ] Ordered Run Event handling, stale-event rejection, and terminal classification presentation remain unchanged.
- [ ] The GUI observes the same immutable terminal result instance committed to the Run Handle and terminal Run Event.
- [ ] GUI integration tests no longer construct or inspect mutable evidence containers.
- [ ] The affected GUI suites and profile-triplet builds pass.
