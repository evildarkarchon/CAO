#include "OptimizationRunService.h"

#include "Run/RunExecutor.h"

#include <cassert>
#include <algorithm>
#include <condition_variable>
#include <cstdio>
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
class RunSharedState final {
   public:
    explicit RunSharedState(RunRequest request) noexcept : _request(std::move(request)) {}

    /// Runs the Optimization Run through the synchronous Run Executor and commits its result.
    void execute() {
        const RunExecutor executor;
        commit(executor.execute(_request, RunServices{_safetyCleanup}, _stop.get_token()));
    }

    /// Requests cooperative cancellation without interrupting an atomic operation or cleanup.
    void requestCancellation() noexcept { _stop.request_stop(); }

    /// Commits the terminal result of a run whose worker could never be started.
    ///
    /// The run already exists by the time scheduling is attempted, so it owes both the one Safety
    /// Cleanup pass every terminal path owes and one terminal result. No work phase was traversed,
    /// so only Safety Cleanup is recorded and Preparing is reported as the furthest phase reached:
    /// a phase the run never entered may not claim an outcome it never observed.
    void commitSchedulingFailure() {
        _safetyCleanup.performSafetyCleanup();
        std::vector<RunPhaseRecord> phases;
        phases.push_back(RunPhaseRecord::executed(RunPhase::SafetyCleanup));
        commit(OptimizationRunResult::terminal(RunOutcome::Failed, RunPhase::Preparing,
                                               std::move(phases)));
    }

    /// Commits the one terminal result of this run and releases every waiter.
    void commit(OptimizationRunResult result) {
        {
            const std::lock_guard lock(_mutex);
            _result.emplace(std::move(result));
            // Publish the result and release the slot together: a waiter must never observe a
            // terminal result while a subsequent start would still see this run as active.
            const std::lock_guard activeLock(activeRunMutex);
            activeRun.reset();
        }

        // Notifying outside the lock keeps a woken waiter from immediately blocking again on a
        // lock this thread still holds.
        _committed.notify_all();
    }

    /// Returns the committed result, or nullptr while the run is still active.
    [[nodiscard]] const OptimizationRunResult* committedResult() const {
        const std::lock_guard lock(_mutex);
        // The result is engaged exactly once and never replaced or reset, so the pointer stays
        // valid and the pointee immutable after the lock is released.
        return _result.has_value() ? &*_result : nullptr;
    }

    /// Blocks until this run commits its terminal result, then returns it.
    [[nodiscard]] const OptimizationRunResult& awaitCommittedResult() const {
        std::unique_lock lock(_mutex);
        _committed.wait(lock, [this] { return _result.has_value(); });
        return *_result;
    }

   private:
    mutable std::mutex _mutex;
    mutable std::condition_variable _committed;
    RunRequest _request;
    UnregisteredArtifactSafetyCleanup _safetyCleanup;
    std::optional<OptimizationRunResult> _result;
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
        if (_worker != nullptr && _worker->isCurrentThread()) {
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
        if (_worker != nullptr && _worker->isCurrentThread())
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

RunStartResult OptimizationRunService::start(RunRequest request) {
    if (const auto conflict = findStructuralConflict(request)) return RunStartResult{*conflict};

    std::shared_ptr<RunSharedState> state;
    {
        const std::lock_guard lock(activeRunMutex);
        if (!activeRun.expired()) return RunStartResult{StartError::ActiveRun};
        state = std::make_shared<RunSharedState>(std::move(request));
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
    } catch (...) {
        // The request cleared both structural conflicts and the active-run check, so a scheduler
        // that cannot start a worker owes the terminal result its run was
        // created to produce. A scheduler that throws started no work, so nothing else can commit
        // this run, and the handle carries no worker and therefore owes no join.
        state->commitSchedulingFailure();
        return RunStartResult{RunHandle{std::move(state), std::move(lifetime)}};
    }

    return RunStartResult{RunHandle{std::move(state), std::move(lifetime)}};
}
}  // namespace cao::run
