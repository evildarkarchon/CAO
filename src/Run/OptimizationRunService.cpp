#include "OptimizationRunService.h"

#include "Run/RunExecutor.h"

#include <cassert>
#include <condition_variable>
#include <mutex>
#include <optional>
#include <utility>

namespace cao::run {
namespace {
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
        commit(executor.execute(_request, RunServices{_safetyCleanup}));
    }

    /// Commits the one terminal result of this run and releases every waiter.
    void commit(OptimizationRunResult result) {
        {
            const std::lock_guard lock(_mutex);
            _result.emplace(std::move(result));
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
};

RunHandle::RunHandle(std::shared_ptr<RunSharedState> state,
                     std::unique_ptr<ScheduledRunWorker> worker) noexcept
    : _state(std::move(state)), _worker(std::move(worker)) {}

RunHandle::RunHandle(RunHandle&& other) noexcept
    : _state(std::move(other._state)), _worker(std::move(other._worker)) {
    // Moving from a shared_ptr and a unique_ptr already emptied the source, so the moved-from
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

void RunHandle::releaseOwnedRun() noexcept {
    if (_worker != nullptr) _worker->join();

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

RunStartResult OptimizationRunService::start(RunRequest request) {
    if (const auto conflict = findStructuralConflict(request)) return RunStartResult{*conflict};

    auto state = std::make_shared<RunSharedState>(std::move(request));
    // The Run Worker holds its own reference to the run state, so the run survives a handle that
    // is destroyed while the worker is still executing.
    auto worker = _scheduler.schedule([state] { state->execute(); });

    return RunStartResult{RunHandle{std::move(state), std::move(worker)}};
}
}  // namespace cao::run
