#include "AssetRun.h"

#include "ArchiveFirstAssetDiscovery.h"
#include "RunExecutor.h"

#include <array>
#include <stdexcept>
#include <utility>

namespace cao::run {
void executeAssetRun(const RunPreparation& preparation, RunWorkEvidence& evidence,
                     RunWorkMilestones& milestones,
                     std::stop_token stop, const AssetRunAdapters& operations) {
    // Cancellation and observation adaptation borrow only this synchronous work call.
    AssetRunAdapters adapters = operations;
    adapters.isCancelled = [&] {
        return stop.stop_requested() || (operations.isCancelled && operations.isCancelled());
    };
    AssetRun(preparation.policy())
        .execute(preparation.modRoots(), evidence, adapters, preparation.archivePrecedence(),
                 milestones);
}

AssetRunDiagnostics::AssetRunDiagnostics(const RunWorkEvidence& evidence) noexcept
    : _evidence(evidence) {}

std::size_t AssetRunDiagnostics::skippedAssetCount(
    const routing::SkipReason reason) const noexcept {
    return _evidence.skippedAssetCount(reason);
}

std::span<const std::filesystem::path> AssetRunDiagnostics::unsupportedExplicitPaths()
    const noexcept {
    const auto* discovery = _evidence.archiveDiscovery();
    return discovery ? discovery->unsupportedExplicitPaths()
                     : std::span<const std::filesystem::path>{};
}

std::size_t AssetRunDiagnostics::nestedArchiveCount() const noexcept {
    const auto* discovery = _evidence.archiveDiscovery();
    return discovery ? discovery->nestedArchiveCount() : 0;
}

std::span<const RunDiagnostic> AssetRunDiagnostics::diagnostics() const noexcept {
    return _evidence.diagnostics();
}

AssetRun::AssetRun(routing::RoutingPolicy policy) noexcept : _policy(std::move(policy)) {}

void AssetRun::execute(const std::span<const std::filesystem::path> roots,
                       RunWorkEvidence& evidence, const AssetRunAdapters& adapters,
                       const ArchivePrecedence& precedence,
                       RunWorkMilestones& milestones) const {
    // Freeze scopes before adapters can remove files or retarget selected directory aliases.
    std::vector<std::filesystem::path> modRoots;
    for (const auto& root : roots) {
        std::error_code error;
        auto scope = std::filesystem::is_directory(root, error) ? root : root.parent_path();
        if (scope.empty()) scope = ".";
        auto resolved = std::filesystem::weakly_canonical(scope, error);
        modRoots.push_back(error ? scope : std::move(resolved));
    }
    const ArchiveFirstAssetDiscovery discovery(_policy);
    bool cancelled = false;
    bool unsafeArchive = false;
    std::vector<ArchiveExtractionPlan> extractionPlans;
    const auto publishDiagnostics = [&] { evidence.publishDiagnostics(); };
    const auto reportWorkPhaseToAdapter = [&](const RunPhaseRecord& phase) {
        if (adapters.reportPhase)
            evidence.reportSafely(phase.phase(), [&] { adapters.reportPhase(phase); });
    };
    reportWorkPhaseToAdapter(milestones.archiveDiscoveryStarted());
    const auto discoveryResult = discovery.discover(
        roots,
        [&](const std::span<const routing::RoutedAsset> archives) {
            std::size_t completed = 0;
            for (const auto& archive : archives) {
                // An in-flight extraction must finish so cancellation cannot leave a partial
                // Archive.
                if (adapters.isCancelled && adapters.isCancelled()) {
                    cancelled = true;
                    break;
                }
                const auto& plan = extractionPlans.at(completed);
                ArchiveExtractionResult attempt;
                try {
                    attempt = adapters.extractArchiveWithResult(plan);
                } catch (const std::exception& error) {
                    // An adapter exception carries no trustworthy durable mutation evidence.
                    attempt = {archive.executionPath(), execution::MutationState::PartialOrUnknown,
                               ArchiveExtractionFailure::ExtractionFailed, false, error.what()};
                } catch (...) {
                    // Unknown exceptions must obey the same stop rule as typed backend errors.
                    attempt = {archive.executionPath(), execution::MutationState::PartialOrUnknown,
                               ArchiveExtractionFailure::ExtractionFailed, false,
                               "Unknown Archive extraction exception."};
                }
                // Partial mutation is independently unsafe even if an adapter mistakenly
                // claims continuation; retain its original evidence for terminal diagnosis.
                unsafeArchive = !attempt.safeToContinue ||
                                attempt.mutation == execution::MutationState::PartialOrUnknown;
                attempt.modRoot = plan.modRoot;
                evidence.recordArchiveExtractionAttempt(std::move(attempt), archives.size());
                ++completed;
                reportWorkPhaseToAdapter(*evidence.currentPhase());
                if (adapters.reportProgress) {
                    evidence.reportSafely(RunPhase::ExtractingArchives, [&] {
                        adapters.reportProgress(
                            AssetRunProgress{routing::RoutedAssetPhase::ArchiveExtraction,
                                             completed, archives.size()});
                    });
                }
                if (unsafeArchive) {
                    cancelled = adapters.isCancelled && adapters.isCancelled();
                    break;
                }
                // Re-sample after the in-flight extraction and its progress callback so
                // cancellation during the final Archive can stop discovery before the definitive
                // tree traversal.
                if (adapters.isCancelled && adapters.isCancelled()) {
                    cancelled = true;
                    break;
                }
            }
            return !cancelled && !unsafeArchive;
        },
        adapters.isCancelled, precedence,
        [&](std::span<const ArchiveCollision> collisions) {
            // Retain preflight evidence before presentation or later discovery can unwind.
            evidence.recordArchiveCollisions(collisions);
            if (adapters.reportArchiveCollisions)
                evidence.reportSafely(RunPhase::DiscoveringArchives,
                                      [&] { adapters.reportArchiveCollisions(collisions); });
        },
        [&](const std::span<const ArchiveExtractionPlan> plans) {
            extractionPlans.assign(plans.begin(), plans.end());
        },
        [&](RunPhase phase) {
            if (phase == RunPhase::ExtractingArchives &&
                _policy.executionMode() == routing::ExecutionMode::DryRun) {
                reportWorkPhaseToAdapter(milestones.dryRunArchiveExtraction());
                return;
            }
            if (phase == RunPhase::ExtractingArchives) {
                reportWorkPhaseToAdapter(
                    milestones.archiveExtractionPlanned(extractionPlans.size()));
            } else {
                reportWorkPhaseToAdapter(milestones.effectiveAssetTreeStarted());
            }
        },
        [&](const RunDiagnostic& diagnostic) {
            // Retain discovery evidence before later traversal can throw; publish after Assets.
            evidence.retainDiagnostic(diagnostic);
        });
    const routing::AssetRouter router(_policy);
    std::map<routing::SkipReason, std::size_t> skippedArchiveCounts;
    for (const auto reason :
         {routing::SkipReason::DisabledPhase, routing::SkipReason::DisabledAssetKind,
          routing::SkipReason::ExcludedAssetVariant}) {
        const auto count = discoveryResult.skippedArchiveCount(reason);
        if (count != 0) skippedArchiveCounts.emplace(reason, count);
    }
    evidence.recordArchiveDiscovery(ArchiveDiscoveryEvidence{
        std::move(skippedArchiveCounts),
        std::vector<std::filesystem::path>(discoveryResult.unsupportedExplicitPaths().begin(),
                                           discoveryResult.unsupportedExplicitPaths().end()),
        discoveryResult.nestedArchiveCount()});
    const bool discoveryCancelled = unsafeArchive ? cancelled : discoveryResult.cancelled();
    if (discoveryCancelled) evidence.recordCancellationObservation();
    for (const auto& failure : discoveryResult.failures()) evidence.recordFailure(failure);
    // Interrupted discovery can expose a partial tree but cannot promise a definitive ledger.
    if (!unsafeArchive && !discoveryResult.cancelled() && discoveryResult.failures().empty())
        evidence.recordRoutingLedger(router.route(discoveryResult.effectiveAssetTree().paths()));
    if (unsafeArchive) {
        // Discovery's legacy false callback means cancellation. Preserve only separately observed
        // cancellation for an unsafe mutation stop, and never reach Assets or finalization.
        publishDiagnostics();
        return;
    }
    if (!discoveryResult.failures().empty()) {
        // A failed preflight has no trustworthy tree and must never reach mutation or finalization.
        if (adapters.reportDiscoveryFailure)
            for (const auto& failure : discoveryResult.failures())
                evidence.reportSafely(failure.phase(),
                                      [&] { adapters.reportDiscoveryFailure(failure); });
        publishDiagnostics();
        return;
    }
    constexpr std::array targetOrder{routing::OptimizerTarget::Texture,
                                     routing::OptimizerTarget::Mesh,
                                     routing::OptimizerTarget::Animation};
    if (discoveryCancelled) {
        publishDiagnostics();
        return;
    }
    const auto* ledger = evidence.routingLedger();
    const auto total = ledger->routedAssets().size();
    reportWorkPhaseToAdapter(milestones.assetProcessingPlanned(total));
    std::size_t completed = 0;
    for (const auto target : targetOrder) {
        // Target queries preserve ledger-relative order, so only cross-target order changes.
        for (const auto asset : ledger->routedAssets(target)) {
            // An in-flight optimizer attempt must finish so cancellation cannot interrupt mutation.
            if (adapters.isCancelled && adapters.isCancelled()) {
                evidence.recordCancellationObservation();
                publishDiagnostics();
                return;
            }
            bool safeToContinue = true;
            // Resolve before the attempt can remove a converted source or retarget its parent.
            std::error_code pathError;
            const auto resolvedPath =
                std::filesystem::weakly_canonical(asset.get().executionPath(), pathError);
            const auto& attributionPath = pathError ? asset.get().executionPath() : resolvedPath;
            std::filesystem::path modRoot;
            for (const auto& root : modRoots) {
                const auto relative = attributionPath.lexically_relative(root);
                if (!relative.empty() && *relative.begin() != ".." &&
                    root.native().size() > modRoot.native().size())
                    modRoot = root;
            }
            auto attempt = execution::AssetExecutionResult::success();
            try {
                if (modRoot.empty())
                    throw std::logic_error("Routed Asset is outside prepared Mod Roots");
                attempt = adapters.executeAssetWithResult(asset.get(), modRoot);
            } catch (const AssetInitializationCancelled&) {
                // Initialization is read-only and ended before Asset execution, so recording an
                // attempted mutation here would invent evidence and block safe cancellation.
                evidence.recordCancellationObservation();
                publishDiagnostics();
                return;
            } catch (const std::exception& error) {
                // An exception cannot establish whether the adapter committed durable bytes.
                attempt = execution::AssetExecutionResult::failed(
                    execution::AssetExecutionFailure::BackendException, error.what(),
                    execution::MutationState::PartialOrUnknown, false, asset.get().executionPath());
            } catch (...) {
                // Unknown exceptions carry the same uncertain mutation as standard exceptions.
                attempt = execution::AssetExecutionResult::failed(
                    execution::AssetExecutionFailure::BackendException,
                    "Unknown Asset execution exception.",
                    execution::MutationState::PartialOrUnknown, false, asset.get().executionPath());
            }
            safeToContinue = attempt.safeToContinue();
            evidence.recordAssetAttempt({std::move(modRoot), asset.get(), std::move(attempt)},
                                        total);
            ++completed;
            reportWorkPhaseToAdapter(*evidence.currentPhase());
            if (adapters.reportProgress) {
                evidence.reportSafely(RunPhase::ProcessingAssets, [&] {
                    adapters.reportProgress(AssetRunProgress{
                        routing::RoutedAssetPhase::LooseAssetProcessing, completed, total});
                });
            }
            // A failed attempt still completes progress, but an uncertain mutation makes later
            // optimization and packing unsafe even when cancellation has not been requested.
            if (!safeToContinue) {
                if (adapters.isCancelled && adapters.isCancelled())
                    evidence.recordCancellationObservation();
                publishDiagnostics();
                return;
            }
        }
    }
    // Cancellation raised while the final attempt was in flight has no later loop head to observe
    // it, and a finalizer is not required to check cancellation itself, so the run would otherwise
    // report a cancelled attempt sequence as a completed run.
    if (adapters.isCancelled && adapters.isCancelled()) {
        evidence.recordCancellationObservation();
        publishDiagnostics();
        return;
    }
    publishDiagnostics();
    if (adapters.reportDiagnostics) {
        const AssetRunDiagnostics diagnostics(evidence);
        evidence.reportSafely(RunPhase::ProcessingAssets,
                              [&] { adapters.reportDiagnostics(diagnostics); });
    }
    // Reporting may request cancellation even when its exception was isolated; packing is a
    // separate mutation boundary and must observe that request before starting any finalizer.
    if (adapters.isCancelled && adapters.isCancelled()) {
        evidence.recordCancellationObservation();
        publishDiagnostics();
        return;
    }

    const auto hasFinalizer = static_cast<bool>(adapters.finalizeArchiveLifecycleWithResult);
    reportWorkPhaseToAdapter(
        milestones.archiveFinalizationAvailable(_policy.executionMode(), hasFinalizer));
    if (adapters.isCancelled && adapters.isCancelled()) {
        evidence.recordCancellationObservation();
        publishDiagnostics();
        return;
    }

    // The immutable policy is the run authority, so mismatched CLI or programmatic options cannot
    // re-enable Archive packing, creation, source deletion, or cleanup during Dry Run.
    if (_policy.executionMode() == routing::ExecutionMode::Apply &&
        adapters.finalizeArchiveLifecycleWithResult) {
        ArchiveFinalizationResult finalization;
        try {
            finalization = adapters.finalizeArchiveLifecycleWithResult();
        } catch (const RunEvidenceInvariantViolation&) {
            // A broken evidence contract is a programming defect; the executor still cleans up.
            throw;
        } catch (const std::exception& error) {
            // A thrown finalizer supplies no reliable boundary for its durable mutations.
            finalization = ArchiveFinalizationResult{
                {}, ArchiveFinalizationFailure::UnexpectedException, false, false, error.what()};
        } catch (...) {
            // Preserve a phase-level failure even when the adapter throws an untyped exception.
            finalization =
                ArchiveFinalizationResult{{},
                                          ArchiveFinalizationFailure::UnexpectedException,
                                          false,
                                          false,
                                          "Unknown Archive finalization exception."};
        }
        evidence.recordArchiveFinalization(std::move(finalization));
        if (adapters.isCancelled && adapters.isCancelled())
            evidence.recordCancellationObservation();
    }
    publishDiagnostics();
}
}  // namespace cao::run
