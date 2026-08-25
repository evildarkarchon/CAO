#pragma once

#include <functional>
#include <memory>

namespace cao::run {
/// The single worker one Run Scheduler started for one Optimization Run.
///
/// The Run Handle owns the worker and joins it before the run could be abandoned, so a worker
/// never outlives the run state it borrows. A scheduler implementation that cannot survive an
/// unjoined worker, such as one backed by a thread, must join defensively in its own destructor.
class ScheduledRunWorker {
   public:
    ScheduledRunWorker() = default;
    ScheduledRunWorker(const ScheduledRunWorker&) = delete;
    ScheduledRunWorker& operator=(const ScheduledRunWorker&) = delete;
    ScheduledRunWorker(ScheduledRunWorker&&) = delete;
    ScheduledRunWorker& operator=(ScheduledRunWorker&&) = delete;
    virtual ~ScheduledRunWorker() = default;

    /// Blocks until the scheduled work has finished.
    ///
    /// Joining is idempotent: the Run Handle may join in response to either destruction or being
    /// overwritten, and the work still runs at most once.
    virtual void join() = 0;
};

/// Starts the single worker of one Optimization Run.
///
/// This is the injectable scheduling seam of the Optimization Run service. It exists so the
/// lifecycle module owns scheduling in standard C++ without depending on GUI concurrency types,
/// and so tests can make scheduling deterministic. See ADR-0001.
///
/// The scheduler must outlive the Optimization Run service and every Run Handle started through
/// it, because a handle joins its worker through the scheduler's worker object.
class RunScheduler {
   public:
    RunScheduler() = default;
    RunScheduler(const RunScheduler&) = delete;
    RunScheduler& operator=(const RunScheduler&) = delete;
    RunScheduler(RunScheduler&&) = delete;
    RunScheduler& operator=(RunScheduler&&) = delete;
    virtual ~RunScheduler() = default;

    /// Starts `work` and returns the joinable Run Worker running it.
    ///
    /// An implementation must return a worker: the Optimization Run has already been created by
    /// the time it is scheduled, so it owes its caller a terminal result. Reporting a scheduler
    /// that cannot start work arrives with the production scheduler, which is the first
    /// implementation that can fail.
    [[nodiscard]] virtual std::unique_ptr<ScheduledRunWorker> schedule(
        std::function<void()> work) = 0;
};

/// Runs each scheduled worker inline on the calling thread before scheduling returns.
///
/// This makes an Optimization Run deterministic for callers that want the lifecycle contract
/// without concurrency, so a run is already terminal by the time its Run Handle exists. It is the
/// scheduling seam this lifecycle slice ships; production threading arrives with its own
/// scheduler.
class InlineRunScheduler final : public RunScheduler {
   public:
    /// Runs `work` to completion, then returns a worker that has nothing left to join.
    [[nodiscard]] std::unique_ptr<ScheduledRunWorker> schedule(std::function<void()> work) override;
};
}  // namespace cao::run
