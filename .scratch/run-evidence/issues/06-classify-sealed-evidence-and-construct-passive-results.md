# 06: Classify sealed evidence and construct passive terminal results

**What to build:** Consume complete Run Evidence into an immutable value, derive summaries from its authoritative facts, and make the Run Executor explicitly choose the terminal Run Outcome. Terminal result construction becomes a passive combination of that decision, the Run ID, final phase, and sealed evidence.

**Blocked by:** 05: Retain Archive Finalization and cleanup evidence.

**Status:** ready-for-agent

- [ ] Mutation summaries are derived from completed Asset, Archive extraction, and Archive Finalization attempts while immutable evidence is created.
- [ ] Aggregate Skip Reason counts are derived from authoritative discovery and Routing Ledger facts rather than retained as an independent mutable total.
- [ ] The Run Executor preserves the existing precedence among fatal or unsafe work, observed cancellation, safely contained Operation Failures, and Safety Cleanup failures.
- [ ] Run Evidence remains factual and contains no Run Outcome classification logic.
- [ ] Terminal result construction accepts the Run Executor's chosen outcome without inspecting evidence to reclassify it.
- [ ] Focused read-only terminal views expose every evidence category currently needed by GUI, CLI, and tests without revealing storage or publication state.
- [ ] The Run Handle, terminal Run Event, and synchronous waiting path share the same immutable terminal result instance.
- [ ] Outcome precedence, derived-value, ownership-lifetime, and terminal-result identity tests and the affected profile-triplet builds pass.
