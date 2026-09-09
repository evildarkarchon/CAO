#include "Run/AssetRun.h"
#include "Run/ArchiveFirstAssetDiscovery.h"
#include "Run/RunWorkRecord.h"

#include <QtTest>

#include <btu/bsa/archive_data.hpp>
#include <btu/bsa/pack.hpp>
#include <btu/bsa/settings.hpp>

#include <algorithm>
#include <array>
#include <filesystem>
#include <initializer_list>
#include <utility>
#include <vector>

using cao::routing::ExecutionMode;
using cao::routing::ProfileCapabilities;
using cao::routing::ProfileCapability;
using cao::routing::RequestedWork;
using cao::routing::RoutingPolicy;
using cao::routing::RoutingPolicyRequest;
using cao::routing::SkipReason;
using cao::run::AssetRun;
using cao::run::AssetRunAdapters;
using cao::run::AssetRunProgress;
using cao::run::extractArchiveNoOverwrite;

namespace
{
/// Compiles one known-valid policy from concise test inputs and fails with the supplied context.
RoutingPolicy compilePolicy(
    const ExecutionMode mode,
    const std::initializer_list<RequestedWork> work,
    const std::initializer_list<ProfileCapability> capabilities,
    const char *failureMessage)
{
    const auto request = RoutingPolicyRequest::forWork(mode, work);
    const auto profile = ProfileCapabilities::define(".bsa", capabilities);
    const auto result = RoutingPolicy::compile(request, profile);
    if (!result.hasPolicy())
        qFatal("%s", failureMessage);
    return *result.policy();
}

/// Compiles a policy that enables the first archive-to-execution tracer slice.
RoutingPolicy archiveAndTexturePolicy()
{
    return compilePolicy(
        ExecutionMode::Apply,
        {RequestedWork::NativeTextureOptimization, RequestedWork::ArchiveExtraction},
        {ProfileCapability::NativeTextureOptimization,
         ProfileCapability::ArchiveExtraction},
        "The known-valid Asset Run policy failed to compile");
}

/// Compiles a policy enabling every Loose Asset target used by ordering tests.
RoutingPolicy allLooseTargetsPolicy()
{
    return compilePolicy(
        ExecutionMode::Apply,
        {RequestedWork::NativeTextureOptimization,
         RequestedWork::StandardMeshOptimization,
         RequestedWork::AnimationOptimization},
        {ProfileCapability::NativeTextureOptimization,
         ProfileCapability::StandardMeshOptimization,
         ProfileCapability::AnimationOptimization},
        "The known-valid all-target Asset Run policy failed to compile");
}

/// Compiles a selective policy with routed, skipped, unsupported, and multi-operation fixtures.
RoutingPolicy selectiveLoosePolicy()
{
    return compilePolicy(
        ExecutionMode::Apply,
        {RequestedWork::ConvertibleTextureConversion,
         RequestedWork::StandardMeshOptimization},
        {ProfileCapability::ConvertibleTextureConversion,
         ProfileCapability::StandardMeshOptimization,
         ProfileCapability::AnimationOptimization,
         ProfileCapability::MeshReferenceMaintenance},
        "The known-valid selective Asset Run policy failed to compile");
}

/// Compiles a Dry Run policy where Archives are recognized but cannot enter extraction.
RoutingPolicy dryRunArchivePolicy()
{
    return compilePolicy(
        ExecutionMode::DryRun,
        {RequestedWork::NativeTextureOptimization, RequestedWork::ArchiveExtraction},
        {ProfileCapability::NativeTextureOptimization,
         ProfileCapability::ArchiveExtraction},
        "The known-valid Dry Run Asset policy failed to compile");
}

/// Compiles a Dry Run policy carrying conversion, Mesh, and Archive choices together.
RoutingPolicy dryRunLifecyclePolicy()
{
    return compilePolicy(
        ExecutionMode::DryRun,
        {RequestedWork::ConvertibleTextureConversion,
         RequestedWork::StandardMeshOptimization,
         RequestedWork::ArchiveExtraction},
        {ProfileCapability::ConvertibleTextureConversion,
         ProfileCapability::StandardMeshOptimization,
         ProfileCapability::AnimationOptimization,
         ProfileCapability::ArchiveExtraction,
         ProfileCapability::MeshReferenceMaintenance},
        "The known-valid Dry Run lifecycle policy failed to compile");
}

/// Writes one test file after creating its parent directory.
void writeFile(const std::filesystem::path &path,
               const QByteArray &contents = QByteArrayLiteral("fixture"))
{
    QVERIFY(QDir().mkpath(QString::fromStdWString(path.parent_path().wstring())));
    QFile file(QString::fromStdWString(path.wstring()));
    QVERIFY(file.open(QIODevice::WriteOnly));
    QCOMPARE(file.write(contents), contents.size());
}

/// Reads one complete fixture file and fails the test on I/O errors.
QByteArray readFile(const std::filesystem::path &path)
{
    QFile file(QString::fromStdWString(path.wstring()));
    if (!file.open(QIODevice::ReadOnly))
        qFatal("Could not read an Asset Run fixture");
    return file.readAll();
}

/// Captures relative entry types and file bytes so empty-directory or content mutations are visible.
QByteArray snapshotTree(const std::filesystem::path &root)
{
    std::vector<std::filesystem::path> entries;
    for (const auto &entry : std::filesystem::recursive_directory_iterator(root))
        entries.push_back(entry.path());
    std::ranges::sort(entries, {}, [&](const std::filesystem::path &path) {
        return path.lexically_relative(root).generic_wstring();
    });

    QByteArray snapshot;
    for (const auto &path : entries) {
        const auto relative = path.lexically_relative(root);
        const bool directory = std::filesystem::is_directory(path);
        snapshot.append(directory ? "D|" : "F|");
        snapshot.append(QString::fromStdWString(relative.generic_wstring()).toUtf8());
        if (!directory) {
            const auto contents = readFile(path);
            snapshot.append('|');
            snapshot.append(QByteArray::number(contents.size()));
            snapshot.append('|');
            snapshot.append(contents);
        }
        snapshot.append('\n');
    }
    return snapshot;
}

/// Builds one real SSE Texture Archive from a staging tree outside the scanned mod root.
void createTextureArchive(const std::filesystem::path &archivePath,
                          const std::filesystem::path &stagingRoot,
                          const std::span<const std::filesystem::path> files)
{
    auto archive = btu::bsa::ArchiveData(btu::bsa::Settings::get(btu::Game::SSE),
                                         btu::bsa::ArchiveType::Textures);
    for (const auto &file : files)
        QVERIFY(archive.add_file(file));
    archive.set_out_path(archivePath);

    const auto errors = btu::bsa::write(false, std::move(archive), stagingRoot);
    QVERIFY(errors.empty());
    QVERIFY(std::filesystem::is_regular_file(archivePath));
}

/// Creates a valid Archive using private staging so orchestration fixtures stay outside the Mod
/// Root.
void createFixtureArchive(const std::filesystem::path& path) {
    QTemporaryDir stagingDirectory;
    QVERIFY(stagingDirectory.isValid());
    const auto staging = std::filesystem::path(stagingDirectory.path().toStdWString());
    const auto entry = staging / "textures" / "fixture.dds";
    writeFile(entry);
    createTextureArchive(path, staging, std::array{entry});
}
}

