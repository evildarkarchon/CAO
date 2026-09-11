# Deepen Optimization Run Evidence

Status: ready-for-agent

## Problem Statement

Optimization Run facts are retained, published, interpreted, copied, and exposed by several shallow modules. Work attempts and diagnostics live in a mutable record; Run Phases live in separate executor storage; preparation and cleanup failures use additional containers; publication bookkeeping leaks through the observation interface; and terminal construction performs outcome classification while copying these sources together. GUI and CLI adapters, tests, and future maintainers must understand this distributed implementation to use or verify the result correctly.

This weakens locality and makes the current interface a poor test surface. Tests can construct impossible states by editing public containers, a throwing observer can only be understood by tracing multiple modules, and changes to Run Evidence or Run Outcome precedence require coordinated edits across the Run Executor, AssetRun, terminal result construction, and presentation adapters.

## Solution

Create one concrete, deep Run Evidence module as the sole owner of factual Optimization Run state from Preparing through Safety Cleanup. The module retains facts before publication, enforces structural evidence invariants, and is confined to the synchronous Run Worker while mutable. After Safety Cleanup, it is consumed into an immutable Run Evidence value. The Run Executor classifies the Run Outcome from that sealed value and constructs one terminal result shared by the Run Handle, terminal Run Event, and waiting caller.

Preserve the existing Optimization Run behavior, Run Event ordering, cancellation checkpoints, and Run Outcome precedence. Replace the shallow mutable `work()` surface with focused read-only result views that expose all currently available evidence without exposing storage or publication state.

## User Stories

1. As a GUI user, I want terminal run details to remain complete after failures or cancellation, so that I can understand which mutations were committed.
2. As a CLI user, I want the same terminal evidence as the GUI, so that presentation choice does not change what the Optimization Run records.
3. As a user cancelling an Optimization Run, I want completed attempts retained before cancellation takes effect, so that durable work is never hidden.
4. As a user whose run reaches Safety Cleanup, I want every cleanup failure retained in attempted order, so that I can inspect remaining temporary artifacts safely.
5. As a user whose observer fails, I want the Optimization Run to retain an informational Run Diagnostic and continue, so that presentation failure cannot change work or Run Outcome.
6. As a user reviewing an Archive Collision, I want the winning and shadowed Archives preserved in terminal Run Evidence, so that Archive Precedence remains explainable.
7. As a user reviewing a failed attempt, I want its Operation Failure attached to the exact Asset or Archive attempt, so that mutation state and continuation safety remain trustworthy.
8. As a user reviewing a preparation failure, I want a Run Failure without a misleading partial preparation, so that unresolved Mod Roots or policy conflicts are not presented as successful facts.
9. As an observer author, I want every live fact retained before my callback runs, so that an exception in my code cannot erase completed work.
10. As an observer author, I want a published fact delivered at most once through a publication path, so that retries cannot duplicate visible history.
11. As an observer author, I want ObserverFailed diagnostics withheld from the same failing callback path, so that failure reporting cannot recurse.
12. As a presentation adapter author, I want Run Events limited to Run Phase transitions, Run Diagnostics, Run Failures, and the terminal result, so that this refactor does not expand the live event contract.
13. As a presentation adapter author, I want focused read-only terminal views, so that I can render complete evidence without learning its storage structure.
14. As a Run Handle caller, I want the terminal event and waiting result to share one immutable terminal result, so that all observation paths agree exactly.
15. As a maintainer, I want one module to own Run Evidence mutation, so that evidence rules have locality.
16. As a maintainer, I want the Run Executor alone to advance Run Phases, so that lifecycle authority matches the domain model and ADR-0001.
17. As a maintainer, I want lower modules to report work facts and progress without independently defining lifecycle position, so that phase changes have one implementation location.
18. As a maintainer, I want Run Outcome classification to remain separate from Run Evidence retention, so that facts and judgment cannot silently influence each other.
19. As a maintainer, I want terminal construction to be passive, so that outcome precedence is visible in the Run Executor rather than hidden in a result factory.
20. As a maintainer, I want successful Preparing facts retained atomically, so that partially resolved preparation cannot masquerade as a valid run-scoped configuration.
21. As a maintainer, I want mutation summaries and aggregate counts derived from authoritative facts, so that duplicate stored values cannot drift.
22. As a maintainer, I want evidence invariants enforced behind the module interface, so that invalid phase, progress, attempt, or cleanup ordering fails close to its source.
23. As a maintainer, I want mutable evidence confined to the Run Worker, so that evidence ownership does not require additional synchronization.
24. As a maintainer, I want post-cleanup immutability expressed by ownership, so that no producer can append facts after terminal evidence is created.
25. As a test author, I want the Run Evidence interface to be the test surface for retention and publication, so that tests cannot fabricate impossible states through mutable containers.
26. As a test author, I want lifecycle and outcome tests to cross the Run Executor seam, so that tests verify the highest deterministic behavior rather than helper implementation.
27. As a test author, I want AssetRun tests focused on Archive and Asset work semantics, so that they do not duplicate Run Phase authority or publication bookkeeping.
28. As a test author, I want GUI and CLI integration tests to retain their Run Event coverage, so that adapter-visible behavior remains stable.
29. As a future contributor, I want Run Failure, Operation Failure, and Safety Cleanup failure to remain distinct, so that each failure keeps its domain meaning.
30. As a future contributor, I want the completed migration to remove obsolete record and publication interfaces, so that there is one obvious path through the architecture.
31. As an AI coding agent, I want evidence ownership, lifecycle authority, and terminal consumption concentrated behind a small interface, so that changes require less cross-file inference.
32. As a release maintainer, I want the profile-triplet builds and existing run suites to remain green after each migration slice, so that architecture work does not weaken supported build validation.

