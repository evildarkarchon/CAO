#include "AssetExecution/AssetExecutor.h"
#include "AssetRouting/AssetRouter.h"

#include <QTest>

#include <filesystem>
#include <optional>
#include <stdexcept>
#include <utility>

namespace
{
using cao::execution::AssetExecutionBackend;
using cao::execution::AssetExecutionFailure;
using cao::execution::AssetExecutor;
using cao::execution::OperationResult;
using cao::routing::AnimationAsset;
using cao::routing::AssetOperation;
using cao::routing::AssetRouter;
using cao::routing::ExecutionMode;
using cao::routing::MeshVariant;
using cao::routing::OptimizerTarget;
using cao::routing::ProfileCapabilities;
using cao::routing::ProfileCapability;
using cao::routing::RequestedWork;
using cao::routing::RoutedAsset;
using cao::routing::RoutingPolicy;
using cao::routing::RunRequest;
using cao::routing::TextureVariant;

/// Defines complete test Profile Capabilities for every routed execution scenario.
ProfileCapabilities completeCapabilities()
{
    return ProfileCapabilities::define(
        ".bsa",
        {ProfileCapability::NativeTextureOptimization,
         ProfileCapability::ConvertibleTextureConversion,
         ProfileCapability::StandardMeshOptimization,
         ProfileCapability::TerrainMeshOptimization,
         ProfileCapability::AnimationOptimization,
         ProfileCapability::ArchiveExtraction,
         ProfileCapability::MeshReferenceMaintenance});
}

/// Routes one valid test Asset through the public Routing interface or fails test setup loudly.
RoutedAsset routeAsset(const ExecutionMode mode,
                       const std::initializer_list<RequestedWork> work,
                       const std::filesystem::path &path)
{
    const auto result = RoutingPolicy::compile(RunRequest::forWork(mode, work),
                                               completeCapabilities());
    if (!result.hasPolicy())
        throw std::runtime_error("Test Routing Policy unexpectedly failed to compile.");

    const AssetRouter router(*result.policy());
    auto decision = router.route(path);
    if (!std::holds_alternative<RoutedAsset>(decision))
        throw std::runtime_error("Test path unexpectedly failed to route.");
    return std::get<RoutedAsset>(std::move(decision));
}

class RecordingBackend final : public AssetExecutionBackend
{
public:
    bool loadTexture(const std::filesystem::path &path, const TextureVariant variant) override
    {
        texturePath = path;
        textureVariant = variant;
        ++textureLoads;
        return loadSucceeds;
    }

    OperationResult optimizeTexture(const cao::routing::AssetOperations &operations,
                                    const ExecutionMode mode) override
    {
        textureOptimization = operations.contains(AssetOperation::Optimization);
        textureConversion = operations.contains(AssetOperation::Conversion);
        textureMode = mode;
        ++textureOperations;
        return operationResult;
    }

    bool saveTexture(const std::filesystem::path &path) override
    {
        savedTexturePath = path;
        ++textureSaves;
        return saveSucceeds;
    }

    bool removeTexture(const std::filesystem::path &path) override
    {
        removedTexturePath = path;
        ++textureRemovals;
        return removeSucceeds;
    }

    bool loadMesh(const std::filesystem::path &path, const MeshVariant variant) override
    {
        meshPath = path;
        meshVariant = variant;
        ++meshLoads;
        return loadSucceeds;
    }

    OperationResult optimizeMesh(const std::filesystem::path &path,
                                 const ExecutionMode mode) override
    {
        optimizedMeshPath = path;
        meshOptimizationMode = mode;
        ++meshOptimizations;
        if (mode == ExecutionMode::Apply)
            meshContents += " optimized";
        return operationResult;
    }

    OperationResult maintainMeshReferences(const ExecutionMode mode) override
    {
        meshMaintenanceMode = mode;
        ++meshMaintenances;
        if (mode == ExecutionMode::Apply)
            meshContents = "textures/armor.dds";
        return operationResult;
    }

    bool saveMesh(const std::filesystem::path &path) override
    {
        savedMeshPath = path;
        ++meshSaves;
        return saveSucceeds;
    }

