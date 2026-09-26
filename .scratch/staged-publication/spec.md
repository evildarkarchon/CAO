---
id: staged-publication
type: specification
status: open
triage: ready-for-agent
blocked_by: []
---

# Deepen Temporary Ownership Around Staged Publication

## Problem Statement

An Optimization Run publishes changed Textures, Meshes, Animations, extracted Archive entries, output Archives, and loading plugins from temporary files. The publication sequence is repeated across their producers: stage under Temporary Ownership, write bytes, publish a destination, then release ownership of the temporary path. A future change to one producer can diverge from the others at a crash-sensitive point. Archive extraction already differs by lacking the explicit staged-byte flush used by Asset execution and Archive Finalization.

For a user, the important distinction is whether the destination became a Committed Mutation, even when the later ownership release fails or the Optimization Run is cancelled. Safety Cleanup and recovery must remove only temporarily owned paths, never a committed destination, source Asset, or backup. The current split interface makes those guarantees harder to maintain and verify through one seam.

## Solution

Deepen the existing Temporary Ownership module so a producer writes and validates its staged bytes, then asks the module to publish them with an explicit replace or no-replace rule. Temporary Ownership validates the destination, flushes staged bytes, performs same-volume publication, and releases the durable temporary claim. Its result distinguishes a destination that was not published, one that was published while Temporary Ownership remains, and one that was published and released. Asset and Archive producers use those facts to retain Committed Mutations and classify their own Operation Failures.

The external test seam is Temporary Ownership. Focused producer tests continue to verify Asset and Archive behavior, while the existing Run Executor remains responsible for Run Phase authority, Run Outcome classification, and the Safety Cleanup lifetime.

## User Stories

1. As an Optimization Run user, I want a failed staged publication to leave my original Asset intact when no destination was committed, so that failed work does not masquerade as a completed change.
2. As an Optimization Run user, I want every published destination retained as a Committed Mutation even if ownership release later fails, so that terminal Run Evidence describes durable work accurately.
3. As an Optimization Run user, I want Safety Cleanup to leave committed destinations alone, so that cleanup cannot erase completed work.
4. As an Optimization Run user, I want an interruption before publication to leave only temporarily owned paths for recovery, so that a later run can remove incomplete output safely.
5. As an Optimization Run user, I want an interruption after publication but before ownership release to preserve the destination, so that a later recovery cannot undo a Committed Mutation.
6. As a Dry Run user, I want no staging, publication, or ownership release to occur, so that evaluation does not mutate my Mod Root.
7. As a user cancelling an Optimization Run, I want any in-flight Asset or Archive attempt to finish its atomic publication step, so that cancellation does not leave an ambiguous destination.
8. As a user optimizing a Texture, I want staged bytes flushed before the destination is replaced, so that a successful commit has the existing durability guarantee.
9. As a user converting a Texture, I want source removal to remain after destination publication and ownership release, so that a failed conversion keeps usable source or destination bytes.
10. As a user optimizing a Mesh, I want its replacement to retain committed mutation evidence when ownership release fails, so that the run does not report an unchanged Mesh after replacement.
11. As a user optimizing an Animation, I want its replacement to follow the same staged publication rule as other Assets, so that recovery treats it consistently.
12. As a user extracting an Archive, I want a newly appeared Loose Asset to remain authoritative, so that extraction cannot overwrite it after preflight.
13. As a user extracting an Archive, I want staged bytes explicitly flushed before each no-replace publication, so that Archive output follows the same durability rule as other staged output.
14. As a user reviewing a failed Archive merge, I want its existing `PartialOrUnknown` attempt result preserved, so that the Optimization Run still treats an incomplete merge as unsafe.
15. As a user creating an output Archive, I want an occupied destination preserved, so that finalization never overwrites another file.
16. As a user creating a loading plugin, I want its no-replace publication and exact existing-dummy reuse preserved, so that finalization does not replace a competing plugin.
17. As a user whose Archive was committed before source cleanup failed, I want the committed output retained, so that a cleanup failure cannot hide or roll back completed work.
18. As a user processing Several Mods, I want every staged destination confined to its selected Mod Root, so that work for one mod cannot publish into another.
19. As a user encountering a changed or linked destination parent, I want publication rejected before mutation, so that staged output cannot escape its intended Mod Root.
20. As an Asset producer author, I want Temporary Ownership to report whether publication happened, so that I can classify the Asset's Operation Failure without probing the filesystem after an error.
21. As an Archive producer author, I want to state replace or no-replace intent explicitly, so that Temporary Ownership need not infer game rules from Asset Kind.
22. As an Archive extraction author, I want to resolve game-path casing and pin destination parents before publication, so that generic Temporary Ownership does not absorb Archive Precedence rules.
23. As an Archive extraction author, I want to supply the final case-resolved destination after staging, so that preflight and staging order remain intact.
24. As a maintainer, I want durable staged files publishable only through Temporary Ownership, so that no caller can release their durable claim through generic `commit` after performing a separate move.
25. As a maintainer, I want a one-use staged receipt, so that a failed or successful publication attempt cannot be retried through stale authority.
26. As a maintainer, I want a failed publication attempt to leave its temporary path owned for Safety Cleanup or recovery, so that consuming the receipt never abandons cleanup responsibility.
27. As a maintainer, I want destination containment, ordinary-parent identity, volume, and Asset destination matching rechecked at publication, so that assumptions made at staging remain valid after filesystem changes.
28. As a maintainer, I want the final native no-replace operation to reject an occupied destination, so that a race after preflight does not overwrite it.
29. As a maintainer, I want the destination never added to the temporary ownership record, so that recovery can remove only temporary paths.
30. As a maintainer, I want the ownership module to retain the existing lock through Safety Cleanup, so that another Optimization Run cannot take ownership midway through publication or cleanup.
31. As a maintainer, I want non-durable temporary registrations to keep their existing generic commit path, so that this change stays focused on durable staged files.
32. As a test author, I want to exercise replace and no-replace publication through the Temporary Ownership interface, so that the interface is the test surface for the shared protocol.
33. As a test author, I want a deterministic post-publication release failure, so that I can verify committed bytes and remaining Temporary Ownership without a public fault adapter.
34. As a test author, I want focused Asset and Archive tests to verify mutation evidence and source handling, so that tests do not duplicate native move mechanics in every producer.
35. As a future contributor, I want the staging ownership documentation to cover Texture, Mesh, Animation, Archive extraction, Archive Finalization, and loading plugins, so that the documented crash states match current production paths.
36. As a release maintainer, I want the profile-triplet builds and affected run suites verified, so that this deepening does not weaken supported Windows behavior.

