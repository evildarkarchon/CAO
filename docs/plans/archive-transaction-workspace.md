# Durable Archive Transaction workspace implementation plan

## Outcome

Deepen archive workspace ownership so every Archive Transaction has one durable consistency mechanism for ordinary failures and process or machine crashes. Production callers enter through one Asset Work Plan Execution interface; they cannot plan Asset Work Items before Archive Recovery or release Mod locks between recovery and execution.

The implementation must preserve the existing deep `BSAOptimizer` operations and the three-operation `AssetWorkPlanExecutionAdapter` seam. Internal seams exist only where production and deterministic adapters are both required.

## Required invariants

1. Selected Mods are identified before locking; no Asset Work Items are discovered before recovery.
2. All selected Mod locks are acquired in deterministic, case-insensitive canonical-path order or none remain held.
3. Lock ownership is a held kernel file lock, never pathname existence or PID liveness.
4. The locked sibling file durably records workspace bootstrap intent before the reserved root is created.
5. Every journal intent is framed, sequenced, checksummed, and durably flushed before its filesystem mutation.
6. Before commit, rollback recreates the exact pre-transaction Mod state.
7. After commit, recovery recreates the exact committed state and completes only deferred deletion and workspace cleanup.
8. Recovery is idempotent and cannot be interrupted by cancellation.
9. Every live, staged, and rollback path is validated by filesystem identity and confined to the owning Mod or workspace before the first mutation.
10. `.cao-transactions` is never treated as an Asset, plugin source, archive input, Mod, or generic cleanup target.
11. Dry Run validates and reports recovery requirements without changing Assets or deleting empty directories.
12. Unsupported filesystems and ambiguous histories fail before mutation.

## Target module shape

### External seam

`AssetWorkPlanExecution` is the production composition module. Its small interface accepts validated run input and execution callbacks, then owns this ordering:

```text
identify selected Mods
  → classify legacy sibling obstructions
  → acquire all Mod locks
  → discover and perform Archive Recovery
  → plan Asset Work Items
  → execute through the existing three-operation adapter
  → release all locks
```

`Manager` remains the presentation adapter for progress, structured notices, cancellation, and typed archive errors. `AssetWorkPlanExecutor` remains the generic coordinator behind the external seam.

### Internal modules and seams

- `ArchiveTransactionWorkspace`: owns manifest creation, journal append/replay, commit, rollback, committed cleanup, reserved-root cleanup, and workspace identity.
- `ArchiveRecovery`: discovers owned in-scope workspaces and invokes the same replay used for immediate rollback.
- `ModExecutionLock`: acquires kernel-held sibling lock files, writes bootstrap intent, exposes owner diagnostics, and releases through RAII.
- `AssetWorkScope`: resolves Single Mod or Several Mods plus Ignored Mods once, so locking, recovery, and planning cannot drift.
- Durable Windows filesystem adapter: performs native no-overwrite moves, volume and file-identity queries, framed writes, `FlushFileBuffers`, handle-based confinement checks, and filesystem capability preflight.
- Deterministic filesystem and lock adapters: inject failure or simulated process termination after every durable transition.

The workspace, recovery, lock, and durability seams remain private to the production composition and BSA implementation. Tests use their deterministic adapters without enlarging the external execution interface.

## Incremental commit sequence

### 0. Protect the current worktree

- Preserve the existing `src/CMakeLists.txt` and `src/pch.h` filename-casing fixes.
- Treat the existing `AssetWorkPlan.cpp` workspace-name helper and its test as a temporary user change to be superseded only in the final cutover.
- Capture the baseline CTest result before implementation.

Exit check: no unrelated diff is reformatted, reverted, or folded into the architecture work.

### 1. Resolve one shared Asset Work scope

Files:

- Add `src/AssetWorkScope.{h,cpp}`.
- Refactor `src/AssetWorkPlan.{h,cpp}` to consume an already-resolved selected-Mod list when discovering Archive and Loose Asset Work Items.
- Add `tests/AssetWorkScope.Tests.cpp`; adjust `tests/AssetWorkPlan.Tests.cpp` without changing planning behavior.

Behavior:

- Single Mod selects exactly the requested Mod.
- Several Mods selects immediate Mod directories, excluding separators and case-insensitive Ignored Mods.
- Workspace naming is not part of selection.

Exit check: scope tests and existing Asset Work Plan tests pass; the new production composition can obtain the exact lock/recovery scope without discovering Asset Work Items.

### 2. Add kernel-held Mod execution locks and bootstrap records

Files:

- Add `src/ModExecutionLock.{h,cpp}` and `tests/ModExecutionLock.Tests.cpp`.
- Register sources in `src/CMakeLists.txt` and `tests/CMakeLists.txt` while preserving current casing edits.

Behavior:

