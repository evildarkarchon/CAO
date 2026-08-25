#pragma once

#include "Run/RunLifecycle.h"
#include "Run/RunScheduler.h"

#include <memory>
#include <optional>
#include <variant>

namespace cao::run {
/// Stable structural conflicts that prevent an Optimization Run from starting at all.
///
/// A Start Error is detected before any worker exists, so it produces no Run Outcome and no Run
/// Handle. Only malformed Run Request structure is a Start Error: anything that goes wrong once
/// the run exists, including scheduling failure, becomes a terminal result instead. Facts that
/// need the filesystem or the profile, such as whether a Mod Root exists, are not structural and
/// belong to Preparing.
enum class StartError { MissingProfileIdentity, MissingModSelectionDirectory };

/// The internal state one started Optimization Run shares between its worker and its Run Handle.
class RunSharedState;

/// The owning caller-side handle to one started Optimization Run.
///
/// A handle is movable and non-copyable, so exactly one owner is responsible for the run at any
/// time. It exposes no pause, resume, restart, mutable configuration, or worker access.
///
/// Destroying or overwriting an owning handle joins its worker first, so a run is never abandoned
/// while it is still executing. Requesting cancellation before that join arrives with the
/// production scheduler; an inline seam has no work left to cancel once the handle exists.
///
/// Observing a moved-from handle is a contract violation: the run moved to its new owner.
class RunHandle final {
   public:
    RunHandle(const RunHandle&) = delete;
    RunHandle& operator=(const RunHandle&) = delete;
    RunHandle(RunHandle&& other) noexcept;
    RunHandle& operator=(RunHandle&& other) noexcept;
    ~RunHandle();

    /// Returns the committed terminal result, or nullptr while the run is still active.
    ///
    /// The returned result is immutable and stays valid for the lifetime of this handle, because
    /// a run commits exactly one terminal result and never replaces it.
    [[nodiscard]] const OptimizationRunResult* terminalResult() const;

    /// Blocks until the Optimization Run commits its terminal result, then returns it.
    ///
    /// Waiting from the run's own worker would deadlock and is a contract violation; diagnosing
    /// it arrives with the production scheduler, which is the first seam that can detect it.
    [[nodiscard]] const OptimizationRunResult& wait() const;

   private:
    friend class OptimizationRunService;

    RunHandle(std::shared_ptr<RunSharedState> state,
              std::unique_ptr<ScheduledRunWorker> worker) noexcept;

    /// Joins the owned worker, if this handle still owns one, and releases it.
    void releaseOwnedRun() noexcept;

    std::shared_ptr<RunSharedState> _state;
    std::unique_ptr<ScheduledRunWorker> _worker;
};

/// The outcome of one start attempt: either the owning Run Handle or a synchronous Start Error.
class RunStartResult final {
   public:
    RunStartResult(const RunStartResult&) = delete;
    RunStartResult& operator=(const RunStartResult&) = delete;
    RunStartResult(RunStartResult&&) noexcept = default;
    RunStartResult& operator=(RunStartResult&&) noexcept = default;
    ~RunStartResult() = default;

    /// Reports whether the Optimization Run started and therefore owes a terminal result.
    [[nodiscard]] bool started() const noexcept;

    /// Returns the owning Run Handle to move out of, or nullptr when the start was rejected.
    [[nodiscard]] RunHandle* handle() noexcept;

    /// Returns the structural Start Error, or no value when the run started.
    [[nodiscard]] std::optional<StartError> startError() const noexcept;

   private:
    friend class OptimizationRunService;

    explicit RunStartResult(RunHandle handle) noexcept;
    explicit RunStartResult(StartError error) noexcept;

    std::variant<RunHandle, StartError> _outcome;
};

/// Starts Optimization Runs and hands each one back as an owning Run Handle.
///
/// This is the public asynchronous seam adapters use. It validates Run Request structure, owns
/// scheduling through an injected Run Scheduler, and gives each run the services it needs, so
/// callers never discover files, build optimizers, schedule workers, or perform cleanup. Beneath
/// it, the Run Executor remains the synchronous deterministic seam.
///
/// The injected scheduler must outlive this service and every Run Handle started through it.
class OptimizationRunService final {
   public:
    /// Starts runs on `scheduler`, which the caller owns.
    explicit OptimizationRunService(RunScheduler& scheduler) noexcept;

    /// Validates the Run Request structure and starts the Optimization Run.
    ///
    /// Structural conflicts are reported synchronously as a Start Error and create no worker.
    /// Otherwise the run takes ownership of `request` and is scheduled, and the returned handle
    /// owns it. A failure after that point, including a scheduler that cannot start a worker,
    /// commits a terminal Failed result rather than a Start Error.
    [[nodiscard]] RunStartResult start(RunRequest request);

   private:
    RunScheduler& _scheduler;
};
}  // namespace cao::run
