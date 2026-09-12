---
id: run-evidence-02
type: implementation
status: closed
blocked_by:
  - run-evidence-01
completion:
  commits:
    - 6d749a02d8694d1f36e50676fef10ba9b00853d8
---

# 02: Make Run Evidence the publication boundary

**What to build:** Route live Run Phase, Run Diagnostic, and Run Failure publication through Run Evidence so every accepted fact is retained before observers run and is delivered at most once through its publication path. Observer exceptions remain informational evidence and cannot change work, lifecycle traversal, or Run Outcome.

- [x] Each live Run Phase, Run Diagnostic, and Run Failure is retained before its observation callback runs.
- [x] Publication state advances before calling an observer, and a throwing callback is never retried for the offending fact.
- [x] A throwing observer retains exactly one `ObserverFailed` Run Diagnostic for that failure.
- [x] An `ObserverFailed` diagnostic is not recursively delivered through the same failing publication path or replayed at a later boundary.
- [x] Live Run Event payload categories and ordering remain limited to Run Phase transitions, Run Diagnostics, Run Failures, and the terminal result.
- [x] Production and test observation adapters continue to exercise the real publication seam rather than a replacement storage abstraction.
- [x] Focused retain-before-publish and observer-failure tests and the affected profile-triplet builds pass.
