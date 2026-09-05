#include "Run/OptimizationRunService.h"

#include <QtTest>

#include <cstddef>
#include <barrier>
#include <cstdlib>
#include <exception>
#include <filesystem>
#include <functional>
#include <memory>
#include <semaphore>
#include <stdexcept>
#include <string_view>
#include <system_error>
#include <thread>
#include <type_traits>
#include <utility>
#include <vector>

using cao::routing::ExecutionMode;
using cao::routing::RequestedWork;
using cao::run::ModSelection;
using cao::run::OptimizationRunResult;
using cao::run::OptimizationRunService;
using cao::run::RunHandle;
using cao::run::RunOutcome;
using cao::run::RunPhase;
using cao::run::RunPhaseStatus;
using cao::run::RunRequest;
using cao::run::RunScheduler;
using cao::run::runPhaseSequence;
using cao::run::ScheduledRunWorker;
using cao::run::StartError;

namespace
{
/// Runs each worker inline while counting how many workers the service asked it to start.
///
/// The count is what proves a rejected start created no worker at all, rather than creating one
/// that happened to do nothing.
class CountingInlineScheduler final : public RunScheduler
{
public:
    std::unique_ptr<ScheduledRunWorker> schedule(std::function<void()> work) override
    {
        ++_scheduled;
        return _inlineScheduler.schedule(std::move(work));
    }

    [[nodiscard]] std::size_t scheduled() const noexcept { return _scheduled; }

private:
    cao::run::InlineRunScheduler _inlineScheduler;
    std::size_t _scheduled{};
};

/// Holds each scheduled worker's work until it is joined, and abandons it if it never is.
///
/// A real worker would join itself as a last resort, which would hide a missing join in the Run
/// Handle. This substitute deliberately does not, so an abandoned run is observable as work that
/// never completed.
class DeferredRunScheduler final : public RunScheduler
{
public:
    std::unique_ptr<ScheduledRunWorker> schedule(std::function<void()> work) override
    {
        return std::make_unique<DeferredWorker>(*this, std::move(work));
    }

    /// Returns how many scheduled workers ran their work to completion.
    [[nodiscard]] std::size_t completions() const noexcept { return _completions; }

    /// Returns how many times a scheduled worker was joined, including redundant joins.
    [[nodiscard]] std::size_t joins() const noexcept { return _joins; }

    std::function<void()> afterWork;

private:
    class DeferredWorker final : public ScheduledRunWorker
    {
    public:
        DeferredWorker(DeferredRunScheduler &owner, std::function<void()> work)
            : _owner(owner)
            , _work(std::move(work))
        {
        }

        void join() override
        {
            ++_owner._joins;
            if (!_work)
                return;

            const auto work = std::exchange(_work, nullptr);
            work();
            if (_owner.afterWork) _owner.afterWork();
            ++_owner._completions;
        }

        /// No execution thread exists until this deterministic worker is joined.
        [[nodiscard]] bool isCurrentThread() const noexcept override { return false; }

    private:
        DeferredRunScheduler &_owner;
        std::function<void()> _work;
    };

    std::size_t _completions{};
    std::size_t _joins{};
};

/// Fails every scheduling attempt the way a thread-backed scheduler out of resources would.
///
/// Throwing from `schedule` is the realistic failure: `std::thread`'s constructor reports an
/// exhausted thread limit as a `std::system_error` rather than a return value.
class ExhaustedRunScheduler final : public RunScheduler
{
public:
    std::unique_ptr<ScheduledRunWorker> schedule(std::function<void()>) override
    {
        throw std::system_error(std::make_error_code(std::errc::resource_unavailable_try_again),
                                "No Run Worker could be started");
    }
};

/// Gates the real production worker so callers can inspect or cancel a known-active run.
class GatedRunScheduler final : public RunScheduler
{
public:
    /// Starts the production worker, withholding invocation until release() publishes test state.
    std::unique_ptr<ScheduledRunWorker> schedule(std::function<void()> work) override
    {
        return _production.schedule([this, work = std::move(work)] {
            _released.acquire();
            if (beforeWork) beforeWork();
            work();
            if (afterWork) afterWork();
        });
    }

    /// Allows the worker to execute after all captured handle references have been initialized.
    void release() { _released.release(); }

    std::function<void()> beforeWork;
    std::function<void()> afterWork;

private:
    cao::run::StandardRunScheduler _production;
    std::binary_semaphore _released{0};
};

/// Builds a structurally valid Run Request that selects no work at all.
RunRequest noWorkRequest()
{
    return RunRequest::create("SkyrimSE",
                              ExecutionMode::Apply,
                              ModSelection::singleModRoot(std::filesystem::path("mods/Example")),
                              {});
}
}

