#pragma once

#include "AssetRouting/AssetRouter.h"
#include "AssetExecution/AssetExecutor.h"
#include "ArchiveFirstAssetDiscovery.h"
#include "ArchiveExtraction.h"
#include "ArchiveFinalizationResult.h"
#include "RunLifecycle.h"

#include <cstddef>
#include <filesystem>
#include <functional>
#include <map>
#include <span>
#include <stop_token>
#include <vector>

namespace cao::run {
/// Reports completed Routed Asset attempts against the routed-only work total for one phase.
struct AssetRunProgress final {
    routing::RoutedAssetPhase phase;
    std::size_t completed;
    std::size_t total;
};

using AssetRunProgressAdapter = std::function<void(const AssetRunProgress&)>;
using AssetRunCancellationAdapter = std::function<bool()>;

class AssetRunResult;
struct RunWorkRecord;

/// Owns a completed attempt's routed identity, resolved scope, and durable outcome.
struct RoutedAssetAttempt final {
    std::filesystem::path modRoot;
    routing::RoutedAsset asset;
    execution::AssetExecutionResult result;
};

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
/// Result-bearing extraction and execution adapters are required when their phases have work;
/// progress, cancellation, finalization, and result reporting are optional.
struct AssetRunAdapters final {
    AssetRunProgressAdapter reportProgress;
    AssetRunCancellationAdapter isCancelled;
    AssetRunDiagnosticsAdapter reportDiagnostics;
    /// Observes complete preflight collisions before any Archive extraction.
    std::function<void(std::span<const ArchiveCollision>)> reportArchiveCollisions;
    /// Observes fatal discovery failures before returning without mutation.
    std::function<void(const RunFailure&)> reportDiscoveryFailure;
    /// Executes with mutation evidence retained for every completed attempt.
    /// An unsafe result stops subsequent Assets and Archive finalization after attempt progress.
    std::function<execution::AssetExecutionResult(const routing::RoutedAsset&)>
        executeAssetWithResult;
    /// Extracts the completed manifest plan and reports mutation evidence.
    /// Unsafe continuation stops work independently of cancellation.
    std::function<ArchiveExtractionResult(const ArchiveExtractionPlan&)> extractArchiveWithResult;
    /// Optionally finalizes Archives and retains mutation, failure, and cancellation evidence.
    std::function<ArchiveFinalizationResult()> finalizeArchiveLifecycleWithResult;
    /// Observes actual lifecycle boundaries, including empty work phases, before work begins.
    std::function<void(const RunPhaseRecord&)> reportPhase;
};

class RunObservationSink;

/// Runs the production AssetRun composition beneath the synchronous Run Executor.
/// Borrows preparation, executor-owned evidence, observations, and operation closures until return;
/// completed outcomes survive later orchestration exceptions, which propagate to the executor.
/// Operations return outcomes without recording them. The sink supplies phase, progress, diagnostic,
/// and discovery-failure reporting; cancellation combines the stop token with the optional adapter.
/// The executor retains terminal classification and mandatory Safety Cleanup after this call unwinds.
void executeAssetRun(const RunPreparation& preparation, RunWorkRecord& record,
                     RunObservationSink& observations, std::stop_token stop,
                     const AssetRunAdapters& operations);

/// Owns the definitive Routing Ledger and the terminal state of one Asset Run.
class AssetRunResult final {
   public:
    /// Copies the completed run evidence into the lifecycle collector without borrowing adapters.
    [[nodiscard]] RunWorkRecord workRecord() const;

    /// Borrows every completed result-bearing Asset attempt, including successful mutations.
    [[nodiscard]] std::span<const RoutedAssetAttempt> assetAttempts() const noexcept {
        return _assetAttempts;
    }

    /// Borrows finalization evidence, absent when the result-bearing finalizer was not reached.
    [[nodiscard]] const std::optional<ArchiveFinalizationResult>& finalizationResult() const noexcept {
        return _finalizationResult;
    }
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

    /// Borrows fatal preflight failures; a nonempty list means no execution was attempted.
    [[nodiscard]] std::span<const RunFailure> failures() const noexcept { return _failures; }

    /// Borrows failed Asset attempts, including whether their mutation permits continuation.
    [[nodiscard]] std::span<const execution::AssetExecutionResult> executionFailures()
        const noexcept {
        return _executionFailures;
    }

    /// Borrows the complete collision evidence retained from preflight.
    [[nodiscard]] std::span<const ArchiveCollision> collisions() const noexcept {
        return _collisions;
    }

    /// Borrows every result-bearing Archive attempt, including successful mutation evidence.
    [[nodiscard]] std::span<const ArchiveExtractionResult> archiveAttempts() const noexcept {
        return _archiveAttempts;
    }

   private:
    friend class AssetRun;

    /// Takes ownership of the definitive ledger after run orchestration finishes.
    AssetRunResult(routing::RoutingLedger ledger,
                   std::map<routing::SkipReason, std::size_t> skippedArchiveCounts,
                   std::vector<std::filesystem::path> unsupportedExplicitPaths,
                   std::size_t nestedArchiveCount, bool cancelled,
                   std::vector<RunDiagnostic> diagnostics, std::vector<RunFailure> failures,
                   std::vector<ArchiveCollision> collisions) noexcept;

    routing::RoutingLedger _ledger;
    std::map<routing::SkipReason, std::size_t> _skippedArchiveCounts;
    std::vector<std::filesystem::path> _unsupportedExplicitPaths;
    std::size_t _nestedArchiveCount;
    bool _cancelled;
    std::vector<RunDiagnostic> _diagnostics;
    std::vector<RunFailure> _failures;
    std::vector<ArchiveCollision> _collisions;
    std::vector<execution::AssetExecutionResult> _executionFailures;
    std::vector<ArchiveExtractionResult> _archiveAttempts;
    std::vector<RoutedAssetAttempt> _assetAttempts;
    std::optional<ArchiveFinalizationResult> _finalizationResult;
    bool _routingCompleted{};
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
    /// reports cancellation in its result. Filesystem races are skipped during discovery.
    /// Presentation exceptions become informational ObserverFailed diagnostics without discarding
    /// attempts. Extraction exceptions retain unknown
    /// mutation evidence and stop the run. Manifest/order failures stop all mutation.
    /// Archive precedence is validated before the first extraction callback.
    /// Result-bearing execution retains all attempts and converts adapter exceptions to unsafe
    /// outcomes. Unsafe continuation stops further work while retaining concurrent cancellation.
    [[nodiscard]] AssetRunResult execute(
        std::span<const std::filesystem::path> roots, const AssetRunAdapters& adapters,
        const ArchivePrecedence& precedence = ArchivePrecedence::deterministicDiscovery()) const;

   private:
    routing::RoutingPolicy _policy;
};
}  // namespace cao::run