## Implementation Decisions

- Deepen the existing Temporary Ownership module at its current seam. Do not add an external storage or publication adapter merely to route a single implementation.
- All durable staged publication paths are in scope: Texture, Mesh, Animation, Archive extraction, output Archive, and loading plugin. Preserve the existing distinction between replace for Assets and no-replace for Archive work.
- Producers continue to create format-specific bytes and validate them. Archive extraction retains its game-path casing resolution, occupied-leaf checks, and pinned destination parents through publication. Archive Finalization retains output planning and exact existing-dummy reuse. Source and backup handling remains with producers.
- Staging retains its register-before-create ordering, durable ownership record, and Mod Root lock. Dry Run creates no staging. The accepted ownership record continues to contain temporary paths only; no destination or replace policy is added to its durable grammar.
- A durable stage returns a move-only receipt that gives the producer a path to write. The receipt privately retains its owning module and canonical Mod Root. Sibling Asset staging also binds the intended destination, which publication must match. Archive staging permits the final case-resolved destination to be supplied later.
- The receipt has one publication operation that accepts the destination and an explicit closed replace policy. A durable receipt cannot be released through the generic `commit` path. That path remains available for non-durable registrations.
- Publication consumes the receipt on every attempt. A caller cannot retry using the same receipt after success or failure. A failed attempt retains Temporary Ownership of any remaining temporary path for Safety Cleanup or later recovery.
- Immediately before mutation, Temporary Ownership revalidates that the destination is absolute, within the recorded canonical Mod Root, outside reserved staging, on the same physical volume as staged bytes, and reached through an ordinary parent. Sibling Asset publication must match its staged destination. Archive-specific case resolution and parent pins remain outside this generic check.
- Publication explicitly flushes staged bytes for both replace and no-replace paths, including Archive extraction. It uses a same-volume native publication operation with no cross-volume copy fallback. No-replace publication must refuse a destination created after preflight.
- Publication returns an explicit state with error detail: `NotPublished`, `PublishedStillOwned`, or `PublishedAndReleased`. The state becomes published as soon as the native destination publication succeeds, before temporary-name removal or ownership-record release. A failure after that point must not erase the committed fact.
- Temporary Ownership reports publication facts and filesystem errors. Asset and Archive producers remain responsible for Operation Failure, mutation state, and safe-to-continue classification. The Run Executor keeps Run Phase and Run Outcome authority.
- A later Archive merge failure retains its existing attempt-level `PartialOrUnknown` mutation result and unsafe continuation. This work does not add per-entry Archive mutation evidence.
- Successful destination publication precedes durable ownership release; requested source removal and Archive source cleanup follow their existing producer-specific rules. Safety Cleanup and recovery remove only owned temporary paths and never roll back Committed Mutations.
- Preserve support for existing ownership record versions and the current Windows target. Wine runs the same Windows executable; native POSIX behavior remains a compatibility path rather than a new release guarantee.
- Update staging ownership documentation to describe every current durable producer and the new single publication seam. Add concise method documentation and comments for the non-obvious crash, release, and mutation-state ordering.