class AssetRunTests final : public QObject
{
    Q_OBJECT

private slots:
 /// Retains owned successful and failed attempt identities plus finalization evidence.
 void completeAttemptEvidenceSurvivesAdapters();
 /// Exceptions preserve uncertain mutation and concurrent cancellation after the attempt.
 void throwingAttemptRetainsCancellation();
 /// A finalizer exception becomes an owned phase failure, independent of cancellation.
 void throwingFinalizerRetainsFailure();
 /// Presentation errors cannot discard committed attempt evidence or prevent finalization.
 void throwingObserversPreserveCommittedWork();
 /// Relative selection retains the canonical Mod Root even when execution removes the source.
 void relativeSelectionRetainsMutationScope();
 /// Cancellation from a throwing diagnostics observer retains work and skips finalization.
 void throwingDiagnosticsCancellationSkipsFinalization();
 /// Supplies recoverable and uncertain Archive failures at both extraction boundaries.
 void archiveFailuresControlContinuation_data();
 /// Verifies planned attempts retain evidence and stop unsafe work without cancellation.
 void archiveFailuresControlContinuation();
 /// Verifies safe failures continue and unsafe failures stop before another attempt or packing.
 void mutationAwareFailuresControlContinuation_data();
 /// Verifies mutation evidence controls ordering, terminal failure retention, and attempt totals.
 void mutationAwareFailuresControlContinuation();

 /// Supplies safe Animation failures and unsafe exceptions at both attempt boundaries.
 void animationFailuresPreserveProgressAndEvidence_data();
 /// Verifies Animation outcomes advance progress and retain the evidence controlling continuation.
 void animationFailuresPreserveProgressAndEvidence();

 /// Verifies malformed manifests stop every mutation and finalization adapter.
 void unreadableArchiveStopsRunBeforeMutation();

 /// Verifies explicit precedence and owned collision evidence reach callers before extraction.
 void reportsCollisionsBeforeOrderedExtraction();

 /// Covers cancellation in Archive selection, destination census, and definitive traversal.
 void filesystemTraversalPollsCancellation_data();

 /// Verifies filesystem polling stops the run before execution, diagnostics, or finalization.
 void filesystemTraversalPollsCancellation();

 /// Verifies Archive extraction precedes the one definitive Routed Asset work set.
 void archiveExtractionPrecedesDefinitiveRoutedExecution();

 /// Verifies real Archive extraction preserves Loose Asset precedence through execution.
 void realExtractionPreservesLooseAssetPrecedence();

 /// Verifies target ordering while retaining original ledger order and object identity per target.
 void executesOriginalLedgerAssetsInTargetOrder();

 /// Verifies only Routed Asset attempts contribute to work totals and completed progress.
 void progressAndSkipSummaryExcludeNonWork();

 /// Verifies applying runs retain post-execution Archive finalization ordering.
 void applyFinalizesArchivesAfterRoutedExecution();

 /// Verifies excluded links retain structured diagnostics before finalization and on return.
 void linkedAssetsAreReportedBeforeFinalizationWithoutExecution();

 /// Verifies a cancelled Archive finalizer becomes the run's terminal state.
 void cancelledArchiveFinalizationIsReported();

 /// Verifies Archive skips aggregate while only explicit unsupported roots remain reportable.
 void dryRunAggregatesArchiveSkipsAndKeepsDirectoryUnsupportedPathsSilent();

 /// Verifies Dry Run evaluates carried Loose Asset work without changing the complete mod tree.
 void dryRunLeavesCompleteModTreeUnchangedWhileEvaluatingLooseAssets();

 /// Verifies cancellation stops before the next Routed Asset without changing the work total.
 void cancellationStopsBetweenRoutedAssets();

 /// Verifies Archive cancellation returns before definitive Loose Asset discovery.
 void archiveCancellationSkipsDefinitiveDiscovery();

 /// Verifies cancellation during the final Archive also skips definitive discovery.
 void finalArchiveCancellationSkipsDefinitiveDiscovery();

 /// Verifies cancellation raised during the final Asset skips diagnostics and finalization.
 void cancellationDuringFinalAssetSkipsFinalization();

 /// Verifies an Archive produced by extraction is reported but never counted as run work.
 void nestedArchivesAreReportedWithoutInflatingTheWorkTotal();
};

void AssetRunTests::archiveFailuresControlContinuation_data() {
    QTest::addColumn<bool>("safe");
    QTest::addColumn<int>("failedAttempt");
    QTest::addColumn<bool>("partial");
    QTest::newRow("safe-failure") << true << 1 << false;
    QTest::newRow("unsafe-first") << false << 1 << true;
    QTest::newRow("unsafe-last") << false << 2 << true;
    QTest::newRow("partial-overrides-safe-flag") << true << 1 << true;
}

void AssetRunTests::archiveFailuresControlContinuation() {
    QFETCH(bool, safe);
    QFETCH(int, failedAttempt);
    QFETCH(bool, partial);
    const bool canContinue = safe && !partial;
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = std::filesystem::path(directory.path().toStdWString());
    createFixtureArchive(root / "a.bsa");
    createFixtureArchive(root / "b.bsa");
    writeFile(root / "loose.dds");
    std::size_t attempts = 0;
    std::size_t assets = 0;
    bool finalized = false;
    bool legacyCalled = false;
    std::vector<AssetRunProgress> progress;
    AssetRunAdapters adapters;
    adapters.extractArchive = [&](const auto&) { legacyCalled = true; };
    adapters.executeAsset = [&](const auto&) { ++assets; };
    adapters.reportProgress = [&](const auto& update) { progress.push_back(update); };
    adapters.finalizeArchiveLifecycle = [&] { finalized = true; return true; };
    adapters.extractArchiveWithResult = [&](const cao::run::ArchiveExtractionPlan& plan) {
        ++attempts;
        cao::run::ArchiveExtractionResult attempt;
        attempt.archivePath = plan.archivePath;
        if (plan.modRoot != root || plan.entries.size() != 1)
            qFatal("Extraction did not receive the completed manifest plan");
        if (attempts == static_cast<std::size_t>(failedAttempt)) {
            attempt.failure = cao::run::ArchiveExtractionFailure::MergeFailed;
            attempt.mutation = partial ? cao::execution::MutationState::PartialOrUnknown
                                       : cao::execution::MutationState::None;
            attempt.safeToContinue = safe;
            attempt.detail = "Injected merge failure";
        }
        return attempt;
    };
    const auto result = AssetRun(archiveAndTexturePolicy()).execute(std::array{root}, adapters);
    QCOMPARE(attempts, canContinue ? std::size_t{2} : static_cast<std::size_t>(failedAttempt));
    QCOMPARE(assets, canContinue ? std::size_t{1} : std::size_t{0});
    QCOMPARE(finalized, canContinue);
    QCOMPARE(result.workRecord().ledger.has_value(), canContinue);
    QVERIFY(!legacyCalled);
    QVERIFY(!result.cancelled());
    QCOMPARE(result.archiveAttempts().size(), attempts);
    const auto& failure = result.archiveAttempts()[failedAttempt - 1];
    QCOMPARE(failure.modRoot, root);
    QVERIFY(!failure.succeeded());
    QCOMPARE(failure.safeToContinue, safe);
    QCOMPARE(failure.detail, std::string("Injected merge failure"));
    QVERIFY(failure.archivePath == root / (failedAttempt == 1 ? "a.bsa" : "b.bsa"));
    QCOMPARE(progress[attempts - 1].completed, attempts);
    QCOMPARE(progress[attempts - 1].total, std::size_t{2});
}

void AssetRunTests::mutationAwareFailuresControlContinuation_data() {
    QTest::addColumn<bool>("safe");
    QTest::addColumn<int>("failedAttempt");
    QTest::newRow("safe-failure") << true << 1;
    QTest::newRow("unsafe-failure") << false << 1;
    QTest::newRow("unsafe-final-attempt") << false << 2;
}