    OperationResult optimizeAnimation(const std::filesystem::path &path,
                                      const ExecutionMode mode) override
    {
        animationPath = path;
        animationMode = mode;
        ++animationOptimizations;
        return operationResult;
    }

    bool loadSucceeds{true};
    bool saveSucceeds{true};
    bool removeSucceeds{true};
    OperationResult operationResult{OperationResult::changed()};

    int textureLoads{};
    int textureOperations{};
    int textureSaves{};
    int textureRemovals{};
    std::filesystem::path texturePath;
    std::optional<TextureVariant> textureVariant;
    bool textureOptimization{};
    bool textureConversion{};
    std::optional<ExecutionMode> textureMode;
    std::filesystem::path savedTexturePath;
    std::filesystem::path removedTexturePath;

    int meshLoads{};
    int meshOptimizations{};
    int meshMaintenances{};
    int meshSaves{};
    std::filesystem::path meshPath;
    std::optional<MeshVariant> meshVariant;
    std::filesystem::path optimizedMeshPath;
    std::optional<ExecutionMode> meshOptimizationMode;
    std::optional<ExecutionMode> meshMaintenanceMode;
    std::filesystem::path savedMeshPath;
    std::string meshContents{"textures/armor.tga"};

    int animationOptimizations{};
    std::filesystem::path animationPath;
    std::optional<ExecutionMode> animationMode;
};
}

class AssetExecutionTests final : public QObject
{
    Q_OBJECT

private slots:
    /// Defines Apply and Dry Run expectations for conversion-only Texture work.
    void conversionOnlyTextureExecution_data();

    /// Verifies conversion alone executes a convertible Texture without ordinary Texture optimization.
    void conversionOnlyTextureExecution();

    /// Defines standard and terrain Mesh paths whose carried Variant must select loading behavior.
    void meshVariantSelectsLoadMode_data();

    /// Verifies Mesh loading receives the carried Variant and original execution path exactly once.
    void meshVariantSelectsLoadMode();

    /// Defines independent ordinary optimization and Mesh Reference Maintenance combinations.
    void meshOperationsShareOneTransaction_data();

    /// Verifies independent Mesh operations share one load and at most one save transaction.
    void meshOperationsShareOneTransaction();

    /// Verifies Dry Run evaluates Mesh Reference Maintenance without mutation or saving.
    void dryRunMeshMaintenanceDoesNotMutate();

    /// Defines Apply and Dry Run expectations for Animation execution.
    void animationExecution_data();

    /// Verifies Animation execution consumes the carried operation and execution mode.
    void animationExecution();

    /// Verifies an Animation backend failure is returned to the caller.
    void animationFailureIsReported();

    /// Verifies a reported backend failure cannot alter the earlier Routing Decision.
    void executionFailurePreservesRoutedDecision();

    /// Verifies Archive extraction is rejected by the loose-Asset execution seam.
    void archiveIsNotOwnedByAssetExecutor();
};

void AssetExecutionTests::conversionOnlyTextureExecution_data()
{
    QTest::addColumn<int>("mode");
    QTest::addColumn<int>("expectedSaveCount");
    QTest::addColumn<int>("expectedRemoveCount");

    QTest::newRow("Apply") << static_cast<int>(ExecutionMode::Apply) << 1 << 1;
    QTest::newRow("Dry Run") << static_cast<int>(ExecutionMode::DryRun) << 0 << 0;
}

void AssetExecutionTests::conversionOnlyTextureExecution()
{
    QFETCH(int, mode);
    QFETCH(int, expectedSaveCount);
    QFETCH(int, expectedRemoveCount);

    const auto executionMode = static_cast<ExecutionMode>(mode);
    const auto asset = routeAsset(executionMode,
                                  {RequestedWork::ConvertibleTextureConversion},
                                  std::filesystem::path(L"Textures/Source.Name.TgA"));
    RecordingBackend backend;
    const AssetExecutor executor(backend);

    const auto result = executor.execute(asset);

    QVERIFY(result.succeeded());
    QCOMPARE(backend.textureLoads, 1);
    QCOMPARE(backend.textureOperations, 1);
    QCOMPARE(backend.textureVariant.value(), TextureVariant::Convertible);
    QCOMPARE(backend.textureOptimization, false);
    QCOMPARE(backend.textureConversion, true);
    QCOMPARE(backend.textureMode.value(), executionMode);
    QCOMPARE(backend.textureSaves, expectedSaveCount);
    QCOMPARE(backend.textureRemovals, expectedRemoveCount);
    if (executionMode == ExecutionMode::Apply) {
        QVERIFY(backend.savedTexturePath == std::filesystem::path(L"Textures/Source.Name.dds"));
        QVERIFY(backend.removedTexturePath == asset.executionPath());
    }
}

