#include "MainOptimizer.h"
#include "BsaOptimizer.h"
#include "FilesystemOperations.h"
#include "AssetRouting/AssetRouter.h"

#include <nifly/BasicTypes.hpp>
#include <nifly/NifFile.hpp>

#include <QTemporaryDir>
#include <QTest>

#include <array>
#include <filesystem>
#include <stdexcept>
#include <string>
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
}

class MainOptimizerTests final : public QObject
{
    Q_OBJECT

private slots:
 /// Exercises backup and delete source choices after successful and failed extraction.
 void archiveSourceCleanupRequiresSuccessfulMerge_data();
 /// Preserves original Archive bytes on failure and never replaces an existing backup.
 void archiveSourceCleanupRequiresSuccessfulMerge();

 /// Keeps temporary Texture bytes out of archives and their packed-source deletion pass.
 void packingPreservesStagingFiles();

 /// Leaves staging ownership directories to their owner while pruning ordinary empty paths.
 void emptyDirectoryCleanupPreservesStaging();

 /// Rejects ambiguous staging at the selected Mod Root even for deeply nested Textures.
 void nestedTextureUsesSelectedModRoot();

 /// Verifies malformed DDS, TGA, and NIF inputs cannot remain eligible for later Archive packing.
 void loadFailuresQuarantineMalformedAssets();

 /// Verifies a stale quarantine file cannot leave a newly extracted malformed Asset packable.
 void loadFailureUsesCollisionSafeQuarantineName();

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

private:
    QTemporaryDir _temporaryDirectory;
};

void MainOptimizerTests::archiveSourceCleanupRequiresSuccessfulMerge_data() {
    QTest::addColumn<bool>("validArchive");
    QTest::addColumn<bool>("deleteBackup");
    QTest::addColumn<bool>("blockSourceCleanup");
    QTest::newRow("failed-backup") << false << false << false;
    QTest::newRow("failed-delete") << false << true << false;
    QTest::newRow("successful-backup") << true << false << false;
    QTest::newRow("successful-delete") << true << true << false;
#ifdef _WIN32
    QTest::newRow("blocked-backup") << true << false << true;
    QTest::newRow("blocked-delete") << true << true << true;
#endif
}

void MainOptimizerTests::archiveSourceCleanupRequiresSuccessfulMerge() {
    QFETCH(bool, validArchive);
    QFETCH(bool, deleteBackup);
    QFETCH(bool, blockSourceCleanup);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const ScopedCurrentDirectory isolatedWorkingDirectory(directory.path());
    const auto root = std::filesystem::path(directory.path().toStdWString());
    const auto mod = root / "mod";
    const auto source = mod / "assets.bsa";
    const auto backup = mod / "assets.bsa.bak";
    writeFile(backup, QByteArrayLiteral("existing backup bytes"));
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
        QVERIFY(!result.safeToContinue);
    }
    QFile existingBackup(QString::fromStdWString(backup.wstring()));
    QVERIFY(existingBackup.open(QIODevice::ReadOnly));
    QCOMPARE(existingBackup.readAll(), QByteArrayLiteral("existing backup bytes"));
    if (!validArchive || !deleteBackup || blockSourceCleanup) {
        const auto retained = validArchive && !blockSourceCleanup ? mod / "assets.bsa.bak.bak" : source;
        QFile retainedArchive(QString::fromStdWString(retained.wstring()));
        QVERIFY(retainedArchive.open(QIODevice::ReadOnly));
        QCOMPARE(retainedArchive.readAll(), originalBytes);
    }
    QVERIFY(artifacts.performSafetyCleanup().empty());
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
    const auto staged = mod / ".CAO-STAGING" / "run-1" / "pending.dds";
    const auto nestedStaged = mod / "textures" / ".cao-staging-old" / "pending.dds";
    writeFile(staged, QByteArrayLiteral("temporary bytes"));
    writeFile(nestedStaged, QByteArrayLiteral("unverified temporary bytes"));
    writeFile(mod / "textures" / "complete.dds", QByteArrayLiteral("committed bytes"));
    OptionsCAO options;
    options.bBsaCreateDummies = false;
    options.bBsaCompress = false;
    options.bBsaDeleteSource = true;

    BSAOptimizer().packAll(QString::fromStdWString(mod.wstring()), options);

    QVERIFY(std::filesystem::exists(staged));
    QVERIFY(std::filesystem::exists(nestedStaged));
    QVERIFY(!std::filesystem::exists(mod / "textures" / "complete.dds"));
    QVERIFY(
        !QDir(QString::fromStdWString(mod.wstring())).entryList({"*.bsa"}, QDir::Files).isEmpty());
}

void MainOptimizerTests::emptyDirectoryCleanupPreservesStaging() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const QDir root(directory.path());
    QVERIFY(root.mkpath(".CAO-STAGING/run-1"));
    QVERIFY(root.mkpath("ordinary/empty"));

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

QTEST_APPLESS_MAIN(MainOptimizerTests)

#include "MainOptimizerTests.moc"