class OptimizationRunServiceTests final : public QObject
{
    Q_OBJECT

private slots:
    /// Catches missing, reordered, or unidentified phase and terminal observations.
    void inlineEventsOwnAnOrderedRunHistory();

    /// Catches caller queues losing events or borrowing state that dies with the handle.
    void queuedEventsOutliveTheRunOwners();

    /// Catches presentation exceptions changing work or suppressing a healthy observer's history.
    void presentationFailuresDisableOnlyTheFailingObserver();

    /// Catches failures disappearing between phase observations and terminal classification.
    void failureEventsPrecedeCleanupAndTerminal();

    /// Catches stale state at enqueue, callbacks under locks, and worker-side callback self-wait.
    void inlineCallbacksObservePublishedStateAndCanCancel();

    /// Catches an event-driven cancellation traversing phases beyond its observation boundary.
    void cancellationFromASkippedPhaseStopsFurtherTraversal();

    /// Verifies a Run Request naming no profile is rejected before any worker exists.
    void aRequestWithoutAProfileIdentityIsRejectedWithoutCreatingAWorker();

    /// Verifies a Mod Selection naming no directory is rejected for either selection kind.
    void aModSelectionWithoutADirectoryIsRejectedWithoutCreatingAWorker();

    /// Verifies a rejected start reports its Start Error instead of a Run Handle.
    void aRejectedStartExposesAStartErrorInsteadOfAHandle();

    /// Verifies a structurally valid start schedules exactly one worker and returns a handle.
    void aValidStartSchedulesOneWorkerAndReturnsAHandle();

    /// Verifies the Run Handle owns its run exclusively: it may be moved but never copied.
    void aRunHandleIsMovableButNotCopyable();

    /// Verifies waiting returns the immutable terminal result of the no-work run.
    void waitingReturnsTheImmutableTerminalResult();

    /// Verifies repeated waiting observes the same committed result rather than re-running.
    void waitingRepeatedlyObservesTheSameCommittedResult();

    /// Verifies the run owns its request, so a caller's temporary request may die at once.
    void theRunOwnsItsRequestAfterStartReturns();

    /// Verifies terminal query reports an active run before its worker has committed.
    void terminalQueryReportsAnActiveRunAsUncommitted();

    /// Verifies destroying an owning handle joins its worker instead of abandoning the run.
    void destroyingAnActiveHandleJoinsItsWorker();

    /// Verifies a moved-from handle owes no join, so ownership transfers exactly once.
    void movingAHandleTransfersTheJoinObligationExactlyOnce();

    /// Verifies overwriting an owning handle joins the run it replaces.
    void moveAssigningOverAnActiveHandleJoinsTheReplacedRun();

    /// Verifies a scheduler that cannot start a worker yields a terminal Failed run, not a throw.
    void aSchedulerThatCannotStartAWorkerCommitsAFailedRun();

    /// Verifies the active slot is shared across services and rejects without scheduling.
    void aSecondActiveRunIsRejectedAcrossServices();

    /// Verifies service destruction completes cancellation even when its handle survives.
    void destroyingTheServiceCancelsAndJoinsARetainedHandle();

    /// Verifies a worker cannot deadlock itself by waiting for its own result.
    void waitingFromTheWorkerIsDiagnosed();

    /// Verifies adapters need no scheduler ownership for a production run.
    void theDefaultServiceSchedulesAProductionRun();

    /// Verifies repeated cancellation is cooperative and cannot release the active slot early.
    void cancellationKeepsTheSlotUntilTerminalCommit();

    /// Verifies each forbidden worker-side destructor fails promptly with a contract diagnostic.
    void destructionFromTheWorkerIsDiagnosed();

    /// Verifies commit releases the slot while the previous worker still has an epilogue to join.
    void terminalCommitReleasesTheSlotBeforeWorkerJoin();

    /// Verifies racing starts reserve one process-wide slot atomically.
    void simultaneousStartsAdmitExactlyOneRun();

    /// Verifies handle teardown requests cancellation before joining its deferred worker.
    void destroyingTheHandleRequestsCancellationBeforeJoin();

    /// Verifies scheduling failure releases the slot while the failed handle remains readable.
    void schedulingFailureReleasesTheActiveSlot();
};

