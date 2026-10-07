# C++ test coverage for scenario porting

Research for [#465](https://github.com/evildarkarchon/CAO/issues/465) on the map
[#458 Port CAO to Rust and Slint](https://github.com/evildarkarchon/CAO/issues/458).

**Question.** Do CAO's C++ tests cover enough behaviour to be worth porting as Rust test
scenarios, in addition to the differential corpus?

**Answer.** Yes, selectively. The C++ suites cover the run-safety contract thoroughly. That
contract covers Run Outcome precedence, Safety Cleanup, Temporary Ownership and crash recovery,
staged publication, Archive Precedence, cancellation, and Committed Mutation retention. Most of
those scenarios need fault injection, a cancellation checkpoint, concurrency, or a mid-run
filesystem change, and a black-box differential corpus can't reach any of those deterministically.
Port those scenarios as Rust tests. Turn the static scenarios (a fixture tree goes in, outcome
and evidence come out) into hand-written corpus seed fixtures. Drop the CLI-, plog- and Qt-dispatch-bound suites.

The C++ suites barely touch engine output: texture format and compression decisions, mesh
optimization, `hkxcmd` animation conversion, and the content of created BSAs. For those, the
differential corpus is the only oracle.

All sources are local and at `master` 8767f86. Test counts are counts of QtTest slot functions.
A `_data` row table counts as one scenario.

## How the suites are built

- 21 suites in `tests/*.cpp`, one executable each (`tests/CMakeLists.txt`). There are 404 test
  slot functions, not counting `initTestCase`/`cleanupTestCase`. 80 of them are data-driven,
  with about 237 `QTest::newRow` rows, and some rows are generated in loops. That is consistent
  with the ticket's estimate of about 450 cases. The tests total 17,771 lines, against 10,833
  lines in `src/**/*.cpp`.
- The tests are white-box at internal seams: `RunExecutor` with injected `RunServices`
  (`RunWorkService`, `SafetyCleanupService`, `RunConfigurationProvider`, `RunObservationSink`);
  `AssetRun` with `AssetRunAdapters` and test-owned `RunWorkMilestones`
  (`tests/AssetRunTests.cpp:183-245`); `ArchiveFirstAssetDiscovery` with an injected free-space
  query and a preflight hook (`tests/ArchiveFirstAssetDiscoveryTests.cpp:384-387, 669-680`);
  `AssetExecutor` with a `RecordingBackend` whose load/save/remove are `std::function` fault
  points (`tests/AssetExecutionTests.cpp:71-216`); and `TemporaryArtifactRegistry` and the
  publication receipt directly (`tests/DurableStagingTests.cpp`).
- The fixtures are real filesystem trees in `QTemporaryDir`. They include real SSE BSAs written
  through `btu::bsa` (`tests/AssetRunTests.cpp:161-185`), real DDS written with DirectXTex, and
  real NIFs built with nifly (`tests/MainOptimizerTests.cpp:118-199`,
  `tests/MeshReferenceMaintenanceTests.cpp:29-37`). Most "texture" payloads are placeholder bytes
  (`"fixture"`), because the engine work is behind adapters.
- Several suites resolve `profiles/` from the process working directory (34 uses of
  `ScopedCurrentDirectory`/`QDir::setCurrent`, for example
  `tests/ApplicationRunSetupTests.cpp:76-104`).

## Coverage measurement: not measured

Line and branch coverage was not measured. The reasons:

- The only configured build tree is `D:\repos\CAO\build`, an MSVC Debug Ninja build
  (`CMAKE_GENERATOR=Ninja`, MSVC 14.51 `cl.exe`, `CAO_BUILD_GUI=ON`). It holds 19 test
  executables with PDBs. It is stale: the executables are from 2026-09-23, HEAD is from
  2026-10-06, and `cao_archive_finalization_tests.exe` was never built there.
- OpenCppCoverage is not installed, and installing software was out of bounds for this ticket.
  It is the one tool that would work against this tree without a rebuild.
- `llvm-cov` is installed (`C:\Program Files\LLVM\bin`), but it needs clang
  `-fprofile-instr-generate` instrumentation. Getting that means a clang-cl reconfigure and a full
  rebuild of the vcpkg/Qt dependency tree, which is well over the budget of about 15 minutes.
- The `ninja-linux` preset's `--coverage` flag (`CMakePresets.json:78-82`) is a GCC/Linux
  configuration. Windows is the target, and the suites `QSKIP` Windows-only behaviour elsewhere.

The cheapest follow-up, if numbers are ever wanted: install OpenCppCoverage, rebuild the
existing tree at HEAD, and run
`OpenCppCoverage --sources src --export_type cobertura -- build\tests\<suite>.exe` for each
suite. The recommendation below doesn't depend on those numbers. The deciding question is
whether the corpus can *reach* a scenario, and coverage percentages don't measure that.

The assessment is static. Each test body was bucketed with a keyword heuristic, and the
buckets were then hand-checked:

- **concurrency**: threads, futures, async, latches.
- **cancellation**: `stop_source`, `request_stop`, cancel.
- **injection**: `throw`, Fake/Failing/Throwing/Recording/Controlled/Gated adapters, injected
  lambdas.
- **static**: none of the above.

| Suite | Tests | Static | Injection | Cancellation | Concurrency |
| --- | ---: | ---: | ---: | ---: | ---: |
| ApplicationLoggingTests | 3 | 3 | 0 | 0 | 0 |
| ApplicationRunSetupTests | 10 | 8 | 0 | 0 | 2 |
| ArchiveFinalizationTests | 42 | 16 | 19 | 6 | 1 |
| ArchiveFirstAssetDiscoveryTests | 49 | 12 | 33 | 4 | 0 |
| AssetExecutionTests | 31 | 1 | 30 | 0 | 0 |
| AssetRoutingTests | 16 | 16 | 0 | 0 | 0 |
| AssetRunTests | 27 | 1 | 9 | 17 | 0 |
| CliConsoleInterruptTests | 1 | 1 | 0 | 0 | 0 |
| CliExecutionTests | 3 | 3 | 0 | 0 | 0 |
| CliRunTests | 6 | 0 | 0 | 4 | 2 |
| DurableStagingTests | 35 | 34 | 0 | 1 | 0 |
| GuiRunDispatchTests | 5 | 0 | 2 | 1 | 2 |
| GuiRunTests | 6 | 2 | 0 | 4 | 0 |
| MainOptimizerTests | 20 | 18 | 0 | 2 | 0 |
| MeshReferenceMaintenanceTests | 3 | 3 | 0 | 0 | 0 |
| OptimizationRunServiceTests | 51 | 17 | 18 | 12 | 4 |
| OptionsCAOTests | 6 | 6 | 0 | 0 | 0 |
| RunEventDeliveryTests | 9 | 0 | 5 | 1 | 3 |
| RunEvidenceTests | 16 | 12 | 1 | 3 | 0 |
| RunExecutorTests | 59 | 7 | 36 | 16 | 0 |
| RunSetupTests | 6 | 6 | 0 | 0 | 0 |
| **Total** | **404** | **166** | **153** | **71** | **14** |

The heuristic over-counts injection in `ArchiveFirstAssetDiscoveryTests`, because discovery is
always driven through an extractor callback. It also under-counts unreachability in
`DurableStagingTests`: those tests are static, but they call the publication-receipt API
directly. The hand-checked reachability figures are in the next section.

## Coverage by parity invariant

Levels: **well** means at least 3 independent scenarios assert outcome, evidence, or filesystem
state for the invariant. **Thin** means 1–2 scenarios, or only one profile or variant.
**Uncovered** means no test.

| Invariant area (source) | Level | Covering suites / representative tests | Corpus can reach? |
| --- | --- | --- | --- |
| Run Outcome classification and precedence (GLOSSARY; run-evidence spec) | well | RunExecutor `terminalPrecedenceRetainsAllEvidence`, `cleanupFailuresPreserveThePrimaryOutcome`, `fatalFailureRetainsConcurrentCancellation`; CliRun `rendersFocusedTerminalEvidence` | Only for outcomes inducible by tree shape. Precedence needs injected faults. |
| Run Phase sequence, Phase Skip Reason, no-work runs (GLOSSARY) | well | RunExecutor `noWorkApplyRunTraversesTheStablePhaseSequence`, `noWorkRunReportsTheSameReasonsInEveryExecutionMode`, `aRunThatStopsEarlyRecordsOnlyThePhasesItTraversed` | Yes, through the CLI `EVENT:` stream |
| Run Progress (determinate, failed attempts advance) | thin–well | RunExecutor `determinateProgressStartsAtZeroAgainstAnImmutableTotal`, `failedAttemptsAdvanceCompletedProgress`; AssetRun `progressAndSkipSummaryExcludeNonWork`; CliRun `rendersOrderedEvents` | Yes (`PROGRESS:` lines) |
| Run Evidence retention: retain-before-publish, at-most-once, ObserverFailed once, derived summaries (run-evidence spec) | well | RunEvidence (16), RunExecutor `throwingWorkObserversRetainEvidence`, AssetRun `throwingObserversPreserveCommittedWork` | No (needs a throwing observer) |
| Safety Cleanup: exactly once, every terminal path, only owned paths, failures aggregated (GLOSSARY; staging-ownership) | well | RunExecutor `safetyCleanupRunsExactlyOnceBeforeTheTerminalResult`, `registeredArtifactsAreCleanedOnEveryTerminalPath`, `cleanupDoesNotFollowAReplacedParent`; DurableStaging `cleanupContinuesAfterADamagedTemporary` | No (needs injected terminal paths and failures) |
| Temporary Ownership: recovery during Apply Preparing, `.cao-staging` reservation, lock, fail-closed on unverifiable state (staging-ownership) | well | RunExecutor `verifiedStaleStagingIsRecoveredBeforeWork`, `unverifiableStagingIsPreserved` (15 rows: invalid, version, root, run, truncated, trailing, traversal, alias, duplicate, wrong-type, unknown child/sibling, missing lock, reserved file, lookalike), `activeStagingBlocksUntilItsOwnerExits`, `dryRunLeavesStagingUntouched`; DurableStaging recovery tests | Mostly yes: seed `.cao-staging` into the tree, or hold `owner.lock` from the harness |
| `CAO-STAGING` v1–v3 read/recover/write (map Compatibility) | thin | v1 only through a literal fixture (`tests/RunExecutorTests.cpp:47-53`); v3 only as writer output (DurableStaging); **no v2 fixture** | Yes, if the harness seeds literal manifests |
| Staged publication: `NotPublished` / `PublishedStillOwned` / `PublishedAndReleased`, receipt one-use, `Replace`/`NoReplace`, destination revalidation (staged-publication spec) | well | DurableStaging (about 16 publication tests: `publicationReleaseFailurePreservesCommittedDestination`, `occupiedDestinationIsNotPublished`, `archivePublicationRejectsLinkedParent`, `archivePublicationSurvivesProducerTermination`, ...); AssetExecution `*PublicationReleaseFailure` per Asset Kind | No (needs mid-run intervention or a crash window) |
| Committed Mutation retention under cancel or failure (GLOSSARY) | well | RunExecutor `archiveFinalizationCancellationRetainsCommittedOutput`, `cancellationAfterAtomicAssetAttempt`; AssetRun `cancellationDuringFinalAssetSkipsFinalization`; ArchiveFinalization `cancellationPreservesCommittedArchiveLoadingPlugin`; OptimizationRunService `lateCancellationDoesNotChangeCommittedEvidence` | No (cancellation checkpoints) |
| Operation Failure: mutation state and safe-to-continue (GLOSSARY; staging-ownership) | well | AssetExecution (30 injected); AssetRun `archiveFailuresControlContinuation`, `mutationAwareFailuresControlContinuation`; Discovery `partialMergeRetainsCommittedOutput`, `extractionFailureBeforeMergeCommitsNothing`, `laterArchivesRepeatContainmentAfterSafeRejection` | Partly (malformed-asset load failure); mostly no |
| Archive Precedence and Archive Collision (GLOSSARY) | well | Discovery `collisionsAreReportedBeforeExtraction`, `frozenPrecedenceSurvivesFailures`, `collisionsDoNotCrossModRoots`, `invalidExplicitOrder`, `explicitArchiveAliasUsesResolvedScope`; AssetRun `reportsCollisionsBeforeOrderedExtraction` | Yes |
| Loose-over-Archived precedence, Effective Asset Tree (GLOSSARY) | well | Discovery `realExtractionPreservesLooseAssetPrecedence`, `extractsEnabledArchivesBeforeDefinitiveDiscovery`, `archivesProducedByExtractionStayOutOfTheTree`; AssetRun `archiveExtractionPrecedesDefinitiveRoutedExecution` | Yes. The late-loose race (`lateLooseAssetBlocksArchiveMerge`) is not reachable |
| Dry Run is non-mutating (GLOSSARY; staging-ownership) | well | AssetRun `dryRunLeavesCompleteModTreeUnchangedWhileEvaluatingLooseAssets` (full-tree byte snapshot); RunExecutor `dryRunLeavesStagingUntouched`; MainOptimizer `dryRunLoadFailureDoesNotQuarantine`, `textureDryRunMatchesApply`; AssetExecution `dryRunMeshMaintenanceDoesNotMutate` | Yes. This is the corpus's strongest case |
| Archive Finalization and Loading Plugins, Dummy Plugin exact bytes (GLOSSARY; ADR-0002) | well | ArchiveFinalization (42): `finalizationRemovesOnlyExactDummyPlugins`, `plannedExactDummyIsReused`, `existingArchivePluginNamesFollowProfile`, `existingArchivesShareLoadingPlugin`, `finalizationRejectsLinkedDummyPlugins`, `plannedPluginCollisionRetainsCommittedArchive` | Static naming, reuse, and packing cases yes. Guard, cancel, and capacity cases no |
| Empty-directory pruning in Archive Finalization (GLOSSARY) | thin | MainOptimizer `emptyDirectoryCleanupPreservesStaging`; CliExecution `reportsDirectoryPruningMutation` | Yes |
| Routing Policy, Routing Decision, Skip Reason precedence, Routing Ledger (GLOSSARY) | well | AssetRouting (16, pure tables); RunSetup (6); ApplicationRunSetup `meshWorkRoutesStandardAndTerrainMeshes`, `archiveCreationRequiresProfileArchiveSupport` | Indirectly (skip counts in evidence) |
| Mesh Reference Maintenance (GLOSSARY) | well | MeshReferenceMaintenance (3); MainOptimizer `failedConversionSuppressesMeshReferenceMaintenance` and 4 siblings | Yes |
| Mod Selection, Several Mods, ignored-mod exclusions (GLOSSARY) | well | OptimizationRunService `severalMods*` (7), `linkedModRootsRetainCanonicalIdentities`; RunExecutor `filesystemRootSelectionFailsPreparing`, `broadLinkedChildFailsPreparing`; ApplicationRunSetup `providerRejectsUnreadableIgnoredMods` | Yes |
| Run Handle, Start Error, one active run, drop cancels and waits (GLOSSARY; ADR-0001) | well | OptimizationRunService (about 25 handle and scheduling tests); RunEventDelivery (9) | No |
| Run Event ordering and GUI-thread delivery, close deferral (ADR-0001) | well | OptimizationRunService `inlineEventsOwnAnOrderedRunHistory`, `failureEventsPrecedeCleanupAndTerminal`; GuiRunDispatch (5); GuiRun `defersCloseUntilTerminal`, `rejectsStaleObservations` | Ordering yes (CLI stream); delivery no |
| Profile compatibility: `profile.ini`, `common.ini`, `settings.ini`, raw enum integers, QSettings list encoding (map Compatibility) | thin | ApplicationRunSetup reads the shipped SSE/FO4 `profile.ini` and writes `common.ini`. **No TES5 test**, no `settings.ini` round-trip, no raw `DXGI_FORMAT`/`NiFileVersion`/`btu::Game` or list-encoding test | Partly (profile → behaviour) |
| Texture optimization decisions: format and compression choice, BC7, unwanted formats, TGA conversion, interface textures, resize ratio, mipmaps | thin / uncovered | Only MainOptimizer `textureDryRunMatchesApply` (4 resize and mipmap rows) and the load-failure tests | Yes: the corpus is the only oracle |
| Mesh optimization levels, headparts, resave | thin / uncovered | MainOptimizer `severalModsKeepsEveryModRootsHeadparts` (1); AssetExecution mesh tests use a fake backend | Yes: corpus only |
| Animation conversion via `hkxcmd.exe` | uncovered | Always faked (AssetExecution); CliExecution deliberately keeps `hkxcmd` absent (`tests/CliExecutionTests.cpp:86-88`) | Yes: corpus only |
| Created-archive content, beyond existence and size | uncovered | ArchiveFinalization asserts existence, sources, and naming, never entry content | Yes: corpus only |
| GUI structure and enable/disable rules, `MainWindow.ui`, `TexturesFormatSelectDialog` (map Interface) | uncovered | No tests | No (outside the corpus) |
| HTML log location and format (map Compatibility) | uncovered | ApplicationLogging checks plog severity and path redirection only | Partly |

The suites also test behaviour that the parity documents never mention. The port and #467
need to decide about each item explicitly:

- `.caobad` quarantine of load-failed Assets, with collision-safe numeric suffixes
  (`src/MainOptimizer.cpp:18-23`; MainOptimizer `loadFailuresQuarantineMalformedAssets`,
  `loadFailureUsesCollisionSafeQuarantineName`, `failedQuarantineMakesLoadFailureUnsafe`).
- Extraction and finalization free-space preflight grouped by volume (Discovery `capacity*`;
  ArchiveFinalization `finalizationCapacity*`).
- Windows device names and control characters in archive entries block the whole batch
  (Discovery `windowsDeviceNamesBlockBatch`, `windowsControlCharactersBlockBatch`).
- `filesToNotPack` exclusion (ArchiveFinalization `filesToNotPackAreNeitherPackedNorDeleted`).
- Headpart lists kept per Mod Root in Several Mods mode.

## Suite classification

**Portable as scenario** means the test asserts an observable invariant that a Rust
implementation must also satisfy. **Implementation-bound** means it tests a C++ seam, library,
or API shape that won't exist in Rust.

Each portable scenario goes to one of two destinations:

- **Rust test**: the scenario is corpus-unreachable (it needs injection, cancellation,
  concurrency, mid-run intervention, or a crash window), or it is a pure-function table that is
  cheaper to port than to generate.
- **Corpus seed**: the scenario is static. Its fixture tree becomes a hand-written corpus case,
  and the C++ oracle supplies the expected result.

| Suite | Class | Destination (approx.) | Notes |
| --- | --- | --- | --- |
| RunExecutorTests (59) | portable | ~46 Rust, ~13 corpus | Seam is `RunServices`. Rewrite against the Rust executor's injection points. Staging-recovery and no-work cases become corpus seeds. |
| DurableStagingTests (35) | portable | ~19 Rust, ~12 corpus, ~4 design-bound | Move-only and one-use receipt, and generic commit rejection (`publicationReceiptIsMoveOnlyAndOneUse`, `genericCommitCannotReleaseDurableStage`, `expiredPublicationScopeCannotPublish`), become Rust type-system guarantees rather than tests. |
| AssetExecutionTests (31) | portable | ~29 Rust, ~2 corpus | Port the per-Asset-Kind publication, release-failure, and source-removal matrix. The fake backend becomes a Rust trait. |
| AssetRunTests (27) | portable | ~22 Rust, ~5 corpus | Continuation safety, cancellation checkpoints, observer failures. |
| ArchiveFinalizationTests (42) | portable | ~28 Rust, ~14 corpus | Capacity, guard, cancel, and disappearing-plugin cases go to Rust. Naming, reuse, and packing go to the corpus. |
| ArchiveFirstAssetDiscoveryTests (49) | portable | ~19 Rust, ~30 corpus | Capacity, late-loose race, changed manifest, and cancellation go to Rust. Precedence, collisions, links, Windows names, and Unicode go to the corpus. |
| AssetRoutingTests (16) | portable | 16 Rust | Pure data tables, cheapest to port verbatim. |
| GuiRunTests (6) | portable | 6 Rust | `RunViewModel` (`src/GuiRun.h`) has no Qt dependency. Its rules carry over to a Slint view model, and the GUI survives the port. |
| OptimizationRunServiceTests (51) | mixed | ~25 Rust, ~12 corpus, ~14 bound | Port the Run Handle and Start Error invariants after the threading-mapping decision. Scheduler-injection and worker-self-wait diagnostics are C++-shaped. |
| RunEvidenceTests (16) | mostly bound | ~5 Rust | `MutableRunEvidence` is one internal design. Port only retain-before-publish, at-most-once, ObserverFailed-once, and derived-summary invariants at the Rust run seam. |
| MainOptimizerTests (20) | portable | ~4 Rust, ~16 corpus | Quarantine, mesh-reference gating, headparts, and source cleanup. Mostly reachable with real files. |
| ApplicationRunSetupTests (10), RunSetupTests (6) | portable | ~12 Rust, rest corpus | UI choice → requested work → profile capability conflicts. Fixtures must inject the profile root, because Rust resolves `profiles/` from the exe, not the cwd. |
| MeshReferenceMaintenanceTests (3) | portable | 3 Rust | Exercises the nifly shim directly. |
| GuiRunDispatchTests (5), RunEventDeliveryTests (9) | implementation-bound | 0 | Qt queued dispatch and the serialized C++ dispatcher. Re-derive from ADR-0001 once Run Event dispatch is mapped onto Slint threading. |
| ApplicationLoggingTests (3) | implementation-bound | 0 | plog bootstrap. Superseded by the logging decision. |
| CliRunTests (6), CliExecutionTests (3), CliConsoleInterruptTests (1), OptionsCAOTests (6) | out of scope | 0 | The CLI is dropped. CliRun still documents the oracle's output format (see Consequences). |

Rough totals, ±15%: about 235 scenarios become Rust tests, about 105 become corpus seeds, and
about 60 are dropped or replaced by design.

## Recommendation and threshold

**Recommendation: port scenarios, selectively, in addition to the differential corpus. Don't
rely on the corpus alone.**

**Threshold.** Port a suite when both of these hold:

1. At least 25% of its scenarios assert a parity-contract invariant (GLOSSARY, ADR-0001/0002,
   staging-ownership, the two specs, or the map's Compatibility notes) **and** the black-box
   corpus can't reach them deterministically.
2. Those scenarios make at least one invariant area **well covered**: at least 3 independent
   scenarios that assert outcome, evidence, or filesystem state, not internal storage.

A suite that fails rule 1 contributes only its static scenarios, as corpus seeds. A suite bound
to a C++ library or API shape is dropped.

**Result.** About 58% of all scenarios (~235/404) are contract-bearing and corpus-unreachable.
Every suite marked portable above clears rule 1. Seventeen of the 27 invariant areas are well
covered, and every one of those safety-critical areas (Safety Cleanup, Temporary Ownership,
staged publication, Committed Mutation, Operation Failure continuation, Run Outcome precedence)
is unreachable by the corpus. The corpus alone would leave the most dangerous invariants of the
port untested.

**Port, in priority order:**

1. DurableStagingTests
2. RunExecutorTests
3. AssetExecutionTests
4. AssetRunTests
5. ArchiveFinalizationTests
6. ArchiveFirstAssetDiscoveryTests (the injected cases)
7. AssetRoutingTests
8. GuiRunTests
9. The Run Handle subset of OptimizationRunServiceTests
10. Selected RunEvidence, MainOptimizer, setup, and MeshReferenceMaintenance cases

Port each one as a scenario, meaning its intent, fixture tree, and assertions, against whatever
seams the Rust design exposes. Don't transliterate the C++ fixtures.

**Corpus seeds.** The static scenarios become hand-written corpus cases with the C++ oracle's
result as the expectation. The cases are: precedence, collisions, links, Windows names,
Unicode, seeded staging recovery, Dry Run snapshots, quarantine, Loading Plugin naming,
Several Mods, and no-work runs.

**Gaps to fill outside the C++ suites:**

- The corpus must carry engine-output parity: textures, meshes, animations, and archive
  content.
- New Rust tests must be written from the contract for:
  - a v2 `CAO-STAGING` manifest literal;
  - TES5 profile behaviour;
  - the `settings.ini`/`common.ini` round-trip and QSettings list encoding;
  - the GUI enable/disable rules on the map's deviation list.

## Consequences for other tickets

- **Output equivalence (#467).** No C++ test golden-compares engine output. The suites assert
  six things:
  - untouched bytes are preserved (full-tree snapshots, `tests/AssetRunTests.cpp:135-158`);
  - outputs exist or are absent;
  - failure codes (`RunFailureCode`, `AssetExecutionFailure`, `ArchiveExtractionFailure`);
  - mutation state and safe-to-continue;
  - committed-mutation counts by Mod Root and operation kind;
  - Archive Collision winners and shadowed archives.

  That gives #467 a ready-made evidence vocabulary. The equivalence rule for DDS, NIF, HKX, and
  BSA content has no C++ precedent, so #467 must define it from scratch. The undocumented
  behaviours listed above (`.caobad` quarantine, capacity preflight, Windows-name batch blocking,
  `filesToNotPack`, headparts) also need a decision: either add them to the parity contract or
  record them on the deviation list.
- **Differential harness (fog).**
  - **Oracle output.** The C++ oracle's machine-readable surface is the CLI's
    `EVENT:|<Run ID>|<sequence>|` stream plus its exit codes. The codes are 0, 1, 2, and 130
    (`docs/cli.md`), and `tests/CliRunTests.cpp` pins the format. The harness must normalize
    Run IDs, staging nonces, and absolute paths. The Rust driver has no CLI, so it must render
    sealed evidence into the same comparable form.
  - **Cancellation.** The oracle can be cancelled only through Ctrl+C or Ctrl+Break, which is
    nondeterministic, so leave cancellation parity to the ported Rust tests.
  - **Fault-inducing tree shapes.** The harness *can* create:
    - read-only or locked files;
    - a held `owner.lock`, which gives `StagingActive`;
    - junctions;
    - seeded `.cao-staging` manifests, which embed the canonical Mod Root path and so must be
      generated per case;
    - malformed assets;
    - Windows device names.
  - **Free-space preflight.** This isn't reachable without a nearly full volume.
  - **Profiles.** The C++ suites cover SSE and FO4 only, so the corpus should include TES5.
- **Run Handle and Run Event threading (fog).** About 25 OptimizationRunService and 9
  RunEventDelivery scenarios are the executable form of ADR-0001: one active run, drop cancels
  and waits, a terminal event shared with the waiter, delivery after wait, and restart from a
  terminal observer. Use them as the acceptance list for the Rust and Slint mapping.
- **Profile path resolution (deviation list).** The C++ suites depend on the working directory
  to find `profiles/`. Rust ports must inject the profile root. The oracle needs the
  cwd-equals-exe-folder rule the map already states.
