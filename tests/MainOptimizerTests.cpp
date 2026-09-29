#include "MainOptimizer.h"
#include "BsaOptimizer.h"
#include "FilesystemOperations.h"
#include "AssetRouting/AssetRouter.h"
#include "Run/AssetInitializationCancelled.h"
#include "Run/RunEvidence.h"

#include <nifly/BasicTypes.hpp>
#include <nifly/NifFile.hpp>

#include <QProcess>
#include <QTemporaryDir>
#include <QTest>

#include <algorithm>
#include <array>
#include <chrono>
#include <filesystem>
#include <stop_token>
#include <stdexcept>
#include <string>
#include <thread>
#include <utility>
#include <vector>
#ifdef _WIN32
#include <Windows.h>
#endif

namespace
{
using cao::execution::AssetExecutionFailure;
using cao::routing::AssetRouter;
using cao::routing::ExecutionMode;
using cao::routing::ProfileCapabilities;
using cao::routing::ProfileCapability;
using cao::routing::RequestedWork;
using cao::routing::RoutedAsset;
using cao::routing::RoutingPolicy;
using cao::routing::RoutingPolicyRequest;

class ScopedCurrentDirectory final
{
public:
    /// Changes the process working directory and restores it when the test scope exits.
    explicit ScopedCurrentDirectory(const QString &path)
        : _previousPath(QDir::currentPath())
    {
        if (!QDir::setCurrent(path))
            throw std::runtime_error("Could not enter the isolated test working directory.");
    }

    /// Restores the process working directory even when a Qt test assertion returns early.
    ~ScopedCurrentDirectory()
    {
        static_cast<void>(QDir::setCurrent(_previousPath));
    }

    ScopedCurrentDirectory(const ScopedCurrentDirectory &) = delete;
    ScopedCurrentDirectory &operator=(const ScopedCurrentDirectory &) = delete;

private:
    QString _previousPath;
};

/// Routes one supported optimizer input through a policy that enables every quarantined load type.
RoutedAsset routeAsset(const std::filesystem::path &path,
                       const ExecutionMode mode = ExecutionMode::Apply)
{
    const auto policyResult = RoutingPolicy::compile(
        RoutingPolicyRequest::forWork(mode,
                            {RequestedWork::NativeTextureOptimization,
                             RequestedWork::ConvertibleTextureConversion,
                             RequestedWork::StandardMeshOptimization}),
        ProfileCapabilities::define(
            ".bsa",
            {ProfileCapability::NativeTextureOptimization,
             ProfileCapability::ConvertibleTextureConversion,
             ProfileCapability::StandardMeshOptimization,
             ProfileCapability::MeshReferenceMaintenance}));
    if (!policyResult.hasPolicy())
        throw std::runtime_error("Test Routing Policy unexpectedly failed to compile.");

    const AssetRouter router(*policyResult.policy());
    auto decision = router.route(path);
    if (!std::holds_alternative<RoutedAsset>(decision))
        throw std::runtime_error("Malformed test Asset unexpectedly failed to route.");
    return std::get<RoutedAsset>(std::move(decision));
}

/// Routes one input through a policy carrying conversion and its derived Mesh Reference
/// Maintenance, but no ordinary Mesh optimization, so only the dependent rewrite can save a Mesh.
RoutedAsset routeMaintenanceOnly(const std::filesystem::path &path)
{
    const auto policyResult = RoutingPolicy::compile(
        RoutingPolicyRequest::forWork(ExecutionMode::Apply,
                            {RequestedWork::ConvertibleTextureConversion}),
        ProfileCapabilities::define(".bsa",
                                    {ProfileCapability::ConvertibleTextureConversion,
                                     ProfileCapability::MeshReferenceMaintenance}));
    if (!policyResult.hasPolicy())
        throw std::runtime_error("Test maintenance-only Routing Policy failed to compile.");

    const AssetRouter router(*policyResult.policy());
    auto decision = router.route(path);
    if (!std::holds_alternative<RoutedAsset>(decision))
        throw std::runtime_error("Maintenance-only test Asset unexpectedly failed to route.");
    return std::get<RoutedAsset>(std::move(decision));
}

/// Writes a minimal SSE Mesh whose single Texture slot references the supplied name.
void writeMeshWithTexture(const std::filesystem::path &path, const std::string &texturePath)
{
    QVERIFY(QDir().mkpath(QString::fromStdWString(path.parent_path().wstring())));

    nifly::NifFile mesh;
    mesh.Create(nifly::NiVersion::getSSE());
    std::vector<nifly::Vector3> vertices{{0.0F, 0.0F, 0.0F},
                                         {1.0F, 0.0F, 0.0F},
                                         {0.0F, 1.0F, 0.0F}};
    std::vector<nifly::Triangle> triangles{{0, 1, 2}};
    auto *shape = mesh.CreateShapeFromData("TestShape", &vertices, &triangles, nullptr);
    auto mutableTexturePath = texturePath;
    mesh.SetTextureSlot(shape, mutableTexturePath);
    QCOMPARE(mesh.Save(path.u16string()), 0);
}

/// Reads the first Texture slot back from a saved Mesh.
std::string savedTextureSlot(const std::filesystem::path &path)
{
    nifly::NifFile mesh;
    if (mesh.Load(path.u16string()) != 0)
        throw std::runtime_error("The saved test Mesh could not be reloaded.");

    std::string texturePath;
    mesh.GetTextureSlot(mesh.GetShapes().front(), texturePath);
    return texturePath;
}

/// Writes one test fixture after creating its parent directory.
void writeFile(const std::filesystem::path &path, const QByteArray &contents)
{
    QVERIFY(QDir().mkpath(QString::fromStdWString(path.parent_path().wstring())));
    QFile file(QString::fromStdWString(path.wstring()));
    QVERIFY(file.open(QIODevice::WriteOnly));
    QCOMPARE(file.write(contents), contents.size());
}

/// Copies a selected game's canonical Dummy Plugin bytes for independent filesystem fixtures.
QByteArray canonicalDummyBytes(const btu::Game game) {
    const auto& bytes = *btu::bsa::Settings::get(game).s_dummy_plugin;
    return QByteArray(reinterpret_cast<const char*>(bytes.data()), static_cast<int>(bytes.size()));
}
}

class MainOptimizerTests final : public QObject
{
    Q_OBJECT

private slots:
 /// Exercises phase-wide shortage, unknown capacity, and a race between output attempts.
 void finalizationCapacityChecks_data();
 /// Ensures capacity failures preserve unattempted sources and prevent directory pruning.
 void finalizationCapacityChecks();
 /// Allows independent volumes to finalize when each fits its own output estimate.
 void finalizationCapacityIsGroupedByVolume();
 /// An idle Mod Root with unknown identity must not inflate other volumes' output estimates.
 void idleUnknownVolumeDoesNotInflateCapacity();
 /// Rechecks only the remaining dummy-plugin reserve after each same-volume root completes.
 void dummyCapacityDecreasesAfterEachRoot();
 /// Reports plugin-only capacity failure without inventing output progress or pruning folders.
 void finalizationCapacityWithoutOutputs();
 /// Keeps the public output aggregate's positional capacity initializer source-compatible.
 void finalizationOutputRetainsPositionalCapacity();
 /// Stops finalization planning before traversing a cancelled Mod Root.
 void finalizationPlanningObservesCancellation();
 /// Keeps a completed output and reports a contained guarded plugin removal failure.
 void finalizationReportsGuardedPluginRemovalFailure();
 /// Stops plugin cleanup when cancellation arrives between Mod Roots.
 void finalizationCancelsPluginCleanupBetweenRoots();
 /// Retains loading-plugin changes for existing Archives even with zero planned outputs.
 void finalizationRetainsPluginMutations_data();
 /// Derives plugin-only mutation evidence without inventing output attempts.
 void finalizationRetainsPluginMutations();
 /// Exercises exact-byte cleanup and preservation through Archive Finalization on Windows profiles.
 void finalizationRemovesOnlyExactDummyPlugins_data();
 /// Records only the removed canonical Dummy Plugin, including one written outside CAO.
 void finalizationRemovesOnlyExactDummyPlugins();
 /// Covers native link guards for an exact-byte plugin during Windows cleanup.
 void finalizationRejectsLinkedDummyPlugins_data();
 /// Preserves both a linked entry and its canonical-byte target without reporting mutation.
 void finalizationRejectsLinkedDummyPlugins();
 /// Rejects a Mod Root replaced by a junction after the finalization plan is frozen.
 void finalizationRejectsChangedDummyPluginParent();
 /// Retains a different-content replacement introduced after planning and a completed output.
 void finalizationPreservesReplacedDummyPlugin();
 /// Retains earlier root removals when a later native guard fails before deletion.
 void finalizationRetainsRemovalPrefixOnGuardFailure();
 /// Reports an occupied existing-Archive plugin destination without changing it or output progress.
 void existingArchivePluginCollisionFailsSafely();
 /// Uses selected-profile Archive suffix and plugin extension rules for existing Archives.
 void existingArchivePluginNamesFollowProfile_data();
 /// Publishes the profile's canonical bytes at its suffix-free Loading Plugin name.
 void existingArchivePluginNamesFollowProfile();
 /// Covers shared creation, an existing full Loading Plugin, and an exact preexisting Dummy Plugin.
 void existingArchivesShareLoadingPlugin_data();
 /// Preserves an existing Loading Plugin identity while recording only new publications.
 void existingArchivesShareLoadingPlugin();
 /// Exercises backup and delete source choices after successful and failed extraction.
 void archiveSourceCleanupRequiresSuccessfulMerge_data();
 /// Preserves original Archive bytes on failure and never replaces an existing backup.
 void archiveSourceCleanupRequiresSuccessfulMerge();
 /// Rejects a source link so cleanup cannot act on a different directory entry than the reader.
 void archiveSourceLinkIsNotCleaned_data();
 /// Keeps the source link and its target when either cleanup policy is requested.
 void archiveSourceLinkIsNotCleaned();
 /// Checks both cleanup operations after the extracted source name is replaced.
 void extractedSourceReplacementIsNotCleaned_data();
 /// Retains a new Archive at the original path and the earlier extracted source.
 void extractedSourceReplacementIsNotCleaned();

 /// Keeps temporary Texture bytes out of archives and their packed-source deletion pass.
 void packingPreservesStagingFiles();

 /// Retains every Loose Asset when an output cannot include all of its planned sources.
 void failedPackingRetainsSourcesAndExistingArchives();

 /// Keeps a replacement after the packed file pin is released for guarded cleanup.
 void packedSourcePinPreservesReplacement();

 /// Rejects a planned source when a parent is swapped for a junction before packing.
 void packedSourceParentJunctionIsRejected();

 /// Covers cancellation before work, between outputs, and after the final output.
 void finalizationFreezesTotalAndCancelsBetweenOutputs_data();
 /// Observes a complete multi-root plan before mutation and commits only attempted outputs.
 void finalizationFreezesTotalAndCancelsBetweenOutputs();

 /// Reserves distinct names without filesystem placeholders and rejects a later occupied output.
 void plannedNamesAreDistinctAndCommitPreservesNewDestination();

 /// Uses a same-size Loading Plugin for output naming while ignoring an exact Dummy Plugin.
 void plannedOutputNamesUseExactDummyBytes_data();
 /// Checks selected Windows profiles with suffix-free and suffixed Archive names.
 void plannedOutputNamesUseExactDummyBytes();

 /// Covers recognized Loading Plugin names that appear after planning on Windows profiles.
 void plannedOutputRechecksLoadingPluginNames_data();
 /// Reuses the newly recognized plugin without publishing a Dummy Plugin.
 void plannedOutputRechecksLoadingPluginNames();

 /// Keeps a published Archive and its source when a competing plugin occupies the planned name.
 void plannedPluginCollisionRetainsCommittedArchive();

 /// Reuses an exact dummy created after planning without replacing its filesystem entry.
 void plannedExactDummyIsReused();

 /// Publishes one shared Dummy Plugin as a separate effect of two planned Archives.
 void plannedOutputsRecordSharedDummyPublication();

 /// Publishes a Dummy Plugin if the Loading Plugin observed during planning disappears.
 void plannedLoadingPluginDisappearsBeforeCommit();

 /// Checks capacity before either mutation boundary when a planned Loading Plugin disappears.
 void disappearingLoadingPluginCapacityIsReserved_data();
 /// Rejects Archive-only capacity when finalization may need a fallback Dummy Plugin.
 void disappearingLoadingPluginCapacityIsReserved();

 /// Covers a linked Loading Plugin at the chosen or an alternate recognized name.
 void linkedLoadingPluginRemainsUsable_data();
 /// Publishes a Windows fallback so retargeting the linked plugin cannot unload the Archive.
 void linkedLoadingPluginRemainsUsable();

 /// Retains Loose Assets when every recognized Loading Plugin name is occupied by a link.
 void linkedLoadingPluginsWithoutFallbackRetainSources();

 /// Exercises existing and published Loading Plugins during source cleanup.
 void loadingPluginRemainsPinnedThroughSourceCleanup_data();
 /// Prevents deletion of the selected Loading Plugin until all packed sources are removed.
 void loadingPluginRemainsPinnedThroughSourceCleanup();

 /// Exercises retained source recovery with readable and byte-locked files.
 void committedArchiveRetainsLockedSource_data();
 /// Continues later outputs only when committed Archive and retained source bytes are usable.
 void committedArchiveRetainsLockedSource();

 /// Keeps a committed output loadable when cancellation leaves later outputs unattempted.
 void cancellationPreservesCommittedArchiveLoadingPlugin();

 /// Leaves staging ownership directories to their owner while pruning ordinary empty paths.
 void emptyDirectoryCleanupPreservesStaging();

 /// Rejects ambiguous staging at the selected Mod Root even for deeply nested Textures.
 void nestedTextureUsesSelectedModRoot();

 /// Verifies malformed DDS, TGA, and NIF inputs cannot remain eligible for later Archive packing.
 void loadFailuresQuarantineMalformedAssets();

 /// Verifies a stale quarantine file cannot leave a newly extracted malformed Asset packable.
 void loadFailureUsesCollisionSafeQuarantineName();

 /// Stops later mutation when a malformed Asset remains packable after quarantine fails.
 void failedQuarantineMakesLoadFailureUnsafe();

 /// Verifies reporting a malformed input never mutates a Dry Run tree.
 void dryRunLoadFailureDoesNotQuarantine();

 /// Verifies a failed Texture conversion withholds the rewrite of references to that Texture.
 void failedConversionSuppressesMeshReferenceMaintenance();

 /// Verifies a committed DDS still enables dependent rewrites when the TGA cannot be removed.
 void committedConversionWithRetainedSourceMaintainsMeshReferences();

 /// Verifies one failed conversion does not withhold references to Textures that converted.
 void failedConversionKeepsUnrelatedMeshReferences();

 /// Verifies a failure in one Mod Root cannot withhold the same reference in a sibling root.
 void failedConversionInAnotherModRootKeepsMeshReferences();

 /// Verifies the referenced-TGA rewrite still applies when no conversion failed.
 void successfulRunStillMaintainsMeshReferences();

 /// Stops a recursive plugin listing before traversing more entries.
 void pluginListingObservesCancellation();

 /// Aborts lazy backend initialization when the run has been cancelled.
 void optimizerInitializationObservesCancellation();

private:
    QTemporaryDir _temporaryDirectory;
};

void MainOptimizerTests::archiveSourceCleanupRequiresSuccessfulMerge_data() {
    QTest::addColumn<bool>("validArchive");
    QTest::addColumn<bool>("deleteBackup");
    QTest::addColumn<bool>("blockSourceCleanup");
    QTest::addColumn<bool>("linkedBackup");
    QTest::newRow("failed-backup") << false << false << false << false;
    QTest::newRow("failed-delete") << false << true << false << false;
    QTest::newRow("successful-backup") << true << false << false << false;
    QTest::newRow("successful-delete") << true << true << false << false;
    QTest::newRow("dangling-backup") << true << false << false << true;
#ifdef _WIN32
    QTest::newRow("blocked-backup") << true << false << true << false;
    QTest::newRow("blocked-delete") << true << true << true << false;
#endif
}