void OptimizationRunServiceTests::inlineEventsOwnAnOrderedRunHistory()
{
    std::vector<cao::run::RunEvent> events;
    {
        CountingInlineScheduler scheduler;
        OptimizationRunService service{scheduler};
        auto started = service.start(noWorkRequest(),
            [&](const cao::run::RunEvent& event) { events.push_back(event); });
        QCOMPARE(started.handle()->wait().outcome(), RunOutcome::Succeeded);
        QCOMPARE(events.size(), std::size_t{8});
        QVERIFY(!events.front().runId().empty());
        QCOMPARE(events.front().runId(), started.handle()->wait().runId());
    }

    // Copies retain their payloads after the service, handle, and worker have gone away.
    const std::vector<RunPhase> expected{
        RunPhase::Preparing, RunPhase::DiscoveringArchives, RunPhase::ExtractingArchives,
        RunPhase::BuildingEffectiveAssetTree, RunPhase::ProcessingAssets,
        RunPhase::ArchiveFinalization, RunPhase::SafetyCleanup};
    for (std::size_t i = 0; i < events.size(); ++i) {
        QCOMPARE(events[i].sequence(), static_cast<std::uint64_t>(i + 1));
        QCOMPARE(events[i].runId(), events.front().runId());
        if (i < expected.size()) {
            const auto* phase = std::get_if<cao::run::RunPhaseRecord>(&events[i].payload());
            QVERIFY(phase != nullptr);
            QCOMPARE(phase->phase(), expected[i]);
            if (i > 0 && i < 6)
                QCOMPARE(phase->skipReason(), std::optional{cao::run::PhaseSkipReason::NoRequestedWork});
        }
    }
    const auto* terminal = std::get_if<std::shared_ptr<const OptimizationRunResult>>(
        &events.back().payload());
    QVERIFY(terminal != nullptr);
    QCOMPARE((*terminal)->outcome(), RunOutcome::Succeeded);
}

void OptimizationRunServiceTests::queuedEventsOutliveTheRunOwners()
{
    std::vector<std::function<void()>> pending;
    std::vector<cao::run::RunEvent> events;
    {
        CountingInlineScheduler scheduler;
        OptimizationRunService service{scheduler};
        auto started = service.start(noWorkRequest(),
            [&](const cao::run::RunEvent& event) { events.push_back(event); },
            [&](std::function<void()> delivery) { pending.push_back(std::move(delivery)); });
        QCOMPARE(started.handle()->wait().outcome(), RunOutcome::Succeeded);
        QVERIFY(events.empty());
    }
    // An adversarial queue may execute posted work in reverse order. The run must serialize it.
    for (auto task = pending.rbegin(); task != pending.rend(); ++task) (*task)();
    QCOMPARE(events.size(), std::size_t{8});
    for (std::size_t i = 0; i < events.size(); ++i)
        QCOMPARE(events[i].sequence(), static_cast<std::uint64_t>(i + 1));
    QVERIFY(std::holds_alternative<std::shared_ptr<const OptimizationRunResult>>(events.back().payload()));
}

void OptimizationRunServiceTests::presentationFailuresDisableOnlyTheFailingObserver()
{
    for (const bool dispatcherFails : {false, true}) {
        CountingInlineScheduler scheduler;
        OptimizationRunService service{scheduler};
        std::vector<cao::run::RunEvent> healthy;
        std::size_t attempts{};
        std::vector<cao::run::RunObservation> observations;
        observations.push_back({
            [&](const cao::run::RunEvent&) {
                ++attempts;
                if (!dispatcherFails) throw std::runtime_error("view unavailable");
            },
            [&](std::function<void()> delivery) {
                if (dispatcherFails) { ++attempts; throw 42; }
                delivery();
            }});
        observations.push_back({[&](const cao::run::RunEvent& event) { healthy.push_back(event); }, {}});
        auto started = service.start(noWorkRequest(), std::move(observations));

        QCOMPARE(started.handle()->wait().outcome(), RunOutcome::Succeeded);
        QCOMPARE(attempts, std::size_t{1});
        const auto diagnostics = started.handle()->diagnostics();
        QCOMPARE(diagnostics.size(), std::size_t{1});
        QCOMPARE(diagnostics.front().code(), dispatcherFails ? cao::run::RunDiagnosticCode::DispatcherFailed
                                                            : cao::run::RunDiagnosticCode::ObserverFailed);
        QCOMPARE(healthy.size(), std::size_t{9});
        QVERIFY(std::holds_alternative<cao::run::RunPhaseRecord>(healthy[0].payload()));
        QVERIFY(std::holds_alternative<cao::run::RunDiagnostic>(healthy[1].payload()));
        for (std::size_t i = 0; i < healthy.size(); ++i)
            QCOMPARE(healthy[i].sequence(), static_cast<std::uint64_t>(i + 1));
        QVERIFY(std::holds_alternative<std::shared_ptr<const OptimizationRunResult>>(healthy.back().payload()));
    }
}

