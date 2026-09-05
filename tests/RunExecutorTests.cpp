#include "Run/RunExecutor.h"
#include "RunTestConfiguration.h"

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
    return RunRequest::create("SkyrimSE", mode, ModSelection::singleModRoot(testModRoot()), {});
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
 /// Verifies Preparing loads owned facts and retains one canonical Mod Root and its policy.
 void preparingRetainsTheResolvedRootAndPolicy();
 /// Verifies invalid profile facts fail Preparing before any work phases and still clean up.
 void policyConflictsFailPreparing();
 /// Verifies missing and throwing configuration providers cannot escape cleanup or claim success.
 void configurationLoadingFailuresAreTerminal();
 /// Verifies a file cannot become a Mod Root even when its path canonicalizes successfully.
 void aNonDirectorySelectionFailsPreparing();
 /// Verifies explicit high-to-low Archive intent is owned by both request and prepared result.
 void archivePrecedenceIntentIsRetained();
 /// Verifies both modes leave Asset and Archive bytes and timestamps unchanged during Preparing.
 void preparingDoesNotMutateAssetsOrArchives();
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

    const auto result =
        executor.execute(noWorkRequest(ExecutionMode::Apply),
                         RunServices{cleanup, nullptr, testRunConfiguration().get()});

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

    const auto applied =
        executor.execute(noWorkRequest(ExecutionMode::Apply),
                         RunServices{applyCleanup, nullptr, testRunConfiguration().get()});
    const auto dryRun =
        executor.execute(noWorkRequest(ExecutionMode::DryRun),
                         RunServices{dryRunCleanup, nullptr, testRunConfiguration().get()});

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

    const auto result =
        executor.execute(noWorkRequest(ExecutionMode::Apply),
                         RunServices{cleanup, nullptr, testRunConfiguration().get()});

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

    const auto result =
        executor.execute(request, RunServices{cleanup, nullptr, testRunConfiguration().get()});

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

    const auto result =
        executor.execute(noWorkRequest(ExecutionMode::Apply),
                         RunServices{cleanup, nullptr, testRunConfiguration().get()});

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
        result =
            executor.execute(request, RunServices{cleanup, nullptr, testRunConfiguration().get()});
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
    const auto request = RunRequest::create("SkyrimSE", ExecutionMode::Apply,
                                            ModSelection::singleModRoot(testModRoot()),
                                            {RequestedWork::ArchiveExtraction});

    const auto result =
        executor.execute(request, RunServices{cleanup, nullptr, testRunConfiguration().get()});

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
        "SkyrimSE", ExecutionMode::Apply, ModSelection::singleModRoot(testModRoot()),
        {RequestedWork::ArchiveExtraction, RequestedWork::NativeTextureOptimization,
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

void RunExecutorTests::preparingRetainsTheResolvedRootAndPolicy() {
    class Configuration final : public cao::run::RunConfigurationProvider {
       public:
        /// Loads facts only for the requested profile, independently of application selection.
        cao::run::RunConfiguration load(std::string_view identity) const override {
            if (identity != "test-profile") throw std::runtime_error("Unexpected profile");
            return cao::run::RunConfiguration{
                cao::run::SelectedProfileFacts{.archiveExtension = ".BSA"}, {"ignored-mod"}};
        }
    };
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const std::filesystem::path root{directory.path().toStdWString()};
    std::optional<OptimizationRunResult> result;
    {
        const Configuration configuration;
        CountingSafetyCleanup cleanup;
        const auto request = RunRequest::create("test-profile", ExecutionMode::DryRun,
                                                ModSelection::singleModRoot(root / "."), {});
        result = RunExecutor{}.execute(request, RunServices{cleanup, nullptr, &configuration});
        QCOMPARE(cleanup.invocations(), std::size_t{1});
    }
    QCOMPARE(result->outcome(), RunOutcome::Succeeded);
    QVERIFY(result->preparation() != nullptr);
    QCOMPARE(result->preparation()->modRoots().size(), std::size_t{1});
    QCOMPARE(result->preparation()->modRoots().front(), std::filesystem::canonical(root));
    QCOMPARE(result->preparation()->policy().archiveExtension(), std::string(".bsa"));
    QCOMPARE(result->preparation()->policy().executionMode(), ExecutionMode::DryRun);
    QCOMPARE(result->preparation()->configuration().ignoredMods().front(),
             std::string("ignored-mod"));
}

void RunExecutorTests::policyConflictsFailPreparing() {
    class Configuration final : public cao::run::RunConfigurationProvider {
       public:
        /// Returns malformed loaded profile facts to exercise real Routing Policy validation.
        cao::run::RunConfiguration load(std::string_view) const override {
            return cao::run::RunConfiguration{
                cao::run::SelectedProfileFacts{.archiveExtension = ".dds"}};
        }
    } configuration;
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    CountingSafetyCleanup cleanup;
    const auto request = RunRequest::create(
        "invalid-profile", ExecutionMode::Apply,
        ModSelection::singleModRoot(std::filesystem::path(directory.path().toStdWString())), {});
    const auto result =
        RunExecutor{}.execute(request, RunServices{cleanup, nullptr, &configuration});
    QCOMPARE(result.outcome(), RunOutcome::Failed);
    QCOMPARE(result.finalPhase(), RunPhase::Preparing);
    QCOMPARE(result.failures().size(), std::size_t{1});
    QCOMPARE(result.failures().front().code(), cao::run::RunFailureCode::PolicyConflict);
    QCOMPARE(result.failures().front().policyConflicts().size(), std::size_t{1});
    QVERIFY(std::holds_alternative<cao::routing::AmbiguousArchiveExtension>(
        result.failures().front().policyConflicts().front()));
    QVERIFY(result.preparation() == nullptr);
    QCOMPARE(result.phases().size(), std::size_t{2});
    QCOMPARE(cleanup.invocations(), std::size_t{1});
}

void RunExecutorTests::configurationLoadingFailuresAreTerminal() {
    class Configuration final : public cao::run::RunConfigurationProvider {
       public:
        /// Simulates unavailable persistent profile data at the external loading boundary.
        cao::run::RunConfiguration load(std::string_view) const override {
            throw std::runtime_error("Profile file could not be read");
        }
    } configuration;
    for (const auto* provider :
         {static_cast<const cao::run::RunConfigurationProvider*>(nullptr),
          static_cast<const cao::run::RunConfigurationProvider*>(&configuration)}) {
        CountingSafetyCleanup cleanup;
        const auto result = RunExecutor{}.execute(noWorkRequest(ExecutionMode::Apply),
                                                  RunServices{cleanup, nullptr, provider});
        QCOMPARE(result.outcome(), RunOutcome::Failed);
        QCOMPARE(result.finalPhase(), RunPhase::Preparing);
        QCOMPARE(result.failures().size(), std::size_t{1});
        QVERIFY(!result.failures().front().detail().empty());
        QCOMPARE(cleanup.invocations(), std::size_t{1});
    }
}

void RunExecutorTests::aNonDirectorySelectionFailsPreparing() {
    QTemporaryFile file;
    QVERIFY(file.open());
    CountingSafetyCleanup cleanup;
    const auto request = RunRequest::create(
        "profile", ExecutionMode::Apply,
        ModSelection::singleModRoot(std::filesystem::path(file.fileName().toStdWString())), {});
    const auto result =
        RunExecutor{}.execute(request, RunServices{cleanup, nullptr, testRunConfiguration().get()});
    QCOMPARE(result.outcome(), RunOutcome::Failed);
    QCOMPARE(result.finalPhase(), RunPhase::Preparing);
    QVERIFY(result.preparation() == nullptr);
    QCOMPARE(result.failures().size(), std::size_t{1});
    QCOMPARE(cleanup.invocations(), std::size_t{1});
    QVERIFY(file.exists());
}

void RunExecutorTests::archivePrecedenceIntentIsRetained() {
    std::vector<std::filesystem::path> highToLow{"winner.bsa", "shadowed.bsa"};
    const auto request = RunRequest::create("profile", ExecutionMode::Apply,
                                            ModSelection::singleModRoot(testModRoot()), {},
                                            cao::run::ArchivePrecedence::explicitOrder(highToLow));
    highToLow.clear();
    CountingSafetyCleanup cleanup;
    const auto result =
        RunExecutor{}.execute(request, RunServices{cleanup, nullptr, testRunConfiguration().get()});
    QCOMPARE(result.outcome(), RunOutcome::Succeeded);
    QVERIFY(result.preparation() != nullptr);
    const auto& precedence = result.preparation()->archivePrecedence();
    QCOMPARE(precedence.mode(), cao::run::ArchivePrecedenceMode::ExplicitOrder);
    QCOMPARE(precedence.highToLow().size(), std::size_t{2});
    QCOMPARE(precedence.highToLow()[0], std::filesystem::path("winner.bsa"));
    QCOMPARE(precedence.highToLow()[1], std::filesystem::path("shadowed.bsa"));
    QCOMPARE(noWorkRequest(ExecutionMode::Apply).archivePrecedence().mode(),
             cao::run::ArchivePrecedenceMode::DeterministicDiscovery);
}

void RunExecutorTests::preparingDoesNotMutateAssetsOrArchives() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const std::filesystem::path root(directory.path().toStdWString());
    QFile texture(directory.filePath("asset.dds"));
    QFile archive(directory.filePath("source.bsa"));
    for (auto* file : {&texture, &archive}) {
        QVERIFY(file->open(QIODevice::WriteOnly));
        QCOMPARE(file->write("untouched sentinel"), qint64{18});
        file->close();
    }
    const auto textureTime = std::filesystem::last_write_time(root / "asset.dds");
    const auto archiveTime = std::filesystem::last_write_time(root / "source.bsa");
    for (const auto mode : {ExecutionMode::Apply, ExecutionMode::DryRun}) {
        CountingSafetyCleanup cleanup;
        const auto request = RunRequest::create("profile", mode, ModSelection::singleModRoot(root),
                                                {RequestedWork::ArchiveExtraction});
        const auto result = RunExecutor{}.execute(
            request, RunServices{cleanup, nullptr, testRunConfiguration().get()});
        QVERIFY(result.preparation() != nullptr);
        QCOMPARE(result.preparation()->policy().executionMode(), mode);
        QVERIFY(result.preparation()->policy().requests(RequestedWork::ArchiveExtraction));
        QCOMPARE(result.failures().front().code(),
                 cao::run::RunFailureCode::RequestedWorkUnavailable);
        QCOMPARE(std::filesystem::last_write_time(root / "asset.dds"), textureTime);
        QCOMPARE(std::filesystem::last_write_time(root / "source.bsa"), archiveTime);
        QCOMPARE(QDir(directory.path()).entryList(QDir::Files),
                 (QStringList{"asset.dds", "source.bsa"}));
        for (auto* file : {&texture, &archive}) {
            QVERIFY(file->open(QIODevice::ReadOnly));
            QCOMPARE(file->readAll(), QByteArray("untouched sentinel"));
            file->close();
        }
    }
}

QTEST_MAIN(RunExecutorTests)
#include "RunExecutorTests.moc"
