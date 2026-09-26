#pragma once

#include "AssetRouting/AssetRouter.h"
#include "AssetExecution/AssetExecutor.h"
#include "ArchiveFirstAssetDiscovery.h"
#include "ArchiveExtraction.h"
#include "ArchiveFinalizationResult.h"
#include "AssetInitializationCancelled.h"
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

class RunWorkMilestones;
class RunWorkEvidence;

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

    /// Borrows worker-confined evidence for one synchronous reporting callback.
    explicit AssetRunDiagnostics(const RunWorkEvidence& evidence) noexcept;

   private:
    const RunWorkEvidence& _evidence;
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
    /// Executes with the canonical Mod Root frozen before source mutation and retained with the
    /// outcome. Borrows both arguments only until return; Assets outside prepared roots never reach
    /// this adapter. An unsafe result stops subsequent Assets and Archive finalization after
    /// attempt progress. AssetInitializationCancelled signals read-only setup cancellation before
    /// an attempt exists, so Asset Run records cancellation without attempt progress.
    std::function<execution::AssetExecutionResult(const routing::RoutedAsset&,
                                                  const std::filesystem::path&)>
        executeAssetWithResult;
    /// Extracts the completed manifest plan and reports mutation evidence.
    /// Unsafe continuation stops work independently of cancellation.
    std::function<ArchiveExtractionResult(const ArchiveExtractionPlan&)> extractArchiveWithResult;
    /// Optionally finalizes Archives and retains mutation, failure, and cancellation evidence.
    std::function<ArchiveFinalizationResult()> finalizeArchiveLifecycleWithResult;
    /// Observes actual lifecycle boundaries, including empty work phases, before work begins.
    std::function<void(const RunPhaseRecord&)> reportPhase;
};

/// Runs the production AssetRun composition beneath the synchronous Run Executor.
/// Borrows preparation, executor-owned work evidence, milestones, and operations until return;
/// completed outcomes survive later orchestration exceptions, which propagate to the executor.
/// Operations return outcomes without recording them. Archive and Asset facts enter the concrete
/// evidence owner before any presentation callback; milestones let the executor choose phase
/// position and progress. Cancellation combines the stop token with the optional adapter. The
/// executor retains terminal classification and mandatory Safety Cleanup after this call unwinds.
void executeAssetRun(const RunPreparation& preparation, RunWorkEvidence& evidence,
                     RunWorkMilestones& milestones, std::stop_token stop,
                     const AssetRunAdapters& operations);

/// Orchestrates Archive-first discovery, definitive routing, and carried Asset execution.
class AssetRun final {
   public:
    /// Owns the immutable policy used for both Archive selection and definitive routing.
    explicit AssetRun(routing::RoutingPolicy policy) noexcept;

    /// Submits completed Archive discovery, collision, extraction, Routing Ledger, and Asset
    /// attempt facts to the borrowed evidence owner before proceeding or reporting them. Extracts
    /// routed Archives, batch-routes the resulting Effective Asset Tree once, offers the owned
    /// Routed Assets to the execution adapter, and
    /// reports definitive routing diagnostics before Apply-only Archive finalization. Cancellation
    /// is observed between filesystem entries and attempts, and once more after the final attempt.
    /// An adapter is never abandoned mid-operation, and cancellation skips diagnostics and
    /// finalization.
    /// A finalizer reports cancellation in its result. Filesystem races are skipped during
    /// discovery. Presentation exceptions become informational ObserverFailed diagnostics without
    /// discarding attempts. Extraction exceptions retain unknown mutation evidence and stop the
    /// run. Manifest/order failures stop all mutation. Archive precedence is validated before the
    /// first extraction callback. Result-bearing execution retains all attempts and converts
    /// adapter exceptions to unsafe outcomes. Unsafe continuation stops further work while
    /// retaining concurrent cancellation. Resolves each Asset's canonical Mod Root before invoking
    /// processing and retains that same identity with the outcome, even when processing removes the
    /// source. Unmatched Assets are rejected. Read-only backend initialization cancellation leaves
    /// no attempted Asset or mutation evidence.
    void execute(std::span<const std::filesystem::path> roots, RunWorkEvidence& evidence,
                 const AssetRunAdapters& adapters, const ArchivePrecedence& precedence,
                 RunWorkMilestones& milestones) const;

   private:
    routing::RoutingPolicy _policy;
};
}  // namespace cao::run
