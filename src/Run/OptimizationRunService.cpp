#include "OptimizationRunService.h"

#include "Run/RunExecutor.h"

#include <cassert>
#include <algorithm>
#include <condition_variable>
#include <cstdio>
#include <deque>
#include <exception>
#include <mutex>
#include <memory>
#include <optional>
#include <stop_token>
#include <stdexcept>
#include <utility>
#include <vector>

namespace cao::run {
namespace {
// Every service shares this lease because profiles, logging, and filesystem work are process-wide.
std::mutex activeRunMutex;
std::weak_ptr<RunSharedState> activeRun;

/// Tracks nested inline execution without relying on a worker object published after schedule().
class RunExecutionScope final {
   public:
    /// Marks this thread until scope exit; nested terminal observers may start another inline run.
    explicit RunExecutionScope(const RunSharedState* state) noexcept
        : _state(state), _previous(_current) { _current = this; }

    /// Restores the outer run when nested inline execution returns or throws.
    ~RunExecutionScope() { _current = _previous; }

    /// Finds any enclosing run, so a nested callback cannot wait on its suspended outer worker.
    [[nodiscard]] static bool contains(const RunSharedState* state) noexcept {
        for (auto scope = _current; scope != nullptr; scope = scope->_previous)
            if (scope->_state == state) return true;
        return false;
    }

