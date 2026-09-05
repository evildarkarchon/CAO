#pragma once

#include "Run/RunLifecycle.h"
#include "Run/RunScheduler.h"

#include <memory>
#include <functional>
#include <mutex>
#include <optional>
#include <variant>
#include <vector>

namespace cao::run {
/// Receives immutable observations. Copy an event to retain it beyond the callback.
using RunObserver = std::function<void(const RunEvent&)>;

/// Accepts a delivery closure for inline or queued execution. Empty dispatchers execute inline.
/// Queued closures own their run state and may execute after wait() and owner destruction.
/// Returning successfully accepts the closure; throwing disables this registration. A dispatcher
/// must eventually execute or release accepted closures, and keep its captured targets valid.
using RunEventDispatcher = std::function<void(std::function<void()>)>;

/// One independently isolated observer and its caller-selected delivery context, owned by the run.
/// Callbacks are serialized per registration and run outside lifecycle locks. Work terminal delivery
/// may be followed by diagnostics from delayed presentation failures; those never alter Run Outcome.
struct RunObservation final {
    RunObserver observer;
    RunEventDispatcher dispatcher;
};

/// Stable request or active-run conflicts that prevent an Optimization Run from starting at all.
///
/// A Start Error is detected before any worker exists, so it produces no Run Outcome and no Run
/// Handle. Malformed Run Request structure and an active-run conflict are Start Errors. Anything
/// that goes wrong once the run exists, including scheduling failure, becomes a terminal result
/// instead. Facts that need the filesystem or the profile, such as whether a Mod Root exists,
/// are not structural and belong to Preparing.
enum class StartError { MissingProfileIdentity, MissingModSelectionDirectory, ActiveRun };

/// The internal state one started Optimization Run shares between its worker and its Run Handle.
class RunSharedState;
class RunWorkerLifetime;

/// The owning caller-side handle to one started Optimization Run.
///
/// A handle is movable and non-copyable, so exactly one owner is responsible for the run at any
/// time. It exposes no pause, resume, restart, mutable configuration, or worker access.
///
/// Destroying or overwriting an owning handle requests cancellation and joins its worker, so a
/// run is never abandoned while it is still executing.
///
/// Observing a moved-from handle is a contract violation: the run moved to its new owner.
/// Cancellation, snapshot/diagnostic reads, terminal query, and wait may run concurrently while
/// the handle remains alive and unmoved. Moving or destroying it requires caller synchronization.
class RunHandle final {
   public:
    RunHandle(const RunHandle&) = delete;
    RunHandle& operator=(const RunHandle&) = delete;
    RunHandle(RunHandle&& other) noexcept;
    RunHandle& operator=(RunHandle&& other) noexcept;
    ~RunHandle();

    /// Requests cancellation idempotently from any thread without blocking or interrupting work.
    /// The handle must remain alive and unmoved for the duration of this call.
    /// A request after terminal commit remains visible in snapshots but cannot change the result
    /// or cancel a subsequent run, even when that run started from this run's terminal observer.
    void requestCancellation() const noexcept;

    /// Returns the committed terminal result, or nullptr while the run is still active.
    ///
    /// The returned result is immutable and stays valid for the lifetime of this handle, because
    /// a run commits exactly one terminal result and never replaces it.
    [[nodiscard]] const OptimizationRunResult* terminalResult() const;

    /// Copies diagnostics under synchronization, including queued presentation failures after wait().
    /// Late diagnostics never mutate the committed terminal result or its Run Outcome.
    [[nodiscard]] std::vector<RunDiagnostic> diagnostics() const;

    /// Copies the current state from any thread, including an inline observer or dispatcher.
    /// State is published before its event is enqueued; delayed callbacks may see newer state.
    /// Retained copies do not change as the run advances or late presentation diagnostics arrive.
    [[nodiscard]] RunSnapshot snapshot() const;

    /// Blocks until terminal commit and dispatcher admission of every event through terminal.
    /// Queued callbacks need not have executed; a dispatcher failure counts as diagnosed rejection.
    ///
    /// Throws std::logic_error from the run's own worker, including its inline callbacks.
    /// Active destruction from that context is a fatal contract violation (std::terminate).
    [[nodiscard]] const OptimizationRunResult& wait() const;

   private:
    friend class OptimizationRunService;

    /// Retains the immutable result state and the join obligation shared with the service.
    RunHandle(std::shared_ptr<RunSharedState> state,
              std::shared_ptr<RunWorkerLifetime> worker) noexcept;

    /// Cancels and joins the owned worker, if this handle still owns one, and releases it.
    void releaseOwnedRun() noexcept;

    std::shared_ptr<RunSharedState> _state;
    std::shared_ptr<RunWorkerLifetime> _worker;
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

    /// Returns the request or active-run Start Error, or no value when the run started.
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
/// Concurrent start() calls are supported; destroying the service must not overlap start().
class OptimizationRunService final {
   public:
    /// Owns the standard-C++ production scheduler; adapters need no worker or scheduler objects.
    OptimizationRunService() noexcept;

    /// Starts runs on `scheduler`, which the caller owns.
    explicit OptimizationRunService(RunScheduler& scheduler) noexcept;

    OptimizationRunService(const OptimizationRunService&) = delete;
    OptimizationRunService& operator=(const OptimizationRunService&) = delete;

    /// Cancels and joins every retained run before the service's dependencies disappear.
    /// Destruction must not overlap start(); destruction from its worker terminates with a
    /// diagnostic.
    ~OptimizationRunService();

    /// Validates the Run Request structure and starts the Optimization Run.
    ///
    /// Request and active-run conflicts are synchronous Start Errors and create no worker.
    /// Otherwise the run takes ownership of `request` and is scheduled, and the returned handle
    /// owns it. A failure after that point, including a scheduler that cannot start a worker,
    /// commits a terminal Failed result rather than a Start Error.
    [[nodiscard]] RunStartResult start(RunRequest request, RunObserver observer = {},
                                       RunEventDispatcher dispatcher = {});

    /// Starts with independently isolated observers; failure disables only that registration.
    /// Every enabled observer receives the same ordered history, including presentation diagnostics.
    [[nodiscard]] RunStartResult start(RunRequest request, std::vector<RunObservation> observations);

   private:
    StandardRunScheduler _productionScheduler;
    RunScheduler& _scheduler;
    std::mutex _runsMutex;
    std::vector<std::weak_ptr<RunWorkerLifetime>> _runs;
};
}  // namespace cao::run
