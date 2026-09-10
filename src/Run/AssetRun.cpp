#include "AssetRun.h"

#include "ArchiveFirstAssetDiscovery.h"
#include "RunExecutor.h"
#include "RunWorkRecord.h"

#include <array>
#include <stdexcept>
#include <utility>

namespace cao::run {
namespace {
/// Isolates synchronous presentation failures while retaining diagnostic evidence in the run.
template <typename Callback>
void reportSafely(RunWorkRecord& record, RunPhase phase,
                  Callback&& callback) {
    try {
        callback();
    } catch (const std::exception& error) {
        // Presentation cannot suppress a completed attempt or abandon pending finalization.
        record.diagnostics.emplace_back(RunDiagnosticCode::ObserverFailed, phase, error.what());
    } catch (...) {
        // Unknown observer exceptions have the same informational status as standard exceptions.
        record.diagnostics.emplace_back(RunDiagnosticCode::ObserverFailed, phase,
                                        "The observer threw a non-standard exception");
    }
}
}  // namespace

void executeAssetRun(const RunPreparation& preparation, RunWorkRecord& record,
                     RunObservationSink& observations, std::stop_token stop,
                     const AssetRunAdapters& operations) {
    // Cancellation and observation adaptation borrow only this synchronous work call.
    AssetRunAdapters adapters = operations;
    adapters.isCancelled = [&] {
        return stop.stop_requested() || (operations.isCancelled && operations.isCancelled());
    };
    AssetRun(preparation.policy()).execute(preparation.modRoots(), record, adapters,
                                          preparation.archivePrecedence(), &observations);
}

AssetRunDiagnostics::AssetRunDiagnostics(const RunWorkRecord& record) noexcept : _record(record) {}

std::size_t AssetRunDiagnostics::skippedAssetCount(
    const routing::SkipReason reason) const noexcept {
    const auto found = _record.skippedArchiveCounts.find(reason);
    return (_record.ledger ? _record.ledger->skippedAssetCount(reason) : 0) +
           (found == _record.skippedArchiveCounts.end() ? 0 : found->second);
}

std::span<const std::filesystem::path> AssetRunDiagnostics::unsupportedExplicitPaths()
    const noexcept {
    return _record.unsupportedExplicitPaths;
}

std::size_t AssetRunDiagnostics::nestedArchiveCount() const noexcept {
    return _record.nestedArchiveCount;
}

std::span<const RunDiagnostic> AssetRunDiagnostics::diagnostics() const noexcept {
    return _record.diagnostics;
}

AssetRun::AssetRun(routing::RoutingPolicy policy) noexcept : _policy(std::move(policy)) {}

void AssetRun::execute(const std::span<const std::filesystem::path> roots,
                       RunWorkRecord& record, const AssetRunAdapters& adapters,
                       const ArchivePrecedence& precedence,
                       RunObservationSink* observations) const {
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
    std::size_t archiveSucceeded = 0;
    std::size_t publishedDiagnostics = record.diagnostics.size();
    const auto publishDiagnostics = [&] {
        if (!observations) return;
        // Copy each borrowed diagnostic before presentation can append ObserverFailed evidence.
        // A throwing observer is attempted only once per flush, so publication cannot recurse.
        const auto end = record.diagnostics.size();
        while (publishedDiagnostics < end) {
            const auto diagnostic = record.diagnostics[publishedDiagnostics++];
            reportSafely(record, diagnostic.phase(), [&] {
                observations->publishRetainedDiagnostic(diagnostic);
            });
        }
        publishedDiagnostics = record.diagnostics.size();
    };
    const auto reportPhase = [&](const RunPhaseRecord& phase) {
        reportSafely(record, phase.phase(), [&] {
            if (observations) observations->recordPhase(phase);
            if (adapters.reportPhase) adapters.reportPhase(phase);
        });
    };
    reportPhase(RunPhaseRecord::executed(RunPhase::DiscoveringArchives));
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
                if (attempt.succeeded()) ++archiveSucceeded;
                record.archiveAttempts.push_back(std::move(attempt));
                ++completed;
                reportPhase(RunPhaseRecord::executed(RunPhase::ExtractingArchives,
                    RunProgress::determinate(archives.size(), archiveSucceeded,
                                             completed - archiveSucceeded)));
                if (adapters.reportProgress) {
                    reportSafely(record, RunPhase::ExtractingArchives, [&] {
                        adapters.reportProgress(AssetRunProgress{
                            routing::RoutedAssetPhase::ArchiveExtraction, completed, archives.size()});
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
            record.collisions.assign(collisions.begin(), collisions.end());
            if (adapters.reportArchiveCollisions)
                reportSafely(record, RunPhase::DiscoveringArchives, [&] {
                    adapters.reportArchiveCollisions(collisions);
                });
        },
        [&](const std::span<const ArchiveExtractionPlan> plans) {
            extractionPlans.assign(plans.begin(), plans.end());
        }, [&](RunPhase phase) {
            if (phase == RunPhase::ExtractingArchives &&
                _policy.executionMode() == routing::ExecutionMode::DryRun) {
                reportPhase(RunPhaseRecord::skipped(phase, PhaseSkipReason::DryRun));
                return;
            }
            reportPhase(RunPhaseRecord::executed(phase,
                phase == RunPhase::ExtractingArchives
                    ? std::optional{RunProgress::determinate(extractionPlans.size())}
                    : std::nullopt));
        });
    const routing::AssetRouter router(_policy);
    std::map<routing::SkipReason, std::size_t> skippedArchiveCounts;
    for (const auto reason :
         {routing::SkipReason::DisabledPhase, routing::SkipReason::DisabledAssetKind,
          routing::SkipReason::ExcludedAssetVariant}) {
        const auto count = discoveryResult.skippedArchiveCount(reason);
        if (count != 0) skippedArchiveCounts.emplace(reason, count);
    }
    record.skippedArchiveCounts = std::move(skippedArchiveCounts);
    record.unsupportedExplicitPaths.assign(discoveryResult.unsupportedExplicitPaths().begin(),
                                           discoveryResult.unsupportedExplicitPaths().end());
    record.nestedArchiveCount = discoveryResult.nestedArchiveCount();
    record.cancellationObserved = unsafeArchive ? cancelled : discoveryResult.cancelled();
    record.diagnostics.insert(record.diagnostics.end(), discoveryResult.diagnostics().begin(),
                               discoveryResult.diagnostics().end());
    for (const auto& failure : discoveryResult.failures()) {
        reportSafely(record, failure.phase(), [&] {
            if (observations) observations->recordFailure(failure);
            else record.failures.push_back(failure);
        });
    }
    // Interrupted discovery can expose a partial tree but cannot promise a definitive ledger.
    if (!unsafeArchive && !discoveryResult.cancelled() && discoveryResult.failures().empty())
        record.ledger = router.route(discoveryResult.effectiveAssetTree().paths());
    if (unsafeArchive) {
        // Discovery's legacy false callback means cancellation. Preserve only separately observed
        // cancellation for an unsafe mutation stop, and never reach Assets or finalization.
        record.cancellationObserved = cancelled;
        publishDiagnostics();
        return;
    }
    if (!discoveryResult.failures().empty()) {
        // A failed preflight has no trustworthy tree and must never reach mutation or finalization.
        if (adapters.reportDiscoveryFailure)
            for (const auto& failure : discoveryResult.failures())
                reportSafely(record, failure.phase(), [&] {
                    adapters.reportDiscoveryFailure(failure);
                });
        publishDiagnostics();
        return;
    }
    constexpr std::array targetOrder{routing::OptimizerTarget::Texture,
                                     routing::OptimizerTarget::Mesh,
                                     routing::OptimizerTarget::Animation};
    if (record.cancellationObserved) {
        publishDiagnostics();
        return;
    }
    const auto total = (*record.ledger).routedAssets().size();
    reportPhase(RunPhaseRecord::executed(RunPhase::ProcessingAssets,
                                       RunProgress::determinate(total)));
    std::size_t completed = 0;
    std::size_t assetSucceeded = 0;
    for (const auto target : targetOrder) {
        // Target queries preserve ledger-relative order, so only cross-target order changes.
        for (const auto asset : (*record.ledger).routedAssets(target)) {
            // An in-flight optimizer attempt must finish so cancellation cannot interrupt mutation.
            if (adapters.isCancelled && adapters.isCancelled()) {
                record.cancellationObserved = true;
                publishDiagnostics();
                return;
            }
            bool safeToContinue = true;
            // Resolve before the attempt can remove a converted source or retarget its parent.
            std::error_code pathError;
            const auto resolvedPath = std::filesystem::weakly_canonical(
                asset.get().executionPath(), pathError);
            const auto& attributionPath = pathError ? asset.get().executionPath() : resolvedPath;
            std::filesystem::path modRoot;
            for (const auto& root : modRoots) {
                const auto relative = attributionPath.lexically_relative(root);
                if (!relative.empty() && *relative.begin() != ".." &&
                    root.native().size() > modRoot.native().size()) modRoot = root;
            }
            auto attempt = execution::AssetExecutionResult::success();
            try {
                if (modRoot.empty())
                    throw std::logic_error("Routed Asset is outside prepared Mod Roots");
                attempt = adapters.executeAssetWithResult(asset.get(), modRoot);
            } catch (const std::exception& error) {
                // An exception cannot establish whether the adapter committed durable bytes.
                attempt = execution::AssetExecutionResult::failed(
                    execution::AssetExecutionFailure::BackendException, error.what(),
                    execution::MutationState::PartialOrUnknown, false, asset.get().executionPath());
            } catch (...) {
                // Unknown exceptions carry the same uncertain mutation as standard exceptions.
                attempt = execution::AssetExecutionResult::failed(
                    execution::AssetExecutionFailure::BackendException,
                    "Unknown Asset execution exception.", execution::MutationState::PartialOrUnknown,
                    false, asset.get().executionPath());
            }
            safeToContinue = attempt.safeToContinue();
            if (attempt.succeeded()) ++assetSucceeded;
            record.assetAttempts.push_back({std::move(modRoot), asset.get(), std::move(attempt)});
            ++completed;
            reportPhase(RunPhaseRecord::executed(RunPhase::ProcessingAssets,
                RunProgress::determinate(total, assetSucceeded, completed - assetSucceeded)));
            if (adapters.reportProgress) {
                reportSafely(record, RunPhase::ProcessingAssets, [&] {
                    adapters.reportProgress(AssetRunProgress{
                        routing::RoutedAssetPhase::LooseAssetProcessing, completed, total});
                });
            }
            // A failed attempt still completes progress, but an uncertain mutation makes later
            // optimization and packing unsafe even when cancellation has not been requested.
            if (!safeToContinue) {
                record.cancellationObserved = adapters.isCancelled && adapters.isCancelled();
                publishDiagnostics();
                return;
            }
        }
    }
    // Cancellation raised while the final attempt was in flight has no later loop head to observe
    // it, and a finalizer is not required to check cancellation itself, so the run would otherwise
    // report a cancelled attempt sequence as a completed run.
    if (adapters.isCancelled && adapters.isCancelled()) {
        record.cancellationObserved = true;
        publishDiagnostics();
        return;
    }
    publishDiagnostics();
    if (adapters.reportDiagnostics) {
        const AssetRunDiagnostics diagnostics(record);
        reportSafely(record, RunPhase::ProcessingAssets, [&] {
            adapters.reportDiagnostics(diagnostics);
        });
    }
    // Reporting may request cancellation even when its exception was isolated; packing is a
    // separate mutation boundary and must observe that request before starting any finalizer.
    if (adapters.isCancelled && adapters.isCancelled()) {
        record.cancellationObserved = true;
        publishDiagnostics();
        return;
    }

    reportPhase(_policy.executionMode() == routing::ExecutionMode::DryRun
        ? RunPhaseRecord::skipped(RunPhase::ArchiveFinalization, PhaseSkipReason::DryRun)
        : (!adapters.finalizeArchiveLifecycleWithResult
            ? RunPhaseRecord::skipped(RunPhase::ArchiveFinalization, PhaseSkipReason::NoRequestedWork)
            : RunPhaseRecord::executed(RunPhase::ArchiveFinalization)));
    if (adapters.isCancelled && adapters.isCancelled()) {
        record.cancellationObserved = true;
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
        } catch (const std::exception& error) {
            // A thrown finalizer supplies no reliable boundary for its durable mutations.
            finalization = ArchiveFinalizationResult{
                {}, ArchiveFinalizationFailure::UnexpectedException, false, false, error.what()};
        } catch (...) {
            // Preserve a phase-level failure even when the adapter throws an untyped exception.
            finalization = ArchiveFinalizationResult{
                {}, ArchiveFinalizationFailure::UnexpectedException, false, false,
                "Unknown Archive finalization exception."};
        }
        record.finalizations.push_back(std::move(finalization));
        record.cancellationObserved = record.finalizations.back().cancelled ||
                            (adapters.isCancelled && adapters.isCancelled());
    }
    publishDiagnostics();
}
}  // namespace cao::run