void AssetRunTests::mutationAwareFailuresControlContinuation() {
    QFETCH(bool, safe);
    QFETCH(int, failedAttempt);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = std::filesystem::path(directory.path().toStdWString());
    writeFile(root / "first.dds");
    writeFile(root / "second.dds");
    std::size_t attempts = 0;
    bool finalized = false;
    bool legacyCalled = false;
    std::vector<AssetRunProgress> progress;
    AssetRunAdapters adapters;
    adapters.executeAsset = [&](const auto&) { legacyCalled = true; };
    adapters.reportProgress = [&](const auto& update) { progress.push_back(update); };
    adapters.finalizeArchiveLifecycle = [&] {
        finalized = true;
        return true;
    };
    adapters.executeAssetWithResult = [&](const auto& asset) {
        ++attempts;
        return attempts == static_cast<std::size_t>(failedAttempt)
                   ? cao::execution::AssetExecutionResult::failed(
                         cao::execution::AssetExecutionFailure::SaveFailed, "Injected failure",
                         safe ? cao::execution::MutationState::None
                              : cao::execution::MutationState::PartialOrUnknown,
                         safe, asset.executionPath(), "save")
                   : cao::execution::AssetExecutionResult::success();
    };
    const auto result = AssetRun(allLooseTargetsPolicy()).execute(std::array{root}, adapters);
    QCOMPARE(attempts, safe ? std::size_t(2) : static_cast<std::size_t>(failedAttempt));
    QCOMPARE(finalized, safe);
    QVERIFY(!legacyCalled);
    QVERIFY(!result.cancelled());
    QCOMPARE(result.executionFailures().size(), std::size_t(1));
    QCOMPARE(result.executionFailures().front().safeToContinue(), safe);
    QCOMPARE(progress.size(), attempts);
    QCOMPARE(progress.back().completed, attempts);
    QCOMPARE(progress.back().total, std::size_t(2));
}

void AssetRunTests::animationFailuresPreserveProgressAndEvidence_data() {
    QTest::addColumn<bool>("safe");
    QTest::addColumn<int>("failedAttempt");
    QTest::newRow("safe-failure-before-success") << true << 1;
    QTest::newRow("success-before-safe-failure") << true << 2;
    QTest::newRow("unsafe-exception-stops-next-animation") << false << 1;
    QTest::newRow("unsafe-final-exception-stops-finalization") << false << 2;
}

void AssetRunTests::animationFailuresPreserveProgressAndEvidence() {
    QFETCH(bool, safe);
    QFETCH(int, failedAttempt);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = std::filesystem::path(directory.path().toStdWString());
    writeFile(root / "first.hkx");
    writeFile(root / "second.hkx");
    std::size_t attempts = 0;
    bool finalized = false;
    std::filesystem::path failedPath;
    std::vector<AssetRunProgress> progress;
    AssetRunAdapters adapters;
    adapters.reportProgress = [&](const auto& update) { progress.push_back(update); };
    adapters.finalizeArchiveLifecycle = [&] {
        finalized = true;
        return true;
    };
    adapters.executeAssetWithResult = [&](const auto& asset) {
        ++attempts;
        if (attempts != static_cast<std::size_t>(failedAttempt))
            return cao::execution::AssetExecutionResult::success(
                cao::execution::MutationState::Committed);
        failedPath = asset.executionPath();
        // Asset Executor owns exception containment; this seam receives its structured outcome.
        return cao::execution::AssetExecutionResult::failed(
            safe ? cao::execution::AssetExecutionFailure::OperationFailed
                 : cao::execution::AssetExecutionFailure::BackendException,
            "Animation failed", safe ? cao::execution::MutationState::None
                                     : cao::execution::MutationState::PartialOrUnknown,
            safe, failedPath, "optimize_animation", "animation backend diagnostic");
    };

    const auto result = AssetRun(allLooseTargetsPolicy()).execute(std::array{root}, adapters);

    QCOMPARE(attempts, safe ? std::size_t{2} : static_cast<std::size_t>(failedAttempt));
    QCOMPARE(finalized, safe);
    QVERIFY(!result.cancelled());
    QCOMPARE(result.executionFailures().size(), std::size_t{1});
    const auto& failure = result.executionFailures().front();
    QCOMPARE(failure.failure().value(), safe ? cao::execution::AssetExecutionFailure::OperationFailed
                                           : cao::execution::AssetExecutionFailure::BackendException);
    QCOMPARE(failure.failureCategory().value(),
             safe ? cao::execution::ExecutionFailureCategory::Backend
                  : cao::execution::ExecutionFailureCategory::Contract);
    QCOMPARE(failure.mutationState(), safe ? cao::execution::MutationState::None
                                         : cao::execution::MutationState::PartialOrUnknown);
    QCOMPARE(failure.safeToContinue(), safe);
    QCOMPARE(failure.phase(), cao::run::RunPhase::ProcessingAssets);
    QCOMPARE(failure.affectedPath(), failedPath);
    QCOMPARE(failure.operation(), std::string("optimize_animation"));
    QCOMPARE(failure.message(), std::string("Animation failed"));
    QCOMPARE(failure.serviceDetail(), std::string("animation backend diagnostic"));
    QCOMPARE(progress.size(), attempts);
    QCOMPARE(progress.front().completed, std::size_t{1});
    QCOMPARE(progress.front().total, std::size_t{2});
    if (attempts == 2) {
        QCOMPARE(progress.back().completed, std::size_t{2});
        QCOMPARE(progress.back().total, std::size_t{2});
    }
}

void AssetRunTests::unreadableArchiveStopsRunBeforeMutation() {
    QTemporaryDir temporaryDirectory;
    QVERIFY(temporaryDirectory.isValid());
    const auto root = std::filesystem::path(temporaryDirectory.path().toStdWString());
    writeFile(root / "broken.bsa", "invalid manifest");
    writeFile(root / "textures" / "loose.dds");
    const auto before = snapshotTree(root);
    bool extracted = false;
    bool executed = false;
    bool finalized = false;
    std::vector<cao::run::RunFailure> failures;
    const auto result =
        AssetRun(archiveAndTexturePolicy())
            .execute(std::array{root}, AssetRunAdapters{[&](const auto&) { extracted = true; },
                                                        [&](const auto&) { executed = true; },
                                                        {},
                                                        {},
                                                        [&] {
                                                            finalized = true;
                                                            return true;
                                                        },
                                                        {},
                                                        {},
                                                        [&](const cao::run::RunFailure& failure) {
                                                            failures.push_back(failure);
                                                            throw std::runtime_error("failure observer");
                                                        }});
    QCOMPARE(failures.size(), std::size_t{1});
    QCOMPARE(result.failures().size(), failures.size());
    QCOMPARE(failures.front().code(), cao::run::RunFailureCode::ArchiveUnreadable);
    QCOMPARE(result.diagnostics().back().code(), cao::run::RunDiagnosticCode::ObserverFailed);
    QVERIFY(result.collisions().empty());
    QVERIFY(result.ledger().routedAssets().empty());
    QVERIFY(!extracted);
    QVERIFY(!executed);
    QVERIFY(!finalized);
    QVERIFY(!result.cancelled());
    QCOMPARE(snapshotTree(root), before);
}

