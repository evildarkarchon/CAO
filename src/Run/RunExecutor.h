#pragma once

#include "Run/RunLifecycle.h"

#include <stop_token>

namespace cao::run {
/// Receives executor facts synchronously; the owning run handles presentation and isolation.
class RunObservationSink {
   public:
    virtual ~RunObservationSink() = default;

    /// Records a traversed phase before the executor proceeds; implementations must isolate observers.
    virtual void recordPhase(const RunPhaseRecord& phase) = 0;

    /// Records a run-level failure before cleanup and terminal commit.
    virtual void recordFailure(const RunFailure& failure) = 0;

    /// Records an informational observation before execution continues; it cannot change outcome.
    virtual void recordDiagnostic(const RunDiagnostic& diagnostic) = 0;
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

    /// Removes every remaining registered temporary artifact, attempting all of them.
    virtual void performSafetyCleanup() = 0;
};

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
};

/// Executes one Optimization Run synchronously through the stable Run Phase sequence.
///
/// This is the highest deterministic execution seam beneath the asynchronous Optimization Run
/// service. It owns phase sequencing, phase applicability, Safety Cleanup, and terminal
/// classification. It performs no scheduling, dispatches no events, and never reconfigures logging.
///
/// Preparing loads owned facts, resolves the Mod Selection, and compiles policy. Requested work needs
/// the Archive discovery, Asset execution, and Archive Finalization service seams that later
/// lifecycle slices introduce, so it terminates as Failed at Preparing rather than reporting a
/// Succeeded run that performed nothing.
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
};
}  // namespace cao::run
