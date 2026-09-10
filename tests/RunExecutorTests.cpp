#include "Run/RunExecutor.h"
#include "Run/AssetRun.h"
#include "Run/RunWorkRecord.h"
#include "Run/TemporaryArtifactRegistry.h"
#include "Run/StagingRecovery.h"
#include "RunTestConfiguration.h"

#include <QtTest>

#include <btu/bsa/archive_data.hpp>
#include <btu/bsa/pack.hpp>
#include <btu/bsa/settings.hpp>

#include <algorithm>
#include <cstddef>
#include <filesystem>
#include <fstream>
#include <iomanip>
#include <sstream>
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
const std::string staleChildName = "run-407-0123456789abcdef0123456789abcdef";

/// Builds a literal v1 ownership fixture independently of the recovery parser.
std::string staleManifest(const std::filesystem::path& root) {
    const auto utf8 = root.generic_u8string();
    std::ostringstream manifest;
    manifest << "CAO-STAGING 1\n" << std::quoted(std::string(utf8.begin(), utf8.end())) << '\n'
             << "\"407\" \"" << staleChildName << "\"\n2\nD \"" << staleChildName
             << "\"\nF \"" << staleChildName << "/temporary.dds\"\n";
    return manifest.str();
}

/// Seeds abandoned registered staging and returns its child; the lock file is present but unlocked.
std::filesystem::path seedStaleStaging(const std::filesystem::path& root) {
    const auto staging = root / ".cao-staging";
    const auto child = staging / staleChildName;
    std::filesystem::create_directories(child);
    std::ofstream(child / "temporary.dds") << "temporary";
    std::ofstream(staging / "owner.lock");
    std::ofstream(staging / "ownership.manifest", std::ios::binary) << staleManifest(root);
    return child;
}

/// Reads fixture bytes for preservation assertions, including truncated or invalid manifests.
QByteArray stagingBytes(const std::filesystem::path& path) {
    QFile file(QString::fromStdWString(path.wstring()));
    if (!file.open(QIODevice::ReadOnly)) qFatal("Could not read staging fixture");
    return file.readAll();
}

/// Counts Safety Cleanup invocations so tests can prove it happens exactly once per terminal path.
class CountingSafetyCleanup final : public SafetyCleanupService
{
public:
    /// Records one pass and reports an empty failure set.
    std::vector<cao::run::RunFailure> performSafetyCleanup() override {
        ++_invocations;
        return {};
    }

    [[nodiscard]] std::size_t invocations() const noexcept { return _invocations; }

private:
    std::size_t _invocations{};
};

/// Exercises production work composition with controlled operation boundaries and real staging.
class ControlledAssetWork final : public cao::run::RunWorkService {
   public:
    cao::run::AssetRunAdapters adapters;
    std::filesystem::path staged;

    /// Creates temporary evidence under the executor's ownership, then delegates all recording.
    void execute(const cao::run::RunPreparation& preparation, cao::run::RunWorkRecord& record,
                 cao::run::TemporaryArtifactRegistry& artifacts,
                 cao::run::RunObservationSink& observations, std::stop_token stop) override {
        if (preparation.policy().executionMode() == ExecutionMode::Apply) {
            staged = artifacts.stageFile(preparation.modRoots().front(),
                                         preparation.modRoots().front() / "temporary.dds").path;
            std::ofstream(staged) << "temporary";
        }
        cao::run::executeAssetRun(preparation, record, observations, stop, adapters);
    }
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
 /// Covers canonical identity for single and Several Mods through relative selections.
 void processingAndEvidenceShareModRoot_data();
 /// Detects missing or divergent roots supplied to operations beneath production recording.
 void processingAndEvidenceShareModRoot();
 /// Covers later fatal failure and cancellation after source-removing conversion.
 void removedConversionRetainsModRoot_data();
 /// Keeps processing scope and committed mutation evidence after the original Asset disappears.
 void removedConversionRetainsModRoot();
 /// Covers canonical reassignment and rejection when a discovered Asset is retargeted.
 void retargetedAssetUsesCanonicalContainment_data();
 /// Resolves scope at the operation boundary and never processes an Asset outside prepared roots.
 void retargetedAssetUsesCanonicalContainment();
 /// Covers applicable-but-empty work, Dry Run evaluation, and fatal Archive preflight.
 void productionWorkApplicability_data();
 /// Uses the real composition to preserve phase distinctions and prohibit excluded operations.
 void productionWorkApplicability();
 /// Retains Preparing exclusions across completed production work without a presentation sink.
 void preparingDiagnosticsSurviveWork();
 /// Owns complete Archive collision evidence after preparation and adapter lifetimes end.
 void productionWorkRetainsArchiveCollisions();
 /// Covers diagnostic cancellation and exceptions during Preparing and work publication.
 void discoveryDiagnosticCancellationFollowsAssetAttempt_data();
 /// Publishes discovery diagnostics after Asset processing and observes cancellation before packing.
 void discoveryDiagnosticCancellationFollowsAssetAttempt();
 /// Covers progress exceptions with and without concurrent observer cancellation.
 void throwingWorkObserversRetainEvidence_data();
 /// Keeps one failed Asset attempt when downstream progress throws.
 void throwingWorkObserversRetainEvidence();
 /// Retains and publishes a fatal preflight failure once when its observer throws.
 void throwingPreflightFailureObserverRetainsEvidence();
 /// Retains mixed Asset and aggregate finalization evidence after every borrowed dependency dies.
 void mixedWorkEvidenceOutlivesServices();
 /// Covers cancellation at the last protected attempt and concurrent unsafe mutation.
 void cancellationAfterAtomicAssetAttempt_data();
 /// Counts the completed attempt before stopping without reaching Archive finalization.
 void cancellationAfterAtomicAssetAttempt();
 /// Retains successes and recoverable failures in the Archive phase's independent progress.
 void mixedExtractionAttemptsAdvanceProgress();
 /// Covers completed extraction evidence across completion, cancellation, and interruption.
 void committedExtractionSurvivesDiscoveryInterruption_data();
 /// A completed extraction remains exactly one durable attempt across later discovery boundaries.
 void committedExtractionSurvivesDiscoveryInterruption();
 /// Keeps discovered exclusions when discovery unwinds after a committed extraction.
 void discoveryDiagnosticsSurviveInterruption();
 /// Work configuration loading fails Preparing before stale artifacts are recovered.
 void workPreparationFailurePreservesStaleArtifacts();
 /// Exercises shared work composition after stale recovery on completion and orchestration failure.
 void workArtifactsShareRecoveryAndAreCleanedAfterFailure_data();
 /// Verifies committed Assets survive while executor-owned temporary material is cleaned once.
 void workArtifactsShareRecoveryAndAreCleanedAfterFailure();
 /// Verifies a fatal primary failure retains cancellation observed during mandatory cleanup.
 void fatalFailureRetainsConcurrentCancellation();
 /// Verifies cleanup exceptions cannot replace cancellation, including cancellation during cleanup.
 void cleanupExceptionsPreserveCancellation();
 /// Exercises the shared terminal boundary with independent work, cancellation, and cleanup facts.
 void terminalPrecedenceRetainsAllEvidence();
 /// Verifies the filesystem recovery seam observes cancellation before attempting a deletion.
 void cancelledRecoveryPreservesUnattemptedArtifacts();
 /// Verifies linked staging and hard-linked control files never authorize external deletion.
 void linkedStagingIsPreserved();
 /// Verifies a deletion error stops work, retains the remaining artifact, and still cleans up.
 void recoveryFailureStillPerformsSafetyCleanup();
 /// Exercises malformed, mismatched, aliased, and unknown ownership without deleting any contents.
 void unverifiableStagingIsPreserved_data();
 /// Verifies every malformed fixture remains byte-for-byte intact after Preparing fails.
 void unverifiableStagingIsPreserved();
 /// Verifies Dry Run does not recover, rewrite, or create staging, even with valid stale ownership.
 void dryRunLeavesStagingUntouched();
 /// Verifies a separate process's real OS lock blocks Apply, then its exit permits recovery.
 void activeStagingBlocksUntilItsOwnerExits();
 /// Verifies the recovery lock remains held throughout the mandatory Safety Cleanup pass.
 void recoveryLockSurvivesThroughSafetyCleanup();
 /// Verifies versioned ownership recovers registered stale entries before work phases begin.
 void verifiedStaleStagingIsRecoveredBeforeWork();
 /// Verifies a reserved staging name never authorizes deleting user material during Preparing.
 void unownedStagingBlocksApplyAndRemainsUntouched();
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

 /// Verifies cleanup owns paths before creation and releases only explicitly committed output.
 void registeredArtifactsAreCleanedAndCommittedOutputSurvives();

 /// Verifies retained children survive, every cleanup is attempted, and failures remain terminal data.
 void cleanupFailuresAreAggregatedWithoutDeletingRetainedMaterial();

 /// Verifies reverse-order directory cleanup and once-only ownership on success, failure, and cancel.
 void registeredArtifactsAreCleanedOnEveryTerminalPath();

 /// Verifies cleanup errors remain secondary to a fatal failure or observed cancellation.
 void cleanupFailuresPreserveThePrimaryOutcome();

 /// Verifies an unexpected cleanup exception becomes a terminal service-contract failure.
 void cleanupServiceExceptionsAreTerminal();

 /// Verifies existing paths, aliases, and foreign receipts cannot transfer cleanup ownership.
 void artifactRegistrationRejectsUnownedPaths();