void AssetRunTests::reportsCollisionsBeforeOrderedExtraction() {
    QTemporaryDir temporaryDirectory;
    QTemporaryDir stagingDirectory;
    QVERIFY(temporaryDirectory.isValid());
    QVERIFY(stagingDirectory.isValid());
    const auto root = std::filesystem::path(temporaryDirectory.path().toStdWString());
    const auto staging = std::filesystem::path(stagingDirectory.path().toStdWString());
    const auto entry = staging / "textures" / "shared.dds";
    writeFile(entry);
    const auto first = root / "a.bsa";
    const auto second = root / "z.bsa";
    createTextureArchive(first, staging, std::array{entry});
    createTextureArchive(second, staging, std::array{entry});
    writeFile(root / "textures" / "shared.dds", "loose wins");
    bool reported = false;
    std::vector<std::filesystem::path> extractions;
    const auto result =
        AssetRun(archiveAndTexturePolicy())
            .execute(
                std::array{root},
                AssetRunAdapters{[&](const auto& archive) {
                                     QVERIFY(reported);
                                     extractions.push_back(archive.executionPath());
                                 },
                                 [](const auto&) {},
                                 {},
                                 {},
                                 {},
                                 {},
                                 [&](const std::span<const cao::run::ArchiveCollision> collisions) {
                                     QVERIFY(extractions.empty());
                                     QCOMPARE(collisions.size(), std::size_t{1});
                                     QCOMPARE(collisions.front().winningArchive(), second);
                                     QVERIFY(collisions.front().looseAssetWins());
                                     reported = true;
                                     throw std::runtime_error("collision observer");
                                 }},
                cao::run::ArchivePrecedence::explicitOrder({"z.bsa", "a.bsa"}));
    QVERIFY(result.failures().empty());
    QVERIFY(reported);
    QCOMPARE(extractions, (std::vector{second, first}));
    QCOMPARE(result.collisions().size(), std::size_t{1});
    QCOMPARE(result.collisions().front().winningArchive(), second);
    QCOMPARE(result.collisions().front().shadowedArchives().size(), std::size_t{1});
    QCOMPARE(result.collisions().front().shadowedArchives().front(), first);
    QCOMPARE(result.diagnostics().back().code(), cao::run::RunDiagnosticCode::ObserverFailed);
}

void AssetRunTests::filesystemTraversalPollsCancellation_data()
{
    QTest::addColumn<int>("scenario");
    QTest::newRow("extraction-disabled") << 0;
    QTest::newRow("no-selected-archives") << 1;
    QTest::newRow("explicit-archive-destination-census") << 2;
    QTest::newRow("definitive-mod-tree") << 3;
    QTest::newRow("definitive-extraction-destination") << 4;
}

void AssetRunTests::filesystemTraversalPollsCancellation()
{
    QFETCH(int, scenario);
    QTemporaryDir temporaryDirectory;
    QVERIFY(temporaryDirectory.isValid());
    const auto root = std::filesystem::path(temporaryDirectory.path().toStdWString());
    const auto archive = root / "content.bsa";
    if (scenario >= 2) createFixtureArchive(archive);
    // Unsupported files still require directory traversal, but never provide an execution
    // callback where the old implementation could happen to notice cancellation instead.
    for (int index = 0; index < 100; ++index)
        writeFile(root / (std::to_string(index) + ".txt"));

    const AssetRun run(scenario == 0 ? allLooseTargetsPolicy() : archiveAndTexturePolicy());
    const std::array roots{scenario == 2 || scenario == 4 ? archive : root};
    bool armed = scenario < 3;
    int polls = 0;
    int extractions = 0;
    bool executed = false;
    bool finalized = false;
    bool diagnosed = false;
    const auto result = run.execute(roots, AssetRunAdapters{
        [&](const cao::routing::RoutedAsset &) {
            ++extractions;
            armed = true;
        },
        [&](const cao::routing::RoutedAsset &) { executed = true; },
        {},
        [&] { return armed && ++polls >= 4; },
        [&] { finalized = true; return true; },
        [&](const cao::run::AssetRunDiagnostics &) { diagnosed = true; }});

    QVERIFY(result.cancelled());
    QVERIFY(result.ledger().routedAssets().empty());
    QCOMPARE(extractions, scenario < 3 ? 0 : 1);
    QVERIFY(!executed);
    QVERIFY(!finalized);
    QVERIFY(!diagnosed);
}

void AssetRunTests::archiveExtractionPrecedesDefinitiveRoutedExecution()
{
    QTemporaryDir temporaryDirectory;
    QVERIFY(temporaryDirectory.isValid());

    const auto root = std::filesystem::path(temporaryDirectory.path().toStdWString());
    const auto archive = root / "content.bsa";
    const auto looseTexture = root / "textures" / "loose.dds";
    const auto extractedTexture = root / "textures" / "extracted.dds";
    createFixtureArchive(archive);
    writeFile(looseTexture);

    std::vector<std::filesystem::path> executedPaths;
    std::vector<AssetRunProgress> progress;
    bool archiveExtracted = false;
    const AssetRun run(archiveAndTexturePolicy());
    const std::array roots{root};
    const auto result = run.execute(
        roots,
        AssetRunAdapters{
            [&](const cao::routing::RoutedAsset &selectedArchive) {
                QVERIFY(executedPaths.empty());
                QVERIFY(selectedArchive.executionPath() == archive);
                archiveExtracted = true;
                writeFile(extractedTexture);
            },
            [&](const cao::routing::RoutedAsset &asset) {
                QVERIFY(archiveExtracted);
                executedPaths.push_back(asset.executionPath());
            },
            [&](const AssetRunProgress &update) { progress.push_back(update); }});

    QVERIFY(!result.cancelled());
    QCOMPARE(executedPaths.size(), std::size_t{2});
    QCOMPARE(static_cast<std::size_t>(std::count(executedPaths.begin(), executedPaths.end(),
                                                 looseTexture)),
             std::size_t{1});
    QCOMPARE(static_cast<std::size_t>(std::count(executedPaths.begin(), executedPaths.end(),
                                                 extractedTexture)),
             std::size_t{1});
    QCOMPARE(result.ledger().routedAssets().size(), std::size_t{2});
    QCOMPARE(progress.size(), std::size_t{3});
    QCOMPARE(progress[0].phase, cao::routing::RoutedAssetPhase::ArchiveExtraction);
    QCOMPARE(progress[0].completed, std::size_t{1});
    QCOMPARE(progress[0].total, std::size_t{1});
    QCOMPARE(progress[1].phase, cao::routing::RoutedAssetPhase::LooseAssetProcessing);
    QCOMPARE(progress[1].completed, std::size_t{1});
    QCOMPARE(progress[1].total, std::size_t{2});
    QCOMPARE(progress[2].completed, std::size_t{2});
    QCOMPARE(progress[2].total, std::size_t{2});
}