void MainOptimizerTests::archiveSourceCleanupRequiresSuccessfulMerge() {
    QFETCH(bool, validArchive);
    QFETCH(bool, deleteBackup);
    QFETCH(bool, blockSourceCleanup);
    QFETCH(bool, linkedBackup);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const ScopedCurrentDirectory isolatedWorkingDirectory(directory.path());
    const auto root = std::filesystem::path(directory.path().toStdWString());
    const auto mod = root / "mod";
    const auto source = mod / "assets.bsa";
    const auto backup = mod / "assets.bsa.bak";
    writeFile(backup, QByteArrayLiteral("existing backup bytes"));
    const auto occupiedBackup = mod / "assets.bsa.bak.bak";
    if (linkedBackup) {
        std::error_code error;
        std::filesystem::create_symlink(mod / "absent", occupiedBackup, error);
        QVERIFY2(!error, error.message().c_str());
    }
    if (validArchive) {
        const auto fixture = root / "input" / "fixture.dds";
        writeFile(fixture, QByteArrayLiteral("archived bytes"));
        auto archive = btu::bsa::ArchiveData(btu::bsa::Settings::get(btu::Game::SSE),
                                            btu::bsa::ArchiveType::Textures);
        QVERIFY(archive.add_file(fixture));
        archive.set_out_path(source);
        QVERIFY(btu::bsa::write(false, std::move(archive), root / "input").empty());
    } else {
        writeFile(source, QByteArrayLiteral("corrupt archive bytes"));
    }
    QFile original(QString::fromStdWString(source.wstring()));
    QVERIFY(original.open(QIODevice::ReadOnly));
    const auto originalBytes = original.readAll();
    original.close();
#ifdef _WIN32
    // Permit extraction reads while denying rename/delete so the failure occurs after merge.
    const auto lock = blockSourceCleanup
                          ? CreateFileW(source.c_str(), GENERIC_READ, FILE_SHARE_READ, nullptr,
                                        OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr)
                          : INVALID_HANDLE_VALUE;
    QVERIFY(!blockSourceCleanup || lock != INVALID_HANDLE_VALUE);
#endif
    cao::run::TemporaryArtifactRegistry artifacts;
    const auto result = BSAOptimizer().extract(
        {source, mod, {"fixture.dds"}, {"fixture.dds"}}, deleteBackup, artifacts);
#ifdef _WIN32
    if (lock != INVALID_HANDLE_VALUE) CloseHandle(lock);
#endif
    QCOMPARE(result.succeeded(), validArchive && !blockSourceCleanup);
    QCOMPARE(std::filesystem::exists(source), !validArchive || blockSourceCleanup);
    QCOMPARE(std::filesystem::exists(mod / "fixture.dds"), validArchive);
    if (blockSourceCleanup) {
        QVERIFY(result.failure == cao::run::ArchiveExtractionFailure::SourceCleanupFailed);
        QCOMPARE(result.mutation, cao::execution::MutationState::Committed);
        QVERIFY(result.safeToContinue);
    }
    QFile existingBackup(QString::fromStdWString(backup.wstring()));
    QVERIFY(existingBackup.open(QIODevice::ReadOnly));
    QCOMPARE(existingBackup.readAll(), QByteArrayLiteral("existing backup bytes"));
    if (!validArchive || !deleteBackup || blockSourceCleanup) {
        const auto retained = validArchive && !blockSourceCleanup
                                  ? mod / (linkedBackup ? "assets.bsa.bak.bak.bak" : "assets.bsa.bak.bak")
                                  : source;
        QFile retainedArchive(QString::fromStdWString(retained.wstring()));
        QVERIFY(retainedArchive.open(QIODevice::ReadOnly));
        QCOMPARE(retainedArchive.readAll(), originalBytes);
    }
    if (linkedBackup)
        QCOMPARE(std::filesystem::read_symlink(occupiedBackup), mod / "absent");
    QVERIFY(artifacts.performSafetyCleanup().empty());
}

void MainOptimizerTests::archiveSourceLinkIsNotCleaned_data() {
    QTest::addColumn<bool>("deleteBackup");
    QTest::newRow("backup") << false;
    QTest::newRow("delete") << true;
}

void MainOptimizerTests::archiveSourceLinkIsNotCleaned() {
#ifdef _WIN32
    QFETCH(bool, deleteBackup);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const ScopedCurrentDirectory isolatedWorkingDirectory(directory.path());
    const auto root = std::filesystem::path(directory.path().toStdWString());
    const auto mod = root / "mod";
    const auto target = mod / "original.bsa";
    const auto source = mod / "assets.bsa";
    const auto fixture = root / "input" / "fixture.dds";
    QVERIFY(std::filesystem::create_directories(mod));
    writeFile(fixture, QByteArrayLiteral("archived bytes"));
    auto archive = btu::bsa::ArchiveData(btu::bsa::Settings::get(btu::Game::SSE),
                                        btu::bsa::ArchiveType::Textures);
    QVERIFY(archive.add_file(fixture));
    archive.set_out_path(target);
    QVERIFY(btu::bsa::write(false, std::move(archive), root / "input").empty());
    std::error_code linkError;
    std::filesystem::create_symlink(target, source, linkError);
    QVERIFY2(!linkError, linkError.message().c_str());

    cao::run::TemporaryArtifactRegistry artifacts;
    const auto result = BSAOptimizer().extract(
        {source, mod, {"fixture.dds"}, {"fixture.dds"}}, deleteBackup, artifacts);
    QVERIFY(result.failure.has_value());
    QVERIFY(std::filesystem::is_symlink(std::filesystem::symlink_status(source)));
    QVERIFY(std::filesystem::exists(target));
    QVERIFY(!std::filesystem::exists(mod / "fixture.dds"));
    QVERIFY(artifacts.performSafetyCleanup().empty());
#else
    QSKIP("Windows source pins reject reparse points before archive extraction.");
#endif
}

void MainOptimizerTests::extractedSourceReplacementIsNotCleaned_data() {
    QTest::addColumn<bool>("deleteBackup");
    QTest::newRow("backup") << false;
    QTest::newRow("delete") << true;
}

void MainOptimizerTests::extractedSourceReplacementIsNotCleaned() {
#ifdef _WIN32
    QFETCH(bool, deleteBackup);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const ScopedCurrentDirectory isolatedWorkingDirectory(directory.path());
    const auto root = std::filesystem::path(directory.path().toStdWString());
    const auto mod = root / "mod";
    const auto source = mod / "assets.bsa";
    const auto displaced = mod / "earlier.bsa";
    const auto fixture = root / "input" / "fixture.dds";
    writeFile(fixture, QByteArrayLiteral("archived bytes"));
    QVERIFY(std::filesystem::create_directories(mod));
    auto archive = btu::bsa::ArchiveData(btu::bsa::Settings::get(btu::Game::SSE),
                                        btu::bsa::ArchiveType::Textures);
    QVERIFY(archive.add_file(fixture));
    archive.set_out_path(source);
    QVERIFY(btu::bsa::write(false, std::move(archive), root / "input").empty());

    cao::run::SourceFilePin pin(source, mod);
    cao::run::TemporaryArtifactRegistry artifacts;
    const auto result = cao::run::ArchiveExtractor(artifacts).extract(
        {source, mod, {"fixture.dds"}, {"fixture.dds"}});
    QVERIFY(result.succeeded());
    pin.releaseForCleanup();
    QVERIFY(MoveFileExW(source.c_str(), displaced.c_str(), 0));
    writeFile(fixture, QByteArrayLiteral("replacement bytes"));
    auto replacement = btu::bsa::ArchiveData(btu::bsa::Settings::get(btu::Game::SSE),
                                            btu::bsa::ArchiveType::Textures);
    QVERIFY(replacement.add_file(fixture));
    replacement.set_out_path(source);
    QVERIFY(btu::bsa::write(false, std::move(replacement), root / "input").empty());

    if (deleteBackup)
        QVERIFY_EXCEPTION_THROWN(pin.removeIfUnchanged(), std::runtime_error);
    else
        QVERIFY_EXCEPTION_THROWN(pin.backupIfUnchanged(), std::runtime_error);
    QVERIFY_EXCEPTION_THROWN(pin.pinUnchangedForRecovery(), std::runtime_error);
    QVERIFY(std::filesystem::exists(source));
    QVERIFY(std::filesystem::exists(displaced));
    QVERIFY(!std::filesystem::exists(mod / "assets.bsa.bak"));
    QFile extracted(QString::fromStdWString((mod / "fixture.dds").wstring()));
    QVERIFY(extracted.open(QIODevice::ReadOnly));
    QCOMPARE(extracted.readAll(), QByteArrayLiteral("archived bytes"));
    QVERIFY(artifacts.performSafetyCleanup().empty());
#else
    QSKIP("Windows file handles provide the extracted-source identity guard.");
#endif
}

void MainOptimizerTests::packingPreservesStagingFiles() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const ScopedCurrentDirectory isolatedWorkingDirectory(directory.path());
    const auto root = std::filesystem::path(directory.path().toStdWString());
    writeFile(root / "profiles" / "SSE" / "profile.ini",
              QByteArrayLiteral("[BSA]\nbsaEnabled=true\nbsaGame=4\n"));
    Profiles::setCurrentProfile("SSE");
    const auto mod = root / "mod";
    QVERIFY(std::filesystem::create_directories(mod));
    cao::run::TemporaryArtifactRegistry artifacts;
    const auto staged = artifacts.stageArchiveFile(mod).path;
    const auto nestedStaged = mod / "textures" / ".cao-staging-old" / "pending.dds";
    writeFile(staged, QByteArrayLiteral("temporary bytes"));
    writeFile(nestedStaged, QByteArrayLiteral("unverified temporary bytes"));
    writeFile(mod / "textures" / "complete.dds", QByteArrayLiteral("committed bytes"));
    OptionsCAO options;
    options.bBsaCreateDummies = false;
    options.bBsaCompress = false;
    options.bBsaDeleteSource = true;

    const BSAOptimizer optimizer;
    const std::array roots{mod};
    const auto plan = optimizer.planFinalization(roots, options);
    const auto result = optimizer.finalize(plan, artifacts);
    QCOMPARE(result.attempts.size(), std::size_t{1});
    QVERIFY(result.attempts.front().succeeded());

    QVERIFY(std::filesystem::exists(staged));
    QVERIFY(std::filesystem::exists(nestedStaged));
    QVERIFY(!std::filesystem::exists(mod / "textures" / "complete.dds"));
    QVERIFY(
        !QDir(QString::fromStdWString(mod.wstring())).entryList({"*.bsa"}, QDir::Files).isEmpty());
    QVERIFY(artifacts.performSafetyCleanup().empty());
}

void MainOptimizerTests::failedPackingRetainsSourcesAndExistingArchives() {
#ifdef _WIN32
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const ScopedCurrentDirectory isolatedWorkingDirectory(directory.path());
    const auto root = std::filesystem::path(directory.path().toStdWString());
    writeFile(root / "profiles" / "SSE" / "profile.ini",
              QByteArrayLiteral("[BSA]\nbsaEnabled=true\nbsaGame=4\n"));
    Profiles::setCurrentProfile("SSE");
    const auto mod = root / "mod";
    const auto available = mod / "textures" / "available.dds";
    const auto unavailable = mod / "textures" / "unavailable.dds";
    const auto existingArchive = mod / "mod.bsa";
    writeFile(available, QByteArrayLiteral("available source bytes"));
    writeFile(unavailable, QByteArrayLiteral("unavailable source bytes"));
    writeFile(existingArchive, QByteArrayLiteral("previous archive bytes"));
    // Deny reads while allowing rename/delete, so a partial writer result cannot be mistaken
    // for a successful output or hidden by an unrelated quarantine sharing violation.
    const auto locked = CreateFileW(unavailable.c_str(), GENERIC_WRITE,
                                   FILE_SHARE_WRITE | FILE_SHARE_DELETE, nullptr,
                                   OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    QVERIFY(locked != INVALID_HANDLE_VALUE);
    OptionsCAO options;
    options.bBsaCreateDummies = false;
    options.bBsaCompress = false;
    options.bBsaDeleteSource = true;
    BSAOptimizer().packAll(QString::fromStdWString(mod.wstring()), options);
    QVERIFY(CloseHandle(locked));

    QVERIFY(std::filesystem::exists(available));
    QVERIFY(std::filesystem::exists(unavailable));
    QFile archive(QString::fromStdWString(existingArchive.wstring()));
    QVERIFY(archive.open(QIODevice::ReadOnly));
    QCOMPARE(archive.readAll(), QByteArrayLiteral("previous archive bytes"));
#else
    QSKIP("Windows sharing modes provide a deterministic source read failure.");
#endif
}

void MainOptimizerTests::packedSourcePinPreservesReplacement() {
#ifdef _WIN32
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = std::filesystem::path(directory.path().toStdWString());
    const auto source = root / "textures" / "asset.dds";
    const auto displaced = root / "textures" / "displaced.dds";
    writeFile(source, QByteArrayLiteral("original source bytes"));

    cao::run::SourceFilePin pin(source, root);
    // A writer cannot replace the file while the archive reader's pin is open.
    QVERIFY(!MoveFileExW(source.c_str(), displaced.c_str(), 0));
    const auto movedParent = root / "moved-textures";
    QVERIFY(!MoveFileExW(source.parent_path().c_str(), movedParent.c_str(), 0));
    pin.releaseForCleanup();
    // The cleanup gap releases only the file, not the path leading to that file.
    QVERIFY(!MoveFileExW(source.parent_path().c_str(), movedParent.c_str(), 0));
    QVERIFY(MoveFileExW(source.c_str(), displaced.c_str(), 0));
    writeFile(source, QByteArrayLiteral("replacement source bytes"));
    QVERIFY_EXCEPTION_THROWN(pin.removeIfUnchanged(), std::runtime_error);

    QFile retained(QString::fromStdWString(source.wstring()));
    QVERIFY(retained.open(QIODevice::ReadOnly));
    QCOMPARE(retained.readAll(), QByteArrayLiteral("replacement source bytes"));
    QFile old(QString::fromStdWString(displaced.wstring()));
    QVERIFY(old.open(QIODevice::ReadOnly));
    QCOMPARE(old.readAll(), QByteArrayLiteral("original source bytes"));
#else
    QSKIP("Windows file handles provide the packed-source identity guard.");
#endif
}

void MainOptimizerTests::packedSourceParentJunctionIsRejected() {
#ifdef _WIN32
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const ScopedCurrentDirectory isolatedWorkingDirectory(directory.path());
    const auto base = std::filesystem::path(directory.path().toStdWString());
    writeFile(base / "profiles" / "SSE" / "profile.ini",
              QByteArrayLiteral("[BSA]\nbsaEnabled=true\nbsaGame=4\n"));
    Profiles::setCurrentProfile("SSE");
    const auto mod = base / "mod";
    const auto sourceParent = mod / "textures";
    const auto source = sourceParent / "armor" / "asset.dds";
    const auto outside = base / "outside";
    const auto outsideSource = outside / "armor" / "asset.dds";
    writeFile(source, QByteArrayLiteral("planned source bytes"));
    writeFile(outsideSource, QByteArrayLiteral("outside source bytes"));
    writeFile(outside / "marker.txt", QByteArrayLiteral("keep target directory"));
    OptionsCAO options;
    options.bBsaCreateDummies = false;
    options.bBsaCompress = false;
    options.bBsaDeleteSource = true;
    const BSAOptimizer optimizer;
    const std::array roots{mod};
    const auto plan = optimizer.planFinalization(roots, options);
    QCOMPARE(plan.outputs().size(), std::size_t{1});
    const auto retainedParent = mod / "retained-textures";
    std::filesystem::rename(sourceParent, retainedParent);
    const auto quotedPath = [](const std::filesystem::path& path) {
        auto value = QString::fromStdWString(path.wstring());
        value.replace("'", "''");
        return "'" + value + "'";
    };
    QProcess process;
    process.start("powershell.exe", {"-NoProfile", "-NonInteractive", "-Command",
        "New-Item -ItemType Junction -Path " + quotedPath(sourceParent) + " -Value " +
            quotedPath(outside) + " -ErrorAction Stop | Out-Null"});
    QVERIFY(process.waitForFinished());
    QCOMPARE(process.exitCode(), 0);

    cao::run::TemporaryArtifactRegistry artifacts;
    const auto result = optimizer.finalize(plan, artifacts);
    std::error_code cleanupError;
    std::filesystem::remove(sourceParent, cleanupError);
    QVERIFY2(!cleanupError, cleanupError.message().c_str());
    QCOMPARE(result.attempts.size(), std::size_t{1});
    QCOMPARE(result.attempts.front().failure,
             std::optional{cao::run::ArchiveFinalizationFailure::WriteFailed});
    QCOMPARE(result.attempts.front().mutation, cao::execution::MutationState::None);
    QVERIFY(!std::filesystem::exists(plan.outputs().front().archivePath));
    QVERIFY(std::filesystem::exists(retainedParent / "armor" / "asset.dds"));
    QFile outsideFile(QString::fromStdWString(outsideSource.wstring()));
    QVERIFY(outsideFile.open(QIODevice::ReadOnly));
    QCOMPARE(outsideFile.readAll(), QByteArrayLiteral("outside source bytes"));
    QVERIFY(artifacts.performSafetyCleanup().empty());
#else
    QSKIP("Windows junctions provide the parent substitution regression case.");
#endif
}

void MainOptimizerTests::finalizationPlanningObservesCancellation() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const ScopedCurrentDirectory isolatedWorkingDirectory(directory.path());
    const auto root = std::filesystem::path(directory.path().toStdWString());
    writeFile(root / "profiles" / "SSE" / "profile.ini",
              QByteArrayLiteral("[BSA]\nbsaEnabled=true\nbsaGame=4\n"));
    Profiles::setCurrentProfile("SSE");
    const auto mod = root / "mod";
    const auto source = mod / "textures" / "asset.dds";
    writeFile(source, QByteArrayLiteral("source bytes"));
    OptionsCAO options;
    std::stop_source stop;
    stop.request_stop();
    bool cancelled = false;
    try {
        const std::array roots{mod};
        static_cast<void>(BSAOptimizer().planFinalization(roots, options, stop.get_token()));
    } catch (const cao::run::ArchiveFinalizationPlanningCancelled&) {
        cancelled = true;
    }
    QVERIFY(cancelled);
    QVERIFY(std::filesystem::exists(source));
    QVERIFY(!std::filesystem::exists(mod / ".cao-staging"));
}

