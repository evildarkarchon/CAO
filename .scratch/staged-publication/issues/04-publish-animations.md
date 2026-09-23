---
id: staged-publication-04
type: implementation
status: open
triage: ready-for-agent
blocked_by:
  - staged-publication-01
---

# 04: Publish Animations Through Temporary Ownership

## What to build

Changed Animations publish validated replacements through Temporary Ownership, with the same recovery and Committed Mutation rule as other Assets.

## Acceptance criteria

- [ ] Apply-mode Animation execution uses the one-use replace publication receipt. Dry Run, unchanged output, and unusable staged output remain nonmutating.
- [ ] A failure before publication preserves the prior Animation. A failure after publication retains the replacement, reports the Operation Failure, and records a Committed Mutation.
- [ ] Focused Animation tests verify destination bytes, unchanged and failed attempts, Safety Cleanup, and mutation evidence after deterministic ownership-release failure.
