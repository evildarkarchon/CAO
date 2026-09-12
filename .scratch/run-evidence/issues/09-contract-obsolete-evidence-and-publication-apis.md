---
id: run-evidence-09
type: implementation
status: open
triage: ready-for-agent
blocked_by:
  - run-evidence-07
  - run-evidence-08
---

# 09: Contract obsolete evidence and publication APIs

**What to build:** Complete the migration by deleting the old mutable record and publication vocabulary after every producer and consumer uses Run Evidence. The finished architecture has one obvious mutation path, focused terminal views, and tests that cannot fabricate impossible evidence states.

- [ ] The obsolete mutable work record, observation recorder, monolithic `work()` exposure, retained-publication methods, and compatibility shims are removed.
- [ ] No production or test caller directly edits evidence containers or publication cursors.
- [ ] Focused Run Evidence tests cover retention and publication invariants without duplicating scheduling scenarios.
- [ ] Lifecycle, phase traversal, cancellation, preparation failure, mandatory cleanup, and Run Outcome precedence tests cross the Run Executor seam.
- [ ] AssetRun tests cover Archive and Asset work semantics without asserting independent Run Phase authority or publication bookkeeping.
- [ ] GUI and CLI integration coverage proves ordered Run Events, complete terminal detail rendering, and shared terminal-result identity.
- [ ] All affected profile-triplet builds and complete GUI and CLI run suites pass.
- [ ] The project knowledge graph is updated after implementation changes, and no obsolete evidence interface remains in the graph or build.
