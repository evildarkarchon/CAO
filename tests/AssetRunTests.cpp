#include "Run/AssetRun.h"
#include "Run/ArchiveFirstAssetDiscovery.h"

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
using cao::routing::RunRequest;
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
    const auto request = RunRequest::forWork(mode, work);
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
}

class AssetRunTests final : public QObject
{
    Q_OBJECT

private slots:
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
};

void AssetRunTests::archiveExtractionPrecedesDefinitiveRoutedExecution()
{
    QTemporaryDir temporaryDirectory;
    QVERIFY(temporaryDirectory.isValid());

    const auto root = std::filesystem::path(temporaryDirectory.path().toStdWString());
    const auto archive = root / "content.bsa";
    const auto looseTexture = root / "textures" / "loose.dds";
    const auto extractedTexture = root / "textures" / "extracted.dds";
    writeFile(archive);
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
    QCOMPARE(progress[0].phase, cao::routing::RunPhase::ArchiveExtraction);
    QCOMPARE(progress[0].completed, std::size_t{1});
    QCOMPARE(progress[0].total, std::size_t{1});
    QCOMPARE(progress[1].phase, cao::routing::RunPhase::LooseAssetProcessing);
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
    QCOMPARE(result.skippedAssetCount(SkipReason::DisabledPhase), std::size_t{1});
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
    for (const auto &path : paths)
        writeFile(path);

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
    for (const auto &path : paths)
        writeFile(path);

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

QTEST_MAIN(AssetRunTests)
#include "AssetRunTests.moc"
