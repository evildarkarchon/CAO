---
id: run-evidence-04
type: implementation
status: closed
blocked_by:
  - run-evidence-03
completion:
  commits:
    - 5e211201ea64a60896c234a84873315ee51cf68d
    - 6971ecfba0045492f200a8bb945b0cd6bd83bdcb
---

# 04: Retain Routing Ledger and Asset attempt evidence

**What to build:** Carry definitive routing and Asset processing facts through Run Evidence so terminal consumers can inspect which Assets were routed, skipped, attempted, mutated, or failed without accessing mutable storage. Existing work order, progress, continuation safety, and cancellation behavior remain stable.

- [x] A Routing Ledger becomes available only after definitive routing succeeds; interrupted or failed discovery cannot expose a misleading partial ledger.
- [x] Recognized exclusions remain queryable by Skip Reason from authoritative discovery and Routing Ledger facts.
- [x] Every completed Asset attempt retains its Routed Asset identity, canonical Mod Root, mutation state, exact Operation Failure, and continuation safety in attempted order.
- [x] Processing Assets progress advances for successful and failed completed attempts against an immutable routed-work total.
- [x] Unsafe mutation stops later Assets and Archive Finalization without discarding completed evidence.
- [x] Cancellation checkpoints remain between atomic attempts, including the checkpoint after the final completed attempt.
- [x] AssetRun tests remain focused on Routing Ledger and Asset work semantics, while lifecycle and terminal behavior are asserted through the Run Executor.
- [x] The affected profile-triplet builds pass.
