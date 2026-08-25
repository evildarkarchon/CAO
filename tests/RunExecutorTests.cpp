#include "Run/RunExecutor.h"

#include <QtTest>

#include <algorithm>
#include <cstddef>
#include <filesystem>
#include <optional>
#include <utility>
#include <vector>

using cao::routing::ExecutionMode;
using cao::routing::RequestedWork;
using cao::run::ModSelection;
using cao::run::OptimizationRunResult;
using cao::run::PhaseSkipReason;
using cao::run::RunExecutor;
using cao::run::RunOutcome;
using cao::run::RunPhase;
using cao::run::RunPhaseRecord;
using cao::run::RunPhaseStatus;
using cao::run::RunProgress;
using cao::run::RunRequest;
using cao::run::RunServices;
using cao::run::runPhaseSequence;
using cao::run::SafetyCleanupService;

namespace
{
/// Counts Safety Cleanup invocations so tests can prove it happens exactly once per terminal path.
class CountingSafetyCleanup final : public SafetyCleanupService
{
public:
    void performSafetyCleanup() override { ++_invocations; }

    [[nodiscard]] std::size_t invocations() const noexcept { return _invocations; }

private:
    std::size_t _invocations{};
};

/// Builds a valid Run Request that selects no work at all.
RunRequest noWorkRequest(const ExecutionMode mode)
{
    return RunRequest::create("SkyrimSE",
                              mode,
                              ModSelection::singleModRoot(std::filesystem::path("mods/Example")),
                              {});
}

/// Returns the record for one phase, failing the test with context when the run never reached it.
const RunPhaseRecord &requirePhase(const OptimizationRunResult &result, const RunPhase phase)
{
    const auto *record = result.phase(phase);
    if (record == nullptr)
        qFatal("The run did not traverse an expected Run Phase");
    return *record;
}

/// Reports whether every traversed phase omits progress rather than inventing a total.
bool noPhaseReportsProgress(const OptimizationRunResult &result)
{
    return std::none_of(result.phases().begin(),
                        result.phases().end(),
                        [](const RunPhaseRecord &record) {
                            return record.progress().has_value();
                        });
}
}

class RunExecutorTests final : public QObject
{
    Q_OBJECT

private slots:
    /// Verifies a no-work Apply request traverses the canonical sequence with stable skip reasons.
    void noWorkApplyRunTraversesTheStablePhaseSequence();

    /// Verifies execution mode cannot change a run that was asked for no work at all.
    void noWorkRunReportsTheSameReasonsInEveryExecutionMode();

    /// Verifies Safety Cleanup runs exactly once and terminates the traversed phase sequence.
    void safetyCleanupRunsExactlyOnceBeforeTheTerminalResult();

    /// Verifies Safety Cleanup still runs exactly once when the run terminates as Failed.
    void safetyCleanupRunsOnEveryTerminalPath();

    /// Verifies a run that stops early records no phase it never reached.
    void aRunThatStopsEarlyRecordsOnlyThePhasesItTraversed();

    /// Verifies a no-work run succeeds while no phase invents a progress total.
    void noWorkRunSucceedsWithoutInventingProgressTotals();

    /// Verifies the terminal result stays readable after the run and its inputs are destroyed.
    void terminalResultOwnsItsDataAfterTheRunEnds();

    /// Verifies a request retains each closed work choice once in enumeration order.
    void requestedWorkIsADeduplicatedClosedSetInEnumerationOrder();

    /// Verifies determinate progress starts at zero against its immutable total.
    void determinateProgressStartsAtZeroAgainstAnImmutableTotal();

    /// Verifies completed attempts follow succeeded plus failed, so failures advance progress.
    void failedAttemptsAdvanceCompletedProgress();
};