void AssetRunTests::realExtractionPreservesLooseAssetPrecedence()
{
    QTemporaryDir modDirectory;
    QTemporaryDir stagingDirectory;
    QVERIFY(modDirectory.isValid());
    QVERIFY(stagingDirectory.isValid());

    const auto root = std::filesystem::path(modDirectory.path().toStdWString());
    const auto stagingRoot = std::filesystem::path(stagingDirectory.path().toStdWString());
    const auto archive = root / "content.bsa";
    const auto collision = root / "textures" / "collision.dds";
    const auto archivedOnly = root / "textures" / "archived-only.dds";
    const auto stagedCollision = stagingRoot / "textures" / "collision.dds";
    const auto stagedArchivedOnly = stagingRoot / "textures" / "archived-only.dds";
    writeFile(stagedCollision, "archived collision");
    writeFile(stagedArchivedOnly, "archived only");
    const std::array archivedFiles{stagedCollision, stagedArchivedOnly};
    createTextureArchive(archive, stagingRoot, archivedFiles);
    writeFile(collision, "loose collision");

    std::vector<std::filesystem::path> executedPaths;
    const AssetRun run(archiveAndTexturePolicy());
    const std::array roots{root};
    const auto result = run.execute(
        roots,
        AssetRunAdapters{
            [](const cao::routing::RoutedAsset &selectedArchive) {
                extractArchiveNoOverwrite(selectedArchive.executionPath(), false);
            },
            [&](const cao::routing::RoutedAsset &asset) {
                executedPaths.push_back(asset.executionPath());
            }});

    QCOMPARE(readFile(collision), QByteArray("loose collision"));
    QCOMPARE(readFile(archivedOnly), QByteArray("archived only"));
    QCOMPARE(static_cast<std::size_t>(std::count(executedPaths.begin(), executedPaths.end(),
                                                 collision)),
             std::size_t{1});
    QCOMPARE(static_cast<std::size_t>(std::count(executedPaths.begin(), executedPaths.end(),
                                                 archivedOnly)),
             std::size_t{1});
    QCOMPARE(result.ledger().routedAssets().size(), std::size_t{2});
}

void AssetRunTests::executesOriginalLedgerAssetsInTargetOrder()
{
    QTemporaryDir temporaryDirectory;
    QVERIFY(temporaryDirectory.isValid());

    const auto root = std::filesystem::path(temporaryDirectory.path().toStdWString());
    const std::array paths{
        root / "meshes" / "first.nif",
        root / "textures" / "first.dds",
        root / "animations" / "only.hkx",
        root / "textures" / "second.dds",
        root / "meshes" / "second.nif"};
    for (const auto &path : paths)
        writeFile(path);

    std::vector<const cao::routing::RoutedAsset *> executedAssets;
    const AssetRun run(allLooseTargetsPolicy());
    const auto result = run.execute(
        paths,
        AssetRunAdapters{
            [](const cao::routing::RoutedAsset &) {
                qFatal("No Archive should be selected in the Loose Asset ordering test");
            },
            [&](const cao::routing::RoutedAsset &asset) {
                executedAssets.push_back(&asset);
            }});

    const std::array expectedPaths{paths[1], paths[3], paths[0], paths[4], paths[2]};
    QCOMPARE(executedAssets.size(), expectedPaths.size());
    for (std::size_t index = 0; index < expectedPaths.size(); ++index) {
        QVERIFY(executedAssets[index]->executionPath() == expectedPaths[index]);
        const auto ledgerAssets = result.ledger().routedAssets();
        const auto ledgerAsset = std::find_if(
            ledgerAssets.begin(), ledgerAssets.end(), [&](const cao::routing::RoutedAsset &asset) {
                return asset.executionPath() == expectedPaths[index];
            });
        QVERIFY(ledgerAsset != ledgerAssets.end());
        QCOMPARE(executedAssets[index], &*ledgerAsset);
    }
}

void AssetRunTests::progressAndSkipSummaryExcludeNonWork()
{
    QTemporaryDir temporaryDirectory;
    QVERIFY(temporaryDirectory.isValid());

    const auto root = std::filesystem::path(temporaryDirectory.path().toStdWString());
    const std::array paths{
        root / "textures" / "convertible.tga",
        root / "textures" / "excluded.dds",
        root / "meshes" / "both-operations.nif",
        root / "animations" / "disabled.hkx",
        root / "docs" / "unsupported.txt"};
    for (const auto &path : paths)
        writeFile(path);

    std::size_t executionAttempts = 0;
    std::vector<AssetRunProgress> progress;
    const AssetRun run(selectiveLoosePolicy());
    const auto result = run.execute(
        paths,
        AssetRunAdapters{
            [](const cao::routing::RoutedAsset &) {
                qFatal("No Archive should be selected in the progress test");
            },
            [&](const cao::routing::RoutedAsset &) { ++executionAttempts; },
            [&](const AssetRunProgress &update) { progress.push_back(update); }});

    QCOMPARE(result.ledger().routedAssets().size(), std::size_t{2});
    QCOMPARE(executionAttempts, std::size_t{2});
    QCOMPARE(progress.size(), std::size_t{2});
    QCOMPARE(progress[0].completed, std::size_t{1});
    QCOMPARE(progress[0].total, std::size_t{2});
    QCOMPARE(progress[1].completed, std::size_t{2});
    QCOMPARE(progress[1].total, std::size_t{2});
    QCOMPARE(result.skippedAssetCount(SkipReason::ExcludedAssetVariant), std::size_t{1});
    QCOMPARE(result.skippedAssetCount(SkipReason::DisabledAssetKind), std::size_t{1});
    QCOMPARE(result.skippedAssetCount(SkipReason::DisabledPhase), std::size_t{0});
}

void AssetRunTests::applyFinalizesArchivesAfterRoutedExecution()
{
    QTemporaryDir temporaryDirectory;
    QVERIFY(temporaryDirectory.isValid());

    const auto texture = std::filesystem::path(temporaryDirectory.path().toStdWString())
                         / "textures" / "native.dds";
    writeFile(texture);

    std::vector<QByteArray> events;
    const AssetRun run(archiveAndTexturePolicy());
    const std::array roots{texture};
    static_cast<void>(run.execute(
        roots,
        AssetRunAdapters{
            [](const cao::routing::RoutedAsset &) {
                qFatal("No Archive should be selected in the finalization-order test");
            },
            [&](const cao::routing::RoutedAsset &) {
                events.push_back(QByteArrayLiteral("execute"));
            },
            {},
            {},
            [&] {
                events.push_back(QByteArrayLiteral("finalize"));
                return true;
            },
            [&](const cao::run::AssetRunDiagnostics &) {
                events.push_back(QByteArrayLiteral("report"));
            }}));

    const std::vector<QByteArray> expectedEvents{QByteArrayLiteral("execute"),
                                                 QByteArrayLiteral("report"),
                                                 QByteArrayLiteral("finalize")};
    QCOMPARE(events, expectedEvents);
}

void AssetRunTests::linkedAssetsAreReportedBeforeFinalizationWithoutExecution()
{
    QTemporaryDir temporaryDirectory;
    QVERIFY(temporaryDirectory.isValid());

    const auto base = std::filesystem::path(temporaryDirectory.path().toStdWString());
    const auto root = base / "mod";
    const auto texture = root / "native.dds";
    const auto outside = base / "outside.dds";
    const auto link = root / "linked.dds";
    writeFile(texture);
    writeFile(outside, "outside bytes");
    std::error_code error;
    std::filesystem::create_symlink(outside, link, error);
    QVERIFY2(!error, error.message().c_str());

    std::vector<std::filesystem::path> executedPaths;
    std::vector<cao::run::RunDiagnostic> reportedDiagnostics;
    bool finalizedAfterReport = false;
    const AssetRun run(archiveAndTexturePolicy());
    const std::array roots{root};
    const auto result = run.execute(
        roots,
        AssetRunAdapters{
            [](const cao::routing::RoutedAsset &) {
                qFatal("No Archive should be selected in the linked-Asset reporting test");
            },
            [&](const cao::routing::RoutedAsset &asset) {
                executedPaths.push_back(asset.executionPath());
            },
            {},
            {},
            [&] {
                finalizedAfterReport = !reportedDiagnostics.empty();
                return true;
            },
            [&](const cao::run::AssetRunDiagnostics &diagnostics) {
                reportedDiagnostics.assign(diagnostics.diagnostics().begin(),
                                           diagnostics.diagnostics().end());
            }});

    // Remove the link itself before QTemporaryDir cleanup, which does not handle Windows links.
    QVERIFY(std::filesystem::remove(link));
    QVERIFY(!result.cancelled());
    QCOMPARE(executedPaths, std::vector<std::filesystem::path>{texture});
    QCOMPARE(result.ledger().routedAssets().size(), std::size_t{1});
    QVERIFY(finalizedAfterReport);
    QCOMPARE(reportedDiagnostics.size(), std::size_t{1});
    QCOMPARE(result.diagnostics().size(), reportedDiagnostics.size());
    for (std::size_t index = 0; index < reportedDiagnostics.size(); ++index) {
        const auto &reported = reportedDiagnostics[index];
        const auto &retained = result.diagnostics()[index];
        QCOMPARE(reported.code(), cao::run::RunDiagnosticCode::LinkedEntryExcluded);
        QCOMPARE(reported.phase(), cao::run::RunPhase::DiscoveringArchives);
        QVERIFY(reported.path() == link);
        QCOMPARE(retained.code(), reported.code());
        QCOMPARE(retained.phase(), reported.phase());
        QCOMPARE(retained.detail(), reported.detail());
        QVERIFY(retained.path() == reported.path());
    }
    QCOMPARE(readFile(outside), QByteArrayLiteral("outside bytes"));
}

