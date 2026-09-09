#include "AssetRun.h"

#include "ArchiveFirstAssetDiscovery.h"

#include <array>
#include <utility>

namespace cao::run {
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
    const ArchiveFirstAssetDiscovery discovery(_policy);
    bool cancelled = false;
    bool unsafeArchive = false;
    std::vector<ArchiveExtractionPlan> extractionPlans;
    std::vector<ArchiveExtractionResult> archiveAttempts;
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
                    archiveAttempts.push_back(std::move(attempt));
                } else {
                    adapters.extractArchive(archive);
                }
                ++completed;
                if (adapters.reportProgress) {
                    adapters.reportProgress(AssetRunProgress{
                        routing::RoutedAssetPhase::ArchiveExtraction, completed, archives.size()});
                }
                if (unsafeArchive) break;
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
        adapters.isCancelled, precedence, adapters.reportArchiveCollisions,
        [&](const std::span<const ArchiveExtractionPlan> plans) {
            extractionPlans.assign(plans.begin(), plans.end());
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
    if (unsafeArchive) {
        // Discovery's legacy false callback means cancellation. This stop instead comes from
        // mutation evidence, and must neither appear cancelled nor reach assets or finalization.
        result._cancelled = cancelled;
        return result;
    }
    if (!result.failures().empty()) {
        // A failed preflight has no trustworthy tree and must never reach mutation or finalization.
        if (adapters.reportDiscoveryFailure)
            for (const auto& failure : result.failures()) adapters.reportDiscoveryFailure(failure);
        return result;
    }
    constexpr std::array targetOrder{routing::OptimizerTarget::Texture,
                                     routing::OptimizerTarget::Mesh,
                                     routing::OptimizerTarget::Animation};
    if (result.cancelled()) return result;
    const auto total = result.ledger().routedAssets().size();
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
                auto attempt = adapters.executeAssetWithResult(asset.get());
                safeToContinue = attempt.safeToContinue();
                if (!attempt.succeeded()) result._executionFailures.push_back(std::move(attempt));
            } else {
                adapters.executeAsset(asset.get());
            }
            ++completed;
            if (adapters.reportProgress) {
                adapters.reportProgress(AssetRunProgress{
                    routing::RoutedAssetPhase::LooseAssetProcessing, completed, total});
            }
            // A failed attempt still completes progress, but an uncertain mutation makes later
            // optimization and packing unsafe even when cancellation has not been requested.
            if (!safeToContinue) return result;
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
        adapters.reportDiagnostics(diagnostics);
    }

    // The immutable policy is the run authority, so mismatched CLI or programmatic options cannot
    // re-enable Archive packing, creation, source deletion, or cleanup during Dry Run.
    if (_policy.executionMode() == routing::ExecutionMode::Apply &&
        adapters.finalizeArchiveLifecycle) {
        result._cancelled = !adapters.finalizeArchiveLifecycle();
    }
    return result;
}
}  // namespace cao::run