void OptimizationRunServiceTests::failureEventsPrecedeCleanupAndTerminal()
{
    for (const bool schedulingFails : {false, true}) {
        CountingInlineScheduler inlineScheduler;
        ExhaustedRunScheduler exhausted;
        OptimizationRunService service{schedulingFails ? static_cast<RunScheduler&>(exhausted)
                                                       : static_cast<RunScheduler&>(inlineScheduler)};
        std::vector<cao::run::RunEvent> events;
        auto request = RunRequest::create("SkyrimSE", ExecutionMode::Apply,
            ModSelection::singleModRoot("mods/Example"), {RequestedWork::NativeTextureOptimization});
        auto started = service.start(std::move(request),
            [&](const cao::run::RunEvent& event) { events.push_back(event); });
        const auto& result = started.handle()->wait();
        QCOMPARE(result.outcome(), RunOutcome::Failed);
        QCOMPARE(events.size(), schedulingFails ? std::size_t{3} : std::size_t{4});
        const auto& failureEvent = events[events.size() - 3];
        const auto* failure = std::get_if<cao::run::RunFailure>(&failureEvent.payload());
        QVERIFY(failure != nullptr);
        QCOMPARE(failure->code(), schedulingFails ? cao::run::RunFailureCode::SchedulingFailed
                                                 : cao::run::RunFailureCode::RequestedWorkUnavailable);
        QCOMPARE(failure->phase(), RunPhase::Preparing);
        QVERIFY(!failure->detail().empty());
        QCOMPARE(result.failures().size(), std::size_t{1});
        QCOMPARE(result.failures()[0].code(), failure->code());
        QCOMPARE(std::get<cao::run::RunPhaseRecord>(events[events.size() - 2].payload()).phase(),
                 RunPhase::SafetyCleanup);
        QVERIFY(std::holds_alternative<std::shared_ptr<const OptimizationRunResult>>(events.back().payload()));
    }
}

void OptimizationRunServiceTests::inlineCallbacksObservePublishedStateAndCanCancel()
{
    GatedRunScheduler scheduler;
    std::binary_semaphore finished{0};
    scheduler.afterWork = [&] { finished.release(); };
    OptimizationRunService service{scheduler};
    RunHandle* handle{};
    std::vector<RunPhase> phases;
    bool selfWaitDiagnosed{};
    bool statePublished = true;
    auto started = service.start(noWorkRequest(), [&](const cao::run::RunEvent& event) {
        const auto snapshot = handle->snapshot();
        statePublished = statePublished && snapshot.runId() == event.runId();
        if (const auto* phase = std::get_if<cao::run::RunPhaseRecord>(&event.payload())) {
            phases.push_back(phase->phase());
            statePublished = statePublished && snapshot.phase() == phase->phase()
                && !snapshot.progress().has_value();
            if (phase->phase() == RunPhase::Preparing) {
                try { static_cast<void>(handle->wait()); }
                catch (const std::logic_error&) { selfWaitDiagnosed = true; }
                handle->requestCancellation();
                statePublished = statePublished && handle->snapshot().cancellationRequested();
            }
        } else if (std::holds_alternative<std::shared_ptr<const OptimizationRunResult>>(event.payload())) {
            statePublished = statePublished && handle->terminalResult() != nullptr
                && snapshot.outcome() == std::optional{RunOutcome::Cancelled};
        }
    });
    handle = started.handle();
    scheduler.release();
    QCOMPARE(handle->wait().outcome(), RunOutcome::Cancelled);
    // Synchronize before reading callback-owned values: wait promises enqueue, not execution.
    finished.acquire();
    QVERIFY(selfWaitDiagnosed);
    QVERIFY(statePublished);
    QCOMPARE(phases, (std::vector<RunPhase>{RunPhase::Preparing, RunPhase::SafetyCleanup}));
}

void OptimizationRunServiceTests::cancellationFromASkippedPhaseStopsFurtherTraversal()
{
    GatedRunScheduler scheduler;
    std::binary_semaphore finished{0};
    scheduler.afterWork = [&] { finished.release(); };
    OptimizationRunService service{scheduler};
    RunHandle* handle{};
    std::vector<RunPhase> phases;
    auto started = service.start(noWorkRequest(), [&](const cao::run::RunEvent& event) {
        if (const auto* phase = std::get_if<cao::run::RunPhaseRecord>(&event.payload())) {
            phases.push_back(phase->phase());
            if (phase->phase() == RunPhase::DiscoveringArchives) handle->requestCancellation();
        }
    });
    handle = started.handle();
    scheduler.release();
    const auto& terminal = handle->wait();
    finished.acquire();
    QCOMPARE(terminal.outcome(), RunOutcome::Cancelled);
    QCOMPARE(terminal.finalPhase(), RunPhase::DiscoveringArchives);
    QCOMPARE(phases, (std::vector<RunPhase>{RunPhase::Preparing, RunPhase::DiscoveringArchives,
                                          RunPhase::SafetyCleanup}));
}