void RunExecutorTests::noWorkApplyRunTraversesTheStablePhaseSequence()
{
    CountingSafetyCleanup cleanup;
    const RunExecutor executor;

    const auto result = executor.execute(noWorkRequest(ExecutionMode::Apply),
                                         RunServices{cleanup});

    std::vector<RunPhase> traversed;
    for (const auto &record : result.phases())
        traversed.push_back(record.phase());
    const std::vector<RunPhase> canonical(runPhaseSequence().begin(), runPhaseSequence().end());
    QCOMPARE(traversed, canonical);

    QCOMPARE(requirePhase(result, RunPhase::Preparing).status(), RunPhaseStatus::Executed);
    QVERIFY(!requirePhase(result, RunPhase::Preparing).skipReason().has_value());
    QCOMPARE(requirePhase(result, RunPhase::SafetyCleanup).status(), RunPhaseStatus::Executed);

    // Every work phase reports the one fact the run knows when it skips. A phase may never claim
    // the outcome of a phase that never ran, such as no Archives discovered by skipped discovery.
    for (const auto phase : {RunPhase::DiscoveringArchives,
                             RunPhase::ExtractingArchives,
                             RunPhase::BuildingEffectiveAssetTree,
                             RunPhase::ProcessingAssets,
                             RunPhase::ArchiveFinalization}) {
        const auto &record = requirePhase(result, phase);
        QCOMPARE(record.status(), RunPhaseStatus::Skipped);
        QCOMPARE(record.skipReason(), std::optional{PhaseSkipReason::NoRequestedWork});
    }

    QCOMPARE(result.finalPhase(), RunPhase::ArchiveFinalization);
}

void RunExecutorTests::noWorkRunReportsTheSameReasonsInEveryExecutionMode()
{
    CountingSafetyCleanup applyCleanup;
    CountingSafetyCleanup dryRunCleanup;
    const RunExecutor executor;

    const auto applied = executor.execute(noWorkRequest(ExecutionMode::Apply),
                                          RunServices{applyCleanup});
    const auto dryRun = executor.execute(noWorkRequest(ExecutionMode::DryRun),
                                         RunServices{dryRunCleanup});

    // A run asked for nothing is excluded by its empty request, not by its execution mode, so
    // Dry Run must not claim credit for excluding phases that had nothing to do either way.
    QCOMPARE(dryRun.outcome(), RunOutcome::Succeeded);
    QCOMPARE(dryRun.finalPhase(), applied.finalPhase());
    QCOMPARE(dryRun.phases().size(), applied.phases().size());
    for (std::size_t index = 0; index < dryRun.phases().size(); ++index) {
        QCOMPARE(dryRun.phases()[index].phase(), applied.phases()[index].phase());
        QCOMPARE(dryRun.phases()[index].status(), applied.phases()[index].status());
        QCOMPARE(dryRun.phases()[index].skipReason(), applied.phases()[index].skipReason());
    }
}

void RunExecutorTests::safetyCleanupRunsExactlyOnceBeforeTheTerminalResult()
{
    CountingSafetyCleanup cleanup;
    const RunExecutor executor;

    const auto result = executor.execute(noWorkRequest(ExecutionMode::Apply),
                                         RunServices{cleanup});

    QCOMPARE(cleanup.invocations(), std::size_t{1});
    QVERIFY(!result.phases().empty());
    QCOMPARE(result.phases().back().phase(), RunPhase::SafetyCleanup);
    QCOMPARE(result.phases().back().status(), RunPhaseStatus::Executed);
}

void RunExecutorTests::safetyCleanupRunsOnEveryTerminalPath()
{
    CountingSafetyCleanup cleanup;
    const RunExecutor executor;
    // Requested work is not executable at this seam yet, so the run must fail rather than claim
    // success. Safety Cleanup still owes the run its single terminal pass.
    const auto request = RunRequest::create(
        "SkyrimSE",
        ExecutionMode::Apply,
        ModSelection::childModRoots(std::filesystem::path("mods")),
        {RequestedWork::NativeTextureOptimization});

    const auto result = executor.execute(request, RunServices{cleanup});

    QCOMPARE(result.outcome(), RunOutcome::Failed);
    QCOMPARE(result.finalPhase(), RunPhase::Preparing);
    QCOMPARE(cleanup.invocations(), std::size_t{1});
    QCOMPARE(result.phases().back().phase(), RunPhase::SafetyCleanup);
    QCOMPARE(result.phases().back().status(), RunPhaseStatus::Executed);
}