void AssetExecutionTests::meshVariantSelectsLoadMode_data()
{
    QTest::addColumn<QString>("path");
    QTest::addColumn<int>("variant");
    QTest::addColumn<int>("request");

    QTest::newRow("standard") << QStringLiteral("Meshes/Actor.NIF")
                              << static_cast<int>(MeshVariant::Standard)
                              << static_cast<int>(RequestedWork::StandardMeshOptimization);
    QTest::newRow("terrain BTR") << QStringLiteral("Meshes/Landscape.BTR")
                                 << static_cast<int>(MeshVariant::Terrain)
                                 << static_cast<int>(RequestedWork::TerrainMeshOptimization);
    QTest::newRow("terrain BTO") << QStringLiteral("Meshes/Landscape.BTO")
                                 << static_cast<int>(MeshVariant::Terrain)
                                 << static_cast<int>(RequestedWork::TerrainMeshOptimization);
}

void AssetExecutionTests::meshVariantSelectsLoadMode()
{
    QFETCH(QString, path);
    QFETCH(int, variant);
    QFETCH(int, request);

    const auto asset = routeAsset(ExecutionMode::Apply,
                                  {static_cast<RequestedWork>(request)},
                                  std::filesystem::path(path.toStdWString()));
    RecordingBackend backend;
    const AssetExecutor executor(backend);

    const auto result = executor.execute(asset);

    QVERIFY(result.succeeded());
    QCOMPARE(backend.meshLoads, 1);
    QVERIFY(backend.meshPath == asset.executionPath());
    QCOMPARE(backend.meshVariant.value(), static_cast<MeshVariant>(variant));
}

void AssetExecutionTests::meshOperationsShareOneTransaction_data()
{
    QTest::addColumn<bool>("optimize");
    QTest::addColumn<bool>("maintain");
    QTest::addColumn<QString>("path");

    QTest::newRow("optimization only") << true << false << QStringLiteral("Meshes/Actor.nif");
    QTest::newRow("maintenance only") << false << true << QStringLiteral("Meshes/Actor.nif");
    QTest::newRow("optimization and maintenance") << true << true << QStringLiteral("Meshes/Actor.nif");
}

void AssetExecutionTests::meshOperationsShareOneTransaction()
{
    QFETCH(bool, optimize);
    QFETCH(bool, maintain);
    QFETCH(QString, path);

    const auto executionPath = std::filesystem::path(path.toStdWString());
    const auto asset = !optimize
                           ? routeAsset(ExecutionMode::Apply,
                                        {RequestedWork::ConvertibleTextureConversion},
                                        executionPath)
                       : maintain
                           ? routeAsset(ExecutionMode::Apply,
                                        {RequestedWork::StandardMeshOptimization,
                                         RequestedWork::ConvertibleTextureConversion},
                                        executionPath)
                           : routeAsset(ExecutionMode::Apply,
                                        {RequestedWork::StandardMeshOptimization},
                                        executionPath);
    RecordingBackend backend;
    const AssetExecutor executor(backend);

    const auto result = executor.execute(asset);

    QVERIFY(result.succeeded());
    QCOMPARE(backend.meshLoads, 1);
    QCOMPARE(backend.meshOptimizations, optimize ? 1 : 0);
    QCOMPARE(backend.meshMaintenances, maintain ? 1 : 0);
    QCOMPARE(backend.meshSaves, 1);
}