 /// Verifies a replaced parent cannot redirect registered cleanup into unrelated material.
 void cleanupDoesNotFollowAReplacedParent();

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

void RunExecutorTests::discoveryDiagnosticCancellationFollowsAssetAttempt_data() {
    QTest::addColumn<bool>("throwPreparing");
    QTest::addColumn<bool>("throwWork");
    QTest::addColumn<bool>("cancelWork");
    QTest::newRow("cancel-diagnostic") << false << false << true;
    QTest::newRow("throw-diagnostics") << true << true << false;
    QTest::newRow("cancel-then-throw-diagnostic") << true << true << true;
}

void RunExecutorTests::discoveryDiagnosticCancellationFollowsAssetAttempt() {
    QFETCH(bool, throwPreparing);
    QFETCH(bool, throwWork);
    QFETCH(bool, cancelWork);
    class Configuration final : public cao::run::RunConfigurationProvider {
       public:
        /// Supplies a Preparing exclusion to verify it survives later discovery publication.
        cao::run::RunConfiguration load(std::string_view) const override {
            return cao::run::RunConfiguration(testRunConfiguration()->load("SkyrimSE").profile(),
                                               {"ignored"});
        }
    } configuration;
    class Observer final : public cao::run::RunObservationSink {
       public:
        std::stop_source cancellation;
        bool assetCompleted{};
        bool assetCompletedBeforeDiagnostic{};
        std::size_t linkedDiagnostics{};
        bool throwPreparing{};
        bool throwWork{};
        bool cancelWork{};
        QStringList order;
        /// Marks mandatory cleanup after all work observations have been delivered.
        void recordPhase(const RunPhaseRecord& phase) override {
            if (phase.phase() == RunPhase::SafetyCleanup) order << "cleanup";
        }
        /// This fixture expects no run-level failures.
        void recordFailure(const cao::run::RunFailure&) override {}
        /// Exercises Preparing exceptions and optional cancellation before a work diagnostic throws.
        void recordDiagnostic(const cao::run::RunDiagnostic& diagnostic) override {
            if (diagnostic.code() == cao::run::RunDiagnosticCode::IgnoredModExcluded) {
                order << "preparing";
                if (throwPreparing) throw std::runtime_error("Preparing observer failed");
            }
            if (diagnostic.code() != cao::run::RunDiagnosticCode::LinkedEntryExcluded) return;
            order << "work-diagnostic";
            ++linkedDiagnostics;
            assetCompletedBeforeDiagnostic = assetCompleted;
            if (cancelWork) cancellation.request_stop();
            if (throwWork) throw std::runtime_error("work diagnostic observer failed");
        }
    } observer;
    observer.throwPreparing = throwPreparing;
    observer.throwWork = throwWork;
    observer.cancelWork = cancelWork;
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = std::filesystem::canonical(std::filesystem::path(directory.path().toStdWString()));
    const auto selected = root / "selected";
    std::filesystem::create_directory(selected);
    std::filesystem::create_directory(root / "ignored");
    std::ofstream(selected / "asset.dds") << "original";
    std::ofstream(root / "ignored" / "external.dds") << "outside selected root";
    std::error_code error;
    // Contained file links are valid Assets; an ignored sibling target exercises exclusion.
    std::filesystem::create_symlink(root / "ignored" / "external.dds", selected / "linked.dds", error);
    if (error) QSKIP("File symlink creation is unavailable on this host");
    ControlledAssetWork work;
    work.adapters.executeAssetWithResult = [&](const cao::routing::RoutedAsset&, const std::filesystem::path&) {
        observer.assetCompleted = true;
        observer.order << "asset";
        return cao::execution::AssetExecutionResult::success();
    };
    std::size_t finalizations = 0;
    work.adapters.finalizeArchiveLifecycleWithResult = [&] {
        ++finalizations;
        return cao::run::ArchiveFinalizationResult{};
    };
    CountingSafetyCleanup cleanup;
    const auto request = RunRequest::create("SkyrimSE", ExecutionMode::Apply,
        ModSelection::childModRoots(root), {RequestedWork::NativeTextureOptimization});
    const auto result = RunExecutor{}.execute(request,
        RunServices{cleanup, &observer, &configuration, &work}, observer.cancellation.get_token());
    // Remove the link before QTemporaryDir cleanup, which does not handle Windows links.
    QVERIFY(std::filesystem::remove(selected / "linked.dds"));
    observer.order << "completed";
    QCOMPARE(result.outcome(), cancelWork ? RunOutcome::Cancelled : RunOutcome::Succeeded);
    QCOMPARE(observer.order, QStringList({"preparing", "asset", "work-diagnostic", "cleanup", "completed"}));
    QVERIFY(observer.assetCompletedBeforeDiagnostic);
    QCOMPARE(observer.linkedDiagnostics, std::size_t{1});
    QCOMPARE(result.work().assetAttempts.size(), std::size_t{1});
    QCOMPARE(finalizations, cancelWork ? std::size_t{0} : std::size_t{1});
    QCOMPARE(result.phase(RunPhase::ArchiveFinalization) == nullptr, cancelWork);
    const auto countDiagnostic = [&](cao::run::RunDiagnosticCode code) {
        return std::count_if(result.work().diagnostics.begin(), result.work().diagnostics.end(),
                             [=](const auto& diagnostic) { return diagnostic.code() == code; });
    };
    QCOMPARE(countDiagnostic(cao::run::RunDiagnosticCode::IgnoredModExcluded), 1);
    QCOMPARE(countDiagnostic(cao::run::RunDiagnosticCode::LinkedEntryExcluded), 1);
    QCOMPARE(countDiagnostic(cao::run::RunDiagnosticCode::ObserverFailed),
             static_cast<int>(throwPreparing) + static_cast<int>(throwWork));
    QVERIFY(result.failures().empty());
    QCOMPARE(cleanup.invocations(), std::size_t{1});
    QVERIFY(!std::filesystem::exists(work.staged));
}

void RunExecutorTests::throwingWorkObserversRetainEvidence_data() {
    QTest::addColumn<bool>("cancel");
    QTest::newRow("throw-progress") << false;
    QTest::newRow("cancel-then-throw-progress") << true;
}

void RunExecutorTests::throwingWorkObserversRetainEvidence() {
    QFETCH(bool, cancel);
    class Observer final : public cao::run::RunObservationSink {
       public:
        std::stop_source cancellation;
        bool cancel{};
        std::size_t progressCalls{};
        std::size_t failureCalls{};
        /// Throws only after the failed Asset has been recorded, at its completed progress boundary.
        void recordPhase(const RunPhaseRecord& phase) override {
            if (phase.phase() != RunPhase::ProcessingAssets || !phase.progress()
                || phase.progress()->completed() != 1) return;
            ++progressCalls;
            if (cancel) cancellation.request_stop();
            throw std::runtime_error("completed progress observer failed");
        }
        /// Counts unexpected run-level failures separately from the failed Asset operation.
        void recordFailure(const cao::run::RunFailure&) override { ++failureCalls; }
        /// Informational observer errors do not request further work or cancellation.
        void recordDiagnostic(const cao::run::RunDiagnostic&) override {}
    } observer;
    observer.cancel = cancel;
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = std::filesystem::canonical(std::filesystem::path(directory.path().toStdWString()));
    std::ofstream(root / "asset.dds") << "original";
    ControlledAssetWork work;
    std::size_t attempts{};
    std::size_t finalizations{};
    work.adapters.executeAssetWithResult = [&](const cao::routing::RoutedAsset& asset,
                                               const std::filesystem::path&) {
        ++attempts;
        return cao::execution::AssetExecutionResult::failed(
            cao::execution::AssetExecutionFailure::CommitFailed, "controlled recoverable failure",
            cao::execution::MutationState::None, true, asset.executionPath());
    };
    work.adapters.finalizeArchiveLifecycleWithResult = [&] {
        ++finalizations;
        return cao::run::ArchiveFinalizationResult{};
    };
    CountingSafetyCleanup cleanup;
    const auto configuration = testRunConfiguration();
    const auto request = RunRequest::create("SkyrimSE", ExecutionMode::Apply,
        ModSelection::singleModRoot(root), {RequestedWork::NativeTextureOptimization});
    const auto result = RunExecutor{}.execute(request,
        RunServices{cleanup, &observer, configuration.get(), &work}, observer.cancellation.get_token());
    QCOMPARE(result.outcome(), cancel ? RunOutcome::Cancelled : RunOutcome::CompletedWithFailures);
    QCOMPARE(attempts, std::size_t{1});
    QCOMPARE(observer.progressCalls, std::size_t{1});
    QCOMPARE(observer.failureCalls, std::size_t{0});
    QCOMPARE(result.work().assetAttempts.size(), std::size_t{1});
    QVERIFY(result.work().failures.empty());
    QVERIFY(result.failures().empty());
    const auto& attempt = result.work().assetAttempts.front().result;
    QCOMPARE(attempt.failure(), std::optional{cao::execution::AssetExecutionFailure::CommitFailed});
    QCOMPARE(attempt.message(), std::string("controlled recoverable failure"));
    const auto& progress = requirePhase(result, RunPhase::ProcessingAssets).progress();
    QVERIFY(progress.has_value());
    QCOMPARE(progress->completed(), std::size_t{1});
    QCOMPARE(progress->failed(), std::size_t{1});
    QCOMPARE(std::count_if(result.work().diagnostics.begin(), result.work().diagnostics.end(),
        [](const auto& diagnostic) { return diagnostic.code() == cao::run::RunDiagnosticCode::ObserverFailed; }), 1);
    QCOMPARE(finalizations, cancel ? std::size_t{0} : std::size_t{1});
    QCOMPARE(cleanup.invocations(), std::size_t{1});
    QVERIFY(!std::filesystem::exists(work.staged));
    QCOMPARE(stagingBytes(root / "asset.dds"), QByteArray("original"));
}

void RunExecutorTests::throwingPreflightFailureObserverRetainsEvidence() {
    class Observer final : public cao::run::RunObservationSink {
       public:
        std::size_t failureCalls{};
        std::vector<cao::run::RunFailureCode> failures;
        /// The executor's returned phase records supply lifecycle assertions for this fixture.
        void recordPhase(const RunPhaseRecord&) override {}
        /// Fails after accepting preflight evidence so delivery must never be retried.
        void recordFailure(const cao::run::RunFailure& failure) override {
            ++failureCalls;
            failures.push_back(failure.code());
            throw std::runtime_error("preflight failure observer failed");
        }
        /// Observer failures are informational and must not trigger another failure callback.
        void recordDiagnostic(const cao::run::RunDiagnostic&) override {}
    } observer;
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = std::filesystem::canonical(std::filesystem::path(directory.path().toStdWString()));
    std::ofstream(root / "broken.bsa") << "invalid archive";
    ControlledAssetWork work;
    std::size_t extractions{};
    std::size_t finalizations{};
    work.adapters.extractArchiveWithResult = [&](const cao::run::ArchiveExtractionPlan& plan) {
        ++extractions;
        return cao::run::ArchiveExtractionResult{plan.archivePath};
    };
    work.adapters.finalizeArchiveLifecycleWithResult = [&] {
        ++finalizations;
        return cao::run::ArchiveFinalizationResult{};
    };
    CountingSafetyCleanup cleanup;
    const auto configuration = testRunConfiguration();
    const auto request = RunRequest::create("SkyrimSE", ExecutionMode::Apply,
        ModSelection::singleModRoot(root), {RequestedWork::ArchiveExtraction});
    const auto result = RunExecutor{}.execute(request,
        RunServices{cleanup, &observer, configuration.get(), &work});
    QCOMPARE(result.outcome(), RunOutcome::Failed);
    QCOMPARE(observer.failureCalls, std::size_t{1});
    QCOMPARE(observer.failures.front(), cao::run::RunFailureCode::ArchiveUnreadable);
    QCOMPARE(result.work().failures.size(), std::size_t{1});
    QCOMPARE(result.failures().size(), std::size_t{1});
    QCOMPARE(result.failures().front().code(), cao::run::RunFailureCode::ArchiveUnreadable);
    QCOMPARE(std::count_if(result.work().diagnostics.begin(), result.work().diagnostics.end(),
        [](const auto& diagnostic) { return diagnostic.code() == cao::run::RunDiagnosticCode::ObserverFailed; }), 1);
    QCOMPARE(extractions, std::size_t{0});
    QCOMPARE(finalizations, std::size_t{0});
    QVERIFY(result.work().archiveAttempts.empty());
    QVERIFY(result.phase(RunPhase::ArchiveFinalization) == nullptr);
    QCOMPARE(cleanup.invocations(), std::size_t{1});
    QVERIFY(!std::filesystem::exists(work.staged));
    QCOMPARE(stagingBytes(root / "broken.bsa"), QByteArray("invalid archive"));
}

void RunExecutorTests::productionWorkApplicability_data() {
    QTest::addColumn<QString>("scenario");
    QTest::newRow("empty-requested-work") << QString("empty");
    QTest::newRow("dry-run-evaluation") << QString("dry");
    QTest::newRow("fatal-archive-preflight") << QString("preflight");
}

void RunExecutorTests::productionWorkApplicability() {
    QFETCH(QString, scenario);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = std::filesystem::canonical(std::filesystem::path(directory.path().toStdWString()));
    const bool dryRun = scenario == "dry";
    const bool preflight = scenario == "preflight";
    if (dryRun) std::ofstream(root / "asset.dds") << "untouched";
    if (preflight) std::ofstream(root / "broken.bsa") << "invalid archive";
    ControlledAssetWork work;
    std::size_t assets = 0;
    std::size_t extractions = 0;
    std::size_t finalizations = 0;
    work.adapters.executeAssetWithResult = [&](const cao::routing::RoutedAsset&, const std::filesystem::path&) {
        ++assets;
        return cao::execution::AssetExecutionResult::success();
    };
    work.adapters.extractArchiveWithResult = [&](const cao::run::ArchiveExtractionPlan& plan) {
        ++extractions;
        return cao::run::ArchiveExtractionResult{plan.archivePath};
    };
    if (dryRun || preflight) {
        work.adapters.finalizeArchiveLifecycleWithResult = [&] {
            ++finalizations;
            return cao::run::ArchiveFinalizationResult{};
        };
    }
    CountingSafetyCleanup cleanup;
    const auto configuration = testRunConfiguration();
    const auto request = RunRequest::create("SkyrimSE", dryRun ? ExecutionMode::DryRun : ExecutionMode::Apply,
        ModSelection::singleModRoot(root),
        {RequestedWork::NativeTextureOptimization, RequestedWork::ArchiveExtraction});
    const auto result = RunExecutor{}.execute(request, RunServices{cleanup, nullptr, configuration.get(), &work});
    QCOMPARE(result.outcome(), preflight ? RunOutcome::Failed : RunOutcome::Succeeded);
    QCOMPARE(assets, dryRun ? std::size_t{1} : std::size_t{0});
    QCOMPARE(extractions, std::size_t{0});
    QCOMPARE(finalizations, std::size_t{0});
    QVERIFY(result.work().finalizations.empty());
    QVERIFY(result.work().archiveAttempts.empty());
    QVERIFY(result.mutationSummaries().empty());
    QCOMPARE(cleanup.invocations(), std::size_t{1});
    if (preflight) {
        QCOMPARE(result.failures().size(), std::size_t{1});
        QCOMPARE(result.failures().front().code(), cao::run::RunFailureCode::ArchiveUnreadable);
        QVERIFY(!result.work().ledger.has_value());
        QVERIFY(result.phase(RunPhase::ProcessingAssets) == nullptr);
        QVERIFY(result.phase(RunPhase::ArchiveFinalization) == nullptr);
        QCOMPARE(stagingBytes(root / "broken.bsa"), QByteArray("invalid archive"));
    } else {
        QVERIFY(result.work().ledger.has_value());
        const auto& phase = requirePhase(result, RunPhase::ProcessingAssets);
        QCOMPARE(phase.status(), RunPhaseStatus::Executed);
        QVERIFY(phase.progress().has_value());
        QCOMPARE(phase.progress()->total(), dryRun ? std::size_t{1} : std::size_t{0});
        QCOMPARE(phase.progress()->completed(), phase.progress()->total());
        QCOMPARE(requirePhase(result, RunPhase::ArchiveFinalization).skipReason(),
                 std::optional{dryRun ? PhaseSkipReason::DryRun : PhaseSkipReason::NoRequestedWork});
    }
    if (dryRun) {
        QVERIFY(work.staged.empty());
        QVERIFY(!std::filesystem::exists(root / ".cao-staging"));
        QCOMPARE(stagingBytes(root / "asset.dds"), QByteArray("untouched"));
    } else {
        QVERIFY(!work.staged.empty());
        QVERIFY(!std::filesystem::exists(work.staged));
    }
}

void RunExecutorTests::preparingDiagnosticsSurviveWork() {
    class Configuration final : public cao::run::RunConfigurationProvider {
       public:
        /// Loads one child exclusion alongside capabilities for actual production work.
        cao::run::RunConfiguration load(std::string_view) const override {
            return cao::run::RunConfiguration(testRunConfiguration()->load("SkyrimSE").profile(),
                                               {"ignored"});
        }
    };
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = std::filesystem::canonical(std::filesystem::path(directory.path().toStdWString()));
    std::filesystem::create_directory(root / "ignored");
    std::filesystem::create_directory(root / "selected");
    std::optional<OptimizationRunResult> result;
    {
        Configuration configuration;
        CountingSafetyCleanup cleanup;
        ControlledAssetWork work;
        const auto request = RunRequest::create("SkyrimSE", ExecutionMode::Apply,
            ModSelection::childModRoots(root), {RequestedWork::NativeTextureOptimization});
        result = RunExecutor{}.execute(request, RunServices{cleanup, nullptr, &configuration, &work});
        QCOMPARE(cleanup.invocations(), std::size_t{1});
    }
    QCOMPARE(result->outcome(), RunOutcome::Succeeded);
    QVERIFY(result->work().ledger.has_value());
    QCOMPARE(result->work().diagnostics.size(), std::size_t{1});
    const auto& diagnostic = result->work().diagnostics.front();
    QCOMPARE(diagnostic.code(), cao::run::RunDiagnosticCode::IgnoredModExcluded);
    QCOMPARE(diagnostic.phase(), RunPhase::Preparing);
    QCOMPARE(diagnostic.path(), root / "ignored");
    QVERIFY(!diagnostic.detail().empty());
}

void RunExecutorTests::productionWorkRetainsArchiveCollisions() {
    QTemporaryDir directory;
    QTemporaryDir sources;
    QVERIFY(directory.isValid());
    QVERIFY(sources.isValid());
    const auto root = std::filesystem::canonical(std::filesystem::path(directory.path().toStdWString()));
    const auto source = std::filesystem::path(sources.path().toStdWString());
    std::ofstream(source / "shared.dds") << "archived";
    for (const auto* name : {"winner.bsa", "shadowed.bsa"}) {
        auto archive = btu::bsa::ArchiveData(btu::bsa::Settings::get(btu::Game::SSE),
                                           btu::bsa::ArchiveType::Textures);
        QVERIFY(archive.add_file(source / "shared.dds"));
        archive.set_out_path(root / name);
        QVERIFY(btu::bsa::write(false, std::move(archive), source).empty());
    }
    std::optional<OptimizationRunResult> result;
    {
        ControlledAssetWork work;
        work.adapters.extractArchiveWithResult = [](const cao::run::ArchiveExtractionPlan& plan) {
            return cao::run::ArchiveExtractionResult{plan.archivePath};
        };
        CountingSafetyCleanup cleanup;
        const auto configuration = testRunConfiguration();
        const auto request = RunRequest::create("SkyrimSE", ExecutionMode::Apply,
            ModSelection::singleModRoot(root), {RequestedWork::ArchiveExtraction},
            cao::run::ArchivePrecedence::explicitOrder({"winner.bsa", "shadowed.bsa"}));
        result = RunExecutor{}.execute(request, RunServices{cleanup, nullptr, configuration.get(), &work});
        QCOMPARE(cleanup.invocations(), std::size_t{1});
    }
    QCOMPARE(result->outcome(), RunOutcome::Succeeded);
    QCOMPARE(result->work().archiveAttempts.size(), std::size_t{2});
    QCOMPARE(result->work().collisions.size(), std::size_t{1});
    const auto& collision = result->work().collisions.front();
    QCOMPARE(collision.modRoot(), root);
    QCOMPARE(collision.gamePath(), std::filesystem::path("shared.dds"));
    QCOMPARE(collision.winningArchive(), root / "winner.bsa");
    QCOMPARE(collision.shadowedArchives().size(), std::size_t{1});
    QCOMPARE(collision.shadowedArchives().front(), root / "shadowed.bsa");
    QVERIFY(!collision.looseAssetWins());
}

void RunExecutorTests::processingAndEvidenceShareModRoot_data() {
    QTest::addColumn<bool>("several");
    QTest::addColumn<bool>("relative");
    QTest::newRow("single-absolute") << false << false;
    QTest::newRow("single-relative") << false << true;
    QTest::newRow("several-absolute") << true << false;
    QTest::newRow("several-relative") << true << true;
}

void RunExecutorTests::processingAndEvidenceShareModRoot() {
    QFETCH(bool, several);
    QFETCH(bool, relative);
    QTemporaryDir directory(QDir::currentPath() + "/executor-scope-XXXXXX");
    QVERIFY(directory.isValid());
    const auto selected = std::filesystem::canonical(
        std::filesystem::path(directory.path().toStdWString()));
    const std::vector<std::filesystem::path> roots = several
        ? std::vector{selected / "first", selected / "second"} : std::vector{selected};
    for (const auto& root : roots) {
        std::filesystem::create_directories(root / "textures");
        std::ofstream(root / "textures" / "asset.dds") << "original";
    }
    ControlledAssetWork work;
    std::vector<std::filesystem::path> processingRoots;
    work.adapters.executeAssetWithResult = [&](const cao::routing::RoutedAsset&,
                                               const std::filesystem::path& modRoot) {
        processingRoots.push_back(modRoot);
        return cao::execution::AssetExecutionResult::success();
    };
    const auto selection = relative
        ? std::filesystem::relative(selected, std::filesystem::current_path()) : selected;
    const auto request = RunRequest::create("SkyrimSE", ExecutionMode::Apply,
        several ? ModSelection::childModRoots(selection) : ModSelection::singleModRoot(selection),
        {RequestedWork::NativeTextureOptimization});
    CountingSafetyCleanup cleanup;
    const auto configuration = testRunConfiguration();
    const auto result = RunExecutor{}.execute(request,
        RunServices{cleanup, nullptr, configuration.get(), &work});
    QCOMPARE(result.outcome(), RunOutcome::Succeeded);
    QCOMPARE(result.work().assetAttempts.size(), roots.size());
    QCOMPARE(processingRoots.size(), roots.size());
    for (std::size_t index = 0; index < roots.size(); ++index) {
        const auto& attempt = result.work().assetAttempts[index];
        QCOMPARE(processingRoots[index], roots[index]);
        QCOMPARE(attempt.modRoot, processingRoots[index]);
        QCOMPARE(attempt.asset.executionPath(), roots[index] / "textures" / "asset.dds");
    }
    QCOMPARE(cleanup.invocations(), std::size_t{1});
}

void RunExecutorTests::removedConversionRetainsModRoot_data() {
    QTest::addColumn<bool>("cancel");
    QTest::newRow("later-failure") << false;
    QTest::newRow("later-cancellation") << true;
}

void RunExecutorTests::removedConversionRetainsModRoot() {
    QFETCH(bool, cancel);
    QTemporaryDir directory(QDir::currentPath() + "/executor-conversion-XXXXXX");
    QVERIFY(directory.isValid());
    const auto selected = std::filesystem::canonical(
        std::filesystem::path(directory.path().toStdWString()));
    const auto root = selected / "mod";
    std::filesystem::create_directory(root);
    const auto source = root / "original.tga";
    const auto output = root / "original.dds";
    std::ofstream(source) << "original";
    std::optional<OptimizationRunResult> result;
    std::filesystem::path processingRoot;
    {
        ControlledAssetWork work;
        bool converted = false;
        std::stop_source cancellation;
        work.adapters.executeAssetWithResult = [&](const cao::routing::RoutedAsset& asset,
                                                   const std::filesystem::path& modRoot) {
            processingRoot = modRoot;
            std::ofstream(output) << "converted";
            if (!std::filesystem::remove(asset.executionPath()))
                throw std::runtime_error("Conversion source did not exist");
            converted = true;
            return cao::execution::AssetExecutionResult::success(cao::execution::MutationState::Committed);
        };
        // The next cancellation checkpoint runs after the completed outcome has been retained.
        work.adapters.isCancelled = [&] {
            if (!converted) return false;
            if (!cancel) throw std::runtime_error("later orchestration failure");
            cancellation.request_stop();
            return true;
        };
        std::size_t finalizations = 0;
        work.adapters.finalizeArchiveLifecycleWithResult = [&] {
            ++finalizations;
            return cao::run::ArchiveFinalizationResult{};
        };
        CountingSafetyCleanup cleanup;
        const auto configuration = testRunConfiguration();
        const auto request = RunRequest::create("SkyrimSE", ExecutionMode::Apply,
            ModSelection::childModRoots(std::filesystem::relative(selected)),
            {RequestedWork::ConvertibleTextureConversion});
        result = RunExecutor{}.execute(request,
            RunServices{cleanup, nullptr, configuration.get(), &work}, cancellation.get_token());
        QCOMPARE(finalizations, std::size_t{0});
        QCOMPARE(cleanup.invocations(), std::size_t{1});
        QVERIFY(!std::filesystem::exists(work.staged));
    }
    QCOMPARE(result->outcome(), cancel ? RunOutcome::Cancelled : RunOutcome::Failed);
    QCOMPARE(result->work().assetAttempts.size(), std::size_t{1});
    const auto& attempt = result->work().assetAttempts.front();
    QCOMPARE(processingRoot, root);
    QCOMPARE(attempt.modRoot, processingRoot);
    QCOMPARE(attempt.asset.executionPath(), source);
    QVERIFY(attempt.asset.operations().contains(cao::routing::AssetOperation::Conversion));
    QCOMPARE(attempt.result.mutationState(), cao::execution::MutationState::Committed);
    QCOMPARE(result->mutationSummaries().size(), std::size_t{1});
    const auto& summary = result->mutationSummaries().front();
    QCOMPARE(summary.modRoot, processingRoot);
    QCOMPARE(summary.kind, cao::run::MutationKind::AssetProcessing);
    QCOMPARE(summary.committed, std::size_t{1});
    QCOMPARE(summary.partialOrUnknown, std::size_t{0});
    QCOMPARE(requirePhase(*result, RunPhase::ProcessingAssets).progress()->completed(), std::size_t{1});
    QVERIFY(result->phase(RunPhase::ArchiveFinalization) == nullptr);
    QVERIFY(!std::filesystem::exists(source));
    QCOMPARE(stagingBytes(output), QByteArray("converted"));
}

void RunExecutorTests::retargetedAssetUsesCanonicalContainment_data() {
    QTest::addColumn<bool>("outside");
    QTest::newRow("prepared-sibling") << false;
    QTest::newRow("outside-prepared-roots") << true;
}

void RunExecutorTests::retargetedAssetUsesCanonicalContainment() {
    QFETCH(bool, outside);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto base = std::filesystem::canonical(std::filesystem::path(directory.path().toStdWString()));
    const auto selected = base / "mods";
    const auto first = selected / "first";
    const auto second = selected / "second";
    std::filesystem::create_directories(first);
    std::filesystem::create_directory(second);
    const auto source = first / "asset.dds";
    const auto target = (outside ? base : second) / "target.bin";
    std::ofstream(source) << "original";
    std::ofstream(target) << "target bytes";
    ControlledAssetWork work;
    bool retargeted = false;
    work.adapters.reportPhase = [&](const RunPhaseRecord& phase) {
        if (phase.phase() != RunPhase::ProcessingAssets || retargeted) return;
        // Simulate a path change after discovery, before the first protected operation starts.
        std::filesystem::remove(source);
        std::filesystem::create_symlink(target, source);
        retargeted = true;
    };
    std::vector<std::filesystem::path> processingRoots;
    work.adapters.executeAssetWithResult = [&](const cao::routing::RoutedAsset&,
                                               const std::filesystem::path& modRoot) {
        processingRoots.push_back(modRoot);
        return cao::execution::AssetExecutionResult::success();
    };
    CountingSafetyCleanup cleanup;
    const auto configuration = testRunConfiguration();
    const auto request = RunRequest::create("SkyrimSE", ExecutionMode::Apply,
        ModSelection::childModRoots(selected), {RequestedWork::NativeTextureOptimization});
    const auto result = RunExecutor{}.execute(request,
        RunServices{cleanup, nullptr, configuration.get(), &work});
    // Remove the link before QTemporaryDir cleanup, which does not handle Windows links.
    std::filesystem::remove(source);
    QVERIFY(retargeted);
    QCOMPARE(result.work().assetAttempts.size(), std::size_t{1});
    const auto& attempt = result.work().assetAttempts.front();
    if (outside) {
        QCOMPARE(result.outcome(), RunOutcome::Failed);
        QVERIFY(processingRoots.empty());
        QVERIFY(attempt.modRoot.empty());
        QVERIFY(!attempt.result.safeToContinue());
    } else {
        QCOMPARE(result.outcome(), RunOutcome::Succeeded);
        QCOMPARE(processingRoots, std::vector<std::filesystem::path>{second});
        QCOMPARE(attempt.modRoot, processingRoots.front());
    }
    QCOMPARE(stagingBytes(target), QByteArray("target bytes"));
    QCOMPARE(cleanup.invocations(), std::size_t{1});
}

void RunExecutorTests::mixedWorkEvidenceOutlivesServices() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = std::filesystem::canonical(std::filesystem::path(directory.path().toStdWString()));
    const auto successfulAsset = root / "a.dds";
    const auto failedAsset = root / "b.dds";
    const auto output = root / "packed.bsa";
    std::ofstream(successfulAsset) << "original";
    std::ofstream(failedAsset) << "original";
    std::optional<OptimizationRunResult> result;
    std::filesystem::path staged;
    {
        ControlledAssetWork work;
        std::size_t finalizerCalls = 0;
        work.adapters.executeAssetWithResult = [&](const cao::routing::RoutedAsset& asset, const std::filesystem::path&) {
            if (asset.executionPath() == failedAsset)
                return cao::execution::AssetExecutionResult::failed(
                    cao::execution::AssetExecutionFailure::LoadFailed, "controlled load failure",
                    cao::execution::MutationState::None, true, failedAsset, "load_texture", "raw detail");
            std::ofstream(asset.executionPath()) << "committed";
            return cao::execution::AssetExecutionResult::success(cao::execution::MutationState::Committed);
        };
        work.adapters.finalizeArchiveLifecycleWithResult = [&] {
            ++finalizerCalls;
            std::ofstream(output) << "packed";
            return cao::run::ArchiveFinalizationResult{{
                {output, cao::execution::MutationState::Committed, {}, true, {}, root},
                {root / "failed.bsa", cao::execution::MutationState::None,
                 cao::run::ArchiveFinalizationFailure::WriteFailed, true, "write detail", root}}};
        };
        CountingSafetyCleanup cleanup;
        const auto configuration = testRunConfiguration();
        const auto request = RunRequest::create("SkyrimSE", ExecutionMode::Apply,
            ModSelection::singleModRoot(root),
            {RequestedWork::NativeTextureOptimization, RequestedWork::ArchiveCreation});
        result = RunExecutor{}.execute(request, RunServices{cleanup, nullptr, configuration.get(), &work});
        staged = work.staged;
        QCOMPARE(finalizerCalls, std::size_t{1});
        QCOMPARE(cleanup.invocations(), std::size_t{1});
    }
    QCOMPARE(result->outcome(), RunOutcome::CompletedWithFailures);
    QVERIFY(result->work().ledger.has_value());
    QCOMPARE(result->work().ledger->routedAssets().size(), std::size_t{2});
    QCOMPARE(result->work().assetAttempts.size(), std::size_t{2});
    const auto& failed = result->work().assetAttempts.back();
    QCOMPARE(failed.modRoot, root);
    QCOMPARE(failed.asset.executionPath(), failedAsset);
    QCOMPARE(failed.result.affectedPath(), failedAsset);
    QCOMPARE(failed.result.operation(), std::string("load_texture"));
    QCOMPARE(failed.result.serviceDetail(), std::string("raw detail"));
    const auto& progress = requirePhase(*result, RunPhase::ProcessingAssets).progress();
    QVERIFY(progress.has_value());
    QCOMPARE(progress->total(), std::size_t{2});
    QCOMPARE(progress->completed(), std::size_t{2});
    QCOMPARE(progress->succeeded(), std::size_t{1});
    QCOMPARE(progress->failed(), std::size_t{1});
    QCOMPARE(result->work().finalizations.size(), std::size_t{1});
    const auto& finalization = result->work().finalizations.front();
    QCOMPARE(finalization.attempts.size(), std::size_t{2});
    QCOMPARE(finalization.attempts.front().archivePath, output);
    QCOMPARE(finalization.attempts.front().modRoot, root);
    QCOMPARE(finalization.attempts.back().detail, std::string("write detail"));
    QCOMPARE(result->mutationSummaries().size(), std::size_t{2});
    for (const auto& summary : result->mutationSummaries()) {
        QCOMPARE(summary.modRoot, root);
        QCOMPARE(summary.committed, std::size_t{1});
        QCOMPARE(summary.partialOrUnknown, std::size_t{0});
    }
    QVERIFY(result->cleanupFailures().empty());
    QVERIFY(!std::filesystem::exists(staged));
    QCOMPARE(stagingBytes(successfulAsset), QByteArray("committed"));
    QCOMPARE(stagingBytes(failedAsset), QByteArray("original"));
    QCOMPARE(stagingBytes(output), QByteArray("packed"));
    QCOMPARE(result->phases().back().phase(), RunPhase::SafetyCleanup);
}

