# 04: Retain Routing Ledger and Asset attempt evidence

**What to build:** Carry definitive routing and Asset processing facts through Run Evidence so terminal consumers can inspect which Assets were routed, skipped, attempted, mutated, or failed without accessing mutable storage. Existing work order, progress, continuation safety, and cancellation behavior remain stable.

**Blocked by:** 03: Retain Archive discovery and extraction evidence.

**Status:** ready-for-agent

- [ ] A Routing Ledger becomes available only after definitive routing succeeds; interrupted or failed discovery cannot expose a misleading partial ledger.
- [ ] Recognized exclusions remain queryable by Skip Reason from authoritative discovery and Routing Ledger facts.
- [ ] Every completed Asset attempt retains its Routed Asset identity, canonical Mod Root, mutation state, exact Operation Failure, and continuation safety in attempted order.
- [ ] Processing Assets progress advances for successful and failed completed attempts against an immutable routed-work total.
- [ ] Unsafe mutation stops later Assets and Archive Finalization without discarding completed evidence.
- [ ] Cancellation checkpoints remain between atomic attempts, including the checkpoint after the final completed attempt.
- [ ] AssetRun tests remain focused on Routing Ledger and Asset work semantics, while lifecycle and terminal behavior are asserted through the Run Executor.
- [ ] The affected profile-triplet builds pass.
