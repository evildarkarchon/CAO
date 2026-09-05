#include "Run/RunExecutor.h"
#include "Run/TemporaryArtifactRegistry.h"
#include "Run/StagingRecovery.h"
#include "RunTestConfiguration.h"

#include <QtTest>

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

QTEST_MAIN(RunExecutorTests)
#include "RunExecutorTests.moc"