void MainOptimizerTests::finalizationReportsGuardedPluginRemovalFailure() {
#ifdef _WIN32
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const ScopedCurrentDirectory isolatedWorkingDirectory(directory.path());
    const auto parent = std::filesystem::path(directory.path().toStdWString());
    writeFile(parent / "profiles" / "SSE" / "profile.ini",
              QByteArrayLiteral("[BSA]\nbsaEnabled=true\nbsaGame=4\n"));
    Profiles::setCurrentProfile("SSE");
    const auto mod = parent / "mod";
    writeFile(mod / "textures" / "asset.dds", QByteArrayLiteral("source bytes"));
    const auto plugin = mod / "existing.esp";
    writeFile(plugin, canonicalDummyBytes(btu::Game::SSE));
    OptionsCAO options;
    options.bBsaCreateDummies = false;
    options.bBsaCompress = false;
    options.bBsaDeleteSource = false;
    const std::array roots{mod};
    const BSAOptimizer optimizer;
    const auto plan = optimizer.planFinalization(roots, options);
    QCOMPARE(plan.outputs().size(), std::size_t{1});
    cao::run::TemporaryArtifactRegistry artifacts;
    HANDLE heldPlugin = INVALID_HANDLE_VALUE;
    const auto result = optimizer.finalize(
        plan, artifacts, {}, {}, cao::run::availableArchiveCapacity,
        [&](const cao::run::ArchiveFinalizationAttempt&) {
            // The root remains pinned through work; block only the dummy plugin's deletion.
            heldPlugin = CreateFileW(plugin.c_str(), GENERIC_READ, FILE_SHARE_READ, nullptr,
                                     OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
        });
    QVERIFY(heldPlugin != INVALID_HANDLE_VALUE);
    QVERIFY(CloseHandle(heldPlugin));
    QCOMPARE(result.attempts.size(), std::size_t{1});
    QVERIFY(result.attempts.front().succeeded());
    QVERIFY(result.safeToContinue);
    QCOMPARE(result.failure,
             std::optional{cao::run::ArchiveFinalizationFailure::PluginRemovalFailed});
    QVERIFY(result.mutations.empty());
    QVERIFY(!result.detail.empty());
    QVERIFY(std::filesystem::exists(plan.outputs().front().archivePath));
    QVERIFY(std::filesystem::exists(plugin));
    QVERIFY(artifacts.performSafetyCleanup().empty());
#else
    QSKIP("Windows file sharing modes provide a deterministic post-output failure.");
#endif
}

void MainOptimizerTests::finalizationCancelsPluginCleanupBetweenRoots() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const ScopedCurrentDirectory isolatedWorkingDirectory(directory.path());
    const auto parent = std::filesystem::path(directory.path().toStdWString());
    writeFile(parent / "profiles" / "SSE" / "profile.ini",
              QByteArrayLiteral("[BSA]\nbsaEnabled=true\nbsaGame=4\n"));
    Profiles::setCurrentProfile("SSE");
    const std::array roots{parent / "mod-a", parent / "mod-b"};
    for (const auto& root : roots) {
        writeFile(root / "existing.bsa", QByteArrayLiteral("retained archive"));
        std::filesystem::create_directories(root / "empty" / "nested");
    }
    OptionsCAO options;
    options.bBsaCreateDummies = true;
    const BSAOptimizer optimizer;
    const auto plan = optimizer.planFinalization(roots, options);
    QVERIFY(plan.outputs().empty());
    const auto firstPlugin = roots.front() / "existing.esp";
    const auto secondPlugin = roots.back() / "existing.esp";
    std::stop_source stop;
    const auto capacity = [&](const std::filesystem::path& root)
        -> std::optional<std::uintmax_t> {
        if (root == std::filesystem::canonical(roots.back()) &&
            std::filesystem::exists(firstPlugin))
            stop.request_stop();
        return std::numeric_limits<std::uintmax_t>::max();
    };
    cao::run::TemporaryArtifactRegistry artifacts;
    const auto result = optimizer.finalize(plan, artifacts, stop.get_token(), {}, capacity);
    QVERIFY(result.cancelled);
    QVERIFY(result.safeToContinue);
    QVERIFY(std::filesystem::exists(firstPlugin));
    QVERIFY(!std::filesystem::exists(secondPlugin));
    for (const auto& root : roots)
        QVERIFY(std::filesystem::exists(root / "empty" / "nested"));
    QVERIFY(artifacts.performSafetyCleanup().empty());
}

void MainOptimizerTests::finalizationRetainsPluginMutations_data() {
    QTest::addColumn<bool>("createDummies");
    QTest::newRow("create-for-existing-archive") << true;
    QTest::newRow("remove-existing-dummy") << false;
}

void MainOptimizerTests::finalizationRetainsPluginMutations() {
    QFETCH(bool, createDummies);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const ScopedCurrentDirectory isolatedWorkingDirectory(directory.path());
    const auto parent = std::filesystem::path(directory.path().toStdWString());
    writeFile(parent / "profiles" / "SSE" / "profile.ini",
              QByteArrayLiteral("[BSA]\nbsaEnabled=true\nbsaGame=4\n"));
    Profiles::setCurrentProfile("SSE");
    const auto mod = parent / "mod";
    writeFile(mod / "existing.bsa", QByteArrayLiteral("retained archive"));
    const auto plugin = mod / "existing.esp";
    if (!createDummies) writeFile(plugin, canonicalDummyBytes(btu::Game::SSE));
    OptionsCAO options;
    options.bBsaCreateDummies = createDummies;
    const std::array roots{mod};
    const BSAOptimizer optimizer;
    const auto plan = optimizer.planFinalization(roots, options);
    QVERIFY(plan.outputs().empty());
    cao::run::TemporaryArtifactRegistry artifacts;
    const auto result = optimizer.finalize(plan, artifacts);
    QVERIFY(result.attempts.empty());
    QVERIFY(!result.failure);
    QVERIFY(result.safeToContinue);
    QCOMPARE(result.mutations.size(), std::size_t{1});
    const auto& mutation = result.mutations.front();
    QCOMPARE(mutation.modRoot, std::filesystem::canonical(mod));
    QCOMPARE(mutation.path, plugin);
    QCOMPARE(mutation.kind, createDummies
             ? cao::run::ArchiveFinalizationMutationKind::PluginCreation
             : cao::run::ArchiveFinalizationMutationKind::PluginRemoval);
    QCOMPARE(mutation.mutation, cao::execution::MutationState::Committed);
    QCOMPARE(mutation.count, std::size_t{1});
    QCOMPARE(std::filesystem::exists(plugin), createDummies);
    QVERIFY(artifacts.performSafetyCleanup().empty());

    cao::run::MutableRunEvidence evidence;
    evidence.recordPhase(cao::run::RunPhaseRecord::executed(cao::run::RunPhase::Preparing));
    evidence.recordPhase(
        cao::run::RunPhaseRecord::executed(cao::run::RunPhase::ArchiveFinalization));
    evidence.recordArchiveFinalizationPlan(0);
    evidence.recordArchiveFinalization(result);
    evidence.recordPhase(cao::run::RunPhaseRecord::executed(cao::run::RunPhase::SafetyCleanup));
    const auto terminal = std::move(evidence).consume();
    QVERIFY(terminal.archiveFinalization() != nullptr);
    QVERIFY(terminal.archiveFinalization()->attempts.empty());
    QCOMPARE(terminal.phase(cao::run::RunPhase::ArchiveFinalization)->progress()->total(),
             std::size_t{0});
    QCOMPARE(terminal.mutationSummaries().size(), std::size_t{1});
    QCOMPARE(terminal.mutationSummaries().front().committed, std::size_t{1});
}

void MainOptimizerTests::finalizationRemovesOnlyExactDummyPlugins_data() {
    QTest::addColumn<int>("game");
    QTest::addColumn<QString>("profile");
    QTest::newRow("SSE") << 4 << QStringLiteral("SSE");
    QTest::newRow("FO4") << 5 << QStringLiteral("FO4");
}

void MainOptimizerTests::finalizationRemovesOnlyExactDummyPlugins() {
    QFETCH(int, game);
    QFETCH(QString, profile);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const ScopedCurrentDirectory isolatedWorkingDirectory(directory.path());
    const auto parent = std::filesystem::path(directory.path().toStdWString());
    writeFile(parent / "profiles" / profile.toStdWString() / "profile.ini",
              QByteArray("[BSA]\nbsaEnabled=true\nbsaGame=") + QByteArray::number(game) + '\n');
    Profiles::setCurrentProfile(profile);
    const auto mod = parent / "mod";
    writeFile(mod / "existing.bsa", QByteArrayLiteral("retained archive"));
    const auto dummy = canonicalDummyBytes(static_cast<btu::Game>(game));
    QByteArray sameSize = dummy;
    sameSize[0] = static_cast<char>(sameSize[0] ^ 0x5a);
    const auto exact = mod / "external.esp";
    const auto different = mod / "different.esp";
    const auto full = mod / "full.esm";
    const auto unrelated = parent / "outside.esp";
    writeFile(exact, dummy);
    writeFile(different, sameSize);
    writeFile(full, QByteArrayLiteral("full loading plugin"));
    writeFile(unrelated, dummy);

    OptionsCAO options;
    options.bBsaCreateDummies = false;
    const std::array roots{mod};
    const BSAOptimizer optimizer;
    const auto plan = optimizer.planFinalization(roots, options);
    QVERIFY(plan.outputs().empty());
    std::vector<cao::run::ArchiveFinalizationProgress> progress;
    cao::run::TemporaryArtifactRegistry artifacts;
    const auto result = optimizer.finalize(plan, artifacts, {},
                                           [&](const auto& value) { progress.push_back(value); });
    QVERIFY(result.attempts.empty());
    QVERIFY(!result.failure);
    QVERIFY(result.safeToContinue);
    QCOMPARE(progress.size(), std::size_t{1});
    QCOMPARE(progress.front().completed, std::size_t{0});
    QCOMPARE(progress.front().total, std::size_t{0});
    QCOMPARE(result.mutations.size(), std::size_t{1});
    QCOMPARE(result.mutations.front().modRoot, std::filesystem::canonical(mod));
    QCOMPARE(result.mutations.front().path, exact);
    QCOMPARE(result.mutations.front().kind,
             cao::run::ArchiveFinalizationMutationKind::PluginRemoval);
    QCOMPARE(result.mutations.front().mutation, cao::execution::MutationState::Committed);
    QVERIFY(!std::filesystem::exists(exact));
    for (const auto& [path, expected] :
         {std::pair{different, sameSize}, std::pair{full, QByteArrayLiteral("full loading plugin")},
          std::pair{unrelated, dummy}}) {
        QFile retained(QString::fromStdWString(path.wstring()));
        QVERIFY(retained.open(QIODevice::ReadOnly));
        QCOMPARE(retained.readAll(), expected);
    }
    QVERIFY(artifacts.performSafetyCleanup().empty());
}

void MainOptimizerTests::finalizationRejectsLinkedDummyPlugins_data() {
    QTest::addColumn<bool>("symlink");
    QTest::newRow("hard-link") << false;
    QTest::newRow("file-symlink") << true;
}

void MainOptimizerTests::finalizationRejectsLinkedDummyPlugins() {
#ifdef _WIN32
    QFETCH(bool, symlink);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const ScopedCurrentDirectory isolatedWorkingDirectory(directory.path());
    const auto parent = std::filesystem::path(directory.path().toStdWString());
    writeFile(parent / "profiles" / "SSE" / "profile.ini",
              QByteArrayLiteral("[BSA]\nbsaEnabled=true\nbsaGame=4\n"));
    Profiles::setCurrentProfile("SSE");
    const auto mod = parent / "mod";
    writeFile(mod / "existing.bsa", QByteArrayLiteral("retained archive"));
    const auto bytes = canonicalDummyBytes(btu::Game::SSE);
    const auto target = parent / "outside.bin";
    const auto plugin = mod / "external.esp";
    writeFile(target, bytes);
    std::error_code linkError;
    if (symlink)
        std::filesystem::create_symlink(target, plugin, linkError);
    else
        std::filesystem::create_hard_link(target, plugin, linkError);
    if (linkError && symlink) QSKIP("File symlink creation is unavailable on this host");
    QVERIFY2(!linkError, linkError.message().c_str());

    OptionsCAO options;
    options.bBsaCreateDummies = false;
    const std::array roots{mod};
    const BSAOptimizer optimizer;
    const auto plan = optimizer.planFinalization(roots, options);
    QVERIFY(plan.outputs().empty());
    cao::run::TemporaryArtifactRegistry artifacts;
    const auto result = optimizer.finalize(plan, artifacts);
    QVERIFY(result.attempts.empty());
    QCOMPARE(result.failure,
             std::optional{cao::run::ArchiveFinalizationFailure::PluginRemovalFailed});
    QVERIFY(result.safeToContinue);
    QVERIFY(result.mutations.empty());
    QCOMPARE(std::filesystem::is_symlink(std::filesystem::symlink_status(plugin)), symlink);
    QFile outside(QString::fromStdWString(target.wstring()));
    QVERIFY(outside.open(QIODevice::ReadOnly));
    QCOMPARE(outside.readAll(), bytes);
    QVERIFY(std::filesystem::exists(plugin));
    QVERIFY(artifacts.performSafetyCleanup().empty());
#else
    QSKIP("Windows native file identity provides the link guard.");
#endif
}

void MainOptimizerTests::finalizationRejectsChangedDummyPluginParent() {
#ifdef _WIN32
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const ScopedCurrentDirectory isolatedWorkingDirectory(directory.path());
    const auto parent = std::filesystem::path(directory.path().toStdWString());
    writeFile(parent / "profiles" / "SSE" / "profile.ini",
              QByteArrayLiteral("[BSA]\nbsaEnabled=true\nbsaGame=4\n"));
    Profiles::setCurrentProfile("SSE");
    const auto mod = parent / "mod";
    const auto outside = parent / "outside";
    const auto retained = parent / "retained-mod";
    const auto bytes = canonicalDummyBytes(btu::Game::SSE);
    writeFile(mod / "existing.bsa", QByteArrayLiteral("retained archive"));
    writeFile(mod / "original.esp", bytes);
    writeFile(outside / "external.esp", bytes);

    OptionsCAO options;
    options.bBsaCreateDummies = false;
    const std::array roots{mod};
    const BSAOptimizer optimizer;
    const auto plan = optimizer.planFinalization(roots, options);
    QVERIFY(plan.outputs().empty());
    std::filesystem::rename(mod, retained);
    const auto quotedPath = [](const std::filesystem::path& path) {
        auto value = QString::fromStdWString(path.wstring());
        value.replace("'", "''");
        return "'" + value + "'";
    };
    QProcess process;
    process.start("powershell.exe",
                  {"-NoProfile", "-NonInteractive", "-Command",
                   "New-Item -ItemType Junction -Path " + quotedPath(mod) + " -Value " +
                       quotedPath(outside) + " -ErrorAction Stop | Out-Null"});
    QVERIFY(process.waitForFinished());
    QCOMPARE(process.exitCode(), 0);

    cao::run::TemporaryArtifactRegistry artifacts;
    const auto result = optimizer.finalize(plan, artifacts);
    std::error_code cleanupError;
    std::filesystem::remove(mod, cleanupError);
    QVERIFY2(!cleanupError, cleanupError.message().c_str());
    QVERIFY(result.attempts.empty());
    QCOMPARE(result.failure,
             std::optional{cao::run::ArchiveFinalizationFailure::PluginRemovalFailed});
    QVERIFY(result.safeToContinue);
    QVERIFY(result.mutations.empty());
    QVERIFY(std::filesystem::exists(retained / "original.esp"));
    QFile external(QString::fromStdWString((outside / "external.esp").wstring()));
    QVERIFY(external.open(QIODevice::ReadOnly));
    QCOMPARE(external.readAll(), bytes);
    QVERIFY(artifacts.performSafetyCleanup().empty());
#else
    QSKIP("Windows junctions provide the changed-parent regression case.");
#endif
}