- Use a stable sibling lock file keyed to the canonical Mod path.
- Hold an exclusive kernel lock for the execution lifetime; file contents are diagnostics, not lock truth.
- Acquire multiple locks in deterministic canonical-path order and release all on partial acquisition failure.
- Write and flush transaction ID, Mod identity, and intended workspace path before creating `.cao-transactions`.
- Reacquisition after a crashed owner validates the bootstrap record; it never breaks a live lock by PID inspection.

Exit check: two-process contention, stale file contents, different-Mod concurrency, partial acquisition failure, and bootstrap-write failure are deterministic tests.

### 3. Add durable manifest and framed journal storage

Files:

- Add `src/ArchiveTransactionWorkspace.{h,cpp}` and `tests/ArchiveTransactionWorkspace.Tests.cpp`.
- Add private production and deterministic durability adapters with the workspace module.

Behavior:

- Manifest records schema version, transaction ID and kind, canonical Mod and anchor identities, volume identity, workspace identity, and policy facts needed for recovery.
- Journal frames remain human-readable but include sequence, length, and checksum.
- Only a torn final frame may be ignored. Earlier corruption, sequence gaps, duplicate or early commit, unknown versions or records, and impossible transitions fail before mutation.
- Replay distinguishes crash-before-move from crash-after-move by expected source and destination identities.
- Bootstrap recovery safely removes or completes a root created before its internal manifest became durable.

Exit check: parser and bootstrap tests prove that corrupt histories cause zero live mutations.

### 4. Implement path confinement and filesystem qualification

Files:

- Concentrate handle-based path and volume rules inside the durable filesystem implementation.
- Reuse those rules from `ArchiveTransactionWorkspace` and Archive Recovery rather than adding another public path interface.

Behavior:

- Reject absolute, UNC, device, alternate-data-stream, and escaping relative journal paths.
- Reject workspace or root reparse points and validate nonexistent destinations through the nearest existing ancestor handle.
- Detect source replacement through volume and file identity; do not trust lexical lowercase paths.
- Use native no-overwrite same-volume moves; do not use a Qt rename path that can fall back to copy-and-delete.
- Preflight durable flush and atomic-move requirements before the first transaction mutation.

Exit check: confinement tables cover both separators, case variants, junctions, symbolic links, hard-link or file-identity changes, missing ancestors, cross-volume moves, and overwrite attempts.

### 5. Replace extraction’s in-memory journal

Files:

- Refactor `src/BsaOptimizer.cpp` behind its existing `extract` interface.
- Extend `tests/BsaOptimizer.Tests.cpp`; keep engine and filesystem adapters internal.

Behavior:

- Create and flush the workspace before engine or live-file work.
- Flush staged output bytes before publication.
- Move the source archive or existing live output into rollback storage only after its intent is durable.
- Publish with journaled native moves.
- For retained backups, journal the final unique backup as a committed Asset. For delete-backup policy, retain the source only in rollback storage until commit.
- Ordinary failure invokes the same replay path as restart recovery.

Exit check: existing collision and rollback behavior survives; fault injection after every extraction transition yields exact pre-state or exact committed state.

### 6. Replace packing’s in-memory journal

Files:

- Refactor `src/BsaOptimizer.cpp` behind its existing `packAll` interface.
- Update `src/ArchiveEngine.cpp` so bethutil rejects any path with the reserved-root path component.
- Extend `tests/BsaOptimizer.Tests.cpp` and archive engine tests.

Behavior:

- One packing workspace owns the complete archive output set for one Mod.
- Existing destination archives and dummy plugins move to rollback storage before replacement.
- Loose source Assets selected for delete-source policy move to rollback storage before commit; they are never irreversibly deleted before commit.
- Rollback-only copies are removed after commit. Intentionally retained extraction backups are not cleanup material.
- `.cao-transactions` cannot enter bethutil traversal, manifests, staged output, or the packed archive set.

Exit check: multi-output commit, dummy-plugin nonreplacement, retained-backup collisions, source preservation, rollback order, and cleanup recovery all pass the crash matrix.

### 7. Add scoped Archive Recovery

Files:

- Add `src/ArchiveRecovery.{h,cpp}` and `tests/ArchiveRecovery.Tests.cpp`.
- Extend `src/ArchiveExecutionError.h` with typed recovery operation/stage data and structured workspace diagnostics.

Behavior:

- Scan only selected, locked Mods.
- Before lock acquisition, classify exact legacy sibling workspace patterns in the candidate Mod set: in-scope paths block for manual resolution, while Ignored or unrelated paths remain untouched and are omitted from the final planning scope with a diagnostic. This recognition remains private to Archive Recovery.
- Finish any number of committed cleanup-only workspaces deterministically.
- Roll back at most one incomplete workspace per Mod; multiple incomplete histories block without mutation.
- Legacy unjournaled roots, unknown versions, ownership mismatch, and corrupted histories block with exact manual-resolution paths.
- Any in-scope failure blocks the whole execution.
- Cancellation is latched during recovery and returned only after consistency is restored.

