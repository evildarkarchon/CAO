#include "MainOptimizer.h"
#include "BsaOptimizer.h"
#include "FilesystemOperations.h"
#include "TexturesOptimizer.h"
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
#include <optional>
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

/// Writes an uncompressed square B8G8R8A8 DDS with the requested number of mip levels.
void writeDds(const std::filesystem::path &path, const size_t size, const size_t mipLevels)
{
    DirectX::ScratchImage image;
    QVERIFY(SUCCEEDED(image.Initialize2D(DXGI_FORMAT_B8G8R8A8_UNORM, size, size, 1, mipLevels)));
    std::fill_n(image.GetPixels(), image.GetPixelsSize(), uint8_t{0x80});
    QVERIFY(SUCCEEDED(DirectX::SaveToDDSFile(image.GetImages(), image.GetImageCount(),
                                             image.GetMetadata(), DirectX::DDS_FLAGS_NONE,
                                             path.c_str())));
}
}

class MainOptimizerTests final : public QObject
{
    Q_OBJECT

private slots:
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

 /// Keeps a replacement after the packed file pin is released for guarded cleanup.
 void packedSourcePinPreservesReplacement();

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

 /// Covers already-optimal, mipmap, and resize-target Textures.
 void textureDryRunMatchesApply_data();
 /// Verifies a Dry Run reports a Texture as changed exactly when Apply would modify it.
 void textureDryRunMatchesApply();

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

 /// Lists plugins by the Profile game's extensions, ignoring case, files, and directories.
 void pluginListingUsesProfileExtensions();

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

void MainOptimizerTests::textureDryRunMatchesApply_data()
{
    QTest::addColumn<int>("size");
    QTest::addColumn<int>("mipLevels");
    QTest::addColumn<int>("targetSize");
    QTest::addColumn<bool>("expectedChange");
    // A 4x4 Texture's full mip chain is 4x4, 2x2, 1x1. A target size of 0 requests no resize.
    QTest::newRow("already-optimal") << 4 << 3 << 0 << false;
    QTest::newRow("missing-mipmaps") << 4 << 1 << 0 << true;
    // Apply never upscales, so a target larger than the Texture must not predict a resize.
    QTest::newRow("target-larger-than-texture") << 4 << 3 << 16 << false;
    QTest::newRow("target-smaller-than-texture") << 8 << 4 << 4 << true;
}

void MainOptimizerTests::textureDryRunMatchesApply()
{
    QFETCH(int, size);
    QFETCH(int, mipLevels);
    QFETCH(int, targetSize);
    QFETCH(bool, expectedChange);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto texture = std::filesystem::path(directory.path().toStdWString()) / "texture.dds";
    writeDds(texture, static_cast<size_t>(size), static_cast<size_t>(mipLevels));

    // Matching the profile's output format to the fixture isolates resize and mipmap planning
    // from compression, which this fixture's format would otherwise always request.
    auto profile = OptimizerProfileSnapshot::captureIntent();
    profile.texturesFormat = DXGI_FORMAT_B8G8R8A8_UNORM;
    profile.texturesUnwantedFormats.clear();
    profile.texturesCompressInterface = false;
    TexturesOptimizer optimizer(profile);
    const auto target = targetSize > 0 ? std::optional<size_t>(targetSize) : std::nullopt;
    const auto path = QString::fromStdWString(texture.wstring());

    QVERIFY(optimizer.open(path, TexturesOptimizer::DDS));
    QCOMPARE(optimizer.dryOptimize(true, true, true, target, target), expectedChange);

    QVERIFY(optimizer.open(path, TexturesOptimizer::DDS));
    QVERIFY(optimizer.optimize(true, true, true, target, target));
    QCOMPARE(optimizer.modifiedCurrentTexture, expectedChange);
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
        static_cast<void>(FilesystemOperations::listPlugins(
            entries, btu::bsa::Settings::get(btu::Game::SSE).plugin_extensions,
            stop.get_token()));
    } catch (const cao::run::AssetInitializationCancelled&) {
        cancelled = true;
    }
    QVERIFY(cancelled);
}

void MainOptimizerTests::pluginListingUsesProfileExtensions()
{
    QVERIFY(_temporaryDirectory.isValid());
    // A dedicated subtree keeps fixtures from other tests in the shared directory out of the list.
    const QString directory = QDir(_temporaryDirectory.path()).filePath("plugin-extensions");
    const auto root = std::filesystem::path(directory.toStdWString());
    writeFile(root / "Upper.ESP", QByteArrayLiteral("fixture"));
    writeFile(root / "nested" / "Mixed.EsM", QByteArrayLiteral("fixture"));
    writeFile(root / "light.esl", QByteArrayLiteral("fixture"));
    writeFile(root / "readme.txt", QByteArrayLiteral("fixture"));
    QVERIFY(QDir().mkpath(QDir(directory).filePath("folder.esp")));

    const auto listNames = [&directory](const btu::Game game) {
        QDirIterator entries(directory, QDirIterator::Subdirectories);
        QStringList names;
        for (const auto &path : FilesystemOperations::listPlugins(
                 entries, btu::bsa::Settings::get(game).plugin_extensions))
            names << QFileInfo(path).fileName();
        names.sort(Qt::CaseInsensitive);
        return names;
    };

    QCOMPARE(listNames(btu::Game::SSE),
             QStringList({QStringLiteral("light.esl"), QStringLiteral("Mixed.EsM"),
                          QStringLiteral("Upper.ESP")}));
    // FNV has no light plugins, so .esl must not be listed for it.
    QCOMPARE(listNames(btu::Game::FNV),
             QStringList({QStringLiteral("Mixed.EsM"), QStringLiteral("Upper.ESP")}));
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