void AssetRunTests::cancelledArchiveFinalizationIsReported()
{
    QTemporaryDir temporaryDirectory;
    QVERIFY(temporaryDirectory.isValid());

    const auto texture = std::filesystem::path(temporaryDirectory.path().toStdWString())
                         / "textures" / "native.dds";
    writeFile(texture);

    bool resultReportedBeforeFinalization = false;
    const AssetRun run(archiveAndTexturePolicy());
    const std::array roots{texture};
    const auto result = run.execute(
        roots,
        AssetRunAdapters{
            [](const cao::routing::RoutedAsset &) {
                qFatal("No Archive should be selected in the finalization-cancellation test");
            },
            [](const cao::routing::RoutedAsset &) {},
            {},
            {},
            [] { return false; },
            [&](const cao::run::AssetRunDiagnostics &) {
                resultReportedBeforeFinalization = true;
            }});

    QVERIFY(resultReportedBeforeFinalization);
    QVERIFY(result.cancelled());
}

void AssetRunTests::dryRunAggregatesArchiveSkipsAndKeepsDirectoryUnsupportedPathsSilent()
{
    QTemporaryDir temporaryDirectory;
    QVERIFY(temporaryDirectory.isValid());

    const auto base = std::filesystem::path(temporaryDirectory.path().toStdWString());
    const auto directoryRoot = base / "mod";
    const auto texture = directoryRoot / "textures" / "native.dds";
    const auto unsupportedDirectoryEntry = directoryRoot / "docs" / "readme.txt";
    const auto firstArchive = directoryRoot / "first.bsa";
    const auto secondArchive = directoryRoot / "second.bsa";
    const auto explicitUnsupported = base / "explicit.txt";
    for (const auto &path : {texture,
                             unsupportedDirectoryEntry,
                             firstArchive,
                             secondArchive,
                             explicitUnsupported}) {
        writeFile(path);
    }

    bool extractionAttempted = false;
    std::vector<std::filesystem::path> executedPaths;
    const AssetRun run(dryRunArchivePolicy());
    const std::array roots{directoryRoot, explicitUnsupported};
    const auto result = run.execute(
        roots,
        AssetRunAdapters{
            [&](const cao::routing::RoutedAsset &) { extractionAttempted = true; },
            [&](const cao::routing::RoutedAsset &asset) {
                executedPaths.push_back(asset.executionPath());
            }});

    QVERIFY(!extractionAttempted);
    QCOMPARE(executedPaths, std::vector<std::filesystem::path>{texture});
    QCOMPARE(result.skippedAssetCount(SkipReason::DisabledPhase), std::size_t{2});
    const auto unsupported = result.unsupportedExplicitPaths();
    QCOMPARE(unsupported.size(), std::size_t{1});
    QVERIFY(unsupported.front() == explicitUnsupported);
    QVERIFY(std::find(unsupported.begin(), unsupported.end(), unsupportedDirectoryEntry)
            == unsupported.end());
}

void AssetRunTests::dryRunLeavesCompleteModTreeUnchangedWhileEvaluatingLooseAssets()
{
    QTemporaryDir temporaryDirectory;
    QVERIFY(temporaryDirectory.isValid());

    const auto root = std::filesystem::path(temporaryDirectory.path().toStdWString()) / "mod";
    const auto archive = root / "content.bsa";
    const auto convertibleTexture = root / "textures" / "convertible.tga";
    const auto mesh = root / "meshes" / "actor.nif";
    const auto disabledAnimation = root / "animations" / "walk.hkx";
    const auto unsupported = root / "docs" / "readme.txt";
    const auto emptyDirectory = root / "empty" / "nested";
    writeFile(archive, "archive bytes");
    writeFile(root / "nested" / "second.bsa", "invalid manifest bytes");
    writeFile(root / ".CAO-Staging-unknown" / "hidden.bsa", "uncommitted archive bytes");
    writeFile(convertibleTexture, "texture bytes");
    writeFile(mesh, "mesh bytes");
    writeFile(disabledAnimation, "animation bytes");
    writeFile(unsupported, "documentation bytes");
    QVERIFY(QDir().mkpath(QString::fromStdWString(emptyDirectory.wstring())));
    const auto originalTree = snapshotTree(root);

    struct ExecutionObservation final
    {
        std::filesystem::path path;
        ExecutionMode mode;
        bool optimization;
        bool conversion;
        bool meshReferenceMaintenance;
    };
    std::vector<ExecutionObservation> executed;
    std::vector<AssetRunProgress> progress;
    bool extractionAttempted = false;
    bool finalizationAttempted = false;
    const AssetRun run(dryRunLifecyclePolicy());
    const std::array roots{root};
    const auto result = run.execute(
        roots,
        AssetRunAdapters{
            [&](const cao::routing::RoutedAsset &) {
                extractionAttempted = true;
                writeFile(root / "textures" / "extracted.dds", "extracted bytes");
            },
            [&](const cao::routing::RoutedAsset &asset) {
                executed.push_back(ExecutionObservation{
                    asset.executionPath(),
                    asset.executionMode(),
                    asset.operations().contains(cao::routing::AssetOperation::Optimization),
                    asset.operations().contains(cao::routing::AssetOperation::Conversion),
                    asset.operations().contains(
                        cao::routing::AssetOperation::MeshReferenceMaintenance)});
            },
            [&](const AssetRunProgress &update) { progress.push_back(update); },
            {},
            [&] {
                finalizationAttempted = true;
                writeFile(root / "packed.bsa", "packed bytes");
                return std::filesystem::remove(emptyDirectory);
            }});

    QVERIFY(!extractionAttempted);
    QVERIFY(!finalizationAttempted);
    QCOMPARE(snapshotTree(root), originalTree);
    QCOMPARE(executed.size(), std::size_t{2});
    QVERIFY(executed[0].path == convertibleTexture);
    QCOMPARE(executed[0].mode, ExecutionMode::DryRun);
    QVERIFY(!executed[0].optimization);
    QVERIFY(executed[0].conversion);
    QVERIFY(!executed[0].meshReferenceMaintenance);
    QVERIFY(executed[1].path == mesh);
    QCOMPARE(executed[1].mode, ExecutionMode::DryRun);
    QVERIFY(executed[1].optimization);
    QVERIFY(!executed[1].conversion);
    QVERIFY(executed[1].meshReferenceMaintenance);
    QCOMPARE(result.ledger().routedAssets().size(), std::size_t{2});
    QCOMPARE(result.skippedAssetCount(SkipReason::DisabledPhase), std::size_t{2});
    QCOMPARE(result.skippedAssetCount(SkipReason::DisabledAssetKind), std::size_t{1});
    QCOMPARE(progress.size(), std::size_t{2});
    QCOMPARE(progress[0].completed, std::size_t{1});
    QCOMPARE(progress[0].total, std::size_t{2});
    QCOMPARE(progress[1].completed, std::size_t{2});
    QCOMPARE(progress[1].total, std::size_t{2});
}