void OptimizationRunServiceTests::aRequestWithoutAProfileIdentityIsRejectedWithoutCreatingAWorker()
{
    CountingInlineScheduler scheduler;
    OptimizationRunService service{scheduler};

    auto result = service.start(
        RunRequest::create("",
                           ExecutionMode::Apply,
                           ModSelection::singleModRoot(std::filesystem::path("mods/Example")),
                           {}));

    QVERIFY(!result.started());
    QCOMPARE(result.startError(), std::optional{StartError::MissingProfileIdentity});
    // A Start Error produces no Run Outcome, so nothing may have been scheduled to produce one.
    QCOMPARE(scheduler.scheduled(), std::size_t{0});
}

void OptimizationRunServiceTests::aModSelectionWithoutADirectoryIsRejectedWithoutCreatingAWorker()
{
    CountingInlineScheduler scheduler;
    OptimizationRunService service{scheduler};

    auto single = service.start(RunRequest::create("SkyrimSE",
                                                   ExecutionMode::Apply,
                                                   ModSelection::singleModRoot({}),
                                                   {}));
    auto children = service.start(RunRequest::create("SkyrimSE",
                                                     ExecutionMode::DryRun,
                                                     ModSelection::childModRoots({}),
                                                     {RequestedWork::NativeTextureOptimization}));

    QCOMPARE(single.startError(), std::optional{StartError::MissingModSelectionDirectory});
    QCOMPARE(children.startError(), std::optional{StartError::MissingModSelectionDirectory});
    QCOMPARE(scheduler.scheduled(), std::size_t{0});
}

void OptimizationRunServiceTests::aRejectedStartExposesAStartErrorInsteadOfAHandle()
{
    CountingInlineScheduler scheduler;
    OptimizationRunService service{scheduler};

    auto rejected = service.start(RunRequest::create("",
                                                     ExecutionMode::Apply,
                                                     ModSelection::singleModRoot({}),
                                                     {}));
    auto started = service.start(noWorkRequest());

    QVERIFY(rejected.handle() == nullptr);
    QVERIFY(started.handle() != nullptr);
    QVERIFY(!started.startError().has_value());
}

void OptimizationRunServiceTests::aValidStartSchedulesOneWorkerAndReturnsAHandle()
{
    CountingInlineScheduler scheduler;
    OptimizationRunService service{scheduler};

    auto result = service.start(noWorkRequest());

    QVERIFY(result.started());
    QVERIFY(result.handle() != nullptr);
    QCOMPARE(scheduler.scheduled(), std::size_t{1});
    // The inline seam runs the worker before scheduling returns, so the run is already terminal.
    QVERIFY(result.handle()->terminalResult() != nullptr);
}

void OptimizationRunServiceTests::aRunHandleIsMovableButNotCopyable()
{
    static_assert(!std::is_copy_constructible_v<RunHandle>,
                  "Copying a Run Handle would give two owners the same run");
    static_assert(!std::is_copy_assignable_v<RunHandle>,
                  "Copying a Run Handle would give two owners the same run");
    static_assert(std::is_move_constructible_v<RunHandle>, "A Run Handle must be movable");
    static_assert(std::is_move_assignable_v<RunHandle>, "A Run Handle must be movable");

    CountingInlineScheduler scheduler;
    OptimizationRunService service{scheduler};
    auto result = service.start(noWorkRequest());

    // Moving must carry the run with it: the destination observes the same terminal result.
    auto moved = std::move(*result.handle());
    auto reseated = std::move(moved);

    QCOMPARE(reseated.wait().outcome(), RunOutcome::Succeeded);
}

void OptimizationRunServiceTests::waitingReturnsTheImmutableTerminalResult()
{
    static_assert(
        std::is_same_v<decltype(std::declval<const RunHandle &>().wait()),
                       const OptimizationRunResult &>,
        "Waiting must expose the terminal result immutably");

    CountingInlineScheduler scheduler;
    OptimizationRunService service{scheduler};
    auto result = service.start(noWorkRequest());
    auto handle = std::move(*result.handle());

    const auto &terminal = handle.wait();

    QCOMPARE(terminal.outcome(), RunOutcome::Succeeded);
    QCOMPARE(terminal.finalPhase(), RunPhase::ArchiveFinalization);
    QCOMPARE(terminal.phases().size(), runPhaseSequence().size());
    QCOMPARE(terminal.phases().back().phase(), RunPhase::SafetyCleanup);
    QCOMPARE(terminal.phases().back().status(), RunPhaseStatus::Executed);
}

