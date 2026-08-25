#include "MainOptimizer.h"
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
    /// Verifies malformed DDS, TGA, and NIF inputs cannot remain eligible for later Archive packing.
    void loadFailuresQuarantineMalformedAssets();

    /// Verifies a stale quarantine file cannot leave a newly extracted malformed Asset packable.
    void loadFailureUsesCollisionSafeQuarantineName();

    /// Verifies reporting a malformed input never mutates a Dry Run tree.
    void dryRunLoadFailureDoesNotQuarantine();

    /// Verifies a failed Texture conversion withholds the rewrite of references to that Texture.
    void failedConversionSuppressesMeshReferenceMaintenance();

    /// Verifies one failed conversion does not withhold references to Textures that converted.
    void failedConversionKeepsUnrelatedMeshReferences();

    /// Verifies the referenced-TGA rewrite still applies when no conversion failed.
    void successfulRunStillMaintainsMeshReferences();

private:
    QTemporaryDir _temporaryDirectory;
};

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
    QCOMPARE(savedTextureSlot(mesh), std::string("textures\\armor\\body.dds"));
}

QTEST_APPLESS_MAIN(MainOptimizerTests)

#include "MainOptimizerTests.moc"
