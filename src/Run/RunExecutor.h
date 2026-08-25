#pragma once

#include "Run/RunLifecycle.h"

namespace cao::run
{
/// Removes the temporary artifacts one Optimization Run registered.
///
/// Safety Cleanup never rolls back Committed Mutations and never removes backups or failed-output
/// evidence. The Run Executor invokes it exactly once on every terminal path, after the last work
/// phase and before the terminal result is committed, and it is not cancellable.
class SafetyCleanupService
{
public:
    SafetyCleanupService() = default;
    SafetyCleanupService(const SafetyCleanupService &) = delete;
    SafetyCleanupService &operator=(const SafetyCleanupService &) = delete;
    SafetyCleanupService(SafetyCleanupService &&) = delete;
    SafetyCleanupService &operator=(SafetyCleanupService &&) = delete;
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
struct RunServices final
{
    SafetyCleanupService &safetyCleanup;
};

/// Executes one Optimization Run synchronously through the stable Run Phase sequence.
///
/// This is the highest deterministic execution seam beneath the asynchronous Optimization Run
/// service. It owns phase sequencing, phase applicability, Safety Cleanup, and terminal
/// classification. It performs no scheduling, dispatches no events, and never reconfigures logging.
///
/// This slice implements the no-work lifecycle only. A request that carries requested work needs
/// the Archive discovery, Asset execution, and Archive Finalization service seams that later
/// lifecycle slices introduce, so it terminates as Failed at Preparing rather than reporting a
/// Succeeded run that performed nothing.
class RunExecutor final
{
public:
    /// Traverses every Run Phase in canonical order, reporting inapplicable phases as skipped with
    /// a stable reason, then performs Safety Cleanup exactly once and commits the terminal result.
    ///
    /// Returns an owning, self-contained result that outlives this executor, the request, and the
    /// borrowed services.
    [[nodiscard]] OptimizationRunResult execute(const RunRequest &request,
                                                const RunServices &services) const;
};
}