void MainOptimizerTests::finalizationPreservesReplacedDummyPlugin() {
#ifdef _WIN32
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const ScopedCurrentDirectory isolatedWorkingDirectory(directory.path());
    const auto parent = std::filesystem::path(directory.path().toStdWString());
    writeFile(parent / "profiles" / "SSE" / "profile.ini",
              QByteArrayLiteral("[BSA]\nbsaEnabled=true\nbsaGame=4\n"));
    Profiles::setCurrentProfile("SSE");
    const auto mod = parent / "mod";
    writeFile(mod / "textures" / "asset.dds", QByteArrayLiteral("source bytes"));
    const auto bytes = canonicalDummyBytes(btu::Game::SSE);
    QByteArray replacement = bytes;
    replacement[0] = static_cast<char>(replacement[0] ^ 0x5a);
    const auto plugin = mod / "external.esp";
    const auto displaced = parent / "displaced.bin";
    writeFile(plugin, bytes);

    OptionsCAO options;
    options.bBsaCreateDummies = false;
    options.bBsaCompress = false;
    options.bBsaDeleteSource = false;
    const std::array roots{mod};
    const BSAOptimizer optimizer;
    const auto plan = optimizer.planFinalization(roots, options);
    QCOMPARE(plan.outputs().size(), std::size_t{1});
    bool replaced = false;
    cao::run::TemporaryArtifactRegistry artifacts;
    const auto result =
        optimizer.finalize(plan, artifacts, {}, {}, cao::run::availableArchiveCapacity,
                           [&](const cao::run::ArchiveFinalizationAttempt&) {
                               std::filesystem::rename(plugin, displaced);
                               writeFile(plugin, replacement);
                               replaced = true;
                           });
    QVERIFY(replaced);
    QCOMPARE(result.attempts.size(), std::size_t{1});
    QVERIFY(result.attempts.front().succeeded());
    QVERIFY(!result.failure);
    QVERIFY(result.mutations.empty());
    QFile current(QString::fromStdWString(plugin.wstring()));
    QVERIFY(current.open(QIODevice::ReadOnly));
    QCOMPARE(current.readAll(), replacement);
    QFile earlier(QString::fromStdWString(displaced.wstring()));
    QVERIFY(earlier.open(QIODevice::ReadOnly));
    QCOMPARE(earlier.readAll(), bytes);
    QVERIFY(artifacts.performSafetyCleanup().empty());
#else
    QSKIP("Windows file identity guards provide the replacement regression case.");
#endif
}

void MainOptimizerTests::finalizationRetainsRemovalPrefixOnGuardFailure() {
#ifdef _WIN32
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const ScopedCurrentDirectory isolatedWorkingDirectory(directory.path());
    const auto parent = std::filesystem::path(directory.path().toStdWString());
    writeFile(parent / "profiles" / "SSE" / "profile.ini",
              QByteArrayLiteral("[BSA]\nbsaEnabled=true\nbsaGame=4\n"));
    Profiles::setCurrentProfile("SSE");
    const std::array roots{parent / "mod-a", parent / "mod-b"};
    const auto bytes = canonicalDummyBytes(btu::Game::SSE);
    for (const auto& root : roots) {
        writeFile(root / "existing.bsa", QByteArrayLiteral("retained archive"));
        writeFile(root / "external.esp", bytes);
    }

    OptionsCAO options;
    options.bBsaCreateDummies = false;
    const BSAOptimizer optimizer;
    const auto plan = optimizer.planFinalization(roots, options);
    QVERIFY(plan.outputs().empty());
    const auto blocked = roots.back() / "external.esp";
    const auto held = CreateFileW(blocked.c_str(), GENERIC_READ, FILE_SHARE_READ, nullptr,
                                  OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    QVERIFY(held != INVALID_HANDLE_VALUE);
    std::vector<cao::run::ArchiveFinalizationProgress> progress;
    cao::run::TemporaryArtifactRegistry artifacts;
    const auto result = optimizer.finalize(plan, artifacts, {},
                                           [&](const auto& value) { progress.push_back(value); });
    QVERIFY(CloseHandle(held));
    QVERIFY(result.attempts.empty());
    QCOMPARE(result.failure,
             std::optional{cao::run::ArchiveFinalizationFailure::PluginRemovalFailed});
    QVERIFY(result.safeToContinue);
    QCOMPARE(result.mutations.size(), std::size_t{1});
    QCOMPARE(result.mutations.front().modRoot, std::filesystem::canonical(roots.front()));
    QCOMPARE(result.mutations.front().path, roots.front() / "external.esp");
    QCOMPARE(result.mutations.front().kind,
             cao::run::ArchiveFinalizationMutationKind::PluginRemoval);
    QCOMPARE(result.mutations.front().mutation, cao::execution::MutationState::Committed);
    QVERIFY(!std::filesystem::exists(roots.front() / "external.esp"));
    QVERIFY(std::filesystem::exists(blocked));
    QCOMPARE(progress.size(), std::size_t{1});
    QCOMPARE(progress.front().completed, std::size_t{0});
    QCOMPARE(progress.front().total, std::size_t{0});
    QVERIFY(artifacts.performSafetyCleanup().empty());
#else
    QSKIP("Windows sharing modes provide the guarded deletion failure.");
#endif
}

void MainOptimizerTests::existingArchivePluginCollisionFailsSafely() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const ScopedCurrentDirectory isolatedWorkingDirectory(directory.path());
    const auto parent = std::filesystem::path(directory.path().toStdWString());
    writeFile(parent / "profiles" / "SSE" / "profile.ini",
              QByteArrayLiteral("[BSA]\nbsaEnabled=true\nbsaGame=4\n"));
    Profiles::setCurrentProfile("SSE");
    const auto mod = parent / "mod";
    writeFile(mod / "existing.bsa", QByteArrayLiteral("retained archive"));
    OptionsCAO options;
    options.bBsaCreateDummies = true;
    const BSAOptimizer optimizer;
    const std::array roots{mod};
    const auto plan = optimizer.planFinalization(roots, options);
    QVERIFY(plan.outputs().empty());
    const auto occupied = mod / "existing.esp";
    QVERIFY(std::filesystem::create_directory(occupied));

    std::vector<cao::run::ArchiveFinalizationProgress> progress;
    cao::run::TemporaryArtifactRegistry artifacts;
    const auto result = optimizer.finalize(plan, artifacts, {},
                                           [&](const auto& update) { progress.push_back(update); });
    QVERIFY(result.attempts.empty());
    QCOMPARE(result.failure,
             std::optional{cao::run::ArchiveFinalizationFailure::PluginCreationFailed});
    QVERIFY(result.safeToContinue);
    QVERIFY(result.mutations.empty());
    QCOMPARE(progress.size(), std::size_t{1});
    QCOMPARE(progress.front().completed, std::size_t{0});
    QCOMPARE(progress.front().total, std::size_t{0});
    QVERIFY(std::filesystem::is_directory(occupied));
    QVERIFY(artifacts.performSafetyCleanup().empty());
}

void MainOptimizerTests::existingArchivePluginNamesFollowProfile_data() {
    QTest::addColumn<int>("game");
    QTest::addColumn<QString>("profile");
    QTest::addColumn<QString>("archiveName");
    QTest::newRow("SLE suffix-free") << 3 << QStringLiteral("SLE") << QStringLiteral("bundle.bsa");
    QTest::newRow("SSE texture suffix")
        << 4 << QStringLiteral("SSE") << QStringLiteral("bundle - Textures.bsa");
    QTest::newRow("FO4 main suffix")
        << 5 << QStringLiteral("FO4") << QStringLiteral("bundle - Main.ba2");
    QTest::newRow("FO4 texture suffix")
        << 5 << QStringLiteral("FO4") << QStringLiteral("bundle - Textures.ba2");
}

void MainOptimizerTests::existingArchivePluginNamesFollowProfile() {
    QFETCH(int, game);
    QFETCH(QString, profile);
    QFETCH(QString, archiveName);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const ScopedCurrentDirectory isolatedWorkingDirectory(directory.path());
    const auto parent = std::filesystem::path(directory.path().toStdWString());
    writeFile(parent / "profiles" / profile.toStdWString() / "profile.ini",
              QByteArray("[BSA]\nbsaEnabled=true\nbsaGame=") + QByteArray::number(game) + '\n');
    Profiles::setCurrentProfile(profile);
    const auto mod = parent / "mod";
    writeFile(mod / archiveName.toStdWString(), QByteArrayLiteral("retained archive"));
    OptionsCAO options;
    options.bBsaCreateDummies = true;
    const BSAOptimizer optimizer;
    const std::array roots{mod};
    const auto plan = optimizer.planFinalization(roots, options);
    QVERIFY(plan.outputs().empty());

    cao::run::TemporaryArtifactRegistry artifacts;
    const auto result = optimizer.finalize(plan, artifacts);
    QVERIFY(result.attempts.empty());
    QVERIFY(!result.failure);
    QCOMPARE(result.mutations.size(), std::size_t{1});
    const auto plugin = mod / "bundle.esp";
    QCOMPARE(result.mutations.front().modRoot, std::filesystem::canonical(mod));
    QCOMPARE(result.mutations.front().path, plugin);
    QCOMPARE(result.mutations.front().kind,
             cao::run::ArchiveFinalizationMutationKind::PluginCreation);
    QCOMPARE(result.mutations.front().mutation, cao::execution::MutationState::Committed);
    const auto& bytes = *btu::bsa::Settings::get(static_cast<btu::Game>(game)).s_dummy_plugin;
    QFile published(QString::fromStdWString(plugin.wstring()));
    QVERIFY(published.open(QIODevice::ReadOnly));
    QCOMPARE(published.readAll(), QByteArray(reinterpret_cast<const char*>(bytes.data()),
                                             static_cast<int>(bytes.size())));
    published.close();
    QVERIFY(artifacts.performSafetyCleanup().empty());
    // Durable ownership controls remain; Safety Cleanup removes the owned run child.
    for (const auto& entry : std::filesystem::directory_iterator(mod / ".cao-staging"))
        QVERIFY(!entry.is_directory());
    QVERIFY(std::filesystem::exists(plugin));
}

void MainOptimizerTests::existingArchivesShareLoadingPlugin_data() {
    QTest::addColumn<int>("preexisting");
    QTest::newRow("create-once") << 0;
    QTest::newRow("preserve-full-plugin") << 1;
    QTest::newRow("preserve-exact-dummy") << 2;
}

void MainOptimizerTests::existingArchivesShareLoadingPlugin() {
    QFETCH(int, preexisting);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const ScopedCurrentDirectory isolatedWorkingDirectory(directory.path());
    const auto parent = std::filesystem::path(directory.path().toStdWString());
    writeFile(parent / "profiles" / "FO4" / "profile.ini",
              QByteArrayLiteral("[BSA]\nbsaEnabled=true\nbsaGame=5\n"));
    Profiles::setCurrentProfile("FO4");
    const auto mod = parent / "mod";
    writeFile(mod / "bundle - Main.ba2", QByteArrayLiteral("retained main archive"));
    writeFile(mod / "bundle - Textures.ba2", QByteArrayLiteral("retained texture archive"));
    const auto dummy = mod / "bundle.esp";
    const auto full = mod / "bundle.esm";
    const auto& bytes = *btu::bsa::Settings::get(btu::Game::FO4).s_dummy_plugin;
    const auto originalBytes = preexisting == 1
                                   ? QByteArrayLiteral("full loading plugin")
                                   : QByteArray(reinterpret_cast<const char*>(bytes.data()),
                                                static_cast<int>(bytes.size()));
    const auto original = preexisting == 1 ? full : dummy;
    if (preexisting != 0) writeFile(original, originalBytes);
    OptionsCAO options;
    options.bBsaCreateDummies = true;
    const BSAOptimizer optimizer;
    const std::array roots{mod};
    const auto plan = optimizer.planFinalization(roots, options);
    QVERIFY(plan.outputs().empty());

    std::vector<cao::run::ArchiveFinalizationProgress> progress;
    cao::run::TemporaryArtifactRegistry artifacts;
    const auto result = optimizer.finalize(plan, artifacts, {},
                                           [&](const auto& update) { progress.push_back(update); });
    QVERIFY(result.attempts.empty());
    QVERIFY(!result.failure);
    QVERIFY(result.safeToContinue);
    QCOMPARE(progress.size(), std::size_t{1});
    QCOMPARE(progress.front().completed, std::size_t{0});
    QCOMPARE(progress.front().total, std::size_t{0});
    QCOMPARE(result.mutations.size(), preexisting == 0 ? std::size_t{1} : std::size_t{0});
    if (preexisting == 0) {
        QCOMPARE(result.mutations.front().path, dummy);
        QCOMPARE(result.mutations.front().mutation, cao::execution::MutationState::Committed);
    }
    const auto expected = preexisting == 0 ? dummy : original;
    QFile retained(QString::fromStdWString(expected.wstring()));
    QVERIFY(retained.open(QIODevice::ReadOnly));
    QCOMPARE(retained.readAll(), preexisting == 0
                                     ? QByteArray(reinterpret_cast<const char*>(bytes.data()),
                                                  static_cast<int>(bytes.size()))
                                     : originalBytes);
    retained.close();
    if (preexisting == 1) QVERIFY(!std::filesystem::exists(dummy));
    QVERIFY(artifacts.performSafetyCleanup().empty());
    QVERIFY(std::filesystem::exists(expected));
}

void MainOptimizerTests::finalizationFreezesTotalAndCancelsBetweenOutputs_data() {
    QTest::addColumn<int>("cancelAfter");
    QTest::newRow("before-first-output") << 0;
    QTest::newRow("between-outputs") << 1;
    QTest::newRow("after-final-output") << 2;
    QTest::newRow("complete-run") << -1;
}

void MainOptimizerTests::finalizationFreezesTotalAndCancelsBetweenOutputs() {
    QFETCH(int, cancelAfter);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const ScopedCurrentDirectory isolatedWorkingDirectory(directory.path());
    const auto root = std::filesystem::path(directory.path().toStdWString());
    writeFile(root / "profiles" / "SSE" / "profile.ini",
              QByteArrayLiteral("[BSA]\nbsaEnabled=true\nbsaGame=4\n"));
    Profiles::setCurrentProfile("SSE");
    const std::array roots{root / "mod-a", root / "mod-b"};
    for (const auto& mod : roots)
        writeFile(mod / "textures" / "asset.dds", QByteArrayLiteral("source bytes"));
    OptionsCAO options;
    options.bBsaCreateDummies = false;
    options.bBsaCompress = false;
    options.bBsaDeleteSource = true;
    const BSAOptimizer optimizer;
    const auto plan = optimizer.planFinalization(roots, options);
    QCOMPARE(plan.outputs().size(), std::size_t{2});
    for (const auto& output : plan.outputs()) {
        QVERIFY(!std::filesystem::exists(output.archivePath));
        QVERIFY(!std::filesystem::exists(output.modRoot / ".cao-staging"));
        QCOMPARE(output.sources.size(), std::size_t{1});
        QVERIFY(std::filesystem::exists(output.sources.front()));
    }
    cao::run::TemporaryArtifactRegistry artifacts;
    std::stop_source stop;
    std::vector<cao::run::ArchiveFinalizationProgress> progress;
    std::vector<cao::run::ArchiveFinalizationAttempt> completedAttempts;
    const auto result = optimizer.finalize(plan, artifacts, stop.get_token(),
        [&](const cao::run::ArchiveFinalizationProgress& value) {
            QCOMPARE(completedAttempts.size(), value.completed);
            progress.push_back(value);
            if (static_cast<int>(value.completed) == cancelAfter) stop.request_stop();
        }, cao::run::availableArchiveCapacity,
        [&](const cao::run::ArchiveFinalizationAttempt& attempt) {
            completedAttempts.push_back(attempt);
        });
    // Directory pruning belongs to finalization and must wait for the entire output plan.
    for (const auto& mod : roots)
        QCOMPARE(std::filesystem::exists(mod / "textures"), cancelAfter >= 0);
    const auto attempted = cancelAfter < 0 ? std::size_t{2} : static_cast<std::size_t>(cancelAfter);
    QCOMPARE(result.attempts.size(), attempted);
    QCOMPARE(completedAttempts.size(), attempted);
    QCOMPARE(result.cancelled, cancelAfter >= 0);
    QVERIFY(result.safeToContinue);
    QCOMPARE(progress.size(), attempted + 1);
    for (std::size_t index = 0; index < progress.size(); ++index) {
        QCOMPARE(progress[index].total, std::size_t{2});
        QCOMPARE(progress[index].completed, index);
        QCOMPARE(progress[index].succeeded, index);
        QCOMPARE(progress[index].failed, std::size_t{0});
    }
    for (std::size_t index = 0; index < plan.outputs().size(); ++index) {
        const auto& output = plan.outputs()[index];
        QCOMPARE(std::filesystem::exists(output.archivePath), index < attempted);
        QCOMPARE(std::filesystem::exists(output.sources.front()), index >= attempted);
        if (index < attempted) {
            QVERIFY(result.attempts[index].succeeded());
            QCOMPARE(completedAttempts[index].modRoot, output.modRoot);
            QCOMPARE(completedAttempts[index].archivePath, output.archivePath);
            QCOMPARE(completedAttempts[index].mutation,
                     cao::execution::MutationState::Committed);
            QVERIFY(btu::bsa::read_archive(output.archivePath).has_value());
        }
    }
    QVERIFY(artifacts.performSafetyCleanup().empty());
}

