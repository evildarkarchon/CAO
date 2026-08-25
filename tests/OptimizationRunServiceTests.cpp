#include "Run/OptimizationRunService.h"

#include <QtTest>

#include <cstddef>
#include <filesystem>
#include <functional>
#include <memory>
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
            ++_owner._completions;
        }

    private:
        DeferredRunScheduler &_owner;
        std::function<void()> _work;
    };

    std::size_t _completions{};
    std::size_t _joins{};
};

/// Runs each worker on its own thread, so waiting is a real cross-thread wait.
class ThreadRunScheduler final : public RunScheduler
{
public:
    std::unique_ptr<ScheduledRunWorker> schedule(std::function<void()> work) override
    {
        return std::make_unique<ThreadWorker>(std::move(work));
    }

private:
    class ThreadWorker final : public ScheduledRunWorker
    {
    public:
        explicit ThreadWorker(std::function<void()> work)
            : _thread(std::move(work))
        {
        }

        // A started thread must be joined before it is destroyed, so the substitute joins as a
        // last resort even though the Run Handle is expected to have joined it already.
        ~ThreadWorker() override { ThreadWorker::join(); }

        void join() override
        {
            if (_thread.joinable())
                _thread.join();
        }

    private:
        std::thread _thread;
    };
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
};

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
    ThreadRunScheduler scheduler;
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

QTEST_MAIN(OptimizationRunServiceTests)
#include "OptimizationRunServiceTests.moc"