void AssetRunTests::cancellationStopsBetweenRoutedAssets()
{
    QTemporaryDir temporaryDirectory;
    QVERIFY(temporaryDirectory.isValid());

    const auto root = std::filesystem::path(temporaryDirectory.path().toStdWString());
    const std::array paths{root / "textures" / "first.dds",
                           root / "textures" / "second.dds"};
    for (const auto &path : paths)
        writeFile(path);

    std::size_t attempts = 0;
    std::vector<AssetRunProgress> progress;
    const AssetRun run(allLooseTargetsPolicy());
    const auto result = run.execute(
        paths,
        AssetRunAdapters{
            [](const cao::routing::RoutedAsset &) {
                qFatal("No Archive should be selected in the cancellation test");
            },
            [&](const cao::routing::RoutedAsset &) { ++attempts; },
            [&](const AssetRunProgress &update) { progress.push_back(update); },
            [&] { return attempts == 1; }});

    QVERIFY(result.cancelled());
    QCOMPARE(result.ledger().routedAssets().size(), std::size_t{2});
    QCOMPARE(attempts, std::size_t{1});
    QCOMPARE(progress.size(), std::size_t{1});
    QCOMPARE(progress.front().completed, std::size_t{1});
    QCOMPARE(progress.front().total, std::size_t{2});
}

void AssetRunTests::archiveCancellationSkipsDefinitiveDiscovery()
{
    QTemporaryDir temporaryDirectory;
    QVERIFY(temporaryDirectory.isValid());

    const auto root = std::filesystem::path(temporaryDirectory.path().toStdWString());
    const std::array paths{root / "first.bsa",
                           root / "second.bsa",
                           root / "textures" / "loose.dds"};
    for (const auto& path : paths) {
        if (path.extension() == ".bsa")
            createFixtureArchive(path);
        else
            writeFile(path);
    }

    std::size_t extractionAttempts = 0;
    const AssetRun run(archiveAndTexturePolicy());
    const std::array roots{root};
    const auto result = run.execute(
        roots,
        AssetRunAdapters{
            [&](const cao::routing::RoutedAsset &) { ++extractionAttempts; },
            [](const cao::routing::RoutedAsset &) {
                qFatal("No Loose Asset should execute after Archive cancellation");
            },
            {},
            [&] { return extractionAttempts == 1; }});

    QVERIFY(result.cancelled());
    QCOMPARE(extractionAttempts, std::size_t{1});
    QVERIFY(result.ledger().routedAssets().empty());
}

void AssetRunTests::finalArchiveCancellationSkipsDefinitiveDiscovery()
{
    QTemporaryDir temporaryDirectory;
    QVERIFY(temporaryDirectory.isValid());

    const auto root = std::filesystem::path(temporaryDirectory.path().toStdWString());
    const std::array paths{root / "only.bsa", root / "textures" / "loose.dds"};
    for (const auto& path : paths) {
        if (path.extension() == ".bsa")
            createFixtureArchive(path);
        else
            writeFile(path);
    }

    std::size_t extractionAttempts = 0;
    const AssetRun run(archiveAndTexturePolicy());
    const std::array roots{root};
    const auto result = run.execute(
        roots,
        AssetRunAdapters{
            [&](const cao::routing::RoutedAsset &) { ++extractionAttempts; },
            [](const cao::routing::RoutedAsset &) {
                qFatal("No Loose Asset should execute after final Archive cancellation");
            },
            {},
            [&] { return extractionAttempts == 1; }});

    QVERIFY(result.cancelled());
    QCOMPARE(extractionAttempts, std::size_t{1});
    QVERIFY(result.ledger().routedAssets().empty());
}

void AssetRunTests::cancellationDuringFinalAssetSkipsFinalization()
{
    QTemporaryDir temporaryDirectory;
    QVERIFY(temporaryDirectory.isValid());

    const auto root = std::filesystem::path(temporaryDirectory.path().toStdWString());
    const std::array paths{root / "textures" / "first.dds",
                           root / "textures" / "second.dds"};
    for (const auto &path : paths)
        writeFile(path);

    std::size_t attempts = 0;
    bool finalized = false;
    bool reportedDiagnostics = false;
    const AssetRun run(allLooseTargetsPolicy());
    const auto result = run.execute(
        paths,
        AssetRunAdapters{
            [](const cao::routing::RoutedAsset &) {
                qFatal("No Archive should be selected in the final-Asset cancellation test");
            },
            [&](const cao::routing::RoutedAsset &) { ++attempts; },
            {},
            // Cancellation only becomes observable once the last attempt has completed, which is
            // the case no loop head can catch.
            [&] { return attempts == paths.size(); },
            [&] {
                finalized = true;
                return true;
            },
            [&](const cao::run::AssetRunDiagnostics &) { reportedDiagnostics = true; }});

    QVERIFY(result.cancelled());
    QCOMPARE(attempts, paths.size());
    QVERIFY(!finalized);
    QVERIFY(!reportedDiagnostics);
}

void AssetRunTests::nestedArchivesAreReportedWithoutInflatingTheWorkTotal()
{
    QTemporaryDir temporaryDirectory;
    QVERIFY(temporaryDirectory.isValid());

    const auto root = std::filesystem::path(temporaryDirectory.path().toStdWString());
    const auto archive = root / "content.bsa";
    const auto nestedArchive = root / "textures" / "nested.bsa";
    const auto extractedTexture = root / "textures" / "extracted.dds";
    createFixtureArchive(archive);

    std::vector<std::filesystem::path> executedPaths;
    std::size_t reportedNestedArchives = 0;
    std::size_t looseWorkTotal = 0;
    const AssetRun run(archiveAndTexturePolicy());
    const std::array roots{root};
    const auto result = run.execute(
        roots,
        AssetRunAdapters{
            [&](const cao::routing::RoutedAsset &) {
                // The game reads no Archive nested inside another, so an Archive that extraction
                // itself produced is malformed mod content rather than work a later round owes.
                writeFile(nestedArchive);
                writeFile(extractedTexture);
            },
            [&](const cao::routing::RoutedAsset &asset) {
                executedPaths.push_back(asset.executionPath());
            },
            [&](const AssetRunProgress &update) {
                if (update.phase == cao::routing::RoutedAssetPhase::LooseAssetProcessing)
                    looseWorkTotal = update.total;
            },
            {},
            {},
            [&](const cao::run::AssetRunDiagnostics &diagnostics) {
                reportedNestedArchives = diagnostics.nestedArchiveCount();
            }});

    QVERIFY(!result.cancelled());
    // Routing it would have promised a Routed Asset that no post-extraction target executes, so
    // progress would have reported a total the run could never complete.
    QCOMPARE(executedPaths, std::vector<std::filesystem::path>{extractedTexture});
    QCOMPARE(looseWorkTotal, std::size_t{1});
    // Silence would leave the author believing the nested Archive's contents were processed, when
    // the game will not read them either.
    QCOMPARE(reportedNestedArchives, std::size_t{1});
    QCOMPARE(result.nestedArchiveCount(), std::size_t{1});
}