void AssetExecutionTests::dryRunMeshMaintenanceDoesNotMutate()
{
    const auto asset = routeAsset(ExecutionMode::DryRun,
                                  {RequestedWork::ConvertibleTextureConversion},
                                  std::filesystem::path(L"Meshes/Actor.nif"));
    RecordingBackend backend;
    const auto originalContents = backend.meshContents;
    const AssetExecutor executor(backend);

    const auto result = executor.execute(asset);

    QVERIFY(result.succeeded());
    QCOMPARE(backend.meshLoads, 1);
    QCOMPARE(backend.meshMaintenances, 1);
    QCOMPARE(backend.meshMaintenanceMode.value(), ExecutionMode::DryRun);
    QCOMPARE(backend.meshSaves, 0);
    QCOMPARE(backend.meshContents, originalContents);
}

void AssetExecutionTests::animationExecution_data()
{
    QTest::addColumn<int>("mode");

    QTest::newRow("Apply") << static_cast<int>(ExecutionMode::Apply);
    QTest::newRow("Dry Run") << static_cast<int>(ExecutionMode::DryRun);
}

void AssetExecutionTests::animationExecution()
{
    QFETCH(int, mode);

    const auto executionMode = static_cast<ExecutionMode>(mode);
    const auto asset = routeAsset(executionMode,
                                  {RequestedWork::AnimationOptimization},
                                  std::filesystem::path(L"Animations/Walk.HKX"));
    RecordingBackend backend;
    const AssetExecutor executor(backend);

    const auto result = executor.execute(asset);

    QVERIFY(result.succeeded());
    QCOMPARE(backend.animationOptimizations, 1);
    QVERIFY(backend.animationPath == asset.executionPath());
    QCOMPARE(backend.animationMode.value(), executionMode);
}

void AssetExecutionTests::animationFailureIsReported()
{
    const auto asset = routeAsset(ExecutionMode::Apply,
                                  {RequestedWork::AnimationOptimization},
                                  std::filesystem::path(L"Animations/Walk.hkx"));
    RecordingBackend backend;
    backend.operationResult = OperationResult::failed("synthetic animation failure");
    const AssetExecutor executor(backend);

    const auto result = executor.execute(asset);

    QVERIFY(!result.succeeded());
    QCOMPARE(result.failure().value(), AssetExecutionFailure::OperationFailed);
    QCOMPARE(result.message(), std::string("synthetic animation failure"));
}

void AssetExecutionTests::executionFailurePreservesRoutedDecision()
{
    const auto asset = routeAsset(ExecutionMode::Apply,
                                  {RequestedWork::StandardMeshOptimization},
                                  std::filesystem::path(L"Meshes/Actor.nif"));
    const auto originalPath = asset.executionPath();
    const auto originalTarget = asset.target();
    const auto originalMode = asset.executionMode();
    const auto originalOptimization = asset.operations().contains(AssetOperation::Optimization);
    RecordingBackend backend;
    backend.operationResult = OperationResult::failed("synthetic optimizer failure");
    const AssetExecutor executor(backend);

    const auto result = executor.execute(asset);

    QVERIFY(!result.succeeded());
    QCOMPARE(result.failure().value(), AssetExecutionFailure::OperationFailed);
    QVERIFY(asset.executionPath() == originalPath);
    QCOMPARE(asset.target(), originalTarget);
    QCOMPARE(asset.executionMode(), originalMode);
    QCOMPARE(asset.operations().contains(AssetOperation::Optimization), originalOptimization);
}

void AssetExecutionTests::archiveIsNotOwnedByAssetExecutor()
{
    const auto asset = routeAsset(ExecutionMode::Apply,
                                  {RequestedWork::ArchiveExtraction},
                                  std::filesystem::path(L"Archives/Assets.bsa"));
    RecordingBackend backend;
    const AssetExecutor executor(backend);

    const auto result = executor.execute(asset);

    QVERIFY(!result.succeeded());
    QCOMPARE(result.failure().value(), AssetExecutionFailure::UnsupportedTarget);
    QCOMPARE(backend.textureLoads, 0);
    QCOMPARE(backend.meshLoads, 0);
    QCOMPARE(backend.animationOptimizations, 0);
}

QTEST_APPLESS_MAIN(AssetExecutionTests)

#include "AssetExecutionTests.moc"