void RunExecutorTests::cancellationAfterAtomicAssetAttempt_data() {
    QTest::addColumn<bool>("unsafe");
    QTest::newRow("last-successful-attempt") << false;
    QTest::newRow("unsafe-with-pending-work") << true;
}

void RunExecutorTests::cancellationAfterAtomicAssetAttempt() {
    QFETCH(bool, unsafe);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = std::filesystem::canonical(std::filesystem::path(directory.path().toStdWString()));
    std::ofstream(root / "a.dds") << "original";
    if (unsafe) std::ofstream(root / "b.dds") << "unattempted";
    ControlledAssetWork work;
    std::stop_source cancellation;
    std::size_t attempts = 0;
    std::size_t finalizerCalls = 0;
    work.adapters.executeAssetWithResult = [&](const cao::routing::RoutedAsset& asset, const std::filesystem::path&) {
        ++attempts;
        cancellation.request_stop();
        // The atomic callback finishes its mutation despite cancellation being requested inside it.
        std::ofstream(asset.executionPath()) << "finished atomic attempt";
        if (unsafe)
            return cao::execution::AssetExecutionResult::failed(
                cao::execution::AssetExecutionFailure::CommitFailed, "uncertain commit",
                cao::execution::MutationState::PartialOrUnknown, false, asset.executionPath());
        return cao::execution::AssetExecutionResult::success(cao::execution::MutationState::Committed);
    };
    work.adapters.finalizeArchiveLifecycleWithResult = [&] {
        ++finalizerCalls;
        return cao::run::ArchiveFinalizationResult{};
    };
    CountingSafetyCleanup cleanup;
    const auto configuration = testRunConfiguration();
    const auto request = RunRequest::create("SkyrimSE", ExecutionMode::Apply,
        ModSelection::singleModRoot(root), {RequestedWork::NativeTextureOptimization});
    const auto result = RunExecutor{}.execute(request,
        RunServices{cleanup, nullptr, configuration.get(), &work}, cancellation.get_token());
    QCOMPARE(result.outcome(), unsafe ? RunOutcome::Failed : RunOutcome::Cancelled);
    QVERIFY(result.work().cancellationObserved);
    QCOMPARE(attempts, std::size_t{1});
    QCOMPARE(finalizerCalls, std::size_t{0});
    QCOMPARE(result.work().assetAttempts.size(), std::size_t{1});
    QVERIFY(result.work().ledger.has_value());
    const auto& progress = requirePhase(result, RunPhase::ProcessingAssets).progress();
    QVERIFY(progress.has_value());
    QCOMPARE(progress->total(), unsafe ? std::size_t{2} : std::size_t{1});
    QCOMPARE(progress->completed(), std::size_t{1});
    QCOMPARE(progress->failed(), unsafe ? std::size_t{1} : std::size_t{0});
    QCOMPARE(progress->succeeded(), unsafe ? std::size_t{0} : std::size_t{1});
    QVERIFY(result.phase(RunPhase::ArchiveFinalization) == nullptr);
    QVERIFY(result.work().finalizations.empty());
    QCOMPARE(cleanup.invocations(), std::size_t{1});
    QVERIFY(!std::filesystem::exists(work.staged));
    QCOMPARE(stagingBytes(root / "a.dds"), QByteArray("finished atomic attempt"));
    if (unsafe) QCOMPARE(stagingBytes(root / "b.dds"), QByteArray("unattempted"));
}

