#pragma once

#include <functional>
#include <memory>

namespace cao::run {
/// The single worker one Run Scheduler started for one Optimization Run.
///
/// The run lifetime owns the worker and joins it before the run could be abandoned, so a worker
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
    /// Joining is idempotent; the run lifetime serializes callers so the worker need not support
    /// simultaneous joins. Joining from this worker is a diagnosed contract violation.
    virtual void join() = 0;

    /// Reports whether the caller is this worker, including while another caller joins it.
    ///
    /// This query must be thread-safe and must not wait for the worker to finish: the lifetime
    /// checks it before acquiring its join lock to diagnose self-wait without deadlocking.
    [[nodiscard]] virtual bool isCurrentThread() const noexcept = 0;
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
    /// An implementation must return a worker or throw before invoking `work`. The service turns
    /// that scheduling failure into the started run's terminal Failed result. The callback must
    /// not throw, because work already invoked cannot be reported as a scheduling failure.
    [[nodiscard]] virtual std::unique_ptr<ScheduledRunWorker> schedule(
        std::function<void()> work) = 0;
};

/// Runs each Optimization Run on one owned standard-C++ thread.
///
/// The worker owns all thread resources and can be joined after the scheduler has been destroyed.
/// Cooperative cancellation belongs to the run state; thread ownership stays inside this seam.
class StandardRunScheduler final : public RunScheduler {
   public:
    /// Starts `work` on a new thread, throwing only if no work could be started.
    [[nodiscard]] std::unique_ptr<ScheduledRunWorker> schedule(std::function<void()> work) override;
};

/// Runs each scheduled worker inline on the calling thread before scheduling returns.
///
/// This makes an Optimization Run deterministic for callers that want the lifecycle contract
/// without concurrency, so a run is already terminal by the time its Run Handle exists.
class InlineRunScheduler final : public RunScheduler {
   public:
    /// Runs `work` to completion, then returns a worker that has nothing left to join.
    [[nodiscard]] std::unique_ptr<ScheduledRunWorker> schedule(std::function<void()> work) override;
};
}  // namespace cao::run