void MainOptimizerTests::finalizationCapacityChecks_data() {
    QTest::addColumn<int>("scenario");
    QTest::newRow("late-root-shortage") << 0;
    QTest::newRow("unknown-capacity") << 1;
    QTest::newRow("capacity-disappears") << 2;
    QTest::newRow("source-grows-after-planning") << 3;
}

void MainOptimizerTests::finalizationCapacityWithoutOutputs() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const ScopedCurrentDirectory isolatedWorkingDirectory(directory.path());
    const auto root = std::filesystem::path(directory.path().toStdWString());
    writeFile(root / "profiles" / "SSE" / "profile.ini",
              QByteArrayLiteral("[BSA]\nbsaEnabled=true\nbsaGame=4\n"));
    Profiles::setCurrentProfile("SSE");
    const auto mod = root / "mod";
    writeFile(mod / "existing.bsa", QByteArrayLiteral("retained archive"));
    std::filesystem::create_directory(mod / "empty");
    OptionsCAO options;
    options.bBsaCreateDummies = true;
    const BSAOptimizer optimizer;
    const std::array roots{mod};
    const auto plan = optimizer.planFinalization(roots, options);
    QVERIFY(plan.outputs().empty());
    cao::run::TemporaryArtifactRegistry artifacts;
    std::vector<cao::run::ArchiveFinalizationProgress> progress;
    const auto result = optimizer.finalize(plan, artifacts, {},
        [&](const auto& value) { progress.push_back(value); },
        [](const auto&) -> std::optional<std::uintmax_t> { return 0; });
    QVERIFY(result.failure == cao::run::ArchiveFinalizationFailure::InsufficientCapacity);
    QVERIFY(result.attempts.empty());
    QVERIFY(result.safeToContinue);
    QVERIFY(!result.detail.empty());
    QCOMPARE(progress.back().completed, std::size_t{0});
    QCOMPARE(progress.back().total, std::size_t{0});
    QVERIFY(std::filesystem::exists(mod / "empty"));
    QVERIFY(!std::filesystem::exists(mod / "existing.esp"));
    QVERIFY(!std::filesystem::exists(mod / ".cao-staging"));
}

void MainOptimizerTests::finalizationOutputRetainsPositionalCapacity() {
    const cao::run::ArchiveFinalizationOutput output{
        {}, {}, {}, std::nullopt, std::uintmax_t{42}};
    QCOMPARE(output.estimatedCapacityBytes, std::uintmax_t{42});
}

void MainOptimizerTests::finalizationCapacityChecks() {
    QFETCH(int, scenario);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const ScopedCurrentDirectory isolatedWorkingDirectory(directory.path());
    const auto root = std::filesystem::path(directory.path().toStdWString());
    writeFile(root / "profiles" / "SSE" / "profile.ini",
              QByteArrayLiteral("[BSA]\nbsaEnabled=true\nbsaGame=4\n"));
    Profiles::setCurrentProfile("SSE");
    const std::array roots{root / "mod-a", root / "mod-b"};
    for (const auto& mod : roots) {
        writeFile(mod / "meshes" / "asset.nif", QByteArray(8192, 'x'));
        std::filesystem::create_directory(mod / "empty");
    }
    OptionsCAO options;
    options.bBsaCreateDummies = true;
    options.bBsaCompress = true;
    options.bBsaDeleteSource = true;
    const BSAOptimizer optimizer;
    const auto plan = optimizer.planFinalization(roots, options);
    QCOMPARE(plan.outputs().size(), std::size_t{2});
    bool firstCommitted = false;
    cao::run::TemporaryArtifactRegistry artifacts;
    std::vector<cao::run::ArchiveFinalizationAttempt> completedAttempts;
    const auto result = optimizer.finalize(plan, artifacts, {},
        [&](const cao::run::ArchiveFinalizationProgress& value) {
            QCOMPARE(completedAttempts.size(), value.completed);
            firstCommitted = value.succeeded > 0;
            if (scenario == 3 && value.completed == 1)
                writeFile(plan.outputs()[1].sources.front(), QByteArray(1024 * 1024, 'y'));
        }, [&](const std::filesystem::path& path) -> std::optional<std::uintmax_t> {
            if (scenario == 1) return std::nullopt;
            // Each output fits individually, but the shared phase cannot fit at the later root.
            if (scenario == 0 && path == roots[1]) return plan.outputs()[1].estimatedCapacityBytes;
            if (scenario == 3 && firstCommitted) return plan.outputs()[1].estimatedCapacityBytes;
            if (firstCommitted) return 0;
            return std::numeric_limits<std::uintmax_t>::max();
        }, [&](const cao::run::ArchiveFinalizationAttempt& attempt) {
            completedAttempts.push_back(attempt);
        });
    QCOMPARE(completedAttempts.size(), result.attempts.size());
    QVERIFY(result.safeToContinue);
    QVERIFY(!result.cancelled);
    const auto committed = scenario == 0 ? 0u : scenario == 1 ? 2u : 1u;
    for (std::size_t index = 0; index < plan.outputs().size(); ++index) {
        const auto& output = plan.outputs()[index];
        QCOMPARE(std::filesystem::exists(output.archivePath), index < committed);
        QCOMPARE(std::filesystem::exists(output.sources.front()), index >= committed);
        QCOMPARE(std::filesystem::exists(output.modRoot / "empty"), scenario != 1);
        if (index < committed) {
            auto actual = std::filesystem::file_size(output.archivePath);
            if (output.pluginPath) actual += std::filesystem::file_size(*output.pluginPath);
            QVERIFY(output.estimatedCapacityBytes >= actual);
        } else {
            QVERIFY(!std::filesystem::exists(output.modRoot / ".cao-staging"));
            if (output.pluginPath) QVERIFY(!std::filesystem::exists(*output.pluginPath));
        }
    }
    if (scenario != 1) {
        QVERIFY(result.attempts.back().failure == cao::run::ArchiveFinalizationFailure::InsufficientCapacity);
        QCOMPARE(completedAttempts.back().failure,
                 std::optional{cao::run::ArchiveFinalizationFailure::InsufficientCapacity});
        QCOMPARE(result.attempts.back().mutation, cao::execution::MutationState::None);
        QVERIFY(!result.attempts.back().detail.empty());
    }
    QVERIFY(artifacts.performSafetyCleanup().empty());
}

void MainOptimizerTests::finalizationCapacityIsGroupedByVolume() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const ScopedCurrentDirectory isolatedWorkingDirectory(directory.path());
    const auto parent = std::filesystem::path(directory.path().toStdWString());
    writeFile(parent / "profiles" / "SSE" / "profile.ini",
              QByteArrayLiteral("[BSA]\nbsaEnabled=true\nbsaGame=4\n"));
    Profiles::setCurrentProfile("SSE");
    const std::array roots{parent / "mod-a", parent / "mod-b"};
    for (const auto& root : roots)
        writeFile(root / "textures" / "asset.dds", QByteArrayLiteral("source bytes"));
    OptionsCAO options;
    options.bBsaCreateDummies = false;
    options.bBsaCompress = false;
    options.bBsaDeleteSource = false;
    const BSAOptimizer optimizer;
    const auto plan = optimizer.planFinalization(roots, options);
    QCOMPARE(plan.outputs().size(), std::size_t{2});
    const auto capacity = [&](const std::filesystem::path& path)
        -> std::optional<std::uintmax_t> {
        const auto output = std::find_if(plan.outputs().begin(), plan.outputs().end(),
                                         [&](const auto& value) { return value.modRoot == path; });
        return output->estimatedCapacityBytes;
    };
    const auto volume = [&](const std::filesystem::path& path) -> std::optional<std::string> {
        return path == std::filesystem::canonical(roots.front()) ? "first-volume"
                                                              : "second-volume";
    };
    cao::run::TemporaryArtifactRegistry artifacts;
    const auto result = optimizer.finalize(plan, artifacts, {}, {}, capacity, {}, volume);
    QCOMPARE(result.attempts.size(), std::size_t{2});
    QVERIFY(result.safeToContinue);
    QVERIFY(!result.failure);
    for (const auto& attempt : result.attempts) {
        QVERIFY(attempt.succeeded());
        QVERIFY(std::filesystem::exists(attempt.archivePath));
    }
    QVERIFY(artifacts.performSafetyCleanup().empty());
}

void MainOptimizerTests::idleUnknownVolumeDoesNotInflateCapacity() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const ScopedCurrentDirectory isolatedWorkingDirectory(directory.path());
    const auto parent = std::filesystem::path(directory.path().toStdWString());
    writeFile(parent / "profiles" / "SSE" / "profile.ini",
              QByteArrayLiteral("[BSA]\nbsaEnabled=true\nbsaGame=4\n"));
    Profiles::setCurrentProfile("SSE");
    const std::array roots{parent / "mod-a", parent / "mod-b", parent / "mod-idle"};
    for (std::size_t index = 0; index < 2; ++index)
        writeFile(roots[index] / "textures" / "asset.dds", QByteArrayLiteral("source bytes"));
    std::filesystem::create_directory(roots.back());
    OptionsCAO options;
    options.bBsaCreateDummies = false;
    options.bBsaCompress = false;
    const BSAOptimizer optimizer;
    const auto plan = optimizer.planFinalization(roots, options);
    QCOMPARE(plan.outputs().size(), std::size_t{2});
    const auto capacity = [&](const std::filesystem::path& path)
        -> std::optional<std::uintmax_t> {
        if (path == std::filesystem::canonical(roots.back())) return 0;
        const auto output = std::find_if(plan.outputs().begin(), plan.outputs().end(),
                                         [&](const auto& value) { return value.modRoot == path; });
        return output->estimatedCapacityBytes;
    };
    const auto volume = [&](const std::filesystem::path& path) -> std::optional<std::string> {
        if (path == std::filesystem::canonical(roots.back())) return std::nullopt;
        return path == std::filesystem::canonical(roots.front()) ? "first-volume"
                                                              : "second-volume";
    };
    cao::run::TemporaryArtifactRegistry artifacts;
    const auto result = optimizer.finalize(plan, artifacts, {}, {}, capacity, {}, volume);
    QCOMPARE(result.attempts.size(), std::size_t{2});
    QVERIFY(!result.failure);
    for (const auto& attempt : result.attempts) QVERIFY(attempt.succeeded());
    QVERIFY(artifacts.performSafetyCleanup().empty());
}

void MainOptimizerTests::dummyCapacityDecreasesAfterEachRoot() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const ScopedCurrentDirectory isolatedWorkingDirectory(directory.path());
    const auto parent = std::filesystem::path(directory.path().toStdWString());
    writeFile(parent / "profiles" / "SSE" / "profile.ini",
              QByteArrayLiteral("[BSA]\nbsaEnabled=true\nbsaGame=4\n"));
    Profiles::setCurrentProfile("SSE");
    const std::array roots{parent / "mod-a", parent / "mod-b"};
    for (const auto& root : roots)
        writeFile(root / "existing.bsa", QByteArrayLiteral("retained archive"));
    OptionsCAO options;
    options.bBsaCreateDummies = true;
    const BSAOptimizer optimizer;
    const auto plan = optimizer.planFinalization(roots, options);
    QVERIFY(plan.outputs().empty());
    const auto firstPlugin = roots.front() / "existing.esp";
    const auto capacity = [&](const std::filesystem::path&)
        -> std::optional<std::uintmax_t> {
        if (std::filesystem::exists(firstPlugin)) return std::filesystem::file_size(firstPlugin);
        return std::numeric_limits<std::uintmax_t>::max();
    };
    std::size_t volumeQueries = 0;
    cao::run::TemporaryArtifactRegistry artifacts;
    const auto result = optimizer.finalize(
        plan, artifacts, {}, {}, capacity, {},
        [&](const auto&) -> std::optional<std::string> {
            ++volumeQueries;
            return "shared-volume";
        });
    QVERIFY(!result.failure);
    QVERIFY(result.attempts.empty());
    QCOMPARE(volumeQueries, roots.size());
    for (const auto& root : roots) QVERIFY(std::filesystem::exists(root / "existing.esp"));
    QVERIFY(artifacts.performSafetyCleanup().empty());
}

void MainOptimizerTests::plannedNamesAreDistinctAndCommitPreservesNewDestination() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const ScopedCurrentDirectory isolatedWorkingDirectory(directory.path());
    const auto root = std::filesystem::path(directory.path().toStdWString());
    writeFile(root / "profiles" / "SSE" / "profile.ini",
              QByteArrayLiteral("[BSA]\nbsaEnabled=true\nbsaGame=4\n"));
    Profiles::setCurrentProfile("SSE");
    const auto mod = root / "mod";
    writeFile(mod / "meshes" / "asset.nif", QByteArrayLiteral("mesh bytes"));
    writeFile(mod / "sound" / "asset.wav", QByteArrayLiteral("sound bytes"));
    OptionsCAO options;
    options.bBsaCreateDummies = false;
    options.bBsaCompress = false;
    options.bBsaDeleteSource = true;
    options.bBsaMergeIncomp = false;
    options.bBsaMergeTexture = false;
    const BSAOptimizer optimizer;
    const std::array roots{mod};
    const auto plan = optimizer.planFinalization(roots, options);
    QCOMPARE(plan.outputs().size(), std::size_t{2});
    QVERIFY(plan.outputs()[0].archivePath != plan.outputs()[1].archivePath);
    for (const auto& output : plan.outputs()) {
        QVERIFY(!std::filesystem::exists(output.archivePath));
    }
    cao::run::TemporaryArtifactRegistry artifacts;
    std::vector<cao::run::ArchiveFinalizationProgress> progress;
    const auto result = optimizer.finalize(plan, artifacts, {},
        [&](const cao::run::ArchiveFinalizationProgress& value) {
            progress.push_back(value);
            // Simulate a competing creator after planning and before the first attempt.
            if (value.completed == 0)
                writeFile(plan.outputs()[0].archivePath, QByteArrayLiteral("competing creator bytes"));
        });
    QCOMPARE(result.attempts.size(), std::size_t{2});
    QVERIFY(result.safeToContinue);
    QVERIFY(!result.cancelled);
    QCOMPARE(progress.back().completed, std::size_t{2});
    QCOMPARE(progress.back().failed, std::size_t{1});
    QCOMPARE(progress.back().succeeded, std::size_t{1});
    QCOMPARE(progress.size(), std::size_t{3});
    QCOMPARE(progress[1].total, std::size_t{2});
    QCOMPARE(progress[1].completed, std::size_t{1});
    QCOMPARE(progress[1].failed, std::size_t{1});
    QCOMPARE(progress[1].succeeded, std::size_t{0});
    for (std::size_t index = 0; index < plan.outputs().size(); ++index) {
        QCOMPARE(result.attempts[index].succeeded(), index == 1);
        QCOMPARE(result.attempts[index].mutation, index == 0 ? cao::execution::MutationState::None
                                                           : cao::execution::MutationState::Committed);
        const auto& output = plan.outputs()[index];
        for (const auto& source : output.sources)
            QCOMPARE(std::filesystem::exists(source), index == 0);
        QFile destination(QString::fromStdWString(output.archivePath.wstring()));
        QVERIFY(destination.open(QIODevice::ReadOnly));
        if (index == 0)
            QCOMPARE(destination.readAll(), QByteArrayLiteral("competing creator bytes"));
        else
            QVERIFY(btu::bsa::read_archive(output.archivePath).has_value());
    }
    QVERIFY(artifacts.performSafetyCleanup().empty());
}