void RunExecutorTests::mixedExtractionAttemptsAdvanceProgress() {
    QTemporaryDir directory;
    QTemporaryDir sources;
    QVERIFY(directory.isValid());
    QVERIFY(sources.isValid());
    const auto root = std::filesystem::canonical(std::filesystem::path(directory.path().toStdWString()));
    const auto source = std::filesystem::path(sources.path().toStdWString());
    for (const auto* name : {"a", "b"}) {
        const auto asset = source / (std::string(name) + ".dds");
        std::ofstream(asset) << "archived";
        auto archive = btu::bsa::ArchiveData(btu::bsa::Settings::get(btu::Game::SSE),
                                           btu::bsa::ArchiveType::Textures);
        QVERIFY(archive.add_file(asset));
        archive.set_out_path(root / (std::string(name) + ".bsa"));
        QVERIFY(btu::bsa::write(false, std::move(archive), source).empty());
    }
    ControlledAssetWork work;
    std::size_t attempts = 0;
    work.adapters.extractArchiveWithResult = [&](const cao::run::ArchiveExtractionPlan& plan) {
        ++attempts;
        if (plan.archivePath.filename() == "b.bsa")
            return cao::run::ArchiveExtractionResult{plan.archivePath,
                cao::execution::MutationState::None, cao::run::ArchiveExtractionFailure::ExtractionFailed,
                true, "controlled extraction failure"};
        std::ofstream(root / "a.dds") << "extracted";
        return cao::run::ArchiveExtractionResult{plan.archivePath, cao::execution::MutationState::Committed};
    };
    CountingSafetyCleanup cleanup;
    const auto configuration = testRunConfiguration();
    const auto request = RunRequest::create("SkyrimSE", ExecutionMode::Apply,
        ModSelection::singleModRoot(root), {RequestedWork::ArchiveExtraction});
    const auto result = RunExecutor{}.execute(request, RunServices{cleanup, nullptr, configuration.get(), &work});
    QCOMPARE(result.outcome(), RunOutcome::CompletedWithFailures);
    QCOMPARE(attempts, std::size_t{2});
    QCOMPARE(result.work().archiveAttempts.size(), std::size_t{2});
    const auto& progress = requirePhase(result, RunPhase::ExtractingArchives).progress();
    QVERIFY(progress.has_value());
    QCOMPARE(progress->total(), std::size_t{2});
    QCOMPARE(progress->completed(), std::size_t{2});
    QCOMPARE(progress->succeeded(), std::size_t{1});
    QCOMPARE(progress->failed(), std::size_t{1});
    QVERIFY(result.work().assetAttempts.empty());
    for (const auto& attempt : result.work().archiveAttempts) QCOMPARE(attempt.modRoot, root);
    QCOMPARE(result.mutationSummaries().size(), std::size_t{1});
    QCOMPARE(result.mutationSummaries().front().committed, std::size_t{1});
    QCOMPARE(cleanup.invocations(), std::size_t{1});
    QVERIFY(!std::filesystem::exists(work.staged));
    QCOMPARE(stagingBytes(root / "a.dds"), QByteArray("extracted"));
}