## Testing Decisions

- A good test crosses a module interface and checks observable destination bytes, surviving source files, ownership state, recovery behavior, and retained mutation evidence. It does not assert private publication helpers, manifest cursors, or which native call a module chose.
- Use Temporary Ownership as the highest new test seam. Exercise both replace and no-replace publication with real temporary Mod Roots and the local filesystem; no new public fault adapter is required.
- Verify the three publication outcomes. For `NotPublished`, use an occupied no-replace destination or an unavailable staged file and assert the destination is preserved and temporary ownership remains recoverable. For `PublishedAndReleased`, assert destination bytes survive Safety Cleanup and the temporary path is released. For `PublishedStillOwned`, create a conflicting ownership scratch file after staging so the destination publishes but the durable release fails; assert the committed destination survives Safety Cleanup and later recovery.
- Verify receipt one-use behavior, rejection of generic commit for durable staging, rejection of a changed Asset destination, Mod Root escape, reserved staging target, changed or linked parent, and unsupported cross-volume publication where a suitable volume fixture exists.
- Add a focused Archive publication crash-window test that forces ownership release to fail after destination publication with an occupied ownership scratch name, terminates the producer, then verifies recovery retains committed bytes. This exercises the shared flush-and-publish path without a timing-sensitive kill or a claim to simulate physical power loss.
- Retain and adapt the existing durable staging tests for abandoned output, rename-before-deregistration recovery, active lock ownership, corrupt ownership, and cleanup of only registered paths. Tests that commit durable staging through the generic path migrate to the new publication interface.
- Retain focused Asset execution tests for Texture crash windows, Mesh and Animation commit failures, source-removal behavior, and correct Committed Mutation evidence when release fails. Replace duplicated assertions about low-level publication mechanics with Temporary Ownership tests.
- Retain Archive extraction tests for no-replace behavior, case-correct target preparation, linked-parent rejection, and partial-merge evidence. Main Optimizer tests provide prior art for Archive Finalization output-name collisions, loading-plugin publication, and source retention; AssetRun tests cover its cancellation and evidence handoff.
- Keep Optimization Run and Run Evidence tests for terminal mutation retention, cancellation, Safety Cleanup, and Run Outcome precedence. Their expectations must remain behaviorally unchanged.
- Validate the change through the project's profile-triplet build workflow and all affected durable staging, Asset execution, Archive extraction, Archive Finalization, Run Executor, GUI, and CLI test suites. An alternate generator does not replace profile-triplet validation.

## Out of Scope

- Changing Routing Policy, Archive Precedence, Archive Collisions, capacity planning, Effective Asset Tree discovery, or Archive Finalization output planning.
- Moving format-specific Asset or Archive validation, game-path casing resolution, source deletion, backup retention, or failure classification into Temporary Ownership.
- Changing Run Phase authority, Run Outcome categories or precedence, Run Event ordering, Run Evidence ownership, scheduling, or Run Handle lifetime.
- Adding per-entry Archive extraction mutation evidence or changing the existing coarse result after a partial merge.
- Adding rollback of Committed Mutations, retrying a consumed receipt, or cleaning destinations through temporary ownership.
- Replacing the durable ownership record grammar, registering destinations or backups in it, or introducing remote or database-backed ownership storage.
- Expanding native POSIX into a supported release target or adding a new external publication adapter solely for testing.
- Redesigning non-durable temporary registration, profile-backed planning, GUI rendering, or CLI rendering.

## Further Notes

- `CONTEXT.md` defines Temporary Ownership, Committed Mutation, Safety Cleanup, Operation Failure, Mod Root, and Archive Finalization for this work. The staging ownership architecture note records the existing crash and recovery states; its producer list needs updating during implementation.
- The existing scheduling and lifetime ADR remains in force: the Optimization Run Service owns scheduling and lifetime, and the Run Executor remains the deepest synchronous deterministic seam. This spec changes neither decision.
- The current implementation explicitly flushes Asset and Archive Finalization staging before publication, while Archive extraction currently relies on a write-through no-replace move. The agreed shared rule adds an explicit flush to Archive extraction.
- Ownership-release failure can be exercised without a new seam by occupying the ownership scratch name after staging. The destination may then publish while the durable temporary claim remains, which is precisely the `PublishedStillOwned` state.