void MainOptimizerTests::plannedOutputNamesUseExactDummyBytes_data() {
    QTest::addColumn<int>("game");
    QTest::addColumn<QString>("profile");
    QTest::addColumn<QString>("archiveName");
    QTest::newRow("SLE suffix-free") << 3 << QStringLiteral("SLE")
                                      << QStringLiteral("loader.bsa");
    QTest::newRow("SSE texture suffix") << 4 << QStringLiteral("SSE")
                                         << QStringLiteral("loader - Textures.bsa");
    QTest::newRow("FO4 texture suffix") << 5 << QStringLiteral("FO4")
                                         << QStringLiteral("loader - Textures.ba2");
}

void MainOptimizerTests::plannedOutputNamesUseExactDummyBytes() {
    QFETCH(int, game);
    QFETCH(QString, profile);
    QFETCH(QString, archiveName);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const ScopedCurrentDirectory isolatedWorkingDirectory(directory.path());
    const auto root = std::filesystem::path(directory.path().toStdWString());
    writeFile(root / "profiles" / profile.toStdWString() / "profile.ini",
              QByteArray("[BSA]\nbsaEnabled=true\nbsaGame=") + QByteArray::number(game) + '\n');
    Profiles::setCurrentProfile(profile);
    const auto mod = root / "mod";
    const auto source = mod / "textures" / "asset.dds";
    writeFile(source, QByteArrayLiteral("source bytes"));
    const auto& dummy = *btu::bsa::Settings::get(static_cast<btu::Game>(game)).s_dummy_plugin;
    writeFile(mod / "dummy.esp", QByteArray(reinterpret_cast<const char*>(dummy.data()),
                                            static_cast<int>(dummy.size())));
    writeFile(mod / "loader.esm", QByteArray(static_cast<int>(dummy.size()), '\0'));
    OptionsCAO options;
    options.bBsaCreateDummies = true;
    options.bBsaCompress = false;
    options.bBsaDeleteSource = true;
    const BSAOptimizer optimizer;
    const std::array roots{mod};
    const auto plan = optimizer.planFinalization(roots, options);
    QCOMPARE(plan.outputs().size(), std::size_t{1});
    QCOMPARE(plan.outputs().front().archivePath.filename(),
             std::filesystem::path(archiveName.toStdWString()));
    QVERIFY(!plan.outputs().front().pluginPath.has_value());
}

void MainOptimizerTests::plannedOutputRechecksLoadingPluginNames_data() {
    QTest::addColumn<int>("game");
    QTest::addColumn<QString>("profile");
    QTest::addColumn<QString>("pluginName");
    QTest::newRow("SLE suffix-free") << 3 << QStringLiteral("SLE")
                                      << QStringLiteral("mod.esm");
    QTest::newRow("SSE texture suffix") << 4 << QStringLiteral("SSE")
                                         << QStringLiteral("mod - Textures.esl");
    QTest::newRow("FO4 suffix-free") << 5 << QStringLiteral("FO4")
                                     << QStringLiteral("mod.esm");
}

void MainOptimizerTests::plannedOutputRechecksLoadingPluginNames() {
    QFETCH(int, game);
    QFETCH(QString, profile);
    QFETCH(QString, pluginName);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const ScopedCurrentDirectory isolatedWorkingDirectory(directory.path());
    const auto root = std::filesystem::path(directory.path().toStdWString());
    writeFile(root / "profiles" / profile.toStdWString() / "profile.ini",
              QByteArray("[BSA]\nbsaEnabled=true\nbsaGame=") + QByteArray::number(game) + '\n');
    Profiles::setCurrentProfile(profile);
    const auto mod = root / "mod";
    const auto source = mod / (game == 5 ? "meshes" : "textures") /
                        (game == 5 ? "asset.nif" : "asset.dds");
    writeFile(source, QByteArrayLiteral("source bytes"));
    OptionsCAO options;
    options.bBsaCreateDummies = true;
    options.bBsaCompress = false;
    options.bBsaDeleteSource = true;
    const BSAOptimizer optimizer;
    const std::array roots{mod};
    const auto plan = optimizer.planFinalization(roots, options);
    QCOMPARE(plan.outputs().size(), std::size_t{1});
    QVERIFY(plan.outputs().front().pluginPath.has_value());
    const auto alternatePlugin = mod / pluginName.toStdWString();
    writeFile(alternatePlugin, QByteArrayLiteral("existing loading plugin"));
    const auto originalWriteTime = std::filesystem::last_write_time(alternatePlugin) -
                                   std::chrono::hours(24);
    std::filesystem::last_write_time(alternatePlugin, originalWriteTime);

    cao::run::TemporaryArtifactRegistry artifacts;
    const auto result = optimizer.finalize(plan, artifacts);
    QCOMPARE(result.attempts.size(), std::size_t{1});
    QVERIFY2(result.attempts.front().succeeded(), result.attempts.front().detail.c_str());
    QCOMPARE(result.attempts.front().mutation, cao::execution::MutationState::Committed);
    for (const auto& mutation : result.mutations)
        QVERIFY(mutation.kind != cao::run::ArchiveFinalizationMutationKind::PluginCreation);
    QVERIFY(btu::bsa::read_archive(plan.outputs().front().archivePath).has_value());
    QVERIFY(!std::filesystem::exists(source));
    QVERIFY(!std::filesystem::exists(*plan.outputs().front().pluginPath));
    QCOMPARE(std::filesystem::last_write_time(alternatePlugin), originalWriteTime);
    QVERIFY(artifacts.performSafetyCleanup().empty());
}

void MainOptimizerTests::plannedPluginCollisionRetainsCommittedArchive() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const ScopedCurrentDirectory isolatedWorkingDirectory(directory.path());
    const auto root = std::filesystem::path(directory.path().toStdWString());
    writeFile(root / "profiles" / "SSE" / "profile.ini",
              QByteArrayLiteral("[BSA]\nbsaEnabled=true\nbsaGame=4\n"));
    Profiles::setCurrentProfile("SSE");
    const auto mod = root / "mod";
    const auto source = mod / "textures" / "asset.dds";
    writeFile(source, QByteArrayLiteral("source bytes"));
    OptionsCAO options;
    options.bBsaCreateDummies = true;
    options.bBsaCompress = false;
    options.bBsaDeleteSource = true;
    const BSAOptimizer optimizer;
    const std::array roots{mod};
    const auto plan = optimizer.planFinalization(roots, options);
    QCOMPARE(plan.outputs().size(), std::size_t{1});
    QVERIFY(plan.outputs().front().pluginPath.has_value());
    const auto& output = plan.outputs().front();
    writeFile(*output.pluginPath, QByteArrayLiteral("competing plugin bytes"));

    cao::run::TemporaryArtifactRegistry artifacts;
    const auto result = optimizer.finalize(plan, artifacts);
    QCOMPARE(result.attempts.size(), std::size_t{1});
    QCOMPARE(result.attempts.front().failure,
             std::optional{cao::run::ArchiveFinalizationFailure::PluginCreationFailed});
    QCOMPARE(result.attempts.front().mutation, cao::execution::MutationState::Committed);
    QVERIFY(!result.attempts.front().safeToContinue);
    QVERIFY(result.mutations.empty());
    QVERIFY(btu::bsa::read_archive(output.archivePath).has_value());
    QVERIFY(std::filesystem::exists(source));
    QFile plugin(QString::fromStdWString(output.pluginPath->wstring()));
    QVERIFY(plugin.open(QIODevice::ReadOnly));
    QCOMPARE(plugin.readAll(), QByteArrayLiteral("competing plugin bytes"));
    QVERIFY(artifacts.performSafetyCleanup().empty());
}

void MainOptimizerTests::plannedExactDummyIsReused() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const ScopedCurrentDirectory isolatedWorkingDirectory(directory.path());
    const auto root = std::filesystem::path(directory.path().toStdWString());
    writeFile(root / "profiles" / "SSE" / "profile.ini",
              QByteArrayLiteral("[BSA]\nbsaEnabled=true\nbsaGame=4\n"));
    Profiles::setCurrentProfile("SSE");
    const auto mod = root / "mod";
    const auto source = mod / "textures" / "asset.dds";
    writeFile(source, QByteArrayLiteral("source bytes"));
    OptionsCAO options;
    options.bBsaCreateDummies = true;
    options.bBsaCompress = false;
    options.bBsaDeleteSource = true;
    const BSAOptimizer optimizer;
    const std::array roots{mod};
    const auto plan = optimizer.planFinalization(roots, options);
    QCOMPARE(plan.outputs().size(), std::size_t{1});
    QVERIFY(plan.outputs().front().pluginPath.has_value());
    const auto& output = plan.outputs().front();
    const auto& dummy = btu::bsa::dummy::sse;
    writeFile(*output.pluginPath, QByteArray(reinterpret_cast<const char*>(dummy.data()),
                                             static_cast<int>(dummy.size())));
    const auto originalWriteTime = std::filesystem::last_write_time(*output.pluginPath) -
                                   std::chrono::hours(24);
    std::filesystem::last_write_time(*output.pluginPath, originalWriteTime);

    cao::run::TemporaryArtifactRegistry artifacts;
    const auto result = optimizer.finalize(plan, artifacts);
    QCOMPARE(result.attempts.size(), std::size_t{1});
    QVERIFY(result.attempts.front().succeeded());
    QCOMPARE(result.attempts.front().mutation, cao::execution::MutationState::Committed);
    QVERIFY(btu::bsa::read_archive(output.archivePath).has_value());
    QVERIFY(!std::filesystem::exists(source));
    QCOMPARE(std::filesystem::last_write_time(*output.pluginPath), originalWriteTime);
    QFile plugin(QString::fromStdWString(output.pluginPath->wstring()));
    QVERIFY(plugin.open(QIODevice::ReadOnly));
    QCOMPARE(plugin.readAll(), QByteArray(reinterpret_cast<const char*>(dummy.data()),
                                          static_cast<int>(dummy.size())));
    for (const auto& mutation : result.mutations)
        QVERIFY(mutation.kind != cao::run::ArchiveFinalizationMutationKind::PluginCreation);
    QVERIFY(artifacts.performSafetyCleanup().empty());
}

void MainOptimizerTests::plannedOutputsRecordSharedDummyPublication() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const ScopedCurrentDirectory isolatedWorkingDirectory(directory.path());
    const auto root = std::filesystem::path(directory.path().toStdWString());
    writeFile(root / "profiles" / "SSE" / "profile.ini",
              QByteArrayLiteral("[BSA]\nbsaEnabled=true\nbsaGame=4\n"));
    Profiles::setCurrentProfile("SSE");
    const auto mod = root / "mod";
    writeFile(mod / "meshes" / "asset.nif", QByteArrayLiteral("mesh bytes"));
    writeFile(mod / "textures" / "asset.dds", QByteArrayLiteral("texture bytes"));
    OptionsCAO options;
    options.bBsaCreateDummies = true;
    options.bBsaCompress = false;
    options.bBsaDeleteSource = false;
    options.bBsaMergeTexture = false;
    const BSAOptimizer optimizer;
    const std::array roots{mod};
    const auto plan = optimizer.planFinalization(roots, options);
    QCOMPARE(plan.outputs().size(), std::size_t{2});
    QVERIFY(plan.outputs()[0].pluginPath.has_value());
    QCOMPARE(plan.outputs()[0].pluginPath, plan.outputs()[1].pluginPath);

    cao::run::TemporaryArtifactRegistry artifacts;
    const auto result = optimizer.finalize(plan, artifacts);
    QCOMPARE(result.attempts.size(), std::size_t{2});
    for (const auto& attempt : result.attempts) {
        QVERIFY2(attempt.succeeded(), attempt.detail.c_str());
        QCOMPARE(attempt.mutation, cao::execution::MutationState::Committed);
    }
    QCOMPARE(result.mutations.size(), std::size_t{1});
    const auto& mutation = result.mutations.front();
    QCOMPARE(mutation.modRoot, std::filesystem::canonical(mod));
    QCOMPARE(mutation.path, *plan.outputs()[0].pluginPath);
    QCOMPARE(mutation.kind, cao::run::ArchiveFinalizationMutationKind::PluginCreation);
    QCOMPARE(mutation.mutation, cao::execution::MutationState::Committed);
    const auto& bytes = *btu::bsa::Settings::get(btu::Game::SSE).s_dummy_plugin;
    QFile plugin(QString::fromStdWString(mutation.path.wstring()));
    QVERIFY(plugin.open(QIODevice::ReadOnly));
    QCOMPARE(plugin.readAll(), QByteArray(reinterpret_cast<const char*>(bytes.data()),
                                          static_cast<int>(bytes.size())));

    cao::run::MutableRunEvidence evidence;
    evidence.recordPhase(cao::run::RunPhaseRecord::executed(cao::run::RunPhase::Preparing));
    evidence.recordPhase(cao::run::RunPhaseRecord::executed(
        cao::run::RunPhase::ArchiveFinalization));
    evidence.recordArchiveFinalizationPlan(plan.outputs().size());
    evidence.recordArchiveFinalization(result);
    evidence.recordPhase(cao::run::RunPhaseRecord::executed(cao::run::RunPhase::SafetyCleanup));
    const auto sealed = std::move(evidence).consume();
    QCOMPARE(sealed.mutationSummaries().size(), std::size_t{1});
    QCOMPARE(sealed.mutationSummaries().front().modRoot, std::filesystem::canonical(mod));
    QCOMPARE(sealed.mutationSummaries().front().kind,
             cao::run::MutationKind::ArchiveFinalization);
    QCOMPARE(sealed.mutationSummaries().front().committed, std::size_t{3});
    QCOMPARE(sealed.mutationSummaries().front().partialOrUnknown, std::size_t{0});
    QVERIFY(artifacts.performSafetyCleanup().empty());
}

void MainOptimizerTests::plannedLoadingPluginDisappearsBeforeCommit() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const ScopedCurrentDirectory isolatedWorkingDirectory(directory.path());
    const auto root = std::filesystem::path(directory.path().toStdWString());
    writeFile(root / "profiles" / "SSE" / "profile.ini",
              QByteArrayLiteral("[BSA]\nbsaEnabled=true\nbsaGame=4\n"));
    Profiles::setCurrentProfile("SSE");
    const auto mod = root / "mod";
    const auto source = mod / "textures" / "asset.dds";
    writeFile(source, QByteArrayLiteral("source bytes"));
    const auto earlierPlugin = mod / "mod.esm";
    writeFile(earlierPlugin, QByteArrayLiteral("existing loading plugin"));
    OptionsCAO options;
    options.bBsaCreateDummies = true;
    options.bBsaCompress = false;
    options.bBsaDeleteSource = true;
    const BSAOptimizer optimizer;
    const std::array roots{mod};
    const auto plan = optimizer.planFinalization(roots, options);
    QCOMPARE(plan.outputs().size(), std::size_t{1});
    QVERIFY(!plan.outputs().front().pluginPath.has_value());
    QVERIFY(std::filesystem::remove(earlierPlugin));

    cao::run::TemporaryArtifactRegistry artifacts;
    const auto result = optimizer.finalize(plan, artifacts);
    QCOMPARE(result.attempts.size(), std::size_t{1});
    QVERIFY2(result.attempts.front().succeeded(), result.attempts.front().detail.c_str());
    QCOMPARE(result.attempts.front().mutation, cao::execution::MutationState::Committed);
    QVERIFY(!std::filesystem::exists(source));
    std::size_t pluginCreations = 0;
    for (const auto& mutation : result.mutations) {
        if (mutation.kind != cao::run::ArchiveFinalizationMutationKind::PluginCreation) continue;
        ++pluginCreations;
        QCOMPARE(mutation.modRoot, std::filesystem::canonical(mod));
        QCOMPARE(mutation.path, mod / "mod.esp");
        QCOMPARE(mutation.mutation, cao::execution::MutationState::Committed);
    }
    QCOMPARE(pluginCreations, std::size_t{1});
    QVERIFY(artifacts.performSafetyCleanup().empty());
}

void MainOptimizerTests::disappearingLoadingPluginCapacityIsReserved_data() {
    QTest::addColumn<bool>("shortageAtPreflight");
    QTest::newRow("phase-preflight") << true;
    QTest::newRow("attempt-recheck") << false;
}