void OptimizationRunServiceTests::waitingRepeatedlyObservesTheSameCommittedResult()
{
    CountingInlineScheduler scheduler;
    OptimizationRunService service{scheduler};
    auto result = service.start(noWorkRequest());
    auto handle = std::move(*result.handle());

    const auto &first = handle.wait();
    const auto &second = handle.wait();

    // The result is committed once and never recomputed, so both waits observe one object.
    QVERIFY(&first == &second);
    QVERIFY(handle.terminalResult() == &first);
    QCOMPARE(scheduler.scheduled(), std::size_t{1});
}

void OptimizationRunServiceTests::theRunOwnsItsRequestAfterStartReturns()
{
    cao::run::StandardRunScheduler scheduler;
    OptimizationRunService service{scheduler};

    // The request is a temporary that dies when start returns, while the worker is still using
    // it on another thread. The run must have taken ownership rather than borrowed it.
    auto result = service.start(noWorkRequest());
    auto handle = std::move(*result.handle());

    QCOMPARE(handle.wait().outcome(), RunOutcome::Succeeded);
}

void OptimizationRunServiceTests::terminalQueryReportsAnActiveRunAsUncommitted()
{
    DeferredRunScheduler scheduler;
    OptimizationRunService service{scheduler};

    auto result = service.start(noWorkRequest());
    auto handle = std::move(*result.handle());

    QVERIFY(handle.terminalResult() == nullptr);
    QCOMPARE(scheduler.completions(), std::size_t{0});
}

void OptimizationRunServiceTests::destroyingAnActiveHandleJoinsItsWorker()
{
    DeferredRunScheduler scheduler;
    OptimizationRunService service{scheduler};

    {
        auto result = service.start(noWorkRequest());
        auto handle = std::move(*result.handle());
        QCOMPARE(scheduler.completions(), std::size_t{0});
    }

    // Destroying the owning handle must join the worker, so the run finished rather than being
    // abandoned mid-flight with its run state disappearing underneath it.
    QCOMPARE(scheduler.completions(), std::size_t{1});
}

void OptimizationRunServiceTests::movingAHandleTransfersTheJoinObligationExactlyOnce()
{
    DeferredRunScheduler scheduler;
    OptimizationRunService service{scheduler};

    auto result = service.start(noWorkRequest());
    {
        auto source = std::move(*result.handle());
        {
            const auto destination = std::move(source);
            QCOMPARE(scheduler.joins(), std::size_t{0});
        }

        // The destination owed the join, so the run completed when it was destroyed.
        QCOMPARE(scheduler.completions(), std::size_t{1});
        QCOMPARE(scheduler.joins(), std::size_t{1});
    }

    // Destroying the moved-from handle must not join a worker it no longer owns.
    QCOMPARE(scheduler.joins(), std::size_t{1});
}

void OptimizationRunServiceTests::moveAssigningOverAnActiveHandleJoinsTheReplacedRun()
{
    DeferredRunScheduler scheduler;
    OptimizationRunService service{scheduler};

    auto result = service.start(noWorkRequest());
    auto handle = std::move(*result.handle());

    // The source was emptied by the first move, so this assignment only overwrites: it cannot
    // hide an abandoned run behind a replacement run that happens to complete too. One run is
    // enough to make the point, and the lifecycle allows only one active run in any case.
    handle = std::move(*result.handle());

    // Overwriting an owning handle abandons its run unless the replaced worker is joined first.
    QCOMPARE(scheduler.completions(), std::size_t{1});
    QCOMPARE(scheduler.joins(), std::size_t{1});
}

void OptimizationRunServiceTests::aSchedulerThatCannotStartAWorkerCommitsAFailedRun()
{
    ExhaustedRunScheduler scheduler;
    OptimizationRunService service{scheduler};

    // The request cleared every structural conflict, so the run exists before scheduling is
    // attempted and owes the caller a terminal result rather than an escaping exception.
    auto result = service.start(noWorkRequest());

    QVERIFY(result.started());
    QVERIFY(!result.startError().has_value());
    QVERIFY(result.handle() != nullptr);

    // Waiting must return rather than block forever on a worker that was never started.
    const auto &terminal = result.handle()->wait();

    QCOMPARE(terminal.outcome(), RunOutcome::Failed);
    // No work phase was traversed, so the run reports the first phase as the furthest it reached.
    QCOMPARE(terminal.finalPhase(), RunPhase::Preparing);
    // Every terminal path still owes exactly one Safety Cleanup pass, including this one.
    QCOMPARE(terminal.phases().size(), std::size_t{1});
    QCOMPARE(terminal.phases().back().phase(), RunPhase::SafetyCleanup);
    QCOMPARE(terminal.phases().back().status(), RunPhaseStatus::Executed);
}