void RunExecutorTests::committedExtractionSurvivesDiscoveryInterruption_data() {
    QTest::addColumn<QString>("interruption");
    QTest::newRow("completed") << QString("none");
    QTest::newRow("cancelled-after-commit") << QString("cancel");
    QTest::newRow("throw-after-commit") << QString("throw");
}

void RunExecutorTests::committedExtractionSurvivesDiscoveryInterruption() {
    QFETCH(QString, interruption);
    QTemporaryDir directory;
    QTemporaryDir sourceDirectory;
    QVERIFY(directory.isValid());
    QVERIFY(sourceDirectory.isValid());
    const auto root = std::filesystem::canonical(std::filesystem::path(directory.path().toStdWString()));
    const auto source = std::filesystem::path(sourceDirectory.path().toStdWString());
    const auto archivePath = root / "source.bsa";
    const auto output = root / "asset.dds";
    std::ofstream(source / "asset.dds") << "archived";
    auto archive = btu::bsa::ArchiveData(btu::bsa::Settings::get(btu::Game::SSE),
                                      btu::bsa::ArchiveType::Textures);
    QVERIFY(archive.add_file(source / "asset.dds"));
    archive.set_out_path(archivePath);
    QVERIFY(btu::bsa::write(false, std::move(archive), source).empty());
    const auto originalArchive = stagingBytes(archivePath);
    ControlledAssetWork work;
    bool extracted = false;
    work.adapters.extractArchiveWithResult = [&](const cao::run::ArchiveExtractionPlan& plan) {
        std::ofstream(output) << "committed extraction";
        extracted = true;
        return cao::run::ArchiveExtractionResult{plan.archivePath,
                                                 cao::execution::MutationState::Committed};
    };
    // Cancellation is sampled after extraction returns, outside the protected operation callback.
    work.adapters.isCancelled = [&] {
        if (extracted && interruption == "throw")
            throw std::runtime_error("discovery interrupted after extraction");
        return extracted && interruption == "cancel";
    };
    CountingSafetyCleanup cleanup;
    const auto configuration = testRunConfiguration();
    const auto request = RunRequest::create("SkyrimSE", ExecutionMode::Apply,
        ModSelection::singleModRoot(root), {RequestedWork::ArchiveExtraction});
    const auto result = RunExecutor{}.execute(request,
        RunServices{cleanup, nullptr, configuration.get(), &work});
    if (interruption == "throw") {
        QCOMPARE(result.outcome(), RunOutcome::Failed);
        QCOMPARE(result.failures().size(), std::size_t{1});
        QCOMPARE(result.failures().front().code(), cao::run::RunFailureCode::WorkServiceFailed);
        QCOMPARE(result.failures().front().detail(), std::string("discovery interrupted after extraction"));
    } else {
        QCOMPARE(result.outcome(), interruption == "cancel" ? RunOutcome::Cancelled : RunOutcome::Succeeded);
        QVERIFY(result.failures().empty());
    }
    QCOMPARE(result.work().cancellationObserved, interruption == "cancel");
    const auto& progress = requirePhase(result, RunPhase::ExtractingArchives).progress();
    QVERIFY(progress.has_value());
    QCOMPARE(progress->total(), std::size_t{1});
    QCOMPARE(progress->completed(), std::size_t{1});
    QCOMPARE(progress->succeeded(), std::size_t{1});
    QCOMPARE(progress->failed(), std::size_t{0});
    QCOMPARE(result.work().archiveAttempts.size(), std::size_t{1});
    const auto& attempt = result.work().archiveAttempts.front();
    QCOMPARE(attempt.archivePath, archivePath);
    QCOMPARE(attempt.modRoot, root);
    QCOMPARE(attempt.mutation, cao::execution::MutationState::Committed);
    QVERIFY(attempt.succeeded());
    QCOMPARE(result.work().ledger.has_value(), interruption == "none");
    QVERIFY(result.work().assetAttempts.empty());
    QVERIFY(result.work().finalizations.empty());
    QCOMPARE(result.mutationSummaries().size(), std::size_t{1});
    const auto& summary = result.mutationSummaries().front();
    QCOMPARE(summary.modRoot, root);
    QCOMPARE(summary.kind, cao::run::MutationKind::ArchiveExtraction);
    QCOMPARE(summary.committed, std::size_t{1});
    QCOMPARE(summary.partialOrUnknown, std::size_t{0});
    QCOMPARE(cleanup.invocations(), std::size_t{1});
    QVERIFY(result.cleanupFailures().empty());
    QVERIFY(!work.staged.empty());
    QVERIFY(!std::filesystem::exists(work.staged));
    QCOMPARE(stagingBytes(output), QByteArray("committed extraction"));
    QCOMPARE(stagingBytes(archivePath), originalArchive);
    QCOMPARE(result.phases().back().phase(), RunPhase::SafetyCleanup);
    QCOMPARE(result.phase(RunPhase::ArchiveFinalization) != nullptr, interruption == "none");
}

void RunExecutorTests::discoveryDiagnosticsSurviveInterruption() {
    QTemporaryDir directory;
    QTemporaryDir sourceDirectory;
    QVERIFY(directory.isValid());
    QVERIFY(sourceDirectory.isValid());
    const auto root = std::filesystem::canonical(std::filesystem::path(directory.path().toStdWString()));
    const auto source = std::filesystem::path(sourceDirectory.path().toStdWString());
    std::ofstream(source / "asset.dds") << "archived";
    auto archive = btu::bsa::ArchiveData(btu::bsa::Settings::get(btu::Game::SSE),
                                      btu::bsa::ArchiveType::Textures);
    QVERIFY(archive.add_file(source / "asset.dds"));
    archive.set_out_path(root / "source.bsa");
    QVERIFY(btu::bsa::write(false, std::move(archive), source).empty());
    std::error_code error;
    std::filesystem::create_symlink(source / "asset.dds", root / "linked.dds", error);
    if (error) QSKIP("File symlink creation is unavailable on this host");
    ControlledAssetWork work;
    bool extracted{};
    work.adapters.extractArchiveWithResult = [&](const cao::run::ArchiveExtractionPlan& plan) {
        extracted = true;
        std::ofstream(root / "asset.dds") << "committed";
        return cao::run::ArchiveExtractionResult{plan.archivePath, cao::execution::MutationState::Committed};
    };
    // The cancellation boundary is outside the protected extraction callback and unwinds discovery.
    work.adapters.isCancelled = [&] {
        if (extracted) throw std::runtime_error("interrupted discovery after exclusion");
        return false;
    };
    CountingSafetyCleanup cleanup;
    const auto configuration = testRunConfiguration();
    const auto request = RunRequest::create("SkyrimSE", ExecutionMode::Apply,
        ModSelection::singleModRoot(root), {RequestedWork::ArchiveExtraction});
    const auto result = RunExecutor{}.execute(request,
        RunServices{cleanup, nullptr, configuration.get(), &work});
    // Remove the link before QTemporaryDir cleanup, which does not handle Windows links.
    QVERIFY(std::filesystem::remove(root / "linked.dds"));
    QCOMPARE(result.outcome(), RunOutcome::Failed);
    QCOMPARE(result.work().archiveAttempts.size(), std::size_t{1});
    QCOMPARE(result.failures().size(), std::size_t{1});
    QCOMPARE(result.failures().front().code(), cao::run::RunFailureCode::WorkServiceFailed);
    QCOMPARE(result.work().diagnostics.size(), std::size_t{1});
    QCOMPARE(result.work().diagnostics.front().code(), cao::run::RunDiagnosticCode::LinkedEntryExcluded);
    QCOMPARE(result.work().diagnostics.front().path(), root / "linked.dds");
    QVERIFY(!result.work().ledger.has_value());
    QVERIFY(result.phase(RunPhase::ArchiveFinalization) == nullptr);
    QCOMPARE(cleanup.invocations(), std::size_t{1});
    QVERIFY(!std::filesystem::exists(work.staged));
    QCOMPARE(stagingBytes(root / "asset.dds"), QByteArray("committed"));
}

void RunExecutorTests::cancelledRecoveryPreservesUnattemptedArtifacts() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = std::filesystem::canonical(std::filesystem::path(directory.path().toStdWString()));
    const auto child = seedStaleStaging(root);
    std::stop_source cancellation;
    cancellation.request_stop();
    cao::run::StagingRecovery recovery;
    QVERIFY(!recovery.recover(root, cancellation.get_token()).has_value());
    QVERIFY(std::filesystem::exists(child / "temporary.dds"));
}

void RunExecutorTests::linkedStagingIsPreserved() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto base = std::filesystem::canonical(std::filesystem::path(directory.path().toStdWString()));
    const auto root = base / "mod";
    const auto outside = base / "outside";
    std::filesystem::create_directory(root);
    std::filesystem::create_directory(outside);
    const auto child = seedStaleStaging(root);
    std::ofstream(outside / "keep.dds") << "external";
    std::error_code linkError;
    std::filesystem::create_directory_symlink(outside, child / "linked", linkError);
    if (linkError) QSKIP("Directory symlink creation is unavailable on this host");
    CountingSafetyCleanup cleanup;
    const auto configuration = testRunConfiguration();
    const auto request = RunRequest::create("SkyrimSE", ExecutionMode::Apply,
                                           ModSelection::singleModRoot(root), {});
    auto result = RunExecutor{}.execute(request, RunServices{cleanup, nullptr, configuration.get()});
    QCOMPARE(result.outcome(), RunOutcome::Failed);
    QCOMPARE(stagingBytes(outside / "keep.dds"), QByteArray("external"));
    QCOMPARE(stagingBytes(child / "temporary.dds"), QByteArray("temporary"));
    std::filesystem::remove(child / "linked");
    std::filesystem::create_hard_link(root / ".cao-staging" / "ownership.manifest", outside / "copy");
    result = RunExecutor{}.execute(request, RunServices{cleanup, nullptr, configuration.get()});
    QCOMPARE(result.outcome(), RunOutcome::Failed);
    QCOMPARE(result.failures().front().code(), cao::run::RunFailureCode::StagingOwnershipUnverified);
    QCOMPARE(stagingBytes(child / "temporary.dds"), QByteArray("temporary"));
}

