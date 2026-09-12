---
id: run-evidence-01
type: implementation
status: closed
blocked_by: []
completion:
  commits:
    - 60d1da5e845aaf8d54ad1d54331c851b7c44296d
---

# 01: Seal Preparing and lifecycle facts as Run Evidence

**What to build:** Introduce the concrete mutable and immutable Run Evidence ownership pair and use it for a complete Preparing-to-terminal path. Successful preparation becomes visible atomically, lifecycle facts remain structurally valid, and consuming the mutable value creates self-contained terminal evidence that no producer can mutate afterward.

- [x] Run Evidence accepts one complete successful preparation containing resolved Mod Roots, Routing Policy, configuration, and Archive Precedence, while failed or cancelled preparation exposes none of those facts as a partial success.
- [x] Run Evidence retains the latest record for each traversed Run Phase while preserving first-traversal order.
- [x] Canonical Run Phase order, monotonic phase-local progress, single successful preparation, and rejection of mutation after consumption are enforced as programming invariants.
- [x] Cancellation observation can be retained independently of the eventual Run Outcome.
- [x] Consuming mutable Run Evidence produces a distinct immutable value that survives destruction of the mutable owner and preparation producers.
- [x] Preparing-failure and no-work Run Executor scenarios cross the new ownership boundary without changing phase traversal, cancellation checkpoints, or terminal behavior.
- [x] Focused Run Evidence tests and the affected profile-triplet builds pass.