void OptimizationRunServiceTests::aSecondActiveRunIsRejectedAcrossServices()
{
    DeferredRunScheduler firstScheduler;
    CountingInlineScheduler secondScheduler;
    OptimizationRunService firstService{firstScheduler};
    OptimizationRunService secondService{secondScheduler};
    auto first = firstService.start(noWorkRequest());

    auto second = secondService.start(noWorkRequest());

    QVERIFY(!second.started());
    QCOMPARE(second.startError(), std::optional{StartError::ActiveRun});
    QCOMPARE(secondScheduler.scheduled(), std::size_t{0});
    QVERIFY(first.handle()->terminalResult() == nullptr);
}

void OptimizationRunServiceTests::destroyingTheServiceCancelsAndJoinsARetainedHandle()
{
    DeferredRunScheduler scheduler;
    auto service = std::make_unique<OptimizationRunService>(scheduler);
    auto started = service->start(noWorkRequest());
    auto handle = std::move(*started.handle());

    service.reset();

    QVERIFY(handle.terminalResult() != nullptr);
    QCOMPARE(handle.wait().outcome(), RunOutcome::Cancelled);
    QCOMPARE(handle.wait().phases().back().phase(), RunPhase::SafetyCleanup);
    QCOMPARE(scheduler.joins(), std::size_t{1});
}

void OptimizationRunServiceTests::waitingFromTheWorkerIsDiagnosed()
{
    GatedRunScheduler scheduler;
    OptimizationRunService service{scheduler};
    auto started = service.start(noWorkRequest());
    bool diagnosed = false;
    scheduler.beforeWork = [&] {
        try {
            static_cast<void>(started.handle()->wait());
        } catch (const std::logic_error&) {
            diagnosed = true;
        }
    };
    scheduler.release();

    QCOMPARE(started.handle()->wait().outcome(), RunOutcome::Succeeded);
    QVERIFY(diagnosed);
}

void OptimizationRunServiceTests::theDefaultServiceSchedulesAProductionRun()
{
    OptimizationRunService service;
    auto started = service.start(noWorkRequest());
    QCOMPARE(started.handle()->wait().outcome(), RunOutcome::Succeeded);
}

void OptimizationRunServiceTests::cancellationKeepsTheSlotUntilTerminalCommit()
{
    GatedRunScheduler scheduler;
    OptimizationRunService service{scheduler};
    CountingInlineScheduler nextScheduler;
    OptimizationRunService nextService{nextScheduler};
    auto started = service.start(noWorkRequest());
    started.handle()->requestCancellation();
    started.handle()->requestCancellation();
    auto blocked = nextService.start(noWorkRequest());
    const auto* beforeCommit = started.handle()->terminalResult();
    scheduler.release();

    QCOMPARE(blocked.startError(), std::optional{StartError::ActiveRun});
    QVERIFY(beforeCommit == nullptr);
    const auto& terminal = started.handle()->wait();
    QCOMPARE(terminal.outcome(), RunOutcome::Cancelled);
    QCOMPARE(terminal.phases().size(), std::size_t{2});
    QCOMPARE(terminal.phases().back().phase(), RunPhase::SafetyCleanup);
    auto next = nextService.start(noWorkRequest());
    QVERIFY(next.started());
    QCOMPARE(next.handle()->wait().outcome(), RunOutcome::Succeeded);
    started.handle()->requestCancellation();
    QVERIFY(started.handle()->terminalResult() == &terminal);
}

void OptimizationRunServiceTests::destructionFromTheWorkerIsDiagnosed()
{
    for (const auto& owner : {QStringLiteral("handle"), QStringLiteral("service"),
                              QStringLiteral("inline-service")}) {
        QProcess child;
        child.start(QCoreApplication::applicationFilePath(),
                    {QStringLiteral("--self-destruction"), owner});
        QVERIFY(child.waitForStarted());
        const bool finished = child.waitForFinished(5000);
        if (!finished) {
            child.kill();
            child.waitForFinished();
        }
        QVERIFY2(finished, "Worker-side destruction deadlocked instead of being diagnosed");
        QCOMPARE(child.exitCode(), 86);
        QVERIFY(child.readAllStandardError().contains("An Optimization Run cannot destroy"));
    }
}