Exit check: recovery is idempotent, ignored and unrelated Mods remain untouched, and a second recovery pass is a no-op.

### 8. Add the recovery-first production composition

Files:

- Add `src/AssetWorkPlanExecution.{h,cpp}` and `tests/AssetWorkPlanExecution.Tests.cpp`.
- Move the anonymous production execution composition out of `src/Manager.cpp`.
- Add `ArchiveRecovery` to `AssetWorkPlanExecutionPhase` and update Manager’s progress presentation.

Behavior:

- Expose one high-level execution operation.
- Resolve scope, acquire all locks, recover, plan, execute, and release through one lifetime.
- Dry Run acquires locks and validates journals. Recovery-required state raises the typed recovery diagnostic before Asset Work Items are planned.
- With clean recovery state, Dry Run continues normal planning but performs no filesystem cleanup.
- Recovery progress reports workspace count, rollback versus committed cleanup, and outcome without exposing low-level journal facts as caller knowledge.

Exit check: composition tests cover success, lock contention, recovery failure, recovery-required Dry Run, cancellation during recovery, exception-safe lock release, and exactly-once notices.

### 9. Exclude the reserved root from every discovery path

Files:

- Update `src/AssetWorkPlan.cpp`, `src/ArchiveEngine.cpp`, `src/ModAssetMetadata.cpp`, and their tests.
- Remove or scope `FilesystemOperations::deleteEmptyDirectories` from `src/AssetWorkPlanExecutor.cpp`.

Behavior:

- Archive discovery, Loose Asset Discovery, plugin discovery, bethutil packing, and generic empty-directory cleanup never traverse `.cao-transactions`.
- Empty-directory cleanup is limited to selected Mods and disabled entirely during Dry Run, or is removed if it provides insufficient leverage.
- Cleanup failure for the reserved root is a recoverable committed state and prevents later planning until resolved.

Exit check: ignored Mods and Dry Run show byte-for-byte unchanged directory trees; reserved-root fixtures never produce Asset Work Items or metadata reads.

### 10. Remove the planner workaround and replace shallow tests

Files:

- Remove `isCaoArchiveStagingDirectory` and its private-name parsing from `src/AssetWorkPlan.cpp`.
- Replace the current dirty planner staging-name test with Archive Recovery ownership, legacy, and scope tests.
- Remove obsolete extraction and packing rollback bookkeeping and tests that inspect past the new interface.

Behavior:

- The planner knows only selected Mods and domain Asset rules.
- Legacy sibling directories are explicit recovery/manual-resolution diagnostics, never a planner naming convention.

Exit check: searching planning code for `.cao-pack`, `.cao-extract`, workspace manifests, or journal records returns no matches.

### 11. Prove durability and finish the cutover

- Run the deterministic fault matrix after every manifest write, journal frame, flush, directory creation, staged-output flush, source or backup move, publish move, commit flush, rollback action, cleanup deletion, and reserved-root removal.
- Reopen each scenario through a fresh workspace instance and assert byte-for-byte pre-state without commit or byte-for-byte committed state with commit.
- Run a smaller real NTFS suite using separate processes for kernel lock contention and representative process-kill/restart transitions.
- Run the full canonical CTest suite.
- Verify production code has no fallback to copy-and-delete, best-effort journal flush, PID-only lock breaking, or planner knowledge of workspace names.

Exit check: every crash point produces one of two states—exact pre-transaction or exact committed—with no mixed state.

## Acceptance matrix

| Area | Required proof |
|---|---|
| Journal | Torn final frame accepted; earlier corruption and invalid transitions mutate nothing |
| Bootstrap | Crash before root creation, after root creation, and before manifest flush is automatically recoverable |
| Locking | Same Mod excludes; different Mods coexist; stale contents do not imply a live owner |
| Scope | Single/Several Mods, Ignored Mods, separators, and unrelated Mods remain correct |
| Recovery | One incomplete rolls back; many committed clean up; many incomplete block |
| Commit | No commit restores exact pre-state; commit completes exact post-state cleanup |
| Confinement | Reparse points, identity swaps, path escapes, and cross-volume moves fail before mutation |
| Reserved root | Never planned, parsed as metadata, packed, or removed by generic cleanup |
| Dry Run | Locks and validates; recovery-required blocks; no filesystem mutation occurs |
| Cancellation | Recovery completes consistency work before returning `Cancelled` |
| Reporting | Recovery phase and typed failures are delivered exactly once |
| Compatibility | BSA Optimizer and three-operation execution interfaces remain the test surfaces |

## Verification commands

Use the repository’s configured Windows preset and canonical CTest entry point:

```powershell
cmake --preset ninja-windows
cmake --build build
ctest --test-dir build --output-on-failure
```

Run the real NTFS process-kill suite separately if it is intentionally excluded from the fast characterization set; document that split in CTest rather than hiding it behind an ad hoc command.
