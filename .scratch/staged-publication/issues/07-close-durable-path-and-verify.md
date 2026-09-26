---
id: staged-publication-07
type: implementation
status: closed
blocked_by:
  - staged-publication-02
  - staged-publication-03
  - staged-publication-04
  - staged-publication-05
  - staged-publication-06
completion:
  commits:
    - 8cc270671affb5de44b94dcb46f741eba6e93866
---

# 07: Close the Legacy Durable Path and Verify the Optimization Run

## What to build

Every durable staged destination is now publishable only through Temporary Ownership. The documented crash states and integrated Optimization Run behavior match all migrated Asset and Archive producers.

## Acceptance criteria

- [x] Generic commit rejects durable staging and remains available for non-durable temporary registrations. No producer can release a durable claim after performing a separate publication move; tests that used that path now exercise the Temporary Ownership interface.
- [x] Existing recovery tests still cover abandoned output, publication before deregistration, active locks, corrupt ownership, and cleanup of only registered paths. Ownership record grammar and support for existing versions remain unchanged.
- [x] Staging ownership documentation covers Texture, Mesh, Animation, Archive extraction, output Archives, and staged loading plugins, including the `NotPublished`, `PublishedStillOwned`, and `PublishedAndReleased` crash states. Concise method documentation and comments explain non-obvious publication, release, and mutation ordering.
- [x] Profile-triplet builds and affected durable staging, Asset execution, Archive extraction, Archive Finalization, Run Executor, Run Evidence, GUI, and CLI suites pass. Terminal mutation retention, cancellation, Safety Cleanup, and Run Outcome precedence remain behaviorally unchanged; any profile-triplet blocker is diagnosed and reported.
