#pragma once

#include "Run/AssetRun.h"
#include "Run/ArchiveFinalizationResult.h"

namespace cao::run {
/// Executor-owned evidence accumulated at completed boundaries, then copied into the terminal result.
/// A missing ledger means definitive routing was never reached, rather than an empty routed tree.
struct RunWorkRecord final {
    std::optional<routing::RoutingLedger> ledger;
    std::vector<RoutedAssetAttempt> assetAttempts;
    std::vector<ArchiveExtractionResult> archiveAttempts;
    std::vector<ArchiveFinalizationResult> finalizations;
    /// Discovery exclusions not represented in the definitive Loose Asset ledger.
    std::map<routing::SkipReason, std::size_t> skippedArchiveCounts;
    std::vector<std::filesystem::path> unsupportedExplicitPaths;
    std::size_t nestedArchiveCount{};
    std::vector<ArchiveCollision> collisions;
    std::vector<RunDiagnostic> diagnostics;
    std::vector<RunFailure> failures;
    /// Internal publication position shared by Preparing and work; advance before calling observers.
    std::size_t publishedDiagnostics{};
    bool cancellationObserved{};
};
}  // namespace cao::run