   private:
    const RunSharedState* _state;
    const RunExecutionScope* _previous;
    static inline thread_local const RunExecutionScope* _current{};
};

/// Performs the Safety Cleanup pass of a run that registered no temporary artifacts.
///
/// No phase registers artifacts yet, so the pass covers an empty set. It is a real pass rather
/// than a stub: every terminal path owes exactly one, and the registry slice replaces this with
/// the reverse-order cleanup of registered artifacts without changing when it happens.
class UnregisteredArtifactSafetyCleanup final : public SafetyCleanupService {
   public:
    // Nothing was registered, so there is nothing to remove. Backups, committed output, and
    // failed-output evidence are never Safety Cleanup's to touch in the first place.
    void performSafetyCleanup() override {}
};

/// Returns the structural conflict that prevents a Run Request from starting, if any.
///
/// Conflicts are checked in a fixed order so a request with more than one always reports the same
/// Start Error. Only request shape is inspected: nothing here touches the filesystem or the
/// profile, because a run that has not started cannot report what it found.
std::optional<StartError> findStructuralConflict(const RunRequest& request) {
    if (request.profileIdentity().empty()) return StartError::MissingProfileIdentity;

    // An empty selection names no directory in either Mod Selection kind, so Preparing could
    // resolve neither a single Mod Root nor the children beneath a mods directory.
    if (request.modSelection().directory().empty()) return StartError::MissingModSelectionDirectory;

    return std::nullopt;
}
}  // namespace

/// The state one started Optimization Run shares between its worker and its Run Handle.
///
/// The run owns it through a shared pointer so that neither the caller's Run Request nor the
/// handle can disappear out from under the worker. It owns the immutable request data and the
/// per-run services through terminal commit, exactly as the lifecycle requires.
class RunSharedState final : public RunObservationSink,
                             public std::enable_shared_from_this<RunSharedState> {
    /// Tracks dispatcher admission separately from callback completion; guarded by the state lock.
    struct DeliveryTicket {
        bool admitted{};
        bool invoked{};
    };

    /// One newly required drain, reserved before any dispatcher can reenter the run.
    struct DispatchRequest {
        std::size_t observer;
        std::shared_ptr<DeliveryTicket> ticket;
    };

   public:
    /// Owns request and presentation state before scheduling can invoke the worker inline.
    RunSharedState(RunRequest request, std::vector<RunObservation> observations)
        : _request(std::move(request)) {
        for (auto& observation : observations)
            if (observation.observer) _observers.push_back(ObserverState{std::move(observation)});
    }

    /// Assigns the next sequence before invoking presentation outside lifecycle locks.
    void recordPhase(const RunPhaseRecord& phase) override {
        std::vector<DispatchRequest> dispatch;
        {
            const std::lock_guard lock(_mutex);
            _phase = phase.phase();
            _progress = phase.progress();
            dispatch = enqueue(phase);
        }
        dispatchPending(dispatch);
    }

    /// Enqueues the failure before execution proceeds to cleanup or another work boundary.
    void recordFailure(const RunFailure& failure) override {
        std::vector<DispatchRequest> dispatch;
        {
            const std::lock_guard lock(_mutex);
            ++_failureCount;
            dispatch = enqueue(failure);
        }
        dispatchPending(dispatch);
    }

    /// Runs the Optimization Run through the synchronous Run Executor and commits its result.
    void execute() {
        const RunExecutionScope scope(this);
        const RunExecutor executor;
        commit(executor.execute(_request, RunServices{_safetyCleanup, this}, _stop.get_token(), _runId));
    }

    /// Requests cooperative cancellation without interrupting an atomic operation or cleanup.
    void requestCancellation() noexcept { _stop.request_stop(); }

    /// Commits the terminal result of a run whose worker could never be started.
    ///
    /// The run already exists by the time scheduling is attempted, so it owes both the one Safety
    /// Cleanup pass every terminal path owes and one terminal result. No work phase was traversed,
    /// so only Safety Cleanup is recorded and Preparing is reported as the furthest phase reached:
    /// a phase the run never entered may not claim an outcome it never observed.
    void commitSchedulingFailure(std::string detail) {
        const RunExecutionScope scope(this);
        std::vector<RunFailure> failures{
            RunFailure{RunFailureCode::SchedulingFailed, RunPhase::Preparing, std::move(detail)}};
        recordFailure(failures.front());
        _safetyCleanup.performSafetyCleanup();
        std::vector<RunPhaseRecord> phases;
        phases.push_back(RunPhaseRecord::executed(RunPhase::SafetyCleanup));
        recordPhase(phases.back());
        commit(OptimizationRunResult::terminal(RunOutcome::Failed, RunPhase::Preparing,
                                               std::move(phases), _runId, std::move(failures)));
    }

    /// Commits the one terminal result of this run and releases every waiter.
    void commit(OptimizationRunResult result) {
        std::vector<DispatchRequest> dispatch;
        {
            const std::lock_guard lock(_mutex);
            _result = std::make_shared<const OptimizationRunResult>(std::move(result));
            dispatch = enqueue(_result);
            _terminalEnqueued = _dispatching == 0;
            // Terminal is now in every enabled FIFO. Release the lease before calling a dispatcher:
            // its inline observer may start the next run. wait() separately guarantees admission
            // to the caller's dispatcher, without tying the active slot to presentation latency.
            // Publish the result and release the slot together: a waiter must never observe a
            // terminal result while a subsequent start would still see this run as active.
            const std::lock_guard activeLock(activeRunMutex);
            activeRun.reset();
        }

        // Notifying outside the lock keeps a woken waiter from immediately blocking again on a
        // lock this thread still holds.
        _committed.notify_all();
        dispatchPending(dispatch);
    }

    /// Returns the committed result, or nullptr while the run is still active.
    [[nodiscard]] const OptimizationRunResult* committedResult() const {
        const std::lock_guard lock(_mutex);
        // The result is engaged exactly once and never replaced or reset, so the pointer stays
        // valid and the pointee immutable after the lock is released.
        return _result.get();
    }

    /// Copies observations so a later queued failure cannot invalidate a reader's diagnostic list.
    [[nodiscard]] std::vector<RunDiagnostic> diagnostics() const {
        const std::lock_guard lock(_mutex);
        return _diagnostics;
    }

    /// Captures state under the same lock used before enqueuing phase, failure, and terminal facts.
    [[nodiscard]] RunSnapshot snapshot() const {
        const std::lock_guard lock(_mutex);
        return RunSnapshot{_runId, _phase, _progress, _stop.stop_requested(), _diagnostics.size(),
                           _failureCount, _result ? std::optional{_result->outcome()} : std::nullopt};
    }

    /// Blocks until this run commits its terminal result, then returns it.
    [[nodiscard]] const OptimizationRunResult& awaitCommittedResult() const {
        std::unique_lock lock(_mutex);
        _committed.wait(lock, [this] { return _terminalEnqueued; });
        return *_result;
    }

   private:
    /// Enqueues for every observer under the state lock before any dispatcher can reenter.
    std::vector<DispatchRequest> enqueue(RunEvent::Payload payload) {
        const RunEvent event{_runId, ++_sequence, std::move(payload)};
        std::vector<DispatchRequest> dispatch;
        for (std::size_t i = 0; i < _observers.size(); ++i) {
            auto& observer = _observers[i];
            if (observer.disabled) continue;
            observer.pending.push_back(event);
            if (observer.draining) continue;
            observer.draining = true;
            dispatch.push_back({i, std::make_shared<DeliveryTicket>()});
            ++_dispatching;
        }
        return dispatch;
    }

    /// Calls the caller's dispatcher without a lifecycle lock and retains state until delivery.
    void dispatchPending(const std::vector<DispatchRequest>& observers) {
        for (const auto& request : observers) {
            const auto index = request.observer;
            try {
                auto delivery = [state = shared_from_this(), request] {
                    if (state->admitDelivery(request.ticket, true)) state->drain(request.observer);
                };
                const auto& dispatcher = _observers[index].observation.dispatcher;
                if (dispatcher) dispatcher(std::move(delivery));
                else delivery();
            } catch (const std::exception& error) {
                disableObserver(index, RunDiagnosticCode::DispatcherFailed, error.what());
            } catch (...) {
                disableObserver(index, RunDiagnosticCode::DispatcherFailed,
                                "The Run Event dispatcher threw a non-standard exception");
            }
            static_cast<void>(admitDelivery(request.ticket, false));
        }
    }

    /// Publishes admission once at callback entry or dispatcher return, whichever occurs first.
    /// Starting inline delivery must release waiters without requiring that callback to finish.
    [[nodiscard]] bool admitDelivery(const std::shared_ptr<DeliveryTicket>& ticket, bool invoke) {
        bool enter = false;
        {
            const std::lock_guard lock(_mutex);
            if (!ticket->admitted) {
                ticket->admitted = true;
                --_dispatching;
            }
            enter = invoke && !ticket->invoked;
            if (enter) ticket->invoked = true;
            if (_result && _dispatching == 0) _terminalEnqueued = true;
        }
        _committed.notify_all();
        return enter;
    }

    /// Disables once, even if a dispatcher invokes an already failing observer and then throws.
    void disableObserver(std::size_t index, RunDiagnosticCode code, std::string detail) {
        std::vector<DispatchRequest> dispatch;
        {
            const std::lock_guard lock(_mutex);
            auto& observer = _observers[index];
            if (observer.disabled) return;
            observer.disabled = true;
            observer.pending.clear();
            _diagnostics.emplace_back(code, _phase, std::move(detail));
            // Healthy observers already have the triggering event. Enqueue the diagnostic next,
            // including after terminal delivery, without ever changing the committed result.
            dispatch = enqueue(_diagnostics.back());
        }
        dispatchPending(dispatch);
    }

    /// Drains FIFO observations outside locks even if the caller's queue reorders posted work.
    void drain(std::size_t index) {
        for (;;) {
            std::optional<RunEvent> event;
            {
                const std::lock_guard lock(_mutex);
                auto& observer = _observers[index];
                if (observer.disabled || observer.pending.empty()) {
                    observer.draining = false;
                    return;
                }
                event.emplace(std::move(observer.pending.front()));
                observer.pending.pop_front();
            }
            try {
                _observers[index].observation.observer(*event);
            } catch (const std::exception& error) {
                disableObserver(index, RunDiagnosticCode::ObserverFailed, error.what());
            } catch (...) {
                disableObserver(index, RunDiagnosticCode::ObserverFailed,
                                "The Run Event observer threw a non-standard exception");
            }
        }
    }

    /// Registration storage never moves after scheduling; mutable delivery flags use _mutex.
    struct ObserverState {
        RunObservation observation;
        std::deque<RunEvent> pending;
        bool draining{};
        bool disabled{};
    };

    mutable std::mutex _mutex;
    mutable std::condition_variable _committed;
    RunRequest _request;
    RunId _runId{createRunId()};
    std::vector<ObserverState> _observers;
    std::vector<RunDiagnostic> _diagnostics;
    RunPhase _phase{RunPhase::Preparing};
    std::optional<RunProgress> _progress;
    std::size_t _failureCount{};
    std::uint64_t _sequence{};
    std::size_t _dispatching{};
    bool _terminalEnqueued{};
    UnregisteredArtifactSafetyCleanup _safetyCleanup;
    std::shared_ptr<const OptimizationRunResult> _result;
    std::stop_source _stop;
};

/// Shares the join obligation between the owning handle and service without owning a callback
/// that captures itself. The worker captures only RunSharedState, avoiding an ownership cycle.
class RunWorkerLifetime final {
   public:
    /// Retains run state until both owners have finished with the scheduled worker.
    explicit RunWorkerLifetime(std::shared_ptr<RunSharedState> state) noexcept
        : _state(std::move(state)) {}