void MainOptimizerTests::disappearingLoadingPluginCapacityIsReserved() {
    QFETCH(bool, shortageAtPreflight);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const ScopedCurrentDirectory isolatedWorkingDirectory(directory.path());
    const auto root = std::filesystem::path(directory.path().toStdWString());
    writeFile(root / "profiles" / "SSE" / "profile.ini",
              QByteArrayLiteral("[BSA]\nbsaEnabled=true\nbsaGame=4\n"));
    Profiles::setCurrentProfile("SSE");
    const auto mod = root / "mod";
    const auto source = mod / "textures" / "asset.dds";
    writeFile(source, QByteArrayLiteral("source bytes"));
    const auto earlierPlugin = mod / "mod.esm";
    writeFile(earlierPlugin, QByteArrayLiteral("existing loading plugin"));
    OptionsCAO options;
    options.bBsaCreateDummies = false;
    options.bBsaCompress = false;
    options.bBsaDeleteSource = true;
    const BSAOptimizer optimizer;
    const std::array roots{mod};
    // The no-dummy plan supplies an Archive-only allowance independent of the fallback branch.
    const auto archiveOnlyPlan = optimizer.planFinalization(roots, options);
    QCOMPARE(archiveOnlyPlan.outputs().size(), std::size_t{1});
    const auto archiveOnlyCapacity = archiveOnlyPlan.outputs().front().estimatedCapacityBytes;
    options.bBsaCreateDummies = true;
    const auto plan = optimizer.planFinalization(roots, options);
    QCOMPARE(plan.outputs().size(), std::size_t{1});
    const auto& output = plan.outputs().front();
    QCOMPARE(output.archivePath, archiveOnlyPlan.outputs().front().archivePath);
    QVERIFY(!output.pluginPath.has_value());
    QVERIFY(std::filesystem::remove(earlierPlugin));

    std::size_t capacityQueries = 0;
    const auto capacity = [&](const std::filesystem::path&) -> std::optional<std::uintmax_t> {
        ++capacityQueries;
        if (!shortageAtPreflight && capacityQueries == 1)
            return std::numeric_limits<std::uintmax_t>::max();
        return archiveOnlyCapacity;
    };
    cao::run::TemporaryArtifactRegistry artifacts;
    const auto result = optimizer.finalize(plan, artifacts, {}, {}, capacity);
    QCOMPARE(result.attempts.size(), std::size_t{1});
    QCOMPARE(result.attempts.front().failure,
             std::optional{cao::run::ArchiveFinalizationFailure::InsufficientCapacity});
    QCOMPARE(result.attempts.front().mutation, cao::execution::MutationState::None);
    QVERIFY(result.mutations.empty());
    QVERIFY(!std::filesystem::exists(output.archivePath));
    QVERIFY(!std::filesystem::exists(mod / "mod.esp"));
    QVERIFY(std::filesystem::exists(source));
    QVERIFY(artifacts.performSafetyCleanup().empty());
}

void MainOptimizerTests::linkedLoadingPluginRemainsUsable_data() {
    QTest::addColumn<QString>("linkedName");
    QTest::newRow("alternate-name") << QStringLiteral("mod.esm");
    QTest::newRow("dummy-destination") << QStringLiteral("mod.esp");
}

void MainOptimizerTests::linkedLoadingPluginRemainsUsable() {
    QFETCH(QString, linkedName);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const ScopedCurrentDirectory isolatedWorkingDirectory(directory.path());
    const auto root = std::filesystem::path(directory.path().toStdWString());
    writeFile(root / "profiles" / "SSE" / "profile.ini",
              QByteArrayLiteral("[BSA]\nbsaEnabled=true\nbsaGame=4\n"));
    Profiles::setCurrentProfile("SSE");
    const auto mod = root / "mod";
    const auto source = mod / "textures" / "asset.dds";
    writeFile(source, QByteArrayLiteral("source bytes"));
    const auto target = root / "real-plugin.bin";
    writeFile(target, QByteArrayLiteral("real loading plugin bytes"));
    const auto link = mod / linkedName.toStdWString();
    std::error_code linkError;
    std::filesystem::create_symlink(target, link, linkError);
    if (linkError) QSKIP("File symlink creation is unavailable on this host");

    OptionsCAO options;
    options.bBsaCreateDummies = true;
    options.bBsaCompress = false;
    options.bBsaDeleteSource = true;
    const BSAOptimizer optimizer;
    const std::array roots{mod};
    const auto plan = optimizer.planFinalization(roots, options);
    QCOMPARE(plan.outputs().size(), std::size_t{1});
    const auto& output = plan.outputs().front();
    QVERIFY(!output.pluginPath.has_value());

    cao::run::TemporaryArtifactRegistry artifacts;
    const auto result = optimizer.finalize(plan, artifacts);
    QCOMPARE(result.attempts.size(), std::size_t{1});
    QVERIFY2(result.attempts.front().succeeded(), result.attempts.front().detail.c_str());
    QVERIFY(btu::bsa::read_archive(output.archivePath).has_value());
    QVERIFY(!std::filesystem::exists(source));
    QVERIFY(std::filesystem::is_symlink(std::filesystem::symlink_status(link)));
#ifdef _WIN32
    std::size_t createdPlugins = 0;
    std::filesystem::path fallback;
    for (const auto& mutation : result.mutations) {
        if (mutation.kind != cao::run::ArchiveFinalizationMutationKind::PluginCreation) continue;
        ++createdPlugins;
        fallback = mutation.path;
        QVERIFY(fallback != link);
        QVERIFY(std::find(output.loadingPluginPaths.begin(), output.loadingPluginPaths.end(),
                          fallback) != output.loadingPluginPaths.end());
        QVERIFY(std::filesystem::is_regular_file(std::filesystem::symlink_status(fallback)));
    }
    QCOMPARE(createdPlugins, std::size_t{1});
#else
    if (link != mod / "mod.esp") QVERIFY(!std::filesystem::exists(mod / "mod.esp"));
    for (const auto& mutation : result.mutations)
        QVERIFY(mutation.kind != cao::run::ArchiveFinalizationMutationKind::PluginCreation);
#endif
    QVERIFY(artifacts.performSafetyCleanup().empty());
    QVERIFY(std::filesystem::remove(link));
#ifdef _WIN32
    QVERIFY(std::filesystem::exists(fallback));
#endif
}

void MainOptimizerTests::linkedLoadingPluginsWithoutFallbackRetainSources() {
#ifdef _WIN32
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const ScopedCurrentDirectory isolatedWorkingDirectory(directory.path());
    const auto root = std::filesystem::path(directory.path().toStdWString());
    writeFile(root / "profiles" / "SSE" / "profile.ini",
              QByteArrayLiteral("[BSA]\nbsaEnabled=true\nbsaGame=4\n"));
    Profiles::setCurrentProfile("SSE");
    const auto mod = root / "mod";
    const auto source = mod / "textures" / "asset.dds";
    writeFile(source, QByteArrayLiteral("source bytes"));
    const auto target = root / "real-plugin.bin";
    writeFile(target, QByteArrayLiteral("linked loading plugin"));

    OptionsCAO options;
    options.bBsaCreateDummies = true;
    options.bBsaCompress = false;
    options.bBsaDeleteSource = true;
    const BSAOptimizer optimizer;
    const std::array roots{mod};
    const auto plan = optimizer.planFinalization(roots, options);
    QCOMPARE(plan.outputs().size(), std::size_t{1});
    const auto& output = plan.outputs().front();
    for (const auto& path : output.loadingPluginPaths) {
        std::error_code linkError;
        std::filesystem::create_symlink(target, path, linkError);
        if (linkError) QSKIP("File symlink creation is unavailable on this host");
    }

    cao::run::TemporaryArtifactRegistry artifacts;
    const auto result = optimizer.finalize(plan, artifacts);
    QCOMPARE(result.attempts.size(), std::size_t{1});
    QCOMPARE(result.attempts.front().failure,
             std::optional{cao::run::ArchiveFinalizationFailure::PluginCreationFailed});
    QVERIFY(!result.attempts.front().succeeded());
    QVERIFY(std::filesystem::exists(source));
    QVERIFY(btu::bsa::read_archive(output.archivePath).has_value());
    QVERIFY(artifacts.performSafetyCleanup().empty());
#else
    QSKIP("Windows reparse points require an ordinary Loading Plugin fallback.");
#endif
}

void MainOptimizerTests::loadingPluginRemainsPinnedThroughSourceCleanup_data() {
    QTest::addColumn<int>("pluginKind");
    QTest::newRow("existing-plugin") << 0;
    QTest::newRow("published-fallback") << 1;
}

void MainOptimizerTests::loadingPluginRemainsPinnedThroughSourceCleanup() {
#ifdef _WIN32
    QFETCH(int, pluginKind);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const ScopedCurrentDirectory isolatedWorkingDirectory(directory.path());
    const auto root = std::filesystem::path(directory.path().toStdWString());
    writeFile(root / "profiles" / "SSE" / "profile.ini",
              QByteArrayLiteral("[BSA]\nbsaEnabled=true\nbsaGame=4\n"));
    Profiles::setCurrentProfile("SSE");
    const auto mod = root / "mod";
    // A long cleanup makes the interval after plugin selection observable without a test hook.
    for (int index = 0; index < 1024; ++index)
        writeFile(mod / "textures" / ("asset-" + std::to_string(index) + ".dds"),
                  QByteArrayLiteral("source bytes"));

    const auto plugin = mod / (pluginKind == 1 ? "mod.esp" : "mod.esm");
    if (pluginKind == 0)
        writeFile(plugin, QByteArrayLiteral("existing loading plugin"));

    OptionsCAO options;
    options.bBsaCreateDummies = true;
    options.bBsaCompress = false;
    options.bBsaDeleteSource = true;
    const BSAOptimizer optimizer;
    const std::array roots{mod};
    const auto plan = optimizer.planFinalization(roots, options);
    QCOMPARE(plan.outputs().size(), std::size_t{1});
    const auto& output = plan.outputs().front();
    QVERIFY(output.sources.size() == std::size_t{1024});

    bool sawCleanupGap = false;
    bool removedPlugin = false;
    std::jthread remover([&](const std::stop_token stop) {
        while (!stop.stop_requested()) {
            std::error_code firstError;
            std::error_code lastError;
            if (!std::filesystem::exists(output.sources.front(), firstError) && !firstError &&
                std::filesystem::exists(output.sources.back(), lastError) && !lastError) {
                sawCleanupGap = true;
                removedPlugin = DeleteFileW(plugin.c_str()) != 0;
                break;
            }
            std::this_thread::yield();
        }
    });
    cao::run::TemporaryArtifactRegistry artifacts;
    const auto result = optimizer.finalize(plan, artifacts);
    remover.request_stop();
    remover.join();

    QCOMPARE(result.attempts.size(), std::size_t{1});
    QVERIFY2(result.attempts.front().succeeded(), result.attempts.front().detail.c_str());
    QVERIFY2(sawCleanupGap, "The test did not observe source cleanup in progress.");
    QVERIFY(!removedPlugin);
    QVERIFY(btu::bsa::read_archive(output.archivePath).has_value());
    QVERIFY(!std::filesystem::exists(output.sources.back()));
    QVERIFY(std::filesystem::is_regular_file(plugin));
    QVERIFY(DeleteFileW(plugin.c_str()));
    QVERIFY(artifacts.performSafetyCleanup().empty());
#else
    QSKIP("Windows file sharing pins Loading Plugins during cleanup.");
#endif
}

void MainOptimizerTests::cancellationPreservesCommittedArchiveLoadingPlugin() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const ScopedCurrentDirectory isolatedWorkingDirectory(directory.path());
    const auto root = std::filesystem::path(directory.path().toStdWString());
    writeFile(root / "profiles" / "SSE" / "profile.ini",
              QByteArrayLiteral("[BSA]\nbsaEnabled=true\nbsaGame=4\n"));
    Profiles::setCurrentProfile("SSE");
    const std::array roots{root / "mod-a", root / "mod-b"};
    for (const auto& mod : roots)
        writeFile(mod / "textures" / "asset.dds", QByteArrayLiteral("source bytes"));
    OptionsCAO options;
    options.bBsaCreateDummies = true;
    options.bBsaCompress = false;
    options.bBsaDeleteSource = true;
    const BSAOptimizer optimizer;
    const auto plan = optimizer.planFinalization(roots, options);
    QCOMPARE(plan.outputs().size(), std::size_t{2});
    QVERIFY(!std::filesystem::exists(roots[0] / "mod-a.esp"));
    QVERIFY(!std::filesystem::exists(roots[1] / "mod-b.esp"));
    cao::run::TemporaryArtifactRegistry artifacts;
    std::stop_source stop;
    const auto result = optimizer.finalize(plan, artifacts, stop.get_token(),
        [&](const cao::run::ArchiveFinalizationProgress& progress) {
            if (progress.completed == 1) stop.request_stop();
        });
    QCOMPARE(result.attempts.size(), std::size_t{1});
    QVERIFY(result.cancelled);
    QVERIFY(result.attempts.front().succeeded());
    QVERIFY(btu::bsa::read_archive(plan.outputs()[0].archivePath).has_value());
    QVERIFY(!std::filesystem::exists(roots[0] / "textures" / "asset.dds"));
    QVERIFY(std::filesystem::exists(roots[0] / "mod-a.esp"));
    QVERIFY(std::filesystem::exists(roots[1] / "textures" / "asset.dds"));
    QVERIFY(!std::filesystem::exists(plan.outputs()[1].archivePath));
    QVERIFY(!std::filesystem::exists(roots[1] / "mod-b.esp"));
    QVERIFY(artifacts.performSafetyCleanup().empty());
}

void MainOptimizerTests::committedArchiveRetainsLockedSource_data() {
    QTest::addColumn<bool>("denyReads");
    QTest::newRow("readable-source-continues") << false;
    QTest::newRow("unreadable-source-stops") << true;
}

void MainOptimizerTests::committedArchiveRetainsLockedSource() {
#ifdef _WIN32
    QFETCH(bool, denyReads);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const ScopedCurrentDirectory isolatedWorkingDirectory(directory.path());
    const auto root = std::filesystem::path(directory.path().toStdWString());
    writeFile(root / "profiles" / "SSE" / "profile.ini",
              QByteArrayLiteral("[BSA]\nbsaEnabled=true\nbsaGame=4\n"));
    Profiles::setCurrentProfile("SSE");
    const auto mod = root / "mod-a";
    const auto source = mod / "textures" / "asset.dds";
    writeFile(source, QByteArrayLiteral("retained source bytes"));
    const auto laterSource = root / "mod-b" / "textures" / "later.dds";
    writeFile(laterSource, QByteArrayLiteral("later source bytes"));
    const auto emptyDirectory = mod / "empty" / "nested";
    std::filesystem::create_directories(emptyDirectory);
    OptionsCAO options;
    options.bBsaCreateDummies = false;
    options.bBsaCompress = false;
    options.bBsaDeleteSource = true;
    const BSAOptimizer optimizer;
    const std::array roots{mod, root / "mod-b"};
    const auto plan = optimizer.planFinalization(roots, options);
    QCOMPARE(plan.outputs().size(), std::size_t{2});
    // Permit packing and recovery verification reads but deny source deletion after commit.
    const auto locked = CreateFileW(source.c_str(), GENERIC_READ, FILE_SHARE_READ, nullptr,
                                   OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    QVERIFY(locked != INVALID_HANDLE_VALUE);
    // Windows byte locks permit the Archive writer's memory mapping, but reject ordinary
    // retained-source reads. This distinguishes existence from actual recovery evidence.
    OVERLAPPED range{};
    const bool rangeLocked = !denyReads ||
        LockFileEx(locked, LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
                   0, MAXDWORD, MAXDWORD, &range) != 0;
    if (!rangeLocked) CloseHandle(locked);
    QVERIFY(rangeLocked);
    cao::run::TemporaryArtifactRegistry artifacts;
    const auto result = optimizer.finalize(plan, artifacts);
    QVERIFY(CloseHandle(locked));
    QCOMPARE(result.attempts.size(), denyReads ? std::size_t{1} : std::size_t{2});
    QVERIFY(!result.attempts.front().succeeded());
    QCOMPARE(result.attempts.front().failure,
             cao::run::ArchiveFinalizationFailure::SourceCleanupFailed);
    QCOMPARE(result.attempts.front().mutation, cao::execution::MutationState::Committed);
    QCOMPARE(result.safeToContinue, !denyReads);
    QVERIFY(btu::bsa::read_archive(plan.outputs().front().archivePath).has_value());
    QCOMPARE(std::filesystem::exists(laterSource), denyReads);
    QCOMPARE(std::filesystem::exists(plan.outputs()[1].archivePath), !denyReads);
    QCOMPARE(std::filesystem::exists(emptyDirectory), denyReads);
    if (!denyReads) {
        QVERIFY(result.attempts.back().succeeded());
        QVERIFY(btu::bsa::read_archive(plan.outputs()[1].archivePath).has_value());
    }
    QFile retained(QString::fromStdWString(source.wstring()));
    QVERIFY(retained.open(QIODevice::ReadOnly));
    QCOMPARE(retained.readAll(), QByteArrayLiteral("retained source bytes"));
    QVERIFY(artifacts.performSafetyCleanup().empty());
#else
    QSKIP("Windows sharing modes provide a deterministic source cleanup failure.");
#endif
}

void MainOptimizerTests::emptyDirectoryCleanupPreservesStaging() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const QDir root(directory.path());
    QVERIFY(root.mkpath(".CAO-STAGING/run-1"));
    QVERIFY(root.mkpath("ordinary/empty"));
    QVERIFY(root.mkpath("empty-mod/nested"));
    FilesystemOperations::deleteEmptyDirectories(root.filePath("empty-mod"));
    QVERIFY(root.exists("empty-mod"));
    QVERIFY(!root.exists("empty-mod/nested"));

    FilesystemOperations::deleteEmptyDirectories(directory.path());

    QVERIFY(root.exists(".CAO-STAGING/run-1"));
    QVERIFY(!root.exists("ordinary"));
}

