---
id: run-evidence-08
type: implementation
status: closed
blocked_by:
  - run-evidence-06
completion:
  commits:
    - bc9fbfc5c60dbe6569e3516cfc799311be659ffe
---

# 08: Render focused Run Evidence in the CLI

**What to build:** Move CLI terminal presentation to the same focused read-only Run Evidence views as the GUI so presentation choice does not change which run facts are available or how terminal failures and committed mutations are explained.

- [x] CLI terminal output renders Run Failures, Operation Failures, Safety Cleanup failures, Archive Collisions, and mutation summaries through focused read-only views.
- [x] CLI and GUI have evidence-category parity for cancelled, contained-failure, unsafe, and cleanup-failure runs.
- [x] Existing live Run Event rendering, cooperative cancellation behavior, and exit-code behavior remain unchanged.
- [x] The CLI observes the same immutable terminal result instance committed to the Run Handle and terminal Run Event.
- [x] CLI integration tests no longer construct or inspect mutable evidence containers.
- [x] The affected CLI suites and profile-triplet builds pass.