    /// Installs the worker before start() publishes the handle to another thread.
    void setWorker(std::unique_ptr<ScheduledRunWorker> worker) noexcept {
        _worker = std::move(worker);
    }

    /// Cancels active work and joins exactly once, including concurrent service/handle teardown.
    void cancelAndJoin() noexcept {
        // This must precede the join lock: another caller may already hold it while waiting
        // for this very worker to return from its inline callback.
        if (RunExecutionScope::contains(_state.get()) ||
            (_worker != nullptr && _worker->isCurrentThread())) {
            // Destruction cannot return safely or throw; report the contract violation before
            // invoking the application's terminate handler.
            std::fputs("An Optimization Run cannot destroy its own worker\n", stderr);
            std::terminate();
        }
        _state->requestCancellation();
        const std::lock_guard lock(_joinMutex);
        if (!_joined && _worker != nullptr) _worker->join();
        _joined = true;
    }

    /// Throws logic_error when a caller would wait for its own worker to finish.
    void diagnoseSelfWait() const {
        if (RunExecutionScope::contains(_state.get()) ||
            (_worker != nullptr && _worker->isCurrentThread()))
            throw std::logic_error("An Optimization Run cannot wait for or destroy its own worker");
    }

   private:
    std::shared_ptr<RunSharedState> _state;
    std::unique_ptr<ScheduledRunWorker> _worker;
    std::mutex _joinMutex;
    bool _joined{};
};

RunHandle::RunHandle(std::shared_ptr<RunSharedState> state,
                     std::shared_ptr<RunWorkerLifetime> worker) noexcept
    : _state(std::move(state)), _worker(std::move(worker)) {}

RunHandle::RunHandle(RunHandle&& other) noexcept
    : _state(std::move(other._state)), _worker(std::move(other._worker)) {
    // Moving the shared pointers already emptied the source, so the moved-from
    // handle owes no join and its destructor does nothing.
}

RunHandle& RunHandle::operator=(RunHandle&& other) noexcept {
    if (this == &other) return *this;

    // Overwriting an owning handle would otherwise abandon the run it currently owns.
    releaseOwnedRun();
    _state = std::move(other._state);
    _worker = std::move(other._worker);
    return *this;
}

RunHandle::~RunHandle() { releaseOwnedRun(); }

void RunHandle::requestCancellation() const noexcept {
    assert(_state != nullptr &&
           "Cancelling a moved-from Run Handle: the run moved to its new owner");
    _state->requestCancellation();
}

void RunHandle::releaseOwnedRun() noexcept {
    if (_worker != nullptr) _worker->cancelAndJoin();

    _worker.reset();
    // The run state is released only after the join, so the worker can never observe it being
    // destroyed while it is still executing.
    _state.reset();
}

const OptimizationRunResult* RunHandle::terminalResult() const {
    assert(_state != nullptr &&
           "Observing a moved-from Run Handle: the run moved to its new owner");
    return _state->committedResult();
}

std::vector<RunDiagnostic> RunHandle::diagnostics() const {
    assert(_state != nullptr && "Observing a moved-from Run Handle");
    return _state->diagnostics();
}

RunSnapshot RunHandle::snapshot() const {
    assert(_state != nullptr && "Observing a moved-from Run Handle");
    return _state->snapshot();
}

const OptimizationRunResult& RunHandle::wait() const {
    assert(_state != nullptr &&
           "Waiting on a moved-from Run Handle: the run moved to its new owner");
    _worker->diagnoseSelfWait();
    return _state->awaitCommittedResult();
}

RunStartResult::RunStartResult(RunHandle handle) noexcept : _outcome(std::move(handle)) {}

RunStartResult::RunStartResult(StartError error) noexcept : _outcome(error) {}

bool RunStartResult::started() const noexcept {
    return std::holds_alternative<RunHandle>(_outcome);
}

RunHandle* RunStartResult::handle() noexcept { return std::get_if<RunHandle>(&_outcome); }

std::optional<StartError> RunStartResult::startError() const noexcept {
    if (const auto* error = std::get_if<StartError>(&_outcome)) return *error;

    return std::nullopt;
}

OptimizationRunService::OptimizationRunService(RunScheduler& scheduler) noexcept
    : _scheduler(scheduler) {}

OptimizationRunService::OptimizationRunService() noexcept : _scheduler(_productionScheduler) {}

OptimizationRunService::~OptimizationRunService() {
    // No start() may overlap destruction; joining outside the registry lock also lets worker
    // completion inspect run state without depending on a service lock held by its waiter.
    for (const auto& run : _runs) {
        if (const auto lifetime = run.lock()) lifetime->cancelAndJoin();
    }
}

RunStartResult OptimizationRunService::start(RunRequest request, RunObserver observer,
                                             RunEventDispatcher dispatcher) {
    std::vector<RunObservation> observations;
    if (observer) observations.push_back({std::move(observer), std::move(dispatcher)});
    return start(std::move(request), std::move(observations));
}

RunStartResult OptimizationRunService::start(RunRequest request,
                                             std::vector<RunObservation> observations) {
    if (const auto conflict = findStructuralConflict(request)) return RunStartResult{*conflict};

    std::shared_ptr<RunSharedState> state;
    {
        const std::lock_guard lock(activeRunMutex);
        if (!activeRun.expired()) return RunStartResult{StartError::ActiveRun};
        state = std::make_shared<RunSharedState>(std::move(request), std::move(observations));
        activeRun = state;
    }
    auto lifetime = std::make_shared<RunWorkerLifetime>(state);
    {
        const std::lock_guard lock(_runsMutex);
        std::erase_if(_runs, [](const auto& run) { return run.expired(); });
        _runs.push_back(lifetime);
    }
    try {
        // The Run Worker holds its own reference to the run state, so the run survives a handle
        // that is destroyed while the worker is still executing.
        lifetime->setWorker(_scheduler.schedule([state] { state->execute(); }));
    } catch (const std::exception& error) {
        state->commitSchedulingFailure(error.what());
        return RunStartResult{RunHandle{std::move(state), std::move(lifetime)}};
    } catch (...) {
        // The request cleared both structural conflicts and the active-run check, so a scheduler
        // that cannot start a worker owes the terminal result its run was
        // created to produce. A scheduler that throws started no work, so nothing else can commit
        // this run, and the handle carries no worker and therefore owes no join.
        state->commitSchedulingFailure("The Run Scheduler threw a non-standard exception");
        return RunStartResult{RunHandle{std::move(state), std::move(lifetime)}};
    }

    return RunStartResult{RunHandle{std::move(state), std::move(lifetime)}};
}
}  // namespace cao::run
