#include "AssetRun.h"

#include "ArchiveFirstAssetDiscovery.h"

#include <array>
#include <utility>

namespace cao::run {
AssetRunResult::AssetRunResult(routing::RoutingLedger ledger,
                               std::map<routing::SkipReason, std::size_t> skippedArchiveCounts,
                               std::vector<std::filesystem::path> unsupportedExplicitPaths,
                               const std::size_t nestedArchiveCount, const bool cancelled,
                               std::vector<RunDiagnostic> diagnostics) noexcept
    : _ledger(std::move(ledger)),
      _skippedArchiveCounts(std::move(skippedArchiveCounts)),
      _unsupportedExplicitPaths(std::move(unsupportedExplicitPaths)),
      _nestedArchiveCount(nestedArchiveCount),
      _cancelled(cancelled),
      _diagnostics(std::move(diagnostics)) {}

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
                                 const AssetRunAdapters& adapters) const {
    const ArchiveFirstAssetDiscovery discovery(_policy);
    bool cancelled = false;
    const auto discoveryResult =
        discovery.discover(roots, [&](const std::span<const routing::RoutedAsset> archives) {
            std::size_t completed = 0;
            for (const auto& archive : archives) {
                // An in-flight extraction must finish so cancellation cannot leave a partial
                // Archive.
                if (adapters.isCancelled && adapters.isCancelled()) {
                    cancelled = true;
                    break;
                }
                adapters.extractArchive(archive);
                ++completed;
                if (adapters.reportProgress) {
                    adapters.reportProgress(AssetRunProgress{
                        routing::RoutedAssetPhase::ArchiveExtraction, completed, archives.size()});
                }
                // Re-sample after the in-flight extraction and its progress callback so
                // cancellation during the final Archive can stop discovery before the definitive
                // tree traversal.
                if (adapters.isCancelled && adapters.isCancelled()) {
                    cancelled = true;
                    break;
                }
            }
            return !cancelled;
        },
        adapters.isCancelled);
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
                                   discoveryResult.diagnostics().end()));
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
            adapters.executeAsset(asset.get());
            ++completed;
            if (adapters.reportProgress) {
                adapters.reportProgress(AssetRunProgress{
                    routing::RoutedAssetPhase::LooseAssetProcessing, completed, total});
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