void RunExecutorTests::recoveryFailureStillPerformsSafetyCleanup() {
#ifdef _WIN32
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = std::filesystem::canonical(std::filesystem::path(directory.path().toStdWString()));
    const auto child = seedStaleStaging(root);
    const auto temporary = child / "temporary.dds";
    const auto permissions = std::filesystem::status(temporary).permissions();
    std::filesystem::permissions(temporary, std::filesystem::perms::owner_read,
                                std::filesystem::perm_options::replace);
    CountingSafetyCleanup cleanup;
    const auto configuration = testRunConfiguration();
    const auto request = RunRequest::create("SkyrimSE", ExecutionMode::Apply,
                                           ModSelection::singleModRoot(root), {});
    const auto result = RunExecutor{}.execute(request, RunServices{cleanup, nullptr, configuration.get()});
    // Restore fixture permissions even if the result assertions below fail.
    std::filesystem::permissions(temporary, permissions);
    QCOMPARE(result.outcome(), RunOutcome::Failed);
    QCOMPARE(result.finalPhase(), RunPhase::Preparing);
    QCOMPARE(result.failures().front().code(), cao::run::RunFailureCode::StagingRecoveryFailed);
    QCOMPARE(result.failures().front().path(), temporary);
    QCOMPARE(cleanup.invocations(), std::size_t{1});
    QCOMPARE(stagingBytes(temporary), QByteArray("temporary"));
#else
    QSKIP("Windows read-only deletion behavior is the failure fixture");
#endif
}

void RunExecutorTests::unverifiableStagingIsPreserved_data() {
    QTest::addColumn<QString>("problem");
    for (const auto* problem : {"invalid", "version", "root", "run", "truncated", "trailing",
                                "traversal", "alias", "duplicate", "wrong-type", "unknown-child",
                                "unknown-sibling", "missing-lock", "reserved-file", "lookalike"})
        QTest::newRow(problem) << QString(problem);
}

void RunExecutorTests::unverifiableStagingIsPreserved() {
    QFETCH(QString, problem);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = std::filesystem::canonical(std::filesystem::path(directory.path().toStdWString()));
    auto child = seedStaleStaging(root);
    auto staging = root / ".cao-staging";
    auto manifestPath = staging / "ownership.manifest";
    auto manifest = staleManifest(root);
    if (problem == "invalid") manifest = "not CAO ownership";
    if (problem == "version") manifest.replace(manifest.find(" 1"), 2, " 9");
    if (problem == "root") manifest = staleManifest(root / "other-mod");
    if (problem == "run") manifest.replace(manifest.find("\"407\""), 5, "\"408\"");
    if (problem == "truncated") manifest.resize(manifest.size() - 8);
    if (problem == "trailing") manifest += "retained\n";
    if (problem == "traversal") manifest.replace(manifest.find("/temporary.dds"), 14, "/../outside.dds");
    if (problem == "alias") manifest.replace(manifest.find("temporary.dds"), 13, "temporary.dds.");
    if (problem == "duplicate") {
        manifest.replace(manifest.find("\n2\n"), 3, "\n3\n");
        manifest += "F \"" + staleChildName + "/temporary.dds\"\n";
    }
    if (problem == "wrong-type") manifest.replace(manifest.find("\nF "), 3, "\nD ");
    if (problem == "unknown-child") std::ofstream(child / "retained.bsa") << "evidence";
    if (problem == "unknown-sibling") std::ofstream(staging / "backup.bsa") << "evidence";
    if (problem == "missing-lock") std::filesystem::remove(staging / "owner.lock");
    std::ofstream(manifestPath, std::ios::binary | std::ios::trunc) << manifest;
    if (problem == "reserved-file") {
        std::filesystem::rename(staging, root / "user-material");
        std::ofstream(staging) << "reserved filename is user material";
        child = root / "user-material" / staleChildName;
        manifestPath = root / "user-material" / "ownership.manifest";
    }
    if (problem == "lookalike") {
        std::filesystem::rename(staging, root / ".CAO-Staging-abandoned");
        staging = root / ".CAO-Staging-abandoned";
        child = staging / staleChildName;
        manifestPath = staging / "ownership.manifest";
    }
    const auto before = stagingBytes(manifestPath);
    CountingSafetyCleanup cleanup;
    const auto configuration = testRunConfiguration();
    const auto request = RunRequest::create("SkyrimSE", ExecutionMode::Apply,
                                           ModSelection::singleModRoot(root), {});
    const auto result = RunExecutor{}.execute(request, RunServices{cleanup, nullptr, configuration.get()});
    QCOMPARE(result.outcome(), RunOutcome::Failed);
    QCOMPARE(result.failures().front().code(), cao::run::RunFailureCode::StagingOwnershipUnverified);
    QCOMPARE(result.finalPhase(), RunPhase::Preparing);
    QVERIFY(result.failures().front().detail().find("retry") != std::string::npos);
    QCOMPARE(stagingBytes(manifestPath), before);
    QCOMPARE(stagingBytes(child / "temporary.dds"), QByteArray("temporary"));
    if (problem == "unknown-child") QCOMPARE(stagingBytes(child / "retained.bsa"), QByteArray("evidence"));
    if (problem == "unknown-sibling") QCOMPARE(stagingBytes(staging / "backup.bsa"), QByteArray("evidence"));
    QCOMPARE(cleanup.invocations(), std::size_t{1});
}

void RunExecutorTests::dryRunLeavesStagingUntouched() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = std::filesystem::canonical(std::filesystem::path(directory.path().toStdWString()));
    const auto child = seedStaleStaging(root);
    const auto manifest = root / ".cao-staging" / "ownership.manifest";
    const auto before = stagingBytes(manifest);
    const auto modified = std::filesystem::last_write_time(manifest);
    CountingSafetyCleanup cleanup;
    const auto configuration = testRunConfiguration();
    const auto request = RunRequest::create("SkyrimSE", ExecutionMode::DryRun,
                                           ModSelection::singleModRoot(root), {});
    const auto result = RunExecutor{}.execute(request, RunServices{cleanup, nullptr, configuration.get()});
    QCOMPARE(result.outcome(), RunOutcome::Succeeded);
    QCOMPARE(stagingBytes(manifest), before);
    QVERIFY(std::filesystem::last_write_time(manifest) == modified);
    QCOMPARE(stagingBytes(child / "temporary.dds"), QByteArray("temporary"));
}

void RunExecutorTests::activeStagingBlocksUntilItsOwnerExits() {
#ifdef _WIN32
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = std::filesystem::canonical(std::filesystem::path(directory.path().toStdWString()));
    const auto child = seedStaleStaging(root);
    const auto manifest = root / ".cao-staging" / "ownership.manifest";
    const auto before = stagingBytes(manifest);
    QProcess owner;
    auto lockPath = QString::fromStdWString((root / ".cao-staging" / "owner.lock").wstring());
    lockPath.replace("'", "''");
    owner.start("powershell.exe", {"-NoProfile", "-NonInteractive", "-Command",
        "$stream = [IO.File]::Open('" + lockPath +
        "', [IO.FileMode]::Open, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None); "
        "[Console]::Out.WriteLine('locked'); [Console]::Out.Flush(); "
        "[Threading.Thread]::Sleep(-1)"});
    QVERIFY(owner.waitForStarted());
    QVERIFY(owner.waitForReadyRead());
    QCOMPARE(owner.readAllStandardOutput().trimmed(), QByteArray("locked"));
    CountingSafetyCleanup cleanup;
    const auto configuration = testRunConfiguration();
    const auto request = RunRequest::create("SkyrimSE", ExecutionMode::Apply,
                                           ModSelection::singleModRoot(root), {});
    const auto active = RunExecutor{}.execute(request, RunServices{cleanup, nullptr, configuration.get()});
    QCOMPARE(active.outcome(), RunOutcome::Failed);
    QCOMPARE(active.failures().front().code(), cao::run::RunFailureCode::StagingActive);
    QCOMPARE(stagingBytes(manifest), before);
    QCOMPARE(stagingBytes(child / "temporary.dds"), QByteArray("temporary"));
    // Simulate a crashed owner: the OS, rather than orderly application cleanup, releases the lock.
    owner.kill();
    QVERIFY(owner.waitForFinished());
    const auto stale = RunExecutor{}.execute(request, RunServices{cleanup, nullptr, configuration.get()});
    QCOMPARE(stale.outcome(), RunOutcome::Succeeded);
    QVERIFY(!std::filesystem::exists(child));
#else
    QSKIP("The separate-process fixture uses the supported Windows host's FileShare lock");
#endif
}

void RunExecutorTests::workArtifactsShareRecoveryAndAreCleanedAfterFailure_data() {
    QTest::addColumn<bool>("interruptAfterCommit");
    QTest::newRow("completed") << false;
    QTest::newRow("throw-after-commit") << true;
}

void RunExecutorTests::workArtifactsShareRecoveryAndAreCleanedAfterFailure() {
    QFETCH(bool, interruptAfterCommit);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = std::filesystem::canonical(std::filesystem::path(directory.path().toStdWString()));
    const auto stale = seedStaleStaging(root);
    const auto texture = root / "texture.dds";
    std::ofstream(texture) << "original";
    ControlledAssetWork work;
    bool committed = false;
    work.adapters.executeAssetWithResult = [&](const cao::routing::RoutedAsset& asset, const std::filesystem::path&) {
        std::ofstream(asset.executionPath(), std::ios::trunc) << "committed asset";
        committed = true;
        return cao::execution::AssetExecutionResult::success(cao::execution::MutationState::Committed);
    };
    // Interrupt orchestration only after the operation has returned its durable result.
    work.adapters.isCancelled = [&] {
        if (committed && interruptAfterCommit)
            throw std::runtime_error("orchestration interrupted after asset commit");
        return false;
    };
    work.adapters.finalizeArchiveLifecycleWithResult = [] {
        return cao::run::ArchiveFinalizationResult{};
    };
    CountingSafetyCleanup cleanup;
    const auto configuration = testRunConfiguration();
    const auto request = RunRequest::create("SkyrimSE", ExecutionMode::Apply,
        ModSelection::singleModRoot(root), {RequestedWork::NativeTextureOptimization});
    const auto result = RunExecutor{}.execute(request,
        RunServices{cleanup, nullptr, configuration.get(), &work});
    QCOMPARE(result.outcome(), interruptAfterCommit ? RunOutcome::Failed : RunOutcome::Succeeded);
    if (interruptAfterCommit) {
        QCOMPARE(result.failures().size(), std::size_t{1});
        QCOMPARE(result.failures().front().code(), cao::run::RunFailureCode::WorkServiceFailed);
        QCOMPARE(result.failures().front().detail(),
                 std::string("orchestration interrupted after asset commit"));
    } else {
        QVERIFY(result.failures().empty());
        QVERIFY(result.work().ledger.has_value());
    }
    QCOMPARE(result.work().assetAttempts.size(), std::size_t{1});
    // Routing completed before the adapter ran, so later orchestration cannot erase its ledger.
    QVERIFY(result.work().ledger.has_value());
    QCOMPARE(result.work().ledger->routedAssets().size(), std::size_t{1});
    const auto& progress = requirePhase(result, RunPhase::ProcessingAssets).progress();
    QVERIFY(progress.has_value());
    QCOMPARE(progress->total(), std::size_t{1});
    QCOMPARE(progress->completed(), std::size_t{1});
    QCOMPARE(progress->succeeded(), std::size_t{1});
    QCOMPARE(progress->failed(), std::size_t{0});
    const auto& attempt = result.work().assetAttempts.front();
    QCOMPARE(attempt.modRoot, root);
    QCOMPARE(attempt.asset.executionPath(), texture);
    QCOMPARE(attempt.result.mutationState(), cao::execution::MutationState::Committed);
    QVERIFY(attempt.result.succeeded());
    QVERIFY(result.work().archiveAttempts.empty());
    QCOMPARE(result.work().finalizations.size(), interruptAfterCommit ? std::size_t{0} : std::size_t{1});
    QCOMPARE(result.mutationSummaries().size(), std::size_t{1});
    const auto& summary = result.mutationSummaries().front();
    QCOMPARE(summary.modRoot, root);
    QCOMPARE(summary.kind, cao::run::MutationKind::AssetProcessing);
    QCOMPARE(summary.committed, std::size_t{1});
    QCOMPARE(summary.partialOrUnknown, std::size_t{0});
    QVERIFY(!work.staged.empty());
    QVERIFY(!std::filesystem::exists(work.staged));
    QVERIFY(!std::filesystem::exists(stale / "temporary.dds"));
    QCOMPARE(stagingBytes(texture), QByteArray("committed asset"));
    QCOMPARE(cleanup.invocations(), std::size_t{1});
    QVERIFY(result.cleanupFailures().empty());
    QCOMPARE(result.phases().back().phase(), RunPhase::SafetyCleanup);
    QCOMPARE(result.phase(RunPhase::ArchiveFinalization) != nullptr, !interruptAfterCommit);
}

