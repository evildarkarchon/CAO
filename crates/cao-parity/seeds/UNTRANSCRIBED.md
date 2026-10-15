# Untranscribed C++ scenarios

The C++ QtTest scenarios (#465) that could not become corpus seeds (#507), each with the reason. Every
other C++ scenario is a seed under `seeds/<Suite>/`, whose `origin` names it. `tests/seeds.rs` checks
that every C++ test slot is one or the other.

This list feeds the Rust-test backlog: a scenario the differential corpus cannot reach is ported as a Rust
test against the seams the spec names (#476, "Testing Decisions"), except where the reason says its suite is
dropped. Reasons start with a category:

- **Injection**: a fake, throwing or recording collaborator stands in for real work.
- **Cancellation** and **Concurrency**: a stop request, threads, or a race the oracle cannot time.
- **Crash window**, **Held lock** and **Mid-run filesystem change**: the tree changes, or a process holds
  it, while the run is in flight.
- **Capacity**: an injected free-space or volume probe.
- **Internal API**: the test calls a C++ seam directly, and no whole run can observe what it asserts.
- **Pure table**: a value type or lookup table, cheaper to port verbatim than to reach through a run.
- **Explicit Archive Precedence** and **INI hand-edit**: input neither the GUI nor the oracle CLI can give.
- **Recipe vocabulary**: the tree needs bytes the recipe format cannot write, such as a TES3 BSA, a
  directory symlink, or one Archive storing a name twice.
- **Deviation**: the scenario triggers an entry of the spec's deviation list, which the port fixes.
- **Dropped suite**: the spec drops the suite as bound to the CLI, plog or Qt dispatch; its intent is
  re-derived from ADR-0001 where it still applies.
- **Partly transcribed**: the scenario is a seed, but part of what the C++ test asserts needs one of the
  above; that part is the backlog item.

A `[row]` after a test names one `QTest::newRow` row of a data-driven test.

## ApplicationLoggingTests

- `ApplicationLoggingTests::debugBootstrapConfigurationIsSingleAssignment`: Dropped suite — plog bootstrap, superseded by the Application Log decision.
- `ApplicationLoggingTests::infoBootstrapConfigurationIsSingleAssignment`: Dropped suite — plog bootstrap, superseded by the Application Log decision.
- `ApplicationLoggingTests::runConfigurationRedirectsPathAndSeverity`: Dropped suite — plog bootstrap, superseded by the Application Log decision.

## ApplicationRunSetupTests

- `ApplicationRunSetupTests::archiveCreationRequiresProfileArchiveSupport`: INI hand-edit — `bsaEnabled=false`.
- `ApplicationRunSetupTests::disabledTextureWorkDoesNotRequestProfileConversion`: Partly transcribed — the C++ test reads the compiled policy of a request with no work; the seed resaves Meshes so routing runs.
- `ApplicationRunSetupTests::optimizerProfileLoadsAuxiliaryFilesAfterCapture`: INI hand-edit — auxiliary profile files edited between capture and load.
- `ApplicationRunSetupTests::providerLoadsOwnedConfiguration`: Internal API — the configuration provider, with an edited ignoredMods.txt.
- `ApplicationRunSetupTests::providerLoadsRoutingAndBackendFromTheSameUpdatedProfile`: INI hand-edit — profile values the GUI cannot produce, changed between capture and Preparing.
- `ApplicationRunSetupTests::providerRejectsUnreadableIgnoredMods`: INI hand-edit — ignoredMods.txt replaced by a folder.
- `ApplicationRunSetupTests::requestOwnsCallerIntent`: Internal API — request immutability.
- `ApplicationRunSetupTests::requestRejectsInvalidOptionValues`: Internal API — invalid values throw before any run; the oracle prints no event to compare, in every row.

## ArchiveFinalizationTests

- `ArchiveFinalizationTests::cancellationPreservesCommittedArchiveLoadingPlugin`: Cancellation.
- `ArchiveFinalizationTests::cleanupExceptionKeepsRecordedAttempts`: Mid-run filesystem change — a Mod Root is renamed inside a probe, in every row.
- `ArchiveFinalizationTests::committedArchiveRetainsLockedSource`: Partly transcribed — its held handle becomes a read-only source.
- `ArchiveFinalizationTests::committedArchiveRetainsLockedSource [unreadable-source-stops]`: Held lock — a byte-range lock makes the source unreadable.
- `ArchiveFinalizationTests::disappearingLoadingPluginCapacityIsReserved`: Capacity — the plugin is deleted inside the probe, in every row.
- `ArchiveFinalizationTests::dummyCapacityDecreasesAfterEachRoot`: Capacity — injected capacity equal to one plugin.
- `ArchiveFinalizationTests::evidenceFailuresPropagateUnchanged`: Internal API — forced evidence invariant violations, in every row.
- `ArchiveFinalizationTests::existingArchivePluginCollisionFailsSafely`: Partly transcribed — the C++ test creates the folder after planning; the seed has it from the start, which reaches the same creation check.
- `ArchiveFinalizationTests::failedPackingRetainsSourcesAndExistingArchives`: Held lock — a handle denies reading a source.
- `ArchiveFinalizationTests::filesToNotPackAreNeitherPackedNorDeleted`: Partly transcribed — its own exclusion rule is an INI hand-edit, so the seed uses a shipped one.
- `ArchiveFinalizationTests::finalizationCancelsPluginCleanupBetweenRoots`: Cancellation.
- `ArchiveFinalizationTests::finalizationCapacityChecks`: Capacity — injected probes, in every row.
- `ArchiveFinalizationTests::finalizationCapacityEstimateCoversPublishedOutput`: Capacity — compares the internal estimate under an injected probe.
- `ArchiveFinalizationTests::finalizationCapacityIsGroupedByVolume`: Capacity — injected per-volume capacity.
- `ArchiveFinalizationTests::finalizationCapacityWithoutOutputs`: Capacity — injected zero capacity.
- `ArchiveFinalizationTests::finalizationFreezesTotalAndCancelsBetweenOutputs [after-final-output]`: Cancellation — at an output boundary.
- `ArchiveFinalizationTests::finalizationFreezesTotalAndCancelsBetweenOutputs [before-first-output]`: Cancellation — at an output boundary.
- `ArchiveFinalizationTests::finalizationFreezesTotalAndCancelsBetweenOutputs [between-outputs]`: Cancellation — at an output boundary.
- `ArchiveFinalizationTests::finalizationPlanningObservesCancellation`: Cancellation.
- `ArchiveFinalizationTests::finalizationPreservesReplacedDummyPlugin`: Mid-run filesystem change — the dummy is replaced after planning.
- `ArchiveFinalizationTests::finalizationRejectsChangedDummyPluginParent`: Mid-run filesystem change — the Mod Root becomes a junction inside a probe.
- `ArchiveFinalizationTests::finalizationReportsGuardedPluginRemovalFailure`: Partly transcribed — its held handle becomes a read-only plugin.
- `ArchiveFinalizationTests::finalizationRetainsRemovalPrefixOnGuardFailure`: Partly transcribed — its held handle becomes a read-only plugin.
- `ArchiveFinalizationTests::idleUnknownVolumeDoesNotInflateCapacity`: Capacity — an injected unknown volume.
- `ArchiveFinalizationTests::linkedLoadingPluginsWithoutFallbackRetainSources`: Partly transcribed — the C++ test links the names after planning; the seed has them from the start.
- `ArchiveFinalizationTests::loadingPluginRemainsPinnedThroughSourceCleanup`: Concurrency — a remover thread races source cleanup, in every row.
- `ArchiveFinalizationTests::packedSourceParentJunctionIsRejected`: Mid-run filesystem change — a source's parent becomes a junction after planning.
- `ArchiveFinalizationTests::packingPreservesStagingFiles`: Partly transcribed — a file staged through the live run's registry, which only the staging seam can produce.
- `ArchiveFinalizationTests::packingRequiresArchiveCreationRequest`: Partly transcribed — the C++ policy requests nothing but native Texture work through the seam; a whole run optimizes the Texture as well.
- `ArchiveFinalizationTests::plannedExactDummyIsReused`: Mid-run filesystem change — the exact dummy appears after planning.
- `ArchiveFinalizationTests::plannedLoadingPluginDisappearsBeforeCommit`: Mid-run filesystem change — the plugin is deleted after planning.
- `ArchiveFinalizationTests::plannedNamesAreDistinctAndCommitPreservesNewDestination`: Partly transcribed — a competing creator writing mod.bsa after planning is a mid-run filesystem change.
- `ArchiveFinalizationTests::plannedOutputRechecksLoadingPluginNames`: Mid-run filesystem change — a plugin appears after planning, in every row.
- `ArchiveFinalizationTests::plannedPluginCollisionRetainsCommittedArchive`: Mid-run filesystem change — a competing plugin appears after planning.
- `ArchiveFinalizationTests::unexpectedExceptionIsRecordedOnce`: Injection — a throwing volume probe, in every row.
- `ArchiveFinalizationTests::volumeMountPointGrowsForLongMountedFolders`: Injection — a fake volume path query.
- `ArchiveFinalizationTests::volumeQueriesAcceptLongModRoots`: Internal API — volume queries on a path over 600 characters.

## ArchiveFirstAssetDiscoveryTests

- `ArchiveFirstAssetDiscoveryTests::archivesAreOrderedWithinEachModRoot`: Partly transcribed — the caller's Mod Root order (z-mod first) cannot be given; a Several Mods selection orders its children by name.
- `ArchiveFirstAssetDiscoveryTests::cancelledExtractionSkipsDefinitiveTraversal`: Cancellation — the extraction callback cancels.
- `ArchiveFirstAssetDiscoveryTests::capacityIsGroupedByVolume`: Capacity — injected capacity and volume probes.
- `ArchiveFirstAssetDiscoveryTests::changedManifestFailsBeforeMerge`: Internal API — a hand-built plan that disagrees with the Archive.
- `ArchiveFirstAssetDiscoveryTests::collisionObserverCanCancelBeforeExtraction`: Cancellation — the collision observer cancels.
- `ArchiveFirstAssetDiscoveryTests::collisionsAreReportedBeforeExtraction [explicit-file-roots]`: Internal API — Archive files passed as discovery roots; a selection is always a folder.
- `ArchiveFirstAssetDiscoveryTests::collisionsAreReportedBeforeExtraction [explicit]`: Explicit Archive Precedence.
- `ArchiveFirstAssetDiscoveryTests::collisionsAreReportedBeforeExtraction [file-roots]`: Internal API — Archive files passed as discovery roots; a selection is always a folder.
- `ArchiveFirstAssetDiscoveryTests::collisionsAreReportedBeforeExtraction [loose-over-explicit]`: Explicit Archive Precedence.
- `ArchiveFirstAssetDiscoveryTests::compressedShadowedEntryRequiresCapacity`: Capacity — an injected free-space probe.
- `ArchiveFirstAssetDiscoveryTests::directoryLinksAreExcluded [contained symlink]`: Recipe vocabulary — a directory symlink; the junction rows cover the same exclusion.
- `ArchiveFirstAssetDiscoveryTests::directoryLinksAreExcluded [escaping symlink]`: Recipe vocabulary — a directory symlink; the junction rows cover the same exclusion.
- `ArchiveFirstAssetDiscoveryTests::dryRunIgnoresPrecedenceAndManifests`: Deviation 3 — a Dry Run requesting Archive extraction, which the GUI forbids; it also passes an invalid explicit Archive Precedence.
- `ArchiveFirstAssetDiscoveryTests::explicitArchiveAliasUsesResolvedScope`: Explicit Archive Precedence — through an explicit Archive file root.
- `ArchiveFirstAssetDiscoveryTests::explicitArchiveRootDiscoversExtractedSiblings`: Internal API — an Archive file passed as a discovery root.
- `ArchiveFirstAssetDiscoveryTests::extractionCapacityRecheck`: Capacity — calls the extractor directly with an injected probe, in every row.
- `ArchiveFirstAssetDiscoveryTests::extractionFailureBeforeMergeCommitsNothing`: Internal API — a hand-built plan for a missing Archive.
- `ArchiveFirstAssetDiscoveryTests::extractionProducedLinksAreExcluded`: Injection — the extraction callback creates a link, which no Archive can.
- `ArchiveFirstAssetDiscoveryTests::frozenPrecedenceSurvivesFailures`: Mid-run filesystem change — an Archive is corrupted or a Loose Asset deleted after preflight, in every row.
- `ArchiveFirstAssetDiscoveryTests::insufficientCapacityBlocksEntireBatch`: Capacity — an injected free-space probe.
- `ArchiveFirstAssetDiscoveryTests::invalidExplicitOrder`: Explicit Archive Precedence — an invalid explicit order, in every row.
- `ArchiveFirstAssetDiscoveryTests::lateLooseAssetBlocksArchiveMerge`: Mid-run filesystem change — a Loose Asset appears after preflight.
- `ArchiveFirstAssetDiscoveryTests::laterArchivesRepeatContainmentAfterSafeRejection`: Mid-run filesystem change — a junction appears after preflight.
- `ArchiveFirstAssetDiscoveryTests::linksIntoStagingAreExcluded`: Partly transcribed — the C++ test runs Apply with an injected extractor; a whole Apply run would stop at the unverified staging first, and a Dry Run may not request extraction (deviation 3), so the seed is a Dry Run of Texture work.
- `ArchiveFirstAssetDiscoveryTests::mergeRejectsLinkedParent`: Partly transcribed — the C++ test asserts the extractor's own MergeFailed; a whole run rejects the tree earlier, at preflight, and its TES3 Archive becomes an SSE one.
- `ArchiveFirstAssetDiscoveryTests::partialMergeRetainsCommittedOutput`: Internal API — a whole run rejects the occupied parent at preflight, before any partial merge; occupiedArchiveDestinationBlocksBatch covers that.
- `ArchiveFirstAssetDiscoveryTests::rawManifestPaths [format-0-contained]`: Recipe vocabulary — a TES3 BSA; the format-1 and format-2 rows cover the same paths.
- `ArchiveFirstAssetDiscoveryTests::rawManifestPaths [format-0-escaping]`: Recipe vocabulary — a TES3 BSA; the format-1 and format-2 rows cover the same paths.
- `ArchiveFirstAssetDiscoveryTests::removedDirectoryRootDoesNotThrow`: Mid-run filesystem change — the callback deletes the Mod Root.
- `ArchiveFirstAssetDiscoveryTests::removedExplicitArchiveRootDoesNotThrow`: Mid-run filesystem change — the callback deletes an explicit Archive root.
- `ArchiveFirstAssetDiscoveryTests::sameArchiveAliasesBlockBatch [case-folded duplicate]`: Recipe vocabulary — one Archive storing a name twice, which the materialiser rejects before writing.
- `ArchiveFirstAssetDiscoveryTests::sameArchiveAliasesBlockBatch [exact duplicate]`: Recipe vocabulary — one Archive storing a name twice, which the materialiser rejects before writing.
- `ArchiveFirstAssetDiscoveryTests::selectedDirectoryAliasKeepsItsOriginalTarget`: Mid-run filesystem change — the callback retargets the selected alias.
- `ArchiveFirstAssetDiscoveryTests::stagedExtractionCommitsPayload [tes3]`: Recipe vocabulary — a TES3 BSA, which no shipped profile reads and the recipe cannot write.
- `ArchiveFirstAssetDiscoveryTests::stagingIsExcludedFromDiscovery`: Partly transcribed — Apply would stop at the unverified staging first, and a Dry Run may not request extraction (deviation 3), so the seed is a Dry Run of Texture work.
- `ArchiveFirstAssetDiscoveryTests::unknownVolumeWithWorkRetainsBatchCapacity`: Capacity — an injected unknown volume.

## AssetExecutionTests

- `AssetExecutionTests::animationCommitFailure`: Internal API — the routed Animation is a folder, which discovery never routes.
- `AssetExecutionTests::animationFailureIsReported`: Partly transcribed — the C++ failure is injected with its own detail text; the seed's comes from hkxcmd.
- `AssetExecutionTests::animationPublicationReleaseFailure`: Injection — the fake converter occupies the manifest scratch.
- `AssetExecutionTests::animationStagedOutcomes`: Injection — a fake converter writes or throws, in every row.
- `AssetExecutionTests::archiveIsNotOwnedByAssetExecutor`: Internal API — an Archive handed to the Asset executor directly.
- `AssetExecutionTests::conversionOnlyTextureExecution`: Partly transcribed — a conversion-only request is unreachable from the GUI, and the staged-before-destination ordering is checked inside the fake backend.
- `AssetExecutionTests::convertibleSourceReplacementPreservesNewcomer`: Mid-run filesystem change — the source is swapped during load.
- `AssetExecutionTests::executionFailurePreservesRoutedDecision`: Internal API — immutability of the routed Asset object.
- `AssetExecutionTests::failedMeshSavePreservesOriginal`: Injection — a fake partial save fails.
- `AssetExecutionTests::failedTextureSavePreservesOriginal`: Injection — a fake save writes partial output and fails.
- `AssetExecutionTests::interruptedTextureRecovery`: Crash window — a child process is killed at a filesystem boundary, in every row.
- `AssetExecutionTests::meshBackendException`: Injection — a throwing backend, in every row.
- `AssetExecutionTests::meshCommitFailure`: Internal API — the routed Mesh is a folder, which discovery never routes.
- `AssetExecutionTests::meshOperationFailure [maintenance]`: Injection — a fake operation result fails.
- `AssetExecutionTests::meshOperationFailure [optimization]`: Injection — a fake operation result fails.
- `AssetExecutionTests::meshPublicationReleaseFailure`: Injection — the fake save occupies the manifest scratch, in every row.
- `AssetExecutionTests::meshWithoutChanges [unchanged Apply]`: Internal API — the fake backend reports the Mesh unchanged; no recipe guarantees a Mesh the real optimizer leaves alone.
- `AssetExecutionTests::nativeDestinationReplacementDuringLoadIsRejected`: Mid-run filesystem change — the destination is swapped during load.
- `AssetExecutionTests::textureCleanupFailurePreservesPrimaryFailure`: Injection — the fake save replaces the staged file with a folder.
- `AssetExecutionTests::textureOperationFailureDetails`: Injection — a fake optimizer result.
- `AssetExecutionTests::texturePublicationReleaseFailure`: Injection — the fake save occupies the manifest scratch, in every row.
- `AssetExecutionTests::textureReadOnlyException`: Injection — a throwing load or optimize, in every row.
- `AssetExecutionTests::textureSaveException`: Injection — a throwing save.
- `AssetExecutionTests::textureSourceRemovalFailure`: Injection — the fake remove callback damages, deletes or throws, in every row.
- `AssetExecutionTests::textureStagingIsRegisteredBeforeSave`: Internal API — inspects the staged file inside the fake save.

## AssetRoutingTests

- `AssetRoutingTests::batchRoutingMatchesSingleAssetDecisions`: Internal API — single against batch routing.
- `AssetRoutingTests::batchRoutingOmitsUnsupportedPathsAndCountsSkips`: Partly transcribed — the C++ conversion-only request is unreachable, so TES5's native-only Texture work swaps the routed and excluded variants, and its duplicate path cannot exist on disk.
- `AssetRoutingTests::batchRoutingOwnsRoutedAssetsInInputOrder`: Internal API — ledger ownership and duplicate paths.
- `AssetRoutingTests::compilesCompleteRoutingPolicy`: Internal API — the compiled policy object.
- `AssetRoutingTests::conversionOnlyRoutesTextureAndMaintainsMeshes`: Partly transcribed — conversion alone is unreachable from the GUI, so native Texture work is requested too.
- `AssetRoutingTests::dryRunPreservesLooseAssetOperations`: Partly transcribed — the C++ request also extracts Archives in the Dry Run, which is deviation 3.
- `AssetRoutingTests::emptyBatchProducesEmptyLedger`: Internal API — an empty batch; the empty-requested-work seed is its whole-run form.
- `AssetRoutingTests::ledgerQueriesRoutedAssetsByPhaseAndTarget`: Internal API — ledger queries.
- `AssetRoutingTests::nativeTextureTracer [lexically unnormalized caller path]`: Internal API — traversal never yields a `..` path.
- `AssetRoutingTests::profileArchiveExtensionValidation`: Internal API — malformed Archive extensions, which bsaGame never yields, in every row.
- `AssetRoutingTests::returnsAllPolicyValidationErrors`: Partly transcribed — its five exact errors include a missing extension and a Terrain-only request, which no GUI request or shipped profile gives.
- `AssetRoutingTests::routedAssetFacts`: Internal API — a routed Asset's phase, target and operations, in every row; supportedAssetRecognition covers recognition.
- `AssetRoutingTests::routingIgnoresFilesystemState`: Internal API — routes a path that does not exist.
- `AssetRoutingTests::skipReasonPrecedence [excluded native Texture Variant]`: Internal API — the request (conversion without native Textures, or Standard Meshes without Terrain) is unreachable from the GUI.
- `AssetRoutingTests::skipReasonPrecedence [excluded terrain Mesh Variant]`: Internal API — the request (conversion without native Textures, or Standard Meshes without Terrain) is unreachable from the GUI.

## AssetRunTests

- `AssetRunTests::animationFailuresPreserveProgressAndEvidence [unsafe-exception-stops-next-animation]`: Injection — an injected unsafe BackendException.
- `AssetRunTests::animationFailuresPreserveProgressAndEvidence [unsafe-final-exception-stops-finalization]`: Injection — an injected unsafe BackendException.
- `AssetRunTests::archiveCancellationSkipsDefinitiveDiscovery`: Cancellation — during extraction.
- `AssetRunTests::archiveFailuresControlContinuation`: Injection — every row injects an extraction result no tree reliably produces.
- `AssetRunTests::cancellationDuringFinalAssetSkipsFinalization`: Cancellation — during the final Asset.
- `AssetRunTests::cancellationStopsBetweenRoutedAssets`: Cancellation — between routed Assets.
- `AssetRunTests::cancelledArchiveFinalizationIsReported`: Cancellation — a fake finalizer reports cancellation.
- `AssetRunTests::completeAttemptEvidenceSurvivesAdapters`: Injection — fake executor and finalizer adapters.
- `AssetRunTests::dryRunAggregatesArchiveSkipsAndKeepsDirectoryUnsupportedPathsSilent`: Deviation 3 — its point is the Dry Run skips of requested Archive extraction, which the GUI forbids; it also passes an explicit file root.
- `AssetRunTests::dryRunLeavesCompleteModTreeUnchangedWhileEvaluatingLooseAssets`: Partly transcribed — the C++ test also requests Archive extraction in the Dry Run, which is deviation 3, so its DisabledPhase skips are a Rust test.
- `AssetRunTests::executesOriginalLedgerAssetsInTargetOrder`: Partly transcribed — the C++ test passes five explicit file roots and checks the fake executor's call order; the seed selects their Mod Root.
- `AssetRunTests::filesystemTraversalPollsCancellation`: Cancellation — an injected cancellation poll, in every row.
- `AssetRunTests::finalArchiveCancellationSkipsDefinitiveDiscovery`: Cancellation — after the final extraction.
- `AssetRunTests::initializationCancellationDoesNotInventAttempt`: Cancellation — an injected initialization cancellation.
- `AssetRunTests::mutationAwareFailuresControlContinuation [unsafe-failure]`: Injection — an injected PartialOrUnknown SaveFailed.
- `AssetRunTests::mutationAwareFailuresControlContinuation [unsafe-final-attempt]`: Injection — an injected PartialOrUnknown SaveFailed.
- `AssetRunTests::progressAndSkipSummaryExcludeNonWork`: Partly transcribed — the C++ policy (conversion and Meshes without native Textures) is unreachable from the GUI, so the excluded variant is the TGA and the Mesh gets no reference maintenance.
- `AssetRunTests::relativeSelectionRetainsMutationScope`: Injection — a fake executor deletes the source; the selection is relative to the working directory.
- `AssetRunTests::reportsCollisionsBeforeOrderedExtraction`: Explicit Archive Precedence — with a fake extractor and a throwing collision observer.
- `AssetRunTests::throwingAttemptRetainsCancellation`: Injection — a throwing executor adapter that cancels.
- `AssetRunTests::throwingDiagnosticsCancellationSkipsFinalization`: Injection — a throwing diagnostics observer that cancels.
- `AssetRunTests::throwingFinalizerPropagates`: Injection — a throwing finalizer adapter.
- `AssetRunTests::throwingObserversPreserveCommittedWork`: Injection — throwing progress and diagnostics observers.
- `AssetRunTests::unreadableArchiveStopsRunBeforeMutation`: Partly transcribed — the ObserverFailed diagnostic of the injected throwing failure observer.

## CliConsoleInterruptTests

- `CliConsoleInterruptTests::consoleInterruptsRemainCooperative`: Dropped suite — Ctrl+C in a child console.

## CliExecutionTests

- `CliExecutionTests::reportsArgumentExitCodes`: Dropped suite — CLI argument parsing; no run happens.
- `CliExecutionTests::reportsAssetExecutionStatus [missing-animation-tool]`: Deviation 1 — depends on bin/hkxcmd.exe being absent beside the oracle; the harness provisions it, or does not run an Animation case.

## CliRunTests

- `CliRunTests::escapesFreeTextFields`: Dropped suite — the CLI's own event rendering and exit codes, with injected work.
- `CliRunTests::exitCodes`: Dropped suite — the CLI's own event rendering and exit codes, with injected work.
- `CliRunTests::rendersCommittedTerminalResult`: Dropped suite — the CLI's own event rendering and exit codes, with injected work.
- `CliRunTests::rendersFocusedTerminalEvidence`: Dropped suite — the CLI's own event rendering and exit codes, with injected work.
- `CliRunTests::rendersOrderedEvents`: Dropped suite — the CLI's own event rendering and exit codes, with injected work.
- `CliRunTests::rendersUnicodePathsAsUtf8`: Dropped suite — the CLI's own event rendering and exit codes, with injected work.
- `CliRunTests::waitsThroughCancellation`: Dropped suite — the CLI's own event rendering and exit codes, with injected work.

## DurableStagingTests

- `DurableStagingTests::archiveCleanupKeepsCommittedEntry`: Partly transcribed — an abandoned second entry and the receipt misuse after cleanup, which only the staging seam can produce.
- `DurableStagingTests::archivePublicationRejectsDeviceAlias`: Partly transcribed — the publication receipt's own check; a whole run rejects the names earlier, at Archive preflight.
- `DurableStagingTests::archivePublicationRejectsLinkedParent`: Partly transcribed — the receipt's own linked-parent check; a whole run refuses earlier, and a junction stands in for the directory symlink.
- `DurableStagingTests::archivePublicationRejectsUnsafeDestinations`: Partly transcribed — the publication receipt's own check; a whole run rejects the names earlier, at Archive preflight.
- `DurableStagingTests::archivePublicationSurvivesProducerTermination`: Partly transcribed — the StagingActive refusal while the producer is alive; the seed is the state the killed run leaves.
- `DurableStagingTests::assetPublicationRejectsChangedDestination`: Internal API — publishing to another destination than staged.
- `DurableStagingTests::assetPublicationRejectsInPlaceDestinationEdit`: Mid-run filesystem change — an in-place edit after capture.
- `DurableStagingTests::assetPublicationRejectsReplacedDestination`: Mid-run filesystem change — the destination is replaced after staging.
- `DurableStagingTests::assetPublicationRejectsReplacedParent`: Mid-run filesystem change — the parent folder is swapped after staging.
- `DurableStagingTests::cancelledPreparationPreservesDurableSibling`: Cancellation — a stop requested before preparation.
- `DurableStagingTests::cleanRootCannotBeRenamedDuringRecoveryScope`: Concurrency — renames the pinned Mod Root during recovery.
- `DurableStagingTests::cleanupContinuesAfterADamagedTemporary`: Mid-run filesystem change — a registered temporary becomes a folder mid-run.
- `DurableStagingTests::cleanupRemovesTheRunChildAndKeepsCommittedOutput`: Partly transcribed — a second, uncommitted receipt in the same run, which only the staging seam can produce.
- `DurableStagingTests::destinationSnapshotRejectsOpenRenameHandle`: Concurrency — a foreign open handle on the destination.
- `DurableStagingTests::expiredPublicationScopeCannotPublish`: Internal API — a receipt outliving its registry.
- `DurableStagingTests::genericCommitCannotReleaseDurableStage`: Internal API — the registry's generic commit.
- `DurableStagingTests::killedAfterPublicationKeepsDestination`: Partly transcribed — the StagingActive refusal while the killed producer still holds the lock; the seed is the state the killed run leaves.
- `DurableStagingTests::nestedStagedParentsStayPinnedThroughSafetyCleanup`: Concurrency — renames pinned parents during cleanup, in every row.
- `DurableStagingTests::occupiedDestinationIsNotPublished`: Mid-run filesystem change — the destination appears after staging.
- `DurableStagingTests::publicationPoliciesPreserveDestinationBytes`: Partly transcribed — the receipt states behind each publication policy, which only the staging seam can produce.
- `DurableStagingTests::publicationReceiptIsMoveOnlyAndOneUse`: Internal API — C++ move semantics and a second publish.
- `DurableStagingTests::publicationReleaseFailurePreservesCommittedDestination`: Partly transcribed — the PublishedStillOwned receipt and the contender refused while it is held; the seed is the state the killed run leaves.
- `DurableStagingTests::unavailableStagedFileConsumesReceipt`: Internal API — receipt consumption after the staged file is deleted.
- `DurableStagingTests::uppercaseDdsDestinationUsesRecoverableSibling`: Partly transcribed — the producer's lowercase sibling extension, seen only inside its seam; the seed is the state the killed run leaves.

## GuiRunDispatchTests

- `GuiRunDispatchTests::closeWaitsForCleanupAndTerminalDelivery`: Dropped suite — Qt queued dispatch.
- `GuiRunDispatchTests::deliveryIsAlwaysQueued`: Dropped suite — Qt queued dispatch.
- `GuiRunDispatchTests::destroyedTargetDiscardsQueuedDelivery`: Dropped suite — Qt queued dispatch.
- `GuiRunDispatchTests::observerDestructionStopsTheCurrentDrain`: Dropped suite — Qt queued dispatch.
- `GuiRunDispatchTests::workerDeliveryRunsOnTargetThread`: Dropped suite — Qt queued dispatch.

## GuiRunTests

- `GuiRunTests::defersCloseUntilTerminal`: Internal API — the RunViewModel close and cancel state machine.
- `GuiRunTests::presentsAuthoritativeProgress`: Internal API — RunViewModel progress presentation from synthetic events.
- `GuiRunTests::presentsTerminalClassification`: Injection — a fake work service and mid-work stop source, read through RunViewModel.
- `GuiRunTests::presentsTerminalEvidence`: Injection — evidence-only work and failing cleanup fakes, read through RunViewModel.
- `GuiRunTests::preservesUnicodePaths`: Internal API — RunViewModel detail formatting.
- `GuiRunTests::rejectsStaleObservations`: Internal API — RunViewModel filtering by Run ID and sequence.

## MainOptimizerTests

- `MainOptimizerTests::archiveSourceCleanupRequiresSuccessfulMerge`: Partly transcribed — the blocked-delete row's held handle becomes a read-only source.
- `MainOptimizerTests::archiveSourceCleanupRequiresSuccessfulMerge [blocked-backup]`: Held lock — a handle denying the backup rename; a read-only attribute does not block a rename.
- `MainOptimizerTests::archiveSourceLinkIsNotCleaned`: Partly transcribed — the C++ test hands the linked Archive to the optimizer directly; a whole run excludes the link at discovery.
- `MainOptimizerTests::committedConversionWithRetainedSourceMaintainsMeshReferences`: Partly transcribed — its held handle becomes a read-only source.
- `MainOptimizerTests::emptyDirectoryCleanupPreservesStaging`: Internal API — pruning's staging guard; in a whole run recovery or the run's own pinned staging decides first.
- `MainOptimizerTests::extractedSourceReplacementIsNotCleaned`: Mid-run filesystem change — the Archive is replaced between extraction and cleanup, in every row.
- `MainOptimizerTests::failedQuarantineMakesLoadFailureUnsafe`: Held lock — a handle denies the quarantine rename.
- `MainOptimizerTests::forcedCpuBlockCompressionSkipsDevice`: Internal API — the oracle's CPU-encoding override, read through the optimizer.
- `MainOptimizerTests::nestedTextureUsesSelectedModRoot`: Partly transcribed — the C++ test fails at staging creation inside the optimizer; a whole run fails the same check earlier, in Preparing.
- `MainOptimizerTests::optimizerInitializationObservesCancellation`: Cancellation.
- `MainOptimizerTests::packedSourcePinPreservesReplacement`: Mid-run filesystem change — the packed source is replaced under its pin.
- `MainOptimizerTests::pluginListingObservesCancellation`: Cancellation.
- `MainOptimizerTests::pluginListingUsesProfileExtensions`: Partly transcribed — the C++ test lists plugins directly, including FNV's extensions and a .txt holding plugin bytes, which the recipe cannot write; the seed observes the listing through headpart recognition.
- `MainOptimizerTests::textureDryRunMatchesApply`: Partly transcribed — B8G8R8A8 is no GUI output format, so the seeds use R8G8B8A8; the resize rows use an 8x8 target, so one case holds both; comparing each Dry Run's verdict with its Apply is a check across two cases.

## MeshReferenceMaintenanceTests

- `MeshReferenceMaintenanceTests::eligibilityFilterWithholdsRejectedReferences`: Partly transcribed — the C++ test injects the eligibility filter; the seed withholds through a real failed conversion.

## OptimizationRunServiceTests

- `OptimizationRunServiceTests::aModSelectionWithoutADirectoryIsRejectedWithoutCreatingAWorker`: Internal API — an empty selection directory, which no GUI or oracle request carries.
- `OptimizationRunServiceTests::aRejectedStartExposesAStartErrorInsteadOfAHandle`: Internal API — the shape of the start result.
- `OptimizationRunServiceTests::aRequestWithoutAProfileIdentityIsRejectedWithoutCreatingAWorker`: Internal API — an empty profile identity, which no GUI or oracle request carries.
- `OptimizationRunServiceTests::aRunHandleIsMovableButNotCopyable`: Internal API — C++ move and copy semantics.
- `OptimizationRunServiceTests::aSchedulerThatCannotStartAWorkerCommitsAFailedRun`: Injection — an exhausted scheduler throws.
- `OptimizationRunServiceTests::aSecondActiveRunIsRejectedAcrossServices`: Concurrency — a second run while the first is active.
- `OptimizationRunServiceTests::aValidStartSchedulesOneWorkerAndReturnsAHandle`: Internal API — scheduler count and handle shape.
- `OptimizationRunServiceTests::cancellationFromASkippedPhaseStopsFurtherTraversal`: Cancellation — requested from a skipped phase's event.
- `OptimizationRunServiceTests::cancellationFromAnExclusionStopsPreparing`: Cancellation — a diagnostic observer requests cancellation.
- `OptimizationRunServiceTests::cancellationKeepsTheSlotUntilTerminalCommit`: Cancellation — the active-run slot under cancellation.
- `OptimizationRunServiceTests::concurrentCancellationAndSnapshotsPreserveRunState`: Concurrency — threads race cancellation against snapshot reads.
- `OptimizationRunServiceTests::concurrentReadersObservePublishedPhaseTransitions`: Concurrency — barrier-synchronised reader threads.
- `OptimizationRunServiceTests::configurationLoadingFailuresAreTerminalFailures`: Injection — a missing or throwing configuration provider.
- `OptimizationRunServiceTests::configurationLoadsInsidePreparingOnTheWorker`: Concurrency — asserts the worker thread and provider ordering.
- `OptimizationRunServiceTests::destroyingAnActiveHandleJoinsItsWorker`: Injection — a deferred scheduler; the handle destructor joins.
- `OptimizationRunServiceTests::destroyingTheHandleRequestsCancellationBeforeJoin`: Cancellation — handle teardown under a deferred scheduler.
- `OptimizationRunServiceTests::destroyingTheServiceCancelsAndJoinsARetainedHandle`: Cancellation — service teardown cancels the run.
- `OptimizationRunServiceTests::destructionFromTheWorkerIsDiagnosed`: Injection — a child-process self-destruction harness.
- `OptimizationRunServiceTests::failedPreparationExposesNoResolvedFactsAfterProducersExpire`: Injection — no work service is wired; checks provider lifetime.
- `OptimizationRunServiceTests::failureEventsPrecedeCleanupAndTerminal`: Injection — the failures come from an unwired work service and an exhausted scheduler.
- `OptimizationRunServiceTests::failureRetainsObservedCancellationAndOneTerminalEvent`: Cancellation — requested from the failure observer, with a throwing provider.
- `OptimizationRunServiceTests::inlineCallbacksObservePublishedStateAndCanCancel`: Cancellation — cancels from the Preparing callback.
- `OptimizationRunServiceTests::lateCancellationDoesNotChangeCommittedEvidence`: Cancellation — requested after the terminal commit.
- `OptimizationRunServiceTests::moveAssigningOverAnActiveHandleJoinsTheReplacedRun`: Injection — C++ move assignment through a deferred scheduler.
- `OptimizationRunServiceTests::movingAHandleTransfersTheJoinObligationExactlyOnce`: Injection — C++ move semantics through a deferred scheduler.
- `OptimizationRunServiceTests::policyConflictsAreTerminalFailures`: Partly transcribed — no shipped profile disables Textures, which the C++ test injects; the seed conflicts on FO4 Meshes instead.
- `OptimizationRunServiceTests::presentationFailuresDisableOnlyTheFailingObserver`: Injection — throwing observer and dispatcher callbacks.
- `OptimizationRunServiceTests::queuedEventsOutliveTheRunOwners`: Injection — an adversarial dispatcher replays queued deliveries.
- `OptimizationRunServiceTests::schedulingFailureReleasesTheActiveSlot`: Injection — an exhausted scheduler.
- `OptimizationRunServiceTests::severalModsCanPrepareNoSelectedRoots`: Partly transcribed — the seeds use the shipped ignore list (`Nemesis`) where the C++ test injects its own.
- `OptimizationRunServiceTests::severalModsDiagnosesIgnoredChildrenOnce`: Partly transcribed — the injected list with duplicate entries and the non-ASCII case fold of `Āssets` needs an ignore list no shipped profile has.
- `OptimizationRunServiceTests::severalModsRejectsDuplicateLinkedRoots [symlink]`: Internal API — directory symlinks are no seed shape; the junction row covers the same scenario.
- `OptimizationRunServiceTests::severalModsUsesConfiguredSeparatorExclusions`: Partly transcribed — the custom `SECTION` and empty markers are an injected configuration, and the `parent_separator` parent and the One Mod run on `a_separator` are deviation 20 triggers.
- `OptimizationRunServiceTests::simultaneousStartsAdmitExactlyOneRun`: Concurrency — racing starts behind a barrier.
- `OptimizationRunServiceTests::snapshotsArePublishedBeforeDispatch`: Injection — a throwing observer and a snapshot-capturing dispatcher.
- `OptimizationRunServiceTests::terminalCommitReleasesTheSlotBeforeWorkerJoin`: Concurrency — semaphores hold the worker epilogue.
- `OptimizationRunServiceTests::terminalConstructionPreservesChosenOutcome`: Internal API — calls `OptimizationRunResult::terminal` directly.
- `OptimizationRunServiceTests::terminalQueryReportsAnActiveRunAsUncommitted`: Injection — a deferred scheduler holds the run active.
- `OptimizationRunServiceTests::terminalResultExposesSealedEvidenceViews`: Injection — fake work and a throwing cleanup service.
- `OptimizationRunServiceTests::terminalResultSharesSealedEvidenceAcrossObservationPaths`: Injection — a fake work service makes up attempts, collisions and cancellation.
- `OptimizationRunServiceTests::theDefaultServiceSchedulesAProductionRun`: Internal API — constructs the default scheduler with an injected provider.
- `OptimizationRunServiceTests::theRunOwnsItsRequestAfterStartReturns`: Internal API — request lifetime under the threaded scheduler.
- `OptimizationRunServiceTests::waitingFromTheWorkerIsDiagnosed`: Injection — a worker-side wait hook.
- `OptimizationRunServiceTests::waitingRepeatedlyObservesTheSameCommittedResult`: Internal API — object identity across repeated waits.
- `OptimizationRunServiceTests::workExceptionRetainsEarlierEvidence`: Injection — the fake work service records an extraction, then throws.

## OptionsCAOTests

- `OptionsCAOTests::archiveOptionsAcceptExplicitValues`: Dropped suite — CLI argument parsing.
- `OptionsCAOTests::omittedArchiveOptionsKeepDefaults`: Dropped suite — CLI argument parsing.
- `OptionsCAOTests::omittedResizeRatiosDefaultToOne`: Dropped suite — CLI argument parsing.
- `OptionsCAOTests::rejectsInvalidArchiveOptionValues`: Dropped suite — CLI argument parsing.
- `OptionsCAOTests::rejectsInvocationWithoutThreePositionalArguments`: Dropped suite — CLI argument parsing.
- `OptionsCAOTests::rejectsNonNumericMeshOptimizationLevel`: Dropped suite — CLI argument parsing.
- `OptionsCAOTests::rejectsUnknownProfile`: Dropped suite — CLI argument parsing.
- `OptionsCAOTests::rejectsZeroResizeDimensions`: Dropped suite — CLI argument parsing.
- `OptionsCAOTests::resizeOptionsAcceptValues`: Dropped suite — CLI argument parsing.

## RunEventDeliveryTests

- `RunEventDeliveryTests::blockedTerminalObserverCanRestartWhileAnotherThreadWaits`: Dropped suite — the serialized C++ Run Event dispatcher.
- `RunEventDeliveryTests::concurrentDispatcherPreservesSerializedSequences`: Dropped suite — the serialized C++ Run Event dispatcher.
- `RunEventDeliveryTests::dispatcherThatDeliversThenThrowsIsDiagnosedOnce`: Dropped suite — the serialized C++ Run Event dispatcher.
- `RunEventDeliveryTests::dispatcherThatQueuesThenThrowsDisablesDelivery`: Dropped suite — the serialized C++ Run Event dispatcher.
- `RunEventDeliveryTests::queuedFailureAfterWaitPreservesTheResult`: Dropped suite — the serialized C++ Run Event dispatcher.
- `RunEventDeliveryTests::rejectedStartsEmitNoEvents`: Dropped suite — the serialized C++ Run Event dispatcher.
- `RunEventDeliveryTests::releasedDeliveryDisablesItsObserver`: Dropped suite — the serialized C++ Run Event dispatcher.
- `RunEventDeliveryTests::terminalObserverStartsTheNextRun`: Dropped suite — the serialized C++ Run Event dispatcher.
- `RunEventDeliveryTests::waitIncludesTerminalDispatcherAdmission`: Dropped suite — the serialized C++ Run Event dispatcher.

## RunEvidenceTests

- `RunEvidenceTests::cancellationObservationIsSealedIndependently`: Internal API — drives the run evidence object directly with synthetic records.
- `RunEvidenceTests::finalizationAndCleanupFailuresRemainSeparate`: Internal API — drives the run evidence object directly with synthetic records.
- `RunEvidenceTests::finalizationAttemptsRequireRecordedTotal`: Internal API — drives the run evidence object directly with synthetic records.
- `RunEvidenceTests::finalizationProgressPublishesRetainedAttempt`: Internal API — drives the run evidence object directly with synthetic records.
- `RunEvidenceTests::finalizationResultCannotDropStreamedAttempts`: Internal API — drives the run evidence object directly with synthetic records.
- `RunEvidenceTests::interruptedFinalizationRetainsCompletedAttempts`: Internal API — drives the run evidence object directly with synthetic records.
- `RunEvidenceTests::laterPhaseRecordsReplaceWithoutReordering`: Internal API — drives the run evidence object directly with synthetic records.
- `RunEvidenceTests::liveFactsAreRetainedBeforePublication`: Internal API — drives the run evidence object directly with synthetic records.
- `RunEvidenceTests::phaseOrderViolationsAreRejected`: Internal API — drives the run evidence object directly with synthetic records.
- `RunEvidenceTests::pluginOnlyFinalizationMutationsAreSealed`: Internal API — drives the run evidence object directly with synthetic records.
- `RunEvidenceTests::preparationAndPostConsumptionMutationAreRejected`: Internal API — drives the run evidence object directly with synthetic records.
- `RunEvidenceTests::progressRegressionsAreRejected`: Internal API — drives the run evidence object directly with synthetic records.
- `RunEvidenceTests::sealedMutationSummariesReflectCompletedAttempts`: Internal API — drives the run evidence object directly with synthetic records.
- `RunEvidenceTests::successfulPreparationIsAtomicallyOwned`: Internal API — drives the run evidence object directly with synthetic records.
- `RunEvidenceTests::throwingPublicationIsClaimedOnce`: Injection — a throwing observation sink, in every row.
- `RunEvidenceTests::unfinishedPreparationRemainsAbsent`: Internal API — drives the run evidence object directly with synthetic records.

## RunExecutorTests

- `RunExecutorTests::aNonDirectorySelectionFailsPreparing`: Internal API — selects a file; a case's selection is always a folder.
- `RunExecutorTests::aRunThatStopsEarlyRecordsOnlyThePhasesItTraversed`: Injection — the early stop comes from a missing work service; the Preparing-failure seeds show the same two phases.
- `RunExecutorTests::activeStagingBlocksUntilItsOwnerExits`: Held lock — another process holds owner.lock.
- `RunExecutorTests::archiveFinalizationCancellationRetainsCommittedOutput`: Cancellation — a fake finalizer reports cancellation.
- `RunExecutorTests::archivePrecedenceIntentIsRetained`: Explicit Archive Precedence — and reads the request object.
- `RunExecutorTests::artifactRegistrationRejectsUnownedPaths`: Internal API — the temporary registry's registration checks.
- `RunExecutorTests::assetWorkPhasesPrecedeAttempts`: Partly transcribed — the ordering of injected callbacks against phase publication is not observable from a run.
- `RunExecutorTests::broadLinkedChildFailsPreparing [volume-root]`: Internal API — a link to the volume root reaches outside the case tree, which no recipe can target.
- `RunExecutorTests::cancellationAfterAtomicAssetAttempt`: Cancellation — with injected Asset work, in every row.
- `RunExecutorTests::cancellationDuringPreparingPublishesNoPreparation`: Cancellation — the provider requests stop while loading.
- `RunExecutorTests::cancelledRecoveryPreservesUnattemptedArtifacts`: Cancellation — calls staging recovery with a stopped token.
- `RunExecutorTests::cleanupDoesNotFollowAReplacedParent`: Internal API — registry registrations, then a symlink swap.
- `RunExecutorTests::cleanupExceptionsPreserveCancellation`: Injection — the cleanup service throws under cancellation.
- `RunExecutorTests::cleanupFailuresAreAggregatedWithoutDeletingRetainedMaterial`: Internal API — registers artifacts with the temporary registry directly.
- `RunExecutorTests::cleanupFailuresPreserveThePrimaryOutcome`: Internal API — the temporary registry under injected failures.
- `RunExecutorTests::cleanupServiceExceptionsAreTerminal`: Injection — the cleanup service throws.
- `RunExecutorTests::committedExtractionSurvivesDiscoveryInterruption [cancelled-after-commit]`: Cancellation — through the isCancelled hook after the extraction commits.
- `RunExecutorTests::committedExtractionSurvivesDiscoveryInterruption [throw-after-commit]`: Injection — through the isCancelled hook after the extraction commits.
- `RunExecutorTests::configurationLoadingFailuresAreTerminal`: Injection — a null or throwing configuration provider.
- `RunExecutorTests::determinateProgressStartsAtZeroAgainstAnImmutableTotal`: Pure table — the progress value type.
- `RunExecutorTests::discoveryDiagnosticCancellationFollowsAssetAttempt`: Cancellation — observers throw or request stop inside diagnostic callbacks, in every row.
- `RunExecutorTests::discoveryDiagnosticsSurviveInterruption`: Injection — isCancelled throws after a fake extraction.
- `RunExecutorTests::dryRunLeavesStagingUntouched`: Partly transcribed — the manifest's unchanged modification time is ignored by the corpus, which compares bytes only.
- `RunExecutorTests::evidenceInvariantViolationsRemainProgrammingDefects`: Injection — a fake work service breaks an evidence invariant.
- `RunExecutorTests::failedAttemptsAdvanceCompletedProgress`: Pure table — the progress value type.
- `RunExecutorTests::fatalFailureRetainsConcurrentCancellation`: Cancellation — a null provider and a cancelling cleanup service.
- `RunExecutorTests::filesystemRootSelectionFailsPreparing`: Internal API — selects the volume root, outside any case tree.
- `RunExecutorTests::missingPlannedLoadingPluginFailsRunWithArchiveCommit`: Injection — a fake finalization result with an unsafe PluginCreationFailed.
- `RunExecutorTests::mixedExtractionAttemptsAdvanceProgress`: Injection — a faked extraction failure and a throwing progress observer.
- `RunExecutorTests::mixedWorkEvidenceOutlivesServices`: Partly transcribed — the faked finalization result with a WriteFailed Archive, and evidence outliving the C++ service objects.
- `RunExecutorTests::noWorkRunReportsTheSameReasonsInEveryExecutionMode`: Partly transcribed — comparing the two runs' records with each other is a test-side check; each run is a seed.
- `RunExecutorTests::policyConflictsFailPreparing`: INI hand-edit — a `.dds` Archive extension, which no shipped profile has.
- `RunExecutorTests::preparingDoesNotMutateAssetsOrArchives`: Injection — no work service is wired, which a real run always has.
- `RunExecutorTests::preparingRetainsTheResolvedRootAndPolicy`: Internal API — reads the preparation object from a fake provider.
- `RunExecutorTests::processingAndEvidenceShareModRoot [several-relative]`: Internal API — a selection relative to the working directory; both builds are always given an absolute folder.
- `RunExecutorTests::processingAndEvidenceShareModRoot [single-relative]`: Internal API — a selection relative to the working directory; both builds are always given an absolute folder.
- `RunExecutorTests::productionWorkApplicability [dry-run-evaluation]`: Deviation 3 — a Dry Run requesting Archive extraction, which the GUI forbids; its Dry Run skip reasons are a Rust test.
- `RunExecutorTests::productionWorkApplicability [empty-dry-run]`: Deviation 3 — a Dry Run requesting Archive extraction, which the GUI forbids; its Dry Run skip reasons are a Rust test.
- `RunExecutorTests::productionWorkRetainsArchiveCollisions`: Partly transcribed — the explicit Archive Precedence and the throwing collision observer behind its ObserverFailed diagnostic.
- `RunExecutorTests::recoveryLockSurvivesThroughSafetyCleanup`: Held lock — an injected cleanup service starts a nested run under the lock.
- `RunExecutorTests::registeredArtifactsAreCleanedAndCommittedOutputSurvives`: Internal API — registers artifacts with the temporary registry directly.
- `RunExecutorTests::registeredArtifactsAreCleanedOnEveryTerminalPath`: Internal API — the temporary registry under injected terminal paths.
- `RunExecutorTests::removedConversionRetainsModRoot`: Injection — a fake conversion deletes the source, then a hook throws or cancels.
- `RunExecutorTests::requestedWorkIsADeduplicatedClosedSetInEnumerationOrder`: Internal API — request construction.
- `RunExecutorTests::retargetedAssetUsesCanonicalContainment`: Mid-run filesystem change — the Asset becomes a symlink inside a phase hook.
- `RunExecutorTests::safetyCleanupRunsExactlyOnceBeforeTheTerminalResult`: Partly transcribed — the invocation count comes from an injected cleanup counter.
- `RunExecutorTests::safetyCleanupRunsOnEveryTerminalPath`: Injection — no work service and a counting cleanup service.
- `RunExecutorTests::schedulingFailureRetainsOrderedCleanupEvidence`: Internal API — calls the executor's scheduling-failure path directly.
- `RunExecutorTests::terminalPrecedenceRetainsAllEvidence`: Injection — a table of fake work and cleanup outcomes.
- `RunExecutorTests::terminalResultOwnsItsDataAfterTheRunEnds`: Internal API — C++ result lifetime; its facts are the no-work seed's.
- `RunExecutorTests::throwingPreflightFailureObserverRetainsEvidence`: Injection — the failure observer throws.
- `RunExecutorTests::throwingWorkObserversRetainEvidence`: Injection — the progress observer throws and the Asset failure is faked, in every row.
- `RunExecutorTests::workArtifactsShareRecoveryAndAreCleanedAfterFailure [throw-after-commit]`: Injection — isCancelled throws after the commit.
- `RunExecutorTests::workPreparationFailurePreservesStaleArtifacts`: Injection — the work service's prepare throws.

## RunSetupTests

- `RunSetupTests::eachApplicationChoiceMapsToOnlyItsRequestedWork`: Pure table — single choices such as conversion-only are unreachable from the GUI.
- `RunSetupTests::everyConflictVariantHasStructuredPresentation`: Pure table — conflict message strings.
- `RunSetupTests::invalidSetupReturnsEveryStructuredConflict`: Partly transcribed — the missing-extension and derived-operation conflicts need profiles no GUI can produce.
- `RunSetupTests::unsupportedArchiveCreationIsRejected`: INI hand-edit — `bsaEnabled=false`.
- `RunSetupTests::validPolicyRemainsAvailableForLaterRouting`: Internal API — policy copy and lifetime.
- `RunSetupTests::validSetupCarriesEveryRequestedRoutingFact`: Partly transcribed — Archive extraction and creation in the C++ Dry Run request are deviation 3, and the compiled policy's flags are read through the seam.