## Implementation Decisions

- Build one concrete Run Evidence module. Do not introduce an abstract storage seam because only one storage implementation exists.
- Keep the existing production and test observation adapters at the real publication seam.
- Make the Run Evidence module the sole mutation authority. Preparation, AssetRun, the Run Executor, and Safety Cleanup submit typed facts instead of editing storage.
- Confine mutable Run Evidence to the synchronous Run Worker. The Optimization Run Service continues to own synchronization for public snapshots and Run Event dispatch.
- Include successful Preparing facts, the latest record for each traversed Run Phase, Run Diagnostics, Run Failures, Operation Failures within completed attempts, Routing Ledger state, Archive Collisions, recognized exclusions, completed Archive and Asset attempts, Archive Finalization evidence, cancellation observation, and Safety Cleanup failures.
- Treat successful preparation as atomic. Resolved Mod Roots, Routing Policy, configuration, and Archive Precedence become available only after all Preparing work succeeds.
- Retain one latest record per Run Phase while preserving first-traversal order. Publish each accepted phase transition or progress update as it occurs.
- Keep live Run Event payloads limited to Run Phase transitions, Run Diagnostics, Run Failures, and the terminal result. Other Run Evidence remains available through the terminal result.
- Retain a fact before publication and advance publication state before calling an observation adapter.
- Use at-most-once publication through a given path. A publication exception retains an ObserverFailed Run Diagnostic and never retries the offending fact or recursively publishes the diagnostic to the same failing path.
- Remove retained-versus-new publication methods from the observation interface. That distinction is private Run Evidence state.
- Preserve separate meanings for Run Failure, Operation Failure, and Safety Cleanup failure. Do not duplicate the same failure in work and terminal storage.
- Make the Run Executor the only module that advances Run Phases and selects the final work phase. Lower modules provide work facts and progress to the enclosing execution context.
- Enforce structural evidence invariants behind the interface, including canonical phase order, monotonic progress, one successful preparation, attempts only during their applicable work phase, and no mutation after Safety Cleanup.
- Treat violations of evidence invariants as programming defects, not user-facing Run Failures.
- Perform Safety Cleanup before sealing Run Evidence.
- Consume mutable Run Evidence into a distinct immutable value. Do not leave a mutable object with a terminal flag and callable mutators.
- Calculate mutation summaries and aggregate skip counts from authoritative facts while creating the immutable value. Do not retain independent mutable copies of derived information.
- Classify Run Outcome in the Run Executor from the immutable Run Evidence value.
- Preserve the existing precedence among fatal or unsafe work, observed cancellation, safely contained Operation Failures, and Safety Cleanup failures.
- Make terminal result construction passive: it combines the Run Executor's chosen Run Outcome, Run ID, final phase, and immutable Run Evidence without reclassifying them.
- Construct one terminal result and share that same immutable result with the Run Handle, terminal Run Event, and synchronous waiting path.
- Replace the monolithic mutable `work()` exposure with focused read-only terminal views for every evidence category currently visible to GUI, CLI, and tests.
- Do not leave a compatibility shim for the old mutable record in the completed change. The run libraries are repository-internal and are not installed or exported for external consumers.
- Implement in behavior-preserving slices: introduce the concrete mutable and immutable evidence modules; route facts and publication through them; move classification and add focused result views; migrate adapters and tests; then delete the obsolete work record, observation recorder, and retained-publication distinction.
- Preserve ADR-0001: the Optimization Run Service owns scheduling and lifetime, while the Run Executor remains the deepest synchronous deterministic seam and owns terminal classification.
- Add concise documentation for every added or substantially rewritten method and comments for non-obvious publication, ordering, cancellation, and ownership constraints.

