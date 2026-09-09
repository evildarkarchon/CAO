#include "AssetRun.h"

#include "ArchiveFirstAssetDiscovery.h"
#include "RunWorkRecord.h"

#include <array>
#include <utility>

namespace cao::run {
namespace {
/// Isolates synchronous presentation failures while retaining diagnostic evidence in the run.
template <typename Callback>
void reportSafely(std::vector<RunDiagnostic>& diagnostics, RunPhase phase, Callback&& callback) {
    try {
        callback();
    } catch (const std::exception& error) {
        // Presentation cannot suppress a completed attempt or abandon pending finalization.
        diagnostics.emplace_back(RunDiagnosticCode::ObserverFailed, phase, error.what());
    } catch (...) {
        // Unknown observer exceptions have the same informational status as standard exceptions.
        diagnostics.emplace_back(RunDiagnosticCode::ObserverFailed, phase,
                                 "The observer threw a non-standard exception");
    }
}
}  // namespace

RunWorkRecord AssetRunResult::workRecord() const {
    RunWorkRecord record;
    if (_routingCompleted) record.ledger = _ledger;
    record.assetAttempts = _assetAttempts;
    record.archiveAttempts = _archiveAttempts;
    if (_finalizationResult) record.finalizations.push_back(*_finalizationResult);
    record.skippedArchiveCounts = _skippedArchiveCounts;
    record.collisions = _collisions;
    record.diagnostics = _diagnostics;
    record.failures = _failures;
    record.cancellationObserved = _cancelled;
    return record;
}

AssetRunResult::AssetRunResult(routing::RoutingLedger ledger,
                               std::map<routing::SkipReason, std::size_t> skippedArchiveCounts,
                               std::vector<std::filesystem::path> unsupportedExplicitPaths,
                               const std::size_t nestedArchiveCount, const bool cancelled,
                               std::vector<RunDiagnostic> diagnostics,
                               std::vector<RunFailure> failures,
                               std::vector<ArchiveCollision> collisions) noexcept
    : _ledger(std::move(ledger)),
      _skippedArchiveCounts(std::move(skippedArchiveCounts)),
      _unsupportedExplicitPaths(std::move(unsupportedExplicitPaths)),
      _nestedArchiveCount(nestedArchiveCount),
      _cancelled(cancelled),
      _diagnostics(std::move(diagnostics)),
      _failures(std::move(failures)),
      _collisions(std::move(collisions)) {}

const routing::RoutingLedger& AssetRunResult::ledger() const noexcept { return _ledger; }

bool AssetRunResult::cancelled() const noexcept { return _cancelled; }

std::size_t AssetRunResult::skippedAssetCount(const routing::SkipReason reason) const noexcept {
    const auto archiveCount = _skippedArchiveCounts.find(reason);
    return _ledger.skippedAssetCount(reason) +
           (archiveCount == _skippedArchiveCounts.end() ? 0 : archiveCount->second);
}

std::span<const std::filesystem::path> AssetRunResult::unsupportedExplicitPaths() const noexcept {
    return _unsupportedExplicitPaths;
}

std::size_t AssetRunResult::nestedArchiveCount() const noexcept { return _nestedArchiveCount; }

std::span<const RunDiagnostic> AssetRunResult::diagnostics() const noexcept { return _diagnostics; }

AssetRunDiagnostics::AssetRunDiagnostics(const AssetRunResult& result) noexcept : _result(result) {}

std::size_t AssetRunDiagnostics::skippedAssetCount(
    const routing::SkipReason reason) const noexcept {
    return _result.skippedAssetCount(reason);
}

std::span<const std::filesystem::path> AssetRunDiagnostics::unsupportedExplicitPaths()
    const noexcept {
    return _result.unsupportedExplicitPaths();
}

std::size_t AssetRunDiagnostics::nestedArchiveCount() const noexcept {
    return _result.nestedArchiveCount();
}

std::span<const RunDiagnostic> AssetRunDiagnostics::diagnostics() const noexcept {
    return _result.diagnostics();
}

AssetRun::AssetRun(routing::RoutingPolicy policy) noexcept : _policy(std::move(policy)) {}

AssetRunResult AssetRun::execute(const std::span<const std::filesystem::path> roots,
                                 const AssetRunAdapters& adapters,
                                 const ArchivePrecedence& precedence) const {
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
    std::vector<ArchiveExtractionResult> archiveAttempts;
    std::vector<RunDiagnostic> observerDiagnostics;
    auto* phaseDiagnostics = &observerDiagnostics;
    const auto reportPhase = [&](const RunPhaseRecord& phase) {
        if (adapters.reportPhase)
            reportSafely(*phaseDiagnostics, phase.phase(), [&] { adapters.reportPhase(phase); });
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
                if (adapters.extractArchiveWithResult) {
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
                    archiveAttempts.push_back(std::move(attempt));
                } else {
                    adapters.extractArchive(archive);
                }
                ++completed;
                if (adapters.reportProgress) {
                    reportSafely(observerDiagnostics, RunPhase::ExtractingArchives, [&] {
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
            if (adapters.reportArchiveCollisions)
                reportSafely(observerDiagnostics, RunPhase::DiscoveringArchives, [&] {
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
    auto result = AssetRunResult(
        router.route(discoveryResult.effectiveAssetTree().paths()), std::move(skippedArchiveCounts),
        std::vector<std::filesystem::path>(discoveryResult.unsupportedExplicitPaths().begin(),
                                           discoveryResult.unsupportedExplicitPaths().end()),
        discoveryResult.nestedArchiveCount(), discoveryResult.cancelled(),
        std::vector<RunDiagnostic>(discoveryResult.diagnostics().begin(),
                                   discoveryResult.diagnostics().end()),
        std::vector<RunFailure>(discoveryResult.failures().begin(),
                                discoveryResult.failures().end()),
        std::vector<ArchiveCollision>(discoveryResult.collisions().begin(),
                                      discoveryResult.collisions().end()));
    result._archiveAttempts = std::move(archiveAttempts);
    result._diagnostics.insert(result._diagnostics.end(), observerDiagnostics.begin(),
                               observerDiagnostics.end());
    phaseDiagnostics = &result._diagnostics;
    // Interrupted discovery can expose a partial tree but cannot promise a definitive ledger.
    result._routingCompleted = !unsafeArchive && !discoveryResult.cancelled() &&
                               discoveryResult.failures().empty();
    if (unsafeArchive) {
        // Discovery's legacy false callback means cancellation. Preserve only separately observed
        // cancellation for an unsafe mutation stop, and never reach Assets or finalization.
        result._cancelled = cancelled;
        return result;
    }
    if (!result.failures().empty()) {
        // A failed preflight has no trustworthy tree and must never reach mutation or finalization.
        if (adapters.reportDiscoveryFailure)
            for (const auto& failure : result.failures())
                reportSafely(result._diagnostics, failure.phase(), [&] {
                    adapters.reportDiscoveryFailure(failure);
                });
        return result;
    }
    constexpr std::array targetOrder{routing::OptimizerTarget::Texture,
                                     routing::OptimizerTarget::Mesh,
                                     routing::OptimizerTarget::Animation};
    if (result.cancelled()) return result;
    const auto total = result.ledger().routedAssets().size();
    reportPhase(RunPhaseRecord::executed(RunPhase::ProcessingAssets,
                                       RunProgress::determinate(total)));
    std::size_t completed = 0;
    for (const auto target : targetOrder) {
        // Target queries preserve ledger-relative order, so only cross-target order changes.
        for (const auto asset : result.ledger().routedAssets(target)) {
            // An in-flight optimizer attempt must finish so cancellation cannot interrupt mutation.
            if (adapters.isCancelled && adapters.isCancelled()) {
                result._cancelled = true;
                return result;
            }
            bool safeToContinue = true;
            if (adapters.executeAssetWithResult) {
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
                    attempt = adapters.executeAssetWithResult(asset.get());
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
                if (!attempt.succeeded()) result._executionFailures.push_back(attempt);
                result._assetAttempts.push_back({std::move(modRoot), asset.get(), std::move(attempt)});
            } else {
                adapters.executeAsset(asset.get());
            }
            ++completed;
            if (adapters.reportProgress) {
                reportSafely(result._diagnostics, RunPhase::ProcessingAssets, [&] {
                    adapters.reportProgress(AssetRunProgress{
                        routing::RoutedAssetPhase::LooseAssetProcessing, completed, total});
                });
            }
            // A failed attempt still completes progress, but an uncertain mutation makes later
            // optimization and packing unsafe even when cancellation has not been requested.
            if (!safeToContinue) {
                result._cancelled = adapters.isCancelled && adapters.isCancelled();
                return result;
            }
        }
    }
    // Cancellation raised while the final attempt was in flight has no later loop head to observe
    // it, and a finalizer is not required to check cancellation itself, so the run would otherwise
    // report a cancelled attempt sequence as a completed run.
    if (adapters.isCancelled && adapters.isCancelled()) {
        result._cancelled = true;
        return result;
    }
    if (adapters.reportDiagnostics) {
        const AssetRunDiagnostics diagnostics(result);
        reportSafely(result._diagnostics, RunPhase::ProcessingAssets, [&] {
            adapters.reportDiagnostics(diagnostics);
        });
    }
    // Reporting may request cancellation even when its exception was isolated; packing is a
    // separate mutation boundary and must observe that request before starting any finalizer.
    if (adapters.isCancelled && adapters.isCancelled()) {
        result._cancelled = true;
        return result;
    }

    reportPhase(_policy.executionMode() == routing::ExecutionMode::DryRun
        ? RunPhaseRecord::skipped(RunPhase::ArchiveFinalization, PhaseSkipReason::DryRun)
        : (!adapters.finalizeArchiveLifecycleWithResult && !adapters.finalizeArchiveLifecycle
            ? RunPhaseRecord::skipped(RunPhase::ArchiveFinalization, PhaseSkipReason::NoRequestedWork)
            : RunPhaseRecord::executed(RunPhase::ArchiveFinalization)));
    if (adapters.isCancelled && adapters.isCancelled()) {
        result._cancelled = true;
        return result;
    }

    // The immutable policy is the run authority, so mismatched CLI or programmatic options cannot
    // re-enable Archive packing, creation, source deletion, or cleanup during Dry Run.
    if (_policy.executionMode() == routing::ExecutionMode::Apply &&
        adapters.finalizeArchiveLifecycleWithResult) {
        try {
            result._finalizationResult = adapters.finalizeArchiveLifecycleWithResult();
        } catch (const std::exception& error) {
            // A thrown finalizer supplies no reliable boundary for its durable mutations.
            result._finalizationResult = ArchiveFinalizationResult{
                {}, ArchiveFinalizationFailure::UnexpectedException, false, false, error.what()};
        } catch (...) {
            // Preserve a phase-level failure even when the adapter throws an untyped exception.
            result._finalizationResult = ArchiveFinalizationResult{
                {}, ArchiveFinalizationFailure::UnexpectedException, false, false,
                "Unknown Archive finalization exception."};
        }
        result._cancelled = result._finalizationResult->cancelled ||
                            (adapters.isCancelled && adapters.isCancelled());
    } else if (_policy.executionMode() == routing::ExecutionMode::Apply &&
               adapters.finalizeArchiveLifecycle) {
        result._cancelled = !adapters.finalizeArchiveLifecycle();
    }
    return result;
}
}  // namespace cao::run
