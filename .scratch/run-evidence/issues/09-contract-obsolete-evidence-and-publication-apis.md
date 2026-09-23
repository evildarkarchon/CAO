---
id: run-evidence-09
type: implementation
status: closed
blocked_by:
  - run-evidence-07
  - run-evidence-08
completion:
  commits:
    - 63bab4ad9df8769c559d9288ce066deaf49a2063
---

# 09: Contract obsolete evidence and publication APIs

**What to build:** Complete the migration by deleting the old mutable record and publication vocabulary after every producer and consumer uses Run Evidence. The finished architecture has one obvious mutation path, focused terminal views, and tests that cannot fabricate impossible evidence states.

- [x] The obsolete mutable work record, observation recorder, monolithic `work()` exposure, retained-publication methods, and compatibility shims are removed.
- [x] No production or test caller directly edits evidence containers or publication cursors.
- [x] Focused Run Evidence tests cover retention and publication invariants without duplicating scheduling scenarios.
- [x] Lifecycle, phase traversal, cancellation, preparation failure, mandatory cleanup, and Run Outcome precedence tests cross the Run Executor seam.
- [x] AssetRun tests cover Archive and Asset work semantics without asserting independent Run Phase authority or publication bookkeeping.
- [x] GUI and CLI integration coverage proves ordered Run Events, complete terminal detail rendering, and shared terminal-result identity.
- [x] All affected profile-triplet builds and complete GUI and CLI run suites pass.
- [x] The project knowledge graph is updated after implementation changes, and no obsolete evidence interface remains in the graph or build.
