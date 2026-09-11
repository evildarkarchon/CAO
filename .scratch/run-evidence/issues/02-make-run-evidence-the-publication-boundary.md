# 02: Make Run Evidence the publication boundary

**What to build:** Route live Run Phase, Run Diagnostic, and Run Failure publication through Run Evidence so every accepted fact is retained before observers run and is delivered at most once through its publication path. Observer exceptions remain informational evidence and cannot change work, lifecycle traversal, or Run Outcome.

**Blocked by:** 01: Seal Preparing and lifecycle facts as Run Evidence.

**Status:** ready-for-agent

- [ ] Each live Run Phase, Run Diagnostic, and Run Failure is retained before its observation callback runs.
- [ ] Publication state advances before calling an observer, and a throwing callback is never retried for the offending fact.
- [ ] A throwing observer retains exactly one `ObserverFailed` Run Diagnostic for that failure.
- [ ] An `ObserverFailed` diagnostic is not recursively delivered through the same failing publication path or replayed at a later boundary.
- [ ] Live Run Event payload categories and ordering remain limited to Run Phase transitions, Run Diagnostics, Run Failures, and the terminal result.
- [ ] Production and test observation adapters continue to exercise the real publication seam rather than a replacement storage abstraction.
- [ ] Focused retain-before-publish and observer-failure tests and the affected profile-triplet builds pass.
