# 03: Retain Archive discovery and extraction evidence

**What to build:** Carry Archive discovery and extraction facts through Run Evidence from the first discovery transition to terminal inspection. Users retain complete Archive precedence explanations, completed extraction attempts, discovery failures, and recognized exclusions even when cancellation, observer failure, or later work stops the run.

**Blocked by:** 02: Make Run Evidence the publication boundary.

**Status:** ready-for-agent

- [ ] Run Evidence retains Archive Collisions with the winning and shadowed Archives needed to explain Archive Precedence.
- [ ] Discovery exclusions, unsupported explicitly selected paths, nested Archive counts, and discovery Run Diagnostics are retained without duplicating derived counts.
- [ ] Completed Archive extraction attempts retain their Mod Root, mutation state, Operation Failure, and continuation safety in attempted order.
- [ ] Cancellation observed after an Archive attempt cannot hide that completed attempt or its progress.
- [ ] Discovery Run Failures remain distinct from Operation Failures and prevent mutation when the Effective Asset Tree is not trustworthy.
- [ ] The Run Executor owns the corresponding Run Phase position while lower modules report typed discovery, attempt, and progress facts.
- [ ] Archive selection, collision, extraction ordering, observer-failure, and cancellation behavior remain covered at the AssetRun and Run Executor seams.
- [ ] The affected profile-triplet builds pass.