void RunExecutorTests::noWorkRunSucceedsWithoutInventingProgressTotals()
{
    CountingSafetyCleanup cleanup;
    const RunExecutor executor;

    const auto result = executor.execute(noWorkRequest(ExecutionMode::Apply),
                                         RunServices{cleanup});

    QCOMPARE(result.outcome(), RunOutcome::Succeeded);
    QVERIFY(noPhaseReportsProgress(result));
}

void RunExecutorTests::terminalResultOwnsItsDataAfterTheRunEnds()
{
    std::optional<OptimizationRunResult> result;
    {
        CountingSafetyCleanup cleanup;
        const RunExecutor executor;
        const auto request = noWorkRequest(ExecutionMode::Apply);
        result = executor.execute(request, RunServices{cleanup});
    }

    QCOMPARE(result->outcome(), RunOutcome::Succeeded);
    QCOMPARE(result->phases().size(), runPhaseSequence().size());
    QCOMPARE(requirePhase(*result, RunPhase::ProcessingAssets).skipReason(),
             std::optional{PhaseSkipReason::NoRequestedWork});
}

void RunExecutorTests::aRunThatStopsEarlyRecordsOnlyThePhasesItTraversed()
{
    CountingSafetyCleanup cleanup;
    const RunExecutor executor;
    const auto request = RunRequest::create(
        "SkyrimSE",
        ExecutionMode::Apply,
        ModSelection::singleModRoot(std::filesystem::path("mods/Example")),
        {RequestedWork::ArchiveExtraction});

    const auto result = executor.execute(request, RunServices{cleanup});

    // A run that stopped at Preparing knows no reason the later phases were inapplicable, so it
    // must omit them rather than invent a skip reason for work it never considered.
    QCOMPARE(result.finalPhase(), RunPhase::Preparing);
    for (const auto phase : {RunPhase::DiscoveringArchives,
                             RunPhase::ExtractingArchives,
                             RunPhase::BuildingEffectiveAssetTree,
                             RunPhase::ProcessingAssets,
                             RunPhase::ArchiveFinalization})
        QVERIFY(result.phase(phase) == nullptr);

    QCOMPARE(result.phases().size(), std::size_t{2});
    QCOMPARE(result.phases().front().phase(), RunPhase::Preparing);
    QCOMPARE(result.phases().back().phase(), RunPhase::SafetyCleanup);
}

void RunExecutorTests::requestedWorkIsADeduplicatedClosedSetInEnumerationOrder()
{
    const auto request = RunRequest::create(
        "SkyrimSE",
        ExecutionMode::Apply,
        ModSelection::singleModRoot(std::filesystem::path("mods/Example")),
        {RequestedWork::ArchiveExtraction,
         RequestedWork::NativeTextureOptimization,
         RequestedWork::ArchiveExtraction});

    const std::vector<RequestedWork> retained(request.requestedWork().begin(),
                                              request.requestedWork().end());
    const std::vector expected{RequestedWork::NativeTextureOptimization,
                               RequestedWork::ArchiveExtraction};
    QCOMPARE(retained, expected);
    QVERIFY(request.hasRequestedWork());
    QVERIFY(request.requests(RequestedWork::ArchiveExtraction));
    QVERIFY(!request.requests(RequestedWork::AnimationOptimization));
    QVERIFY(!noWorkRequest(ExecutionMode::Apply).hasRequestedWork());
}

void RunExecutorTests::determinateProgressStartsAtZeroAgainstAnImmutableTotal()
{
    const auto progress = RunProgress::determinate(7);

    QCOMPARE(progress.total(), std::size_t{7});
    QCOMPARE(progress.completed(), std::size_t{0});
    QCOMPARE(progress.succeeded(), std::size_t{0});
    QCOMPARE(progress.failed(), std::size_t{0});
}

void RunExecutorTests::failedAttemptsAdvanceCompletedProgress()
{
    const auto progress = RunProgress::determinate(7, 2, 3);

    QCOMPARE(progress.total(), std::size_t{7});
    QCOMPARE(progress.succeeded(), std::size_t{2});
    QCOMPARE(progress.failed(), std::size_t{3});
    // Completed is derived, so a failed attempt can never stall the phase-local account.
    QCOMPARE(progress.completed(), std::size_t{5});
}

QTEST_MAIN(RunExecutorTests)
#include "RunExecutorTests.moc"
