---
id: run-evidence-06
type: implementation
status: closed
blocked_by:
  - run-evidence-05
completion:
  commits:
    - ce5bfbd5654cc37273e49dd57347584b64842de5
---

# 06: Classify sealed evidence and construct passive terminal results

**What to build:** Consume complete Run Evidence into an immutable value, derive summaries from its authoritative facts, and make the Run Executor explicitly choose the terminal Run Outcome. Terminal result construction becomes a passive combination of that decision, the Run ID, final phase, and sealed evidence.

- [x] Mutation summaries are derived from completed Asset, Archive extraction, and Archive Finalization attempts while immutable evidence is created.
- [x] Aggregate Skip Reason counts are derived from authoritative discovery and Routing Ledger facts rather than retained as an independent mutable total.
- [x] The Run Executor preserves the existing precedence among fatal or unsafe work, observed cancellation, safely contained Operation Failures, and Safety Cleanup failures.
- [x] Run Evidence remains factual and contains no Run Outcome classification logic.
- [x] Terminal result construction accepts the Run Executor's chosen outcome without inspecting evidence to reclassify it.
- [x] Focused read-only terminal views expose every evidence category currently needed by GUI, CLI, and tests without revealing storage or publication state.
- [x] The Run Handle, terminal Run Event, and synchronous waiting path share the same immutable terminal result instance.
- [x] Outcome precedence, derived-value, ownership-lifetime, and terminal-result identity tests and the affected profile-triplet builds pass.