void OptimizationRunServiceTests::terminalCommitReleasesTheSlotBeforeWorkerJoin()
{
    GatedRunScheduler scheduler;
    std::binary_semaphore committed{0};
    std::binary_semaphore finishWorker{0};
    scheduler.afterWork = [&] {
        committed.release();
        finishWorker.acquire();
    };
    OptimizationRunService service{scheduler};
    auto first = service.start(noWorkRequest());
    scheduler.release();
    committed.acquire();
    OptimizationRunService nextService;
    auto next = nextService.start(noWorkRequest());
    finishWorker.release();

    QVERIFY(first.handle()->terminalResult() != nullptr);
    QVERIFY(next.started());
    QCOMPARE(next.handle()->wait().outcome(), RunOutcome::Succeeded);
}

void OptimizationRunServiceTests::simultaneousStartsAdmitExactlyOneRun()
{
    GatedRunScheduler firstScheduler;
    GatedRunScheduler secondScheduler;
    OptimizationRunService firstService{firstScheduler};
    OptimizationRunService secondService{secondScheduler};
    std::barrier startTogether{3};
    std::optional<cao::run::RunStartResult> first;
    std::optional<cao::run::RunStartResult> second;
    std::jthread firstCaller([&] {
        startTogether.arrive_and_wait();
        first.emplace(firstService.start(noWorkRequest()));
    });
    std::jthread secondCaller([&] {
        startTogether.arrive_and_wait();
        second.emplace(secondService.start(noWorkRequest()));
    });
    startTogether.arrive_and_wait();
    firstCaller.join();
    secondCaller.join();
    firstScheduler.release();
    secondScheduler.release();

    QVERIFY(first->started() != second->started());
    const auto& rejected = first->started() ? *second : *first;
    QCOMPARE(rejected.startError(), std::optional{StartError::ActiveRun});
    auto& admitted = first->started() ? *first : *second;
    QCOMPARE(admitted.handle()->wait().outcome(), RunOutcome::Succeeded);
}

void OptimizationRunServiceTests::destroyingTheHandleRequestsCancellationBeforeJoin()
{
    DeferredRunScheduler scheduler;
    OptimizationRunService service{scheduler};
    std::optional<RunOutcome> outcome;
    {
        auto started = service.start(noWorkRequest());
        // This observer executes inside the join while the handle's destructor still retains
        // its state. Querying the public result proves cancellation reached the Run Executor.
        scheduler.afterWork = [&] { outcome = started.handle()->terminalResult()->outcome(); };
    }
    QCOMPARE(outcome, std::optional{RunOutcome::Cancelled});
}

void OptimizationRunServiceTests::schedulingFailureReleasesTheActiveSlot()
{
    ExhaustedRunScheduler scheduler;
    OptimizationRunService failedService{scheduler};
    auto failed = failedService.start(noWorkRequest());
    const auto* terminal = failed.handle()->terminalResult();
    OptimizationRunService nextService;
    auto next = nextService.start(noWorkRequest());

    QVERIFY(next.started());
    QCOMPARE(next.handle()->wait().outcome(), RunOutcome::Succeeded);
    QVERIFY(failed.handle()->terminalResult() == terminal);
    QCOMPARE(terminal->outcome(), RunOutcome::Failed);
}

/// Runs fatal lifetime violations in isolation so the parent can assert the diagnosis and timeout.
int main(int argc, char** argv)
{
    if (argc == 3 && std::string_view(argv[1]) == "--self-destruction") {
        if (std::string_view(argv[2]) == "inline-service") {
            // Inline delivery precedes worker publication, but still owes the same diagnosis.
            std::set_terminate([] { std::_Exit(86); });
            cao::run::InlineRunScheduler scheduler;
            auto service = std::make_unique<OptimizationRunService>(scheduler);
            auto started = service->start(noWorkRequest(),
                [&](const cao::run::RunEvent&) { service.reset(); });
            return 0;
        }
        GatedRunScheduler scheduler;
        auto service = std::make_unique<OptimizationRunService>(scheduler);
        auto started = service->start(noWorkRequest());
        std::optional<RunHandle> handle{std::move(*started.handle())};
        std::binary_semaphore finished{0};
        scheduler.beforeWork = [&] {
            // MSVC keeps the terminate handler per thread, so install it on the violating worker.
            // The parent also requires the diagnostic, rather than accepting any termination.
            std::set_terminate([] { std::_Exit(86); });
            if (std::string_view(argv[2]) == "handle") handle.reset();
            else service.reset();
        };
        scheduler.afterWork = [&] { finished.release(); };
        scheduler.release();
        finished.acquire();
        return 0;
    }
    QCoreApplication application(argc, argv);
    OptimizationRunServiceTests tests;
    return QTest::qExec(&tests, argc, argv);
}
#include "OptimizationRunServiceTests.moc"