void RunExecutorTests::recoveryLockSurvivesThroughSafetyCleanup() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = std::filesystem::canonical(std::filesystem::path(directory.path().toStdWString()));
    seedStaleStaging(root);
    class ContendingCleanup final : public SafetyCleanupService {
       public:
        std::filesystem::path root;
        bool blocked{};
        /// Attempts another real run while the first run is still performing Safety Cleanup.
        std::vector<cao::run::RunFailure> performSafetyCleanup() override {
            CountingSafetyCleanup inner;
            const auto configuration = testRunConfiguration();
            const auto request = RunRequest::create("SkyrimSE", ExecutionMode::Apply,
                                                   ModSelection::singleModRoot(root), {});
            const auto result = RunExecutor{}.execute(request, RunServices{inner, nullptr, configuration.get()});
            blocked = result.outcome() == RunOutcome::Failed && !result.failures().empty() &&
                      result.failures().front().code() == cao::run::RunFailureCode::StagingActive;
            return {};
        }
    } cleanup;
    cleanup.root = root;
    const auto configuration = testRunConfiguration();
    const auto request = RunRequest::create("SkyrimSE", ExecutionMode::Apply,
                                           ModSelection::singleModRoot(root), {});
    const auto result = RunExecutor{}.execute(request, RunServices{cleanup, nullptr, configuration.get()});
    QCOMPARE(result.outcome(), RunOutcome::Succeeded);
    QVERIFY(cleanup.blocked);
    CountingSafetyCleanup after;
    QCOMPARE(RunExecutor{}.execute(request, RunServices{after, nullptr, configuration.get()}).outcome(),
             RunOutcome::Succeeded);
}

void RunExecutorTests::verifiedStaleStagingIsRecoveredBeforeWork() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = std::filesystem::canonical(std::filesystem::path(directory.path().toStdWString()));
    const auto staging = root / ".cao-staging";
    const auto child = seedStaleStaging(root);
    class RecoveryObserver final : public cao::run::RunObservationSink {
       public:
        std::filesystem::path child;
        bool recoveredBeforeWork{};
        /// Records whether recovery finished before the first work phase was published.
        void recordPhase(const RunPhaseRecord& phase) override {
            if (phase.phase() == RunPhase::DiscoveringArchives)
                recoveredBeforeWork = !std::filesystem::exists(child);
        }
        /// No failure or diagnostic is expected by this successful recovery fixture.
        void recordFailure(const cao::run::RunFailure&) override {}
        void recordDiagnostic(const cao::run::RunDiagnostic&) override {}
    } observer;
    observer.child = child;
    CountingSafetyCleanup cleanup;
    const auto configuration = testRunConfiguration();
    const auto request = RunRequest::create("SkyrimSE", ExecutionMode::Apply,
                                           ModSelection::singleModRoot(root), {});
    const auto result = RunExecutor{}.execute(request, RunServices{cleanup, &observer, configuration.get()});
    QCOMPARE(result.outcome(), RunOutcome::Succeeded);
    QVERIFY(observer.recoveredBeforeWork);
    QVERIFY(!std::filesystem::exists(child));
    // The stable control files must survive so a waiter cannot acquire a different lock inode.
    QVERIFY(std::filesystem::exists(staging / "owner.lock"));
    QVERIFY(std::filesystem::exists(staging / "ownership.manifest"));
    QCOMPARE(cleanup.invocations(), std::size_t{1});
}

void RunExecutorTests::unownedStagingBlocksApplyAndRemainsUntouched() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = std::filesystem::path(directory.path().toStdWString());
    const auto staging = root / ".cao-staging";
    std::filesystem::create_directory(staging);
    QFile evidence(QString::fromStdWString((staging / "user.dds").wstring()));
    QVERIFY(evidence.open(QIODevice::WriteOnly));
    QCOMPARE(evidence.write("retain me"), qint64{9});
    evidence.close();
    CountingSafetyCleanup cleanup;
    const auto configuration = testRunConfiguration();
    const auto request = RunRequest::create("SkyrimSE", ExecutionMode::Apply,
                                           ModSelection::singleModRoot(root), {});
    const auto result = RunExecutor{}.execute(request, RunServices{cleanup, nullptr, configuration.get()});
    QCOMPARE(result.outcome(), RunOutcome::Failed);
    QCOMPARE(result.finalPhase(), RunPhase::Preparing);
    QCOMPARE(result.failures().size(), std::size_t{1});
    QCOMPARE(result.failures().front().path(), std::filesystem::canonical(staging));
    QVERIFY(!result.failures().front().detail().empty());
    QCOMPARE(cleanup.invocations(), std::size_t{1});
    QVERIFY(evidence.open(QIODevice::ReadOnly));
    QCOMPARE(evidence.readAll(), QByteArray("retain me"));
}

void RunExecutorTests::registeredArtifactsAreCleanedAndCommittedOutputSurvives() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = std::filesystem::path(directory.path().toStdWString());
    const auto temporary = root / "temporary.bin";
    const auto output = root / "output.bin";
    cao::run::TemporaryArtifactRegistry artifacts;
    const auto temporaryRegistration = artifacts.registerArtifact(temporary);
    const auto outputRegistration = artifacts.registerArtifact(output);
    QVERIFY(!std::filesystem::exists(temporary));
    QVERIFY(!std::filesystem::exists(output));
    for (const auto& path : {temporary, output}) {
        QFile file(QString::fromStdWString(path.wstring()));
        QVERIFY(file.open(QIODevice::WriteOnly));
        QCOMPARE(file.write("asset"), qint64{5});
    }
    artifacts.commit(outputRegistration);
    const auto result =
        RunExecutor{}.execute(noWorkRequest(ExecutionMode::Apply),
                              RunServices{artifacts, nullptr, testRunConfiguration().get()});
    QCOMPARE(result.outcome(), RunOutcome::Succeeded);
    QVERIFY(!std::filesystem::exists(temporary));
    QVERIFY(std::filesystem::exists(output));
    // A consumed registration cannot acquire ownership again after terminal cleanup.
    QVERIFY_EXCEPTION_THROWN(artifacts.commit(temporaryRegistration), std::logic_error);
}

void RunExecutorTests::cleanupFailuresAreAggregatedWithoutDeletingRetainedMaterial() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = std::filesystem::path(directory.path().toStdWString());
    cao::run::TemporaryArtifactRegistry artifacts;
    const auto temporary = root / "temporary";
    (void)artifacts.registerArtifact(temporary);
    QVERIFY(std::filesystem::create_directory(temporary));
    const auto staging = root / "staging";
    const auto evidence = root / "evidence";
    for (const auto& path : {staging, evidence}) {
        (void)artifacts.registerArtifact(path);
        QVERIFY(std::filesystem::create_directory(path));
    }
    const auto output = staging / "committed.bin";
    const auto committed = artifacts.registerArtifact(output);
    for (const auto& path : {output, staging / "backup.bsa", evidence / "failed.bin"}) {
        QFile file(QString::fromStdWString(path.wstring()));
        QVERIFY(file.open(QIODevice::WriteOnly));
        QCOMPARE(file.write("retained"), qint64{8});
    }
    artifacts.commit(committed);
    std::optional<OptimizationRunResult> result;
    try {
        result =
            RunExecutor{}.execute(noWorkRequest(ExecutionMode::Apply),
                                  RunServices{artifacts, nullptr, testRunConfiguration().get()});
    } catch (const std::exception& error) {
        QFAIL(error.what());
    }
    QCOMPARE(result->outcome(), RunOutcome::CompletedWithFailures);
    QCOMPARE(result->cleanupFailures().size(), std::size_t{2});
    QCOMPARE(result->cleanupFailures()[0].path(), std::filesystem::canonical(evidence));
    QCOMPARE(result->cleanupFailures()[1].path(), std::filesystem::canonical(staging));
    for (const auto& failure : result->cleanupFailures()) {
        QCOMPARE(failure.code(), cao::run::RunFailureCode::TemporaryArtifactCleanupFailed);
        QCOMPARE(failure.phase(), RunPhase::SafetyCleanup);
        QVERIFY(!failure.detail().empty());
    }
    QVERIFY(!std::filesystem::exists(temporary));
    for (const auto& path : {output, staging / "backup.bsa", evidence / "failed.bin"}) {
        QFile file(QString::fromStdWString(path.wstring()));
        QVERIFY(file.open(QIODevice::ReadOnly));
        QCOMPARE(file.readAll(), QByteArray("retained"));
    }
}

void RunExecutorTests::registeredArtifactsAreCleanedOnEveryTerminalPath() {
    for (const auto expected : {RunOutcome::Succeeded, RunOutcome::Cancelled, RunOutcome::Failed}) {
        QTemporaryDir directory;
        QVERIFY(directory.isValid());
        const auto root = std::filesystem::path(directory.path().toStdWString());
        cao::run::TemporaryArtifactRegistry artifacts;
        const auto parent = root / "staging";
        const auto child = parent / "child";
        (void)artifacts.registerArtifact(parent);
        (void)artifacts.registerArtifact(child);
        // Creation may never start, or fail midway, after registration.
        (void)artifacts.registerArtifact(child / "never-created.bin");
        QVERIFY(std::filesystem::create_directories(child));
        std::stop_source stop;
        if (expected == RunOutcome::Cancelled) stop.request_stop();
        const auto result = RunExecutor{}.execute(
            noWorkRequest(ExecutionMode::Apply),
            RunServices{artifacts, nullptr,
                        expected == RunOutcome::Failed ? nullptr : testRunConfiguration().get()},
            stop.get_token());
        QCOMPARE(result.outcome(), expected);
        QVERIFY(result.cleanupFailures().empty());
        QVERIFY(!std::filesystem::exists(parent));
        QCOMPARE(std::count_if(
                     result.phases().begin(), result.phases().end(),
                     [](const auto& phase) { return phase.phase() == RunPhase::SafetyCleanup; }),
                 1);
        // Recreating a name after terminal must not let a second cleanup pass delete new data.
        QVERIFY(std::filesystem::create_directories(child));
        QVERIFY(artifacts.performSafetyCleanup().empty());
        QVERIFY(std::filesystem::exists(child));
        QVERIFY_EXCEPTION_THROWN((void)artifacts.registerArtifact(root / "late"), std::logic_error);
    }
}

void RunExecutorTests::cleanupFailuresPreserveThePrimaryOutcome() {
    for (const bool fatal : {false, true}) {
        QTemporaryDir directory;
        QVERIFY(directory.isValid());
        const auto root = std::filesystem::path(directory.path().toStdWString());
        cao::run::TemporaryArtifactRegistry artifacts;
        const auto retained = root / "retained";
        (void)artifacts.registerArtifact(retained);
        QVERIFY(std::filesystem::create_directories(retained / "unregistered"));
        std::stop_source stop;
        class CleanupObservation final : public cao::run::RunObservationSink {
           public:
            /// Cancels at cleanup entry and observes failure ordering without interrupting removal.
            explicit CleanupObservation(std::stop_source& stop) : _stop(stop) {}
            /// Requests cancellation after the work outcome has already been determined.
            void recordPhase(const RunPhaseRecord& phase) override {
                lastPhase = phase.phase();
                if (lastPhase == RunPhase::SafetyCleanup) _stop.request_stop();
            }
            /// Captures the phase associated with each failure for ordering assertions.
            void recordFailure(const cao::run::RunFailure&) override {
                failurePhases.push_back(lastPhase);
            }
            /// Diagnostics do not participate in these cleanup observations.
            void recordDiagnostic(const cao::run::RunDiagnostic&) override {
                // This test observes only cleanup phase and failure ordering.
            }
            std::vector<RunPhase> failurePhases;
            RunPhase lastPhase{RunPhase::Preparing};

           private:
            std::stop_source& _stop;
        } observations(stop);
        const auto result = RunExecutor{}.execute(
            noWorkRequest(ExecutionMode::Apply),
            RunServices{artifacts, &observations, fatal ? nullptr : testRunConfiguration().get()},
            stop.get_token());
        QCOMPARE(result.outcome(), fatal ? RunOutcome::Failed : RunOutcome::Cancelled);
        QCOMPARE(result.cleanupFailures().size(), std::size_t{1});
        QCOMPARE(result.failures().size(), fatal ? std::size_t{1} : std::size_t{0});
        QCOMPARE(observations.failurePhases.back(), RunPhase::SafetyCleanup);
        QVERIFY(std::filesystem::exists(retained / "unregistered"));
    }
}

