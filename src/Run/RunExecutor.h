#pragma once

#include "Run/RunEvidence.h"

#include <stop_token>
#include <string>

namespace cao::run {
class TemporaryArtifactRegistry;

/// Receives typed work milestones while the Run Executor alone selects Run Phases.
///
/// Each method returns the accepted current phase for optional synchronous work presentation.
/// Evidence invariant violations propagate as RunEvidenceInvariantViolation.
class RunWorkMilestones {
   public:
    /// Releases the borrowed milestone interface without owning executor evidence.
    virtual ~RunWorkMilestones() = default;

    /// Enters Archive discovery before any collision or extraction facts.
    virtual RunPhaseRecord archiveDiscoveryStarted() = 0;
    /// Starts Archive extraction with its immutable attempted-work total.
    virtual RunPhaseRecord archiveExtractionPlanned(std::size_t total) = 0;
    /// Records the Dry Run exclusion of Archive extraction.
    virtual RunPhaseRecord dryRunArchiveExtraction() = 0;
    /// Enters definitive Effective Asset Tree discovery after Archive work.
    virtual RunPhaseRecord effectiveAssetTreeStarted() = 0;
    /// Starts Asset processing with the retained Routing Ledger's routed total.
    virtual RunPhaseRecord assetProcessingPlanned(std::size_t total) = 0;
    /// Selects the final work phase from prepared execution mode and finalizer availability.
    virtual RunPhaseRecord archiveFinalizationAvailable(routing::ExecutionMode mode,
                                                        bool hasFinalizer) = 0;
};

/// Performs requested work while recording owned evidence before proceeding to another attempt.
/// The executor retains completed facts if a later boundary throws and always owns cleanup.
class RunWorkService {
   public:
    virtual ~RunWorkService() = default;

    /// Loads work-specific owned configuration during Preparing, before recovery or asset mutation.
    /// Stateless services need no preparation. Exceptions fail Preparing and still trigger cleanup.
    virtual void prepare() {}

    /// Uses prepared inputs until return and submits complete facts through a phase-restricted
    /// evidence view. Reports typed milestones to the executor and checks stop between atomic
    /// attempts without retaining references.
    /// Exceptions become fatal WorkServiceFailed evidence without discarding earlier records. The
    /// executor owns artifacts through mandatory Safety Cleanup; work never cleans the registry.
    virtual void execute(const RunPreparation& preparation, RunWorkEvidence& evidence,
                         TemporaryArtifactRegistry& artifacts,
                         RunWorkMilestones& milestones, std::stop_token stop) = 0;
};

/// Removes the temporary artifacts one Optimization Run registered.
///
/// Safety Cleanup never rolls back Committed Mutations and never removes backups or failed-output
/// evidence. The Run Executor invokes it exactly once on every terminal path, after the last work
/// phase and before the terminal result is committed, and it is not cancellable.
class SafetyCleanupService {
   public:
    SafetyCleanupService() = default;
    SafetyCleanupService(const SafetyCleanupService&) = delete;
    SafetyCleanupService& operator=(const SafetyCleanupService&) = delete;
    SafetyCleanupService(SafetyCleanupService&&) = delete;
    SafetyCleanupService& operator=(SafetyCleanupService&&) = delete;
    virtual ~SafetyCleanupService() = default;

    /// Attempts every remaining artifact and returns owning failures in cleanup order.
    /// Implementations isolate individual errors and accept no cancellation token.
    virtual std::vector<RunFailure> performSafetyCleanup() = 0;
};

/// Invokes one mandatory cleanup pass and converts unexpected service exceptions to failures.
/// Returned failures own their details; publication and terminal classification belong to the run.
[[nodiscard]] std::vector<RunFailure> collectSafetyCleanupFailures(SafetyCleanupService& service);

/// The narrow services the Run Executor borrows for the duration of one synchronous run.
///
/// The caller owns each service and must keep it alive until `execute` returns.
///
/// Safety Cleanup is held by reference rather than by optional pointer because it is mandatory:
/// every terminal path owes the run exactly one cleanup pass. A run holding no registered
/// artifacts still performs that pass over an empty set, so an absent service would make an
/// executed phase indistinguishable from one that never happened.
struct RunServices final {
    SafetyCleanupService& safetyCleanup;
    RunObservationSink* observations{};
    /// Missing providers produce a structured Preparing failure, including for no-work requests.
    const RunConfigurationProvider* configuration{};
    /// Optional until application cutover; requested work fails explicitly when no service exists.
    RunWorkService* work{};
};

/// Executes one Optimization Run synchronously through the stable Run Phase sequence.
///
/// This is the highest deterministic execution seam beneath the asynchronous Optimization Run
/// service. It owns phase sequencing, phase applicability, Safety Cleanup, and terminal
/// classification. It performs no scheduling, dispatches no events, and never reconfigures logging.
///
/// Preparing loads owned facts, resolves the Mod Selection, and compiles policy. Apply Preparing
/// also recovers verified stale staging under OS locks retained through cleanup. Requested work
/// uses the injected work service and retains its observations and completed attempt evidence.
/// Without that service, requested work terminates as Failed at Preparing.
class RunExecutor final {
   public:
    /// Traverses every Run Phase in canonical order, reporting inapplicable phases as skipped with
    /// a stable reason, then performs Safety Cleanup exactly once and commits the terminal result.
    ///
    /// Returns an owning, self-contained result that outlives this executor, the request, and the
    /// borrowed services. The optional stop token is observed between phases; Safety Cleanup
    /// always finishes even when cancellation was requested. The caller may supply the public
    /// run's identity; standalone executions generate one. Observations record synchronous facts
    /// through RunServices, while scheduling and presentation dispatch stay with the owning run.
    [[nodiscard]] OptimizationRunResult execute(const RunRequest& request,
                                                const RunServices& services,
                                                std::stop_token stop = {},
                                                RunId runId = createRunId()) const;

    /// Commits a worker scheduling failure through the same mandatory cleanup boundary.
    /// No work phase was traversed, so Preparing remains the final work phase while only Safety
    /// Cleanup is recorded. Cleanup is non-cancellable; a request observed before or during it is
    /// retained after the single pass. The returned result owns both failure categories.
    [[nodiscard]] OptimizationRunResult schedulingFailure(
        std::string detail, SafetyCleanupService& cleanup,
        RunObservationSink* observations = nullptr, std::stop_token stop = {},
        RunId runId = createRunId()) const;
};
}  // namespace cao::run
