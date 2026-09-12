---
id: run-evidence-03
type: implementation
status: closed
blocked_by:
  - run-evidence-02
completion:
  commits:
    - 348164f7e28df013cf69c18a14b2ab27f3495c1b
    - 9cd7c5861504e8770bb55660af405b8390184465
---

# 03: Retain Archive discovery and extraction evidence

**What to build:** Carry Archive discovery and extraction facts through Run Evidence from the first discovery transition to terminal inspection. Users retain complete Archive precedence explanations, completed extraction attempts, discovery failures, and recognized exclusions even when cancellation, observer failure, or later work stops the run.

- [x] Run Evidence retains Archive Collisions with the winning and shadowed Archives needed to explain Archive Precedence.
- [x] Discovery exclusions, unsupported explicitly selected paths, nested Archive counts, and discovery Run Diagnostics are retained without duplicating derived counts.
- [x] Completed Archive extraction attempts retain their Mod Root, mutation state, Operation Failure, and continuation safety in attempted order.
- [x] Cancellation observed after an Archive attempt cannot hide that completed attempt or its progress.
- [x] Discovery Run Failures remain distinct from Operation Failures and prevent mutation when the Effective Asset Tree is not trustworthy.
- [x] The Run Executor owns the corresponding Run Phase position while lower modules report typed discovery, attempt, and progress facts.
- [x] Archive selection, collision, extraction ordering, observer-failure, and cancellation behavior remain covered at the AssetRun and Run Executor seams.
- [x] The affected profile-triplet builds pass.