## Testing Decisions

- Good tests cross a module interface and assert observable behavior. They do not inspect mutable storage, publication cursors, helper calls, or private sequencing machinery.
- Add focused Run Evidence tests for successful fact retention, phase replacement with preserved traversal order, monotonic progress, atomic preparation, attempt ordering, cancellation observation, failure-category separation, and rejection of post-cleanup mutation.
- Add focused Run Evidence tests proving retain-before-publish and at-most-once publication when an observation adapter throws.
- Verify that ObserverFailed is retained exactly once and is not recursively delivered through the failing publication path.
- Verify that consuming mutable Run Evidence produces a self-contained immutable value that survives destruction of the mutable module and every producer.
- Verify derived mutation summaries and skip counts against mixed successful, contained-failure, unsafe, cancelled, and cleanup-failure evidence.
- Keep Run Outcome precedence, canonical Run Phase traversal, no-work phase handling, preparation failure, cancellation checkpoints, mandatory Safety Cleanup, and terminal result identity tests at the Run Executor interface.
- Keep Archive selection, Effective Asset Tree, Routing Ledger, Archive and Asset attempt ordering, mutation safety, and Archive Finalization behavior at the AssetRun interface.
- Retain GUI and CLI integration coverage for ordered Run Events, terminal detail rendering, and shared terminal-result identity.
- Migrate existing observer-throwing and cancellation tests from direct work-record assertions to the new Run Evidence and Run Executor interfaces.
- Remove or rewrite tests that fabricate impossible states by mutating work-record fields directly. Preserve the behavioral scenario at the highest applicable seam.
- Use existing Run Executor, AssetRun, Optimization Run Service, Run Event delivery, GUI Run, and CLI Run tests as prior art for fixtures and observable assertions.
- Keep focused invariant tests small; avoid repeating scheduling scenarios in evidence tests or work semantics in lifecycle tests.
- Validate every completed migration slice through the project's configured profile triplets. An alternate generator does not replace profile-triplet validation.
- Run the full affected GUI and CLI test suites before completion and update the project knowledge graph after implementation changes.

## Out of Scope

- Changing Run Outcome categories or their precedence.
- Changing the public Run Event payload categories, ordering, or dispatch semantics.
- Changing cancellation checkpoints or interrupting in-flight Asset or Archive operations.
- Changing Archive Precedence, Archive Collision rules, Effective Asset Tree discovery, Routing Policy, or Routing Ledger semantics.
- Deepening the broader AssetRun operation-adapter interface beyond what is required to route evidence and phase authority correctly.
- Deepening application Optimization Run intake, profile-backed planning, Archive discovery, or temporary ownership as separate architecture efforts.
- Changing the Optimization Run Service's scheduling, single-active-run, Run Handle destruction, or worker-lifetime contracts.
- Adding persistent, remote, database-backed, or interchangeable Run Evidence storage.
- Adding new user-visible diagnostics or failure categories for internal invariant violations.
- Preserving a compatibility shim for the old mutable work-record interface after migration.

## Further Notes

- `CONTEXT.md` defines Run Evidence, Run Failure, and Run Event for this work. The implementation must use those terms consistently.
- Run Evidence is factual; Run Outcome is the Run Executor's terminal judgment.
- Run Event history is a selected live publication view and is not the complete Run Evidence record.
- One adapter means a hypothetical seam and two adapters mean a real one. Evidence storage has one implementation; observation has production and test adapters.
- The deletion test supports deepening rather than removing evidence retention: deleting centralized retain-before-publish behavior would spread ordering, replay prevention, and exception isolation back across AssetRun and the Run Executor.
- This spec reinforces ADR-0001 and does not require a new ADR.
