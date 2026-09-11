# 08: Render focused Run Evidence in the CLI

**What to build:** Move CLI terminal presentation to the same focused read-only Run Evidence views as the GUI so presentation choice does not change which run facts are available or how terminal failures and committed mutations are explained.

**Blocked by:** 06: Classify sealed evidence and construct passive terminal results.

**Status:** ready-for-agent

- [ ] CLI terminal output renders Run Failures, Operation Failures, Safety Cleanup failures, Archive Collisions, and mutation summaries through focused read-only views.
- [ ] CLI and GUI have evidence-category parity for cancelled, contained-failure, unsafe, and cleanup-failure runs.
- [ ] Existing live Run Event rendering, cooperative cancellation behavior, and exit-code behavior remain unchanged.
- [ ] The CLI observes the same immutable terminal result instance committed to the Run Handle and terminal Run Event.
- [ ] CLI integration tests no longer construct or inspect mutable evidence containers.
- [ ] The affected CLI suites and profile-triplet builds pass.