void AssetRunTests::completeAttemptEvidenceSurvivesAdapters() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = std::filesystem::path(directory.path().toStdWString());
    writeFile(root / "a.dds");
    writeFile(root / "b.dds");
    AssetRunAdapters adapters;
    int calls = 0;
    adapters.executeAssetWithResult = [&](const auto&) {
        using namespace cao::execution;
        return ++calls == 1 ? AssetExecutionResult::success(MutationState::Committed)
            : AssetExecutionResult::failed(AssetExecutionFailure::LoadFailed, "retained");
    };
    adapters.finalizeArchiveLifecycleWithResult = [&] {
        cao::run::ArchiveFinalizationResult finalization;
        finalization.attempts.push_back({root / "output.bsa",
            cao::execution::MutationState::Committed, {}, true, "", root});
        return finalization;
    };
    const auto result = AssetRun(archiveAndTexturePolicy()).execute(std::array{root}, adapters);
    adapters = {};
    QCOMPARE(result.assetAttempts().size(), std::size_t{2});
    QCOMPARE(result.assetAttempts()[0].modRoot, std::filesystem::canonical(root));
    QVERIFY(result.assetAttempts()[0].result.succeeded());
    QCOMPARE(result.assetAttempts()[0].result.mutationState(), cao::execution::MutationState::Committed);
    QCOMPARE(result.assetAttempts()[0].asset.executionPath(), root / "a.dds");
    QVERIFY(!result.assetAttempts()[1].result.succeeded());
    QCOMPARE(result.executionFailures().size(), std::size_t{1});
    QVERIFY(result.finalizationResult().has_value());
    QCOMPARE(result.finalizationResult()->attempts.front().modRoot, root);
    const auto record = result.workRecord();
    QVERIFY(record.ledger.has_value());
    QCOMPARE(record.assetAttempts.size(), std::size_t{2});
    QCOMPARE(record.finalizations.size(), std::size_t{1});
}

void AssetRunTests::throwingAttemptRetainsCancellation() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = std::filesystem::path(directory.path().toStdWString());
    writeFile(root / "a.dds");
    writeFile(root / "b.dds");
    AssetRunAdapters adapters;
    bool cancelled = false;
    adapters.isCancelled = [&] { return cancelled; };
    adapters.executeAssetWithResult = [&](const auto&) -> cao::execution::AssetExecutionResult {
        cancelled = true;
        throw std::runtime_error("adapter failed");
    };
    const auto result = AssetRun(archiveAndTexturePolicy()).execute(std::array{root}, adapters);
    QVERIFY(result.cancelled());
    QCOMPARE(result.assetAttempts().size(), std::size_t{1});
    QCOMPARE(result.assetAttempts()[0].result.mutationState(), cao::execution::MutationState::PartialOrUnknown);
    QVERIFY(!result.assetAttempts()[0].result.safeToContinue());
    QCOMPARE(result.assetAttempts()[0].result.message(), std::string("adapter failed"));
}

void AssetRunTests::throwingFinalizerRetainsFailure() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = std::filesystem::path(directory.path().toStdWString());
    AssetRunAdapters adapters;
    adapters.finalizeArchiveLifecycleWithResult = []() -> cao::run::ArchiveFinalizationResult {
        throw std::runtime_error("finalizer failed");
    };
    const auto result = AssetRun(archiveAndTexturePolicy()).execute(std::array{root}, adapters);
    QVERIFY(!result.cancelled());
    QVERIFY(result.finalizationResult().has_value());
    QCOMPARE(result.finalizationResult()->failure,
             std::optional{cao::run::ArchiveFinalizationFailure::UnexpectedException});
    QVERIFY(!result.finalizationResult()->safeToContinue);
    QCOMPARE(result.finalizationResult()->detail, std::string("finalizer failed"));
}

void AssetRunTests::throwingObserversPreserveCommittedWork() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = std::filesystem::path(directory.path().toStdWString());
    writeFile(root / "a.dds");
    AssetRunAdapters adapters;
    adapters.executeAssetWithResult = [](const auto&) {
        return cao::execution::AssetExecutionResult::success(cao::execution::MutationState::Committed);
    };
    adapters.reportProgress = [](const auto&) { throw std::runtime_error("progress observer"); };
    adapters.reportDiagnostics = [](const auto&) { throw std::runtime_error("diagnostics observer"); };
    bool finalized = false;
    adapters.finalizeArchiveLifecycleWithResult = [&] {
        finalized = true;
        return cao::run::ArchiveFinalizationResult{};
    };
    const auto record = AssetRun(archiveAndTexturePolicy()).execute(std::array{root}, adapters).workRecord();
    QCOMPARE(record.assetAttempts.size(), std::size_t{1});
    QCOMPARE(record.assetAttempts.front().result.mutationState(), cao::execution::MutationState::Committed);
    QCOMPARE(record.diagnostics.size(), std::size_t{2});
    for (const auto& diagnostic : record.diagnostics)
        QCOMPARE(diagnostic.code(), cao::run::RunDiagnosticCode::ObserverFailed);
    QVERIFY(record.failures.empty());
    QVERIFY(finalized);
}

void AssetRunTests::relativeSelectionRetainsMutationScope() {
    QTemporaryDir directory(QDir::currentPath() + "/asset-scope-XXXXXX");
    QVERIFY(directory.isValid());
    const auto root = std::filesystem::canonical(
        std::filesystem::path(directory.path().toStdWString()));
    writeFile(root / "a.dds");
    AssetRunAdapters adapters;
    adapters.executeAssetWithResult = [](const auto& asset) {
        std::filesystem::remove(asset.executionPath());
        return cao::execution::AssetExecutionResult::success(cao::execution::MutationState::Committed);
    };
    const auto relative = std::filesystem::relative(root, std::filesystem::current_path());
    const auto record = AssetRun(archiveAndTexturePolicy()).execute(std::array{relative}, adapters).workRecord();
    QCOMPARE(record.assetAttempts.size(), std::size_t{1});
    QCOMPARE(record.assetAttempts.front().modRoot, root);
    QVERIFY(!std::filesystem::exists(root / "a.dds"));
}

void AssetRunTests::throwingDiagnosticsCancellationSkipsFinalization() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = std::filesystem::path(directory.path().toStdWString());
    writeFile(root / "a.dds");
    AssetRunAdapters adapters;
    bool cancelled = false;
    bool finalized = false;
    adapters.isCancelled = [&] { return cancelled; };
    adapters.executeAssetWithResult = [](const auto&) {
        return cao::execution::AssetExecutionResult::success(cao::execution::MutationState::Committed);
    };
    adapters.reportDiagnostics = [&](const auto&) {
        cancelled = true;
        throw std::runtime_error("diagnostics requested cancellation");
    };
    adapters.finalizeArchiveLifecycleWithResult = [&] {
        finalized = true;
        return cao::run::ArchiveFinalizationResult{};
    };
    const auto record = AssetRun(archiveAndTexturePolicy()).execute(std::array{root}, adapters).workRecord();
    QVERIFY(record.cancellationObserved);
    QVERIFY(!finalized);
    QVERIFY(record.finalizations.empty());
    QCOMPARE(record.assetAttempts.size(), std::size_t{1});
    QCOMPARE(record.assetAttempts.front().result.mutationState(), cao::execution::MutationState::Committed);
    QCOMPARE(record.diagnostics.size(), std::size_t{1});
    QCOMPARE(record.diagnostics.front().code(), cao::run::RunDiagnosticCode::ObserverFailed);
}

QTEST_MAIN(AssetRunTests)
#include "AssetRunTests.moc"
