#pragma once

#include "AssetRouting/AssetRouter.h"
#include "RunLifecycle.h"

#include <cstddef>
#include <filesystem>
#include <functional>
#include <map>
#include <span>
#include <vector>

namespace cao::run {
using ArchiveAssetAdapter = std::function<void(const routing::RoutedAsset&)>;
using RoutedAssetExecutionAdapter = std::function<void(const routing::RoutedAsset&)>;

/// Reports completed Routed Asset attempts against the routed-only work total for one phase.
struct AssetRunProgress final {
    routing::RoutedAssetPhase phase;
    std::size_t completed;
    std::size_t total;
};

using AssetRunProgressAdapter = std::function<void(const AssetRunProgress&)>;
using AssetRunCancellationAdapter = std::function<bool()>;
/// Completes post-execution Archive mutations and reports whether finalization finished.
using ArchiveLifecycleFinalizationAdapter = std::function<bool()>;

class AssetRunResult;

/// Read-only diagnostics that are definitive before Apply-mode Archive finalization begins.
class AssetRunDiagnostics final {
   public:
    /// Returns the aggregate count for one stable recognized-Asset Skip Reason.
    [[nodiscard]] std::size_t skippedAssetCount(routing::SkipReason reason) const noexcept;

    /// Returns unsupported roots that were explicitly supplied as files, never directory entries.
    [[nodiscard]] std::span<const std::filesystem::path> unsupportedExplicitPaths() const noexcept;

    /// Returns how many distinct Archives appeared only after extraction and were left alone.
    [[nodiscard]] std::size_t nestedArchiveCount() const noexcept;

    /// Borrows structured discovery observations for the duration of the reporting callback.
    [[nodiscard]] std::span<const RunDiagnostic> diagnostics() const noexcept;

   private:
    friend class AssetRun;

    /// Borrows one in-flight result for the duration of its synchronous diagnostics callback.
    explicit AssetRunDiagnostics(const AssetRunResult& result) noexcept;

    const AssetRunResult& _result;
};

using AssetRunDiagnosticsAdapter = std::function<void(const AssetRunDiagnostics&)>;

/// Supplies the production or test adapters used at the run's filesystem and execution seams.
/// Extraction and execution are required; progress, cancellation, finalization, and result
/// reporting are optional.
struct AssetRunAdapters final {
    ArchiveAssetAdapter extractArchive;
    RoutedAssetExecutionAdapter executeAsset;
    AssetRunProgressAdapter reportProgress;
    AssetRunCancellationAdapter isCancelled;
    ArchiveLifecycleFinalizationAdapter finalizeArchiveLifecycle;
    AssetRunDiagnosticsAdapter reportDiagnostics;
};

/// Owns the definitive Routing Ledger and the terminal state of one Asset Run.
class AssetRunResult final {
   public:
    /// Returns the definitive owned Routing Ledger; cancellation may leave some Assets unexecuted.
    [[nodiscard]] const routing::RoutingLedger& ledger() const noexcept;

    /// Reports whether the run stopped early at a cancellation seam.
    [[nodiscard]] bool cancelled() const noexcept;

    /// Returns the aggregate count for one stable recognized-Asset Skip Reason.
    [[nodiscard]] std::size_t skippedAssetCount(routing::SkipReason reason) const noexcept;

    /// Returns unsupported roots that were explicitly supplied as files, never directory entries.
    [[nodiscard]] std::span<const std::filesystem::path> unsupportedExplicitPaths() const noexcept;

    /// Returns how many distinct Archives appeared only after extraction, which the game would
    /// not read nested and the run therefore left alone.
    [[nodiscard]] std::size_t nestedArchiveCount() const noexcept;

    /// Borrows owned discovery observations, including excluded linked entries and their paths.
    [[nodiscard]] std::span<const RunDiagnostic> diagnostics() const noexcept;

   private:
    friend class AssetRun;

    /// Takes ownership of the definitive ledger after run orchestration finishes.
    AssetRunResult(routing::RoutingLedger ledger,
                   std::map<routing::SkipReason, std::size_t> skippedArchiveCounts,
                   std::vector<std::filesystem::path> unsupportedExplicitPaths,
                   std::size_t nestedArchiveCount, bool cancelled,
                   std::vector<RunDiagnostic> diagnostics) noexcept;

    routing::RoutingLedger _ledger;
    std::map<routing::SkipReason, std::size_t> _skippedArchiveCounts;
    std::vector<std::filesystem::path> _unsupportedExplicitPaths;
    std::size_t _nestedArchiveCount;
    bool _cancelled;
    std::vector<RunDiagnostic> _diagnostics;
};

/// Orchestrates Archive-first discovery, definitive routing, and carried Asset execution.
class AssetRun final {
   public:
    /// Owns the immutable policy used for both Archive selection and definitive routing.
    explicit AssetRun(routing::RoutingPolicy policy) noexcept;

    /// Extracts routed Archives, batch-routes the resulting Effective Asset Tree once, offers the
    /// owned Routed Assets to the execution adapter, reports definitive routing diagnostics, then
    /// finalizes Archives in Apply mode only. Cancellation is observed between filesystem entries
    /// and attempts, and once more after the final attempt, so an adapter is never abandoned
    /// mid-operation and a cancelled run never reaches diagnostics or finalization. A finalizer
    /// reports cancellation by returning false. Filesystem races are skipped during discovery;
    /// adapter exceptions propagate.
    [[nodiscard]] AssetRunResult execute(std::span<const std::filesystem::path> roots,
                                         const AssetRunAdapters& adapters) const;

   private:
    routing::RoutingPolicy _policy;
};
}  // namespace cao::run
