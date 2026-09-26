---
id: run-evidence-05
type: implementation
status: closed
blocked_by:
  - run-evidence-04
completion:
  commits:
    - c8c6df60c7b36fccb0e0cba69e79b81cb45ef27d
---

# 05: Retain Archive Finalization and cleanup evidence

**What to build:** Complete factual Run Evidence ownership through Archive Finalization and Safety Cleanup. Every finalization and cleanup fact is retained in attempted order before evidence is consumed, while the Run Executor becomes the sole authority for the final work phase and mandatory cleanup traversal.

- [x] Archive Finalization attempts retain their Mod Root, mutation state, Operation Failure, and continuation safety without duplicating failures in run-level storage.
- [x] Finalization cancellation is retained and prevents any later cancellable work while preserving completed mutations.
- [x] Attempt-local cleanup failures remain distinct from Run Failures and final Safety Cleanup failures.
- [x] Every Safety Cleanup failure is retained in attempted order, including failures produced when the cleanup service throws unexpectedly.
- [x] Safety Cleanup executes exactly once on every terminal path, is not cancellable, and completes before mutable Run Evidence is consumed.
- [x] Lower work modules no longer independently define Run Phase position; the Run Executor alone advances phases and selects the final work phase.
- [x] Archive Finalization, mandatory cleanup, cleanup ordering, and failure-category separation remain covered at their highest deterministic seams.
- [x] The affected profile-triplet builds pass.
