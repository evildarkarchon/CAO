# 05: Retain Archive Finalization and cleanup evidence

**What to build:** Complete factual Run Evidence ownership through Archive Finalization and Safety Cleanup. Every finalization and cleanup fact is retained in attempted order before evidence is consumed, while the Run Executor becomes the sole authority for the final work phase and mandatory cleanup traversal.

**Blocked by:** 04: Retain Routing Ledger and Asset attempt evidence.

**Status:** ready-for-agent

- [ ] Archive Finalization attempts retain their Mod Root, mutation state, Operation Failure, and continuation safety without duplicating failures in run-level storage.
- [ ] Finalization cancellation is retained and prevents any later cancellable work while preserving completed mutations.
- [ ] Attempt-local cleanup failures remain distinct from Run Failures and final Safety Cleanup failures.
- [ ] Every Safety Cleanup failure is retained in attempted order, including failures produced when the cleanup service throws unexpectedly.
- [ ] Safety Cleanup executes exactly once on every terminal path, is not cancellable, and completes before mutable Run Evidence is consumed.
- [ ] Lower work modules no longer independently define Run Phase position; the Run Executor alone advances phases and selects the final work phase.
- [ ] Archive Finalization, mandatory cleanup, cleanup ordering, and failure-category separation remain covered at their highest deterministic seams.
- [ ] The affected profile-triplet builds pass.