void MainOptimizerTests::nestedTextureUsesSelectedModRoot() {
    for (const auto mode : {OptionsCAO::SingleMod, OptionsCAO::SeveralMods}) {
        QTemporaryDir directory;
        QVERIFY(directory.isValid());
        const ScopedCurrentDirectory isolatedWorkingDirectory(directory.path());
        const auto selection = std::filesystem::path(directory.path().toStdWString());
        const auto modRoot = mode == OptionsCAO::SingleMod ? selection : selection / "mod-a";
        const auto texture = modRoot / "textures" / "armor" / "nested.tga";
        writeFile(texture, QByteArray::fromHex("0000020000000000000000000100010018000000ff"));
        writeFile(modRoot / ".cao-staging" / "unknown", QByteArrayLiteral("preserve"));
        OptionsCAO options;
        options.mode = mode;
        options.userPath = directory.path();
        MainOptimizer optimizer(options);

        const auto result = optimizer.process(routeMaintenanceOnly(texture));

        QVERIFY(!result.succeeded());
        QVERIFY(std::filesystem::exists(texture));
        QVERIFY(!std::filesystem::exists(modRoot / "textures" / "armor" / "nested.dds"));
        QVERIFY(std::filesystem::exists(modRoot / ".cao-staging" / "unknown"));
        QVERIFY(!std::filesystem::exists(modRoot / "textures" / "armor" / ".cao-staging"));
    }
}

void MainOptimizerTests::loadFailuresQuarantineMalformedAssets()
{
    QVERIFY(_temporaryDirectory.isValid());

    const ScopedCurrentDirectory isolatedWorkingDirectory(_temporaryDirectory.path());

    const auto root = std::filesystem::path(_temporaryDirectory.path().toStdWString());
    writeFile(root / "profiles" / "SSE" / "profile.ini", QByteArrayLiteral("[Textures]\n"));
    writeFile(root / "profiles" / "SSE" / "customHeadparts.txt",
              QByteArrayLiteral("test-headpart.nif\n"));
    OptionsCAO options;
    options.mode = OptionsCAO::SingleMod;
    options.userPath = _temporaryDirectory.path();
    options.iMeshesOptimizationLevel = 1;
    MainOptimizer optimizer(options);

    const std::array paths{root / "apply-malformed.dds",
                           root / "apply-malformed.tga",
                           root / "apply-malformed.nif"};
    for (const auto &path : paths) {
        writeFile(path, QByteArrayLiteral("malformed"));

        const auto result = optimizer.process(routeAsset(path));

        QVERIFY(!result.succeeded());
        QCOMPARE(result.failure().value(), AssetExecutionFailure::LoadFailed);
        QCOMPARE(result.mutationState(), cao::execution::MutationState::Committed);
        QVERIFY(result.safeToContinue());
        QVERIFY(!std::filesystem::exists(path));
        QVERIFY(std::filesystem::is_regular_file(path.wstring() + L".caobad"));
    }
}

void MainOptimizerTests::loadFailureUsesCollisionSafeQuarantineName()
{
    QVERIFY(_temporaryDirectory.isValid());

    const ScopedCurrentDirectory isolatedWorkingDirectory(_temporaryDirectory.path());

    const auto root = std::filesystem::path(_temporaryDirectory.path().toStdWString());
    const auto malformedTexture = root / "collision-malformed.dds";
    const auto staleQuarantine = root / "collision-malformed.dds.caobad";
    writeFile(malformedTexture, QByteArrayLiteral("new malformed input"));
    writeFile(staleQuarantine, QByteArrayLiteral("previous malformed input"));

    OptionsCAO options;
    options.mode = OptionsCAO::SingleMod;
    options.userPath = _temporaryDirectory.path();
    MainOptimizer optimizer(options);

    const auto result = optimizer.process(routeAsset(malformedTexture));

    QVERIFY(!result.succeeded());
    QCOMPARE(result.failure().value(), AssetExecutionFailure::LoadFailed);
    QVERIFY(!std::filesystem::exists(malformedTexture));
    QVERIFY(std::filesystem::is_regular_file(staleQuarantine));
    QVERIFY(std::filesystem::is_regular_file(root / "collision-malformed.dds.caobad.1"));
}

void MainOptimizerTests::failedQuarantineMakesLoadFailureUnsafe()
{
#ifndef _WIN32
    QSKIP("Windows sharing modes provide a deterministic quarantine rename failure.");
#else
    QVERIFY(_temporaryDirectory.isValid());

    const ScopedCurrentDirectory isolatedWorkingDirectory(_temporaryDirectory.path());
    const auto root = std::filesystem::path(_temporaryDirectory.path().toStdWString());
    const auto malformedTexture = root / "locked-malformed.dds";
    writeFile(malformedTexture, QByteArrayLiteral("malformed"));

    // The optimizer can read the malformed input, while Windows denies its quarantine rename.
    const auto nativeHandle = CreateFileW(malformedTexture.c_str(), GENERIC_READ,
                                          FILE_SHARE_READ, nullptr, OPEN_EXISTING,
                                          FILE_ATTRIBUTE_NORMAL, nullptr);
    QVERIFY(nativeHandle != INVALID_HANDLE_VALUE);
    const std::unique_ptr<void, decltype(&CloseHandle)> handle(nativeHandle, &CloseHandle);

    OptionsCAO options;
    options.mode = OptionsCAO::SingleMod;
    options.userPath = _temporaryDirectory.path();
    MainOptimizer optimizer(options);

    const auto result = optimizer.process(routeAsset(malformedTexture));

    QCOMPARE(result.failure(), AssetExecutionFailure::LoadFailed);
    QVERIFY(!result.safeToContinue());
    QCOMPARE(result.mutationState(), cao::execution::MutationState::None);
    QVERIFY(std::filesystem::is_regular_file(malformedTexture));
#endif
}

void MainOptimizerTests::dryRunLoadFailureDoesNotQuarantine()
{
    QVERIFY(_temporaryDirectory.isValid());

    const auto root = std::filesystem::path(_temporaryDirectory.path().toStdWString());
    const auto malformedTexture = root / "dry-run-malformed.dds";
    writeFile(malformedTexture, QByteArrayLiteral("malformed"));

    OptionsCAO options;
    options.mode = OptionsCAO::SingleMod;
    options.userPath = _temporaryDirectory.path();
    options.bDryRun = true;
    MainOptimizer optimizer(options);

    const auto result = optimizer.process(routeAsset(malformedTexture, ExecutionMode::DryRun));

    QVERIFY(!result.succeeded());
    QCOMPARE(result.failure().value(), AssetExecutionFailure::LoadFailed);
    QVERIFY(std::filesystem::is_regular_file(malformedTexture));
    QVERIFY(!std::filesystem::exists(malformedTexture.wstring() + L".caobad"));
}

void MainOptimizerTests::failedConversionSuppressesMeshReferenceMaintenance()
{
    QVERIFY(_temporaryDirectory.isValid());

    const ScopedCurrentDirectory isolatedWorkingDirectory(_temporaryDirectory.path());

    const auto root = std::filesystem::path(_temporaryDirectory.path().toStdWString());
    const auto malformedTexture = root / "textures" / "armor" / "suppressed.tga";
    const auto mesh = root / "suppressed-reference.nif";
    writeFile(malformedTexture, QByteArrayLiteral("malformed"));
    writeMeshWithTexture(mesh, "textures\\armor\\suppressed.tga");

    OptionsCAO options;
    options.mode = OptionsCAO::SingleMod;
    options.userPath = _temporaryDirectory.path();
    MainOptimizer optimizer(options);

    // Asset Run always completes the Texture target first, so the conversion failure is already
    // definitive when the Mesh is executed.
    const auto conversion = optimizer.process(routeMaintenanceOnly(malformedTexture));
    QVERIFY(!conversion.succeeded());
    QCOMPARE(conversion.failure().value(), AssetExecutionFailure::LoadFailed);

    const auto maintenance = optimizer.process(routeMaintenanceOnly(mesh));

    // The Mesh itself is intact, so the run continues; only the rewrite that would point at the
    // DDS this very conversion failed to produce is withheld.
    QVERIFY(maintenance.succeeded());
    QCOMPARE(savedTextureSlot(mesh), std::string("textures\\armor\\suppressed.tga"));
}

void MainOptimizerTests::committedConversionWithRetainedSourceMaintainsMeshReferences() {
#ifndef _WIN32
    QSKIP("The retained-source fixture uses Windows delete-sharing semantics.");
#else
    QVERIFY(_temporaryDirectory.isValid());
    const ScopedCurrentDirectory isolatedWorkingDirectory(_temporaryDirectory.path());
    const auto root = std::filesystem::path(_temporaryDirectory.path().toStdWString());
    const auto texture = root / "textures" / "retained.tga";
    const auto mesh = root / "retained-reference.nif";
    // A one-pixel uncompressed true-color TGA exercises the real conversion and save adapter.
    writeFile(texture, QByteArray::fromHex("0000020000000000000000000100010018000000ff"));
    writeMeshWithTexture(mesh, "textures\\retained.tga");
    const auto nativeHandle =
        CreateFileW(texture.c_str(), GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE, nullptr,
                    OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    QVERIFY(nativeHandle != INVALID_HANDLE_VALUE);
    const std::unique_ptr<void, decltype(&CloseHandle)> handle(nativeHandle, &CloseHandle);
    OptionsCAO options;
    options.mode = OptionsCAO::SingleMod;
    options.userPath = _temporaryDirectory.path();
    MainOptimizer optimizer(options);
    const auto conversion = optimizer.process(routeMaintenanceOnly(texture));
    QVERIFY(!conversion.succeeded());
    QCOMPARE(conversion.failure().value(), AssetExecutionFailure::SourceRemovalFailed);
    QVERIFY(!conversion.serviceDetail().empty());
    QCOMPARE(conversion.mutationState(), cao::execution::MutationState::Committed);
    QVERIFY(conversion.safeToContinue());
    QVERIFY(std::filesystem::is_regular_file(root / "textures" / "retained.dds"));
    QVERIFY(std::filesystem::is_regular_file(texture));
    QVERIFY(optimizer.process(routeMaintenanceOnly(mesh)).succeeded());
    QCOMPARE(savedTextureSlot(mesh), std::string("textures\\retained.dds"));
#endif
}

void MainOptimizerTests::failedConversionKeepsUnrelatedMeshReferences()
{
    QVERIFY(_temporaryDirectory.isValid());

    const ScopedCurrentDirectory isolatedWorkingDirectory(_temporaryDirectory.path());

    const auto root = std::filesystem::path(_temporaryDirectory.path().toStdWString());
    const auto malformedTexture = root / "textures" / "armor" / "unrelated-failure.tga";
    const auto mesh = root / "unrelated-reference.nif";
    writeFile(malformedTexture, QByteArrayLiteral("malformed"));
    writeMeshWithTexture(mesh, "textures\\armor\\converted.tga");

    OptionsCAO options;
    options.mode = OptionsCAO::SingleMod;
    options.userPath = _temporaryDirectory.path();
    MainOptimizer optimizer(options);

    const auto conversion = optimizer.process(routeMaintenanceOnly(malformedTexture));
    QVERIFY(!conversion.succeeded());

    const auto maintenance = optimizer.process(routeMaintenanceOnly(mesh));

    // converted.tga is deleted once its own DDS replacement is saved, so a run-wide failure bit
    // would leave this Mesh naming a file the run itself removed.
    QVERIFY(maintenance.succeeded());
    QCOMPARE(savedTextureSlot(mesh), std::string("textures\\armor\\converted.dds"));
}

void MainOptimizerTests::failedConversionInAnotherModRootKeepsMeshReferences()
{
    QVERIFY(_temporaryDirectory.isValid());

    const ScopedCurrentDirectory isolatedWorkingDirectory(_temporaryDirectory.path());

    // Several Mods scans sibling Mod Roots in one pass, and the same Texture path routinely
    // appears in more than one of them.
    const auto root = std::filesystem::path(_temporaryDirectory.path().toStdWString());
    const auto failedTexture = root / "mod-a" / "textures" / "armor" / "shared.tga";
    const auto mesh = root / "mod-b" / "meshes" / "armor" / "shared-reference.nif";
    writeFile(failedTexture, QByteArrayLiteral("malformed"));
    writeMeshWithTexture(mesh, "textures\\armor\\shared.tga");

    OptionsCAO options;
    options.mode = OptionsCAO::SeveralMods;
    options.userPath = _temporaryDirectory.path();
    MainOptimizer optimizer(options);

    const auto conversion = optimizer.process(routeMaintenanceOnly(failedTexture));
    QVERIFY(!conversion.succeeded());

    const auto maintenance = optimizer.process(routeMaintenanceOnly(mesh));

    // mod-b's own copy of the Texture converted and its TGA source was deleted with it, so
    // withholding this rewrite on the strength of mod-a's failure would leave the Mesh naming a
    // file the run itself removed.
    QVERIFY(maintenance.succeeded());
    QCOMPARE(savedTextureSlot(mesh), std::string("textures\\armor\\shared.dds"));
}

void MainOptimizerTests::successfulRunStillMaintainsMeshReferences()
{
    QVERIFY(_temporaryDirectory.isValid());

    const ScopedCurrentDirectory isolatedWorkingDirectory(_temporaryDirectory.path());

    const auto root = std::filesystem::path(_temporaryDirectory.path().toStdWString());
    const auto mesh = root / "maintained-reference.nif";
    writeMeshWithTexture(mesh, "textures\\armor\\body.tga");

    OptionsCAO options;
    options.mode = OptionsCAO::SingleMod;
    options.userPath = _temporaryDirectory.path();
    MainOptimizer optimizer(options);

    const auto maintenance = optimizer.process(routeMaintenanceOnly(mesh));

    QVERIFY(maintenance.succeeded());
    QCOMPARE(maintenance.mutationState(), cao::execution::MutationState::Committed);
    QVERIFY(maintenance.safeToContinue());
    QCOMPARE(savedTextureSlot(mesh), std::string("textures\\armor\\body.dds"));
}

void MainOptimizerTests::pluginListingObservesCancellation()
{
    QVERIFY(_temporaryDirectory.isValid());
    const auto root = std::filesystem::path(_temporaryDirectory.path().toStdWString());
    writeFile(root / "plugins" / "fixture.esp", QByteArrayLiteral("fixture"));
    QDirIterator entries(_temporaryDirectory.path(), QDirIterator::Subdirectories);
    std::stop_source stop;
    stop.request_stop();

    bool cancelled = false;
    try {
        static_cast<void>(FilesystemOperations::listPlugins(entries, stop.get_token()));
    } catch (const cao::run::AssetInitializationCancelled&) {
        cancelled = true;
    }
    QVERIFY(cancelled);
}

void MainOptimizerTests::optimizerInitializationObservesCancellation()
{
    QVERIFY(_temporaryDirectory.isValid());
    const auto root = std::filesystem::path(_temporaryDirectory.path().toStdWString());
    const auto plugin = root / "mod-a" / "fixture.esp";
    writeFile(plugin, QByteArrayLiteral("fixture"));
    OptionsCAO options;
    options.mode = OptionsCAO::SeveralMods;
    options.userPath = _temporaryDirectory.path();
    std::stop_source stop;
    stop.request_stop();

    bool cancelled = false;
    try {
        MainOptimizer optimizer(options, OptimizerProfileSnapshot::capture(), stop.get_token());
    } catch (const cao::run::AssetInitializationCancelled&) {
        cancelled = true;
    }
    QVERIFY(cancelled);
    QVERIFY(std::filesystem::is_regular_file(plugin));
}

QTEST_APPLESS_MAIN(MainOptimizerTests)

#include "MainOptimizerTests.moc"