void RunExecutorTests::fatalFailureRetainsConcurrentCancellation() {
    class CancellingCleanup final : public SafetyCleanupService {
       public:
        /// Borrows the request source until the synchronous cleanup pass returns.
        explicit CancellingCleanup(std::stop_source& stop) : _stop(stop) {}
        /// Observes concurrent cancellation and reports a safely contained removal failure.
        std::vector<cao::run::RunFailure> performSafetyCleanup() override {
            _stop.request_stop();
            return {{cao::run::RunFailureCode::TemporaryArtifactCleanupFailed,
                     RunPhase::SafetyCleanup, "retained temporary artifact"}};
        }
       private:
        std::stop_source& _stop;
    };
    std::stop_source stop;
    CancellingCleanup cleanup(stop);
    const auto result = RunExecutor{}.execute(noWorkRequest(ExecutionMode::Apply),
                                              RunServices{cleanup}, stop.get_token());
    QCOMPARE(result.outcome(), RunOutcome::Failed);
    QVERIFY(result.cancellationObserved());
    QCOMPARE(result.failures().size(), std::size_t{1});
    QCOMPARE(result.failures().front().code(),
             cao::run::RunFailureCode::ConfigurationLoadingFailed);
    QCOMPARE(result.cleanupFailures().size(), std::size_t{1});
    QCOMPARE(result.cleanupFailures().front().detail(), std::string("retained temporary artifact"));
}

void RunExecutorTests::terminalPrecedenceRetainsAllEvidence() {
    struct Case {
        RunOutcome work;
        bool cancelled;
        bool cleanup;
        RunOutcome expected;
    };
    const Case cases[] = {
        {RunOutcome::Succeeded, false, false, RunOutcome::Succeeded},
        {RunOutcome::Succeeded, false, true, RunOutcome::CompletedWithFailures},
        {RunOutcome::Succeeded, true, false, RunOutcome::Cancelled},
        {RunOutcome::Succeeded, true, true, RunOutcome::Cancelled},
        {RunOutcome::CompletedWithFailures, false, false, RunOutcome::CompletedWithFailures},
        {RunOutcome::CompletedWithFailures, false, true, RunOutcome::CompletedWithFailures},
        {RunOutcome::CompletedWithFailures, true, false, RunOutcome::Cancelled},
        {RunOutcome::CompletedWithFailures, true, true, RunOutcome::Cancelled},
        {RunOutcome::Failed, false, false, RunOutcome::Failed},
        {RunOutcome::Failed, false, true, RunOutcome::Failed},
        {RunOutcome::Failed, true, false, RunOutcome::Failed},
        {RunOutcome::Failed, true, true, RunOutcome::Failed},
    };
    for (const auto& test : cases) {
        std::vector<cao::run::RunFailure> cleanup;
        if (test.cleanup)
            cleanup.emplace_back(cao::run::RunFailureCode::TemporaryArtifactCleanupFailed,
                                 RunPhase::SafetyCleanup, "retained artifact");
        const auto result = OptimizationRunResult::terminal(
            test.work, RunPhase::ArchiveFinalization,
            {RunPhaseRecord::executed(RunPhase::SafetyCleanup)}, cao::run::createRunId(),
            {}, {}, std::move(cleanup), test.cancelled);
        QCOMPARE(result.outcome(), test.expected);
        QCOMPARE(result.cancellationObserved(), test.cancelled);
        QCOMPARE(result.cleanupFailures().size(), test.cleanup ? std::size_t{1} : std::size_t{0});
        if (test.cleanup)
            QCOMPARE(result.cleanupFailures().front().detail(), std::string("retained artifact"));
    }
}

void RunExecutorTests::cleanupExceptionsPreserveCancellation() {
    for (const bool cancelBeforeCleanup : {false, true}) {
        class ThrowingCleanup final : public SafetyCleanupService {
           public:
            /// Borrows cancellation state for the synchronous cleanup attempt.
            explicit ThrowingCleanup(std::stop_source& stop) : _stop(stop) {}
            /// Simulates cancellation racing with a cleanup service exception.
            std::vector<cao::run::RunFailure> performSafetyCleanup() override {
                ++invocations;
                _stop.request_stop();
                throw std::runtime_error("cleanup service failure");
            }
            int invocations{};
           private:
            std::stop_source& _stop;
        };
        std::stop_source stop;
        ThrowingCleanup cleanup(stop);
        if (cancelBeforeCleanup) stop.request_stop();
        const auto result = RunExecutor{}.execute(
            noWorkRequest(ExecutionMode::Apply),
            RunServices{cleanup, nullptr, testRunConfiguration().get()}, stop.get_token());
        QCOMPARE(result.outcome(), RunOutcome::Cancelled);
        QVERIFY(result.cancellationObserved());
        QCOMPARE(cleanup.invocations, 1);
        QVERIFY(result.failures().empty());
        QCOMPARE(result.cleanupFailures().size(), std::size_t{1});
        QCOMPARE(result.cleanupFailures().front().code(),
                 cao::run::RunFailureCode::SafetyCleanupServiceFailed);
    }
}

void RunExecutorTests::cleanupServiceExceptionsAreTerminal() {
    for (const bool standard : {false, true}) {
        class ThrowingCleanup final : public SafetyCleanupService {
           public:
            /// Selects a standard or non-standard service exception.
            explicit ThrowingCleanup(bool standard) : _standard(standard) {}
            /// Simulates an unexpected service failure after cleanup has started.
            std::vector<cao::run::RunFailure> performSafetyCleanup() override {
                ++invocations;
                if (_standard) throw std::runtime_error("cleanup service failure");
                throw 42;
            }
            int invocations{};

           private:
            bool _standard;
        } cleanup(standard);
        const auto result =
            RunExecutor{}.execute(noWorkRequest(ExecutionMode::Apply),
                                  RunServices{cleanup, nullptr, testRunConfiguration().get()});
        QCOMPARE(result.outcome(), RunOutcome::Failed);
        QCOMPARE(cleanup.invocations, 1);
        QCOMPARE(result.cleanupFailures().size(), std::size_t{1});
        QCOMPARE(result.cleanupFailures().front().code(),
                 cao::run::RunFailureCode::SafetyCleanupServiceFailed);
    }
}

void RunExecutorTests::artifactRegistrationRejectsUnownedPaths() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = std::filesystem::path(directory.path().toStdWString());
    cao::run::TemporaryArtifactRegistry artifacts;
    cao::run::TemporaryArtifactRegistry other;
    QVERIFY_EXCEPTION_THROWN((void)artifacts.registerArtifact(root), std::invalid_argument);
    QVERIFY_EXCEPTION_THROWN((void)artifacts.registerArtifact("relative.bin"),
                             std::invalid_argument);
    const auto receipt = artifacts.registerArtifact(root / "Output.bin");
    QVERIFY_EXCEPTION_THROWN((void)artifacts.registerArtifact(root / "." / "Output.bin"),
                             std::invalid_argument);
#ifdef _WIN32
    QVERIFY_EXCEPTION_THROWN((void)artifacts.registerArtifact(root / "output.bin"),
                             std::invalid_argument);
    for (const auto* alias : {"Output.bin.", "Output.bin ", "Output.bin:stream"})
        QVERIFY_EXCEPTION_THROWN((void)artifacts.registerArtifact(root / alias),
                                 std::invalid_argument);
#endif
    QVERIFY_EXCEPTION_THROWN(other.commit(receipt), std::logic_error);
    QFile output(QString::fromStdWString((root / "Output.bin").wstring()));
    QVERIFY(output.open(QIODevice::WriteOnly));
    QCOMPARE(output.write("committed"), qint64{9});
    output.close();
    artifacts.commit(receipt);
    QVERIFY_EXCEPTION_THROWN(artifacts.commit(receipt), std::logic_error);
    QVERIFY_EXCEPTION_THROWN((void)artifacts.registerArtifact(root / "Output.bin"),
                             std::invalid_argument);
    const auto result = RunExecutor{}.execute(noWorkRequest(ExecutionMode::Apply),
        RunServices{artifacts, nullptr, testRunConfiguration().get()});
    QCOMPARE(result.outcome(), RunOutcome::Succeeded);
    QVERIFY(output.open(QIODevice::ReadOnly));
    QCOMPARE(output.readAll(), QByteArray("committed"));
}

void RunExecutorTests::cleanupDoesNotFollowAReplacedParent() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = std::filesystem::path(directory.path().toStdWString());
    const auto staging = root / "staging";
    const auto external = root / "unrelated";
    QVERIFY(std::filesystem::create_directory(external));
    QFile file(QString::fromStdWString((external / "asset.bin").wstring()));
    QVERIFY(file.open(QIODevice::WriteOnly));
    QCOMPARE(file.write("original"), qint64{8});
    file.close();
    cao::run::TemporaryArtifactRegistry artifacts;
    (void)artifacts.registerArtifact(staging);
    (void)artifacts.registerArtifact(staging / "asset.bin");
    std::error_code error;
    std::filesystem::create_directory_symlink(external, staging, error);
    QVERIFY2(!error, error.message().c_str());
    const auto result =
        RunExecutor{}.execute(noWorkRequest(ExecutionMode::Apply),
                              RunServices{artifacts, nullptr, testRunConfiguration().get()});
    QVERIFY(!std::filesystem::exists(staging));
    QVERIFY(file.open(QIODevice::ReadOnly));
    QCOMPARE(file.readAll(), QByteArray("original"));
    QCOMPARE(result.outcome(), RunOutcome::CompletedWithFailures);
    QCOMPARE(result.cleanupFailures().size(), std::size_t{1});
}

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

void RunExecutorTests::workPreparationFailurePreservesStaleArtifacts() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const std::filesystem::path root(directory.path().toStdWString());
    const auto stale = seedStaleStaging(root);
    class FailingPreparation final : public cao::run::RunWorkService {
       public:
        /// Simulates an auxiliary configuration failure before any mutation is authorized.
        void prepare() override { throw std::runtime_error("auxiliary configuration failure"); }
        /// Records any incorrect traversal beyond failed preparation.
        void execute(const cao::run::RunPreparation&, cao::run::RunWorkRecord&,
                     cao::run::TemporaryArtifactRegistry&, cao::run::RunObservationSink&,
                     std::stop_token) override { executed = true; }
        bool executed{};
    } work;
    CountingSafetyCleanup cleanup;
    const auto request = RunRequest::create("profile", ExecutionMode::Apply,
        ModSelection::singleModRoot(root), {RequestedWork::NativeTextureOptimization});
    const auto result = RunExecutor{}.execute(request,
        RunServices{cleanup, nullptr, testRunConfiguration().get(), &work});
    QCOMPARE(result.outcome(), RunOutcome::Failed);
    QCOMPARE(result.finalPhase(), RunPhase::Preparing);
    QVERIFY(!work.executed);
    QVERIFY(std::filesystem::exists(stale / "temporary.dds"));
    QCOMPARE(cleanup.invocations(), std::size_t{1});
}

QTEST_MAIN(RunExecutorTests)
#include "RunExecutorTests.moc"
