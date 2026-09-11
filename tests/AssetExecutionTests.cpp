#include "AssetExecution/AssetExecutor.h"
#include "AssetRouting/AssetRouter.h"
#include "Run/StagingPaths.h"
#include "Run/StagingRecovery.h"

#include <QTest>
#include <QTemporaryDir>
#include <QCoreApplication>
#include <QProcess>
#include <QThread>

#include <filesystem>
#include <fstream>
#include <functional>
#include <optional>
#include <stdexcept>
#include <utility>

namespace {
/// Reads fixture bytes independently of the optimizer backend.
std::string readBytes(const std::filesystem::path& path) {
    std::ifstream input(path, std::ios::binary);
    return {std::istreambuf_iterator<char>(input), std::istreambuf_iterator<char>()};
}

using cao::execution::AssetExecutionBackend;
using cao::execution::AssetExecutionFailure;
using cao::execution::AssetExecutor;
using cao::execution::MutationState;
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
using cao::routing::RoutingPolicyRequest;
using cao::routing::TextureVariant;

/// Defines complete test Profile Capabilities for every routed execution scenario.
ProfileCapabilities completeCapabilities() {
    return ProfileCapabilities::define(
        ".bsa",
        {ProfileCapability::NativeTextureOptimization,
         ProfileCapability::ConvertibleTextureConversion,
         ProfileCapability::StandardMeshOptimization, ProfileCapability::TerrainMeshOptimization,
         ProfileCapability::AnimationOptimization, ProfileCapability::ArchiveExtraction,
         ProfileCapability::MeshReferenceMaintenance});
}

/// Routes one valid test Asset through the public Routing interface or fails test setup loudly.
RoutedAsset routeAsset(const ExecutionMode mode, const std::initializer_list<RequestedWork> work,
                       const std::filesystem::path& path) {
    const auto result =
        RoutingPolicy::compile(RoutingPolicyRequest::forWork(mode, work), completeCapabilities());
    if (!result.hasPolicy())
        throw std::runtime_error("Test Routing Policy unexpectedly failed to compile.");

    const AssetRouter router(*result.policy());
    auto decision = router.route(path);
    if (!std::holds_alternative<RoutedAsset>(decision))
        throw std::runtime_error("Test path unexpectedly failed to route.");
    return std::get<RoutedAsset>(std::move(decision));
}

class RecordingBackend final : public AssetExecutionBackend {
   public:
    bool loadTexture(const std::filesystem::path& path, const TextureVariant variant) override {
        if (throwAt == "load_texture") throw std::runtime_error("load backend threw");
        texturePath = path;
        textureVariant = variant;
        ++textureLoads;
        return loadSucceeds;
    }

    OperationResult optimizeTexture(const cao::routing::AssetOperations& operations,
                                    const ExecutionMode mode) override {
        if (throwAt == "optimize_texture") throw 42;
        textureOptimization = operations.contains(AssetOperation::Optimization);
        textureConversion = operations.contains(AssetOperation::Conversion);
        textureMode = mode;
        ++textureOperations;
        return operationResult;
    }

    bool saveTexture(const std::filesystem::path& path) override {
        savedTexturePath = path;
        ++textureSaves;
        if (textureSave) return textureSave(path);
        return saveSucceeds;
    }

    bool removeTexture(const std::filesystem::path& path) override {
        removedTexturePath = path;
        ++textureRemovals;
        if (textureRemove) return textureRemove(path);
        return removeSucceeds;
    }

    /// Records Mesh loading and injects a standard exception before any output is written.
    bool loadMesh(const std::filesystem::path& path, const MeshVariant variant) override {
        if (throwAt == "load_mesh") throw std::runtime_error("load backend threw");
        meshPath = path;
        meshVariant = variant;
        ++meshLoads;
        return loadSucceeds;
    }

    /// Evaluates optimization with an injectable unknown backend exception.
    OperationResult optimizeMesh(const std::filesystem::path& path,
                                 const ExecutionMode mode) override {
        if (throwAt == "optimize_mesh") throw 42;
        optimizedMeshPath = path;
        meshOptimizationMode = mode;
        ++meshOptimizations;
        if (mode == ExecutionMode::Apply) meshContents += " optimized";
        return operationResult;
    }

    /// Evaluates reference maintenance with an injectable standard backend exception.
    OperationResult maintainMeshReferences(const ExecutionMode mode) override {
        if (throwAt == "maintain_mesh_references")
            throw std::runtime_error("maintenance backend threw");
        meshMaintenanceMode = mode;
        ++meshMaintenances;
        if (mode == ExecutionMode::Apply) meshContents = "textures/armor.dds";
        return operationResult;
    }

    /// Writes fixture output to the executor-selected path, with injectable partial-save failure.
    bool saveMesh(const std::filesystem::path& path) override {
        savedMeshPath = path;
        ++meshSaves;
        if (meshSave) return meshSave(path);
        if (saveSucceeds) std::ofstream(path) << meshContents;
        return saveSucceeds;
    }

    /// Writes converted fixture bytes only to the supplied output path in Apply mode.
    OperationResult optimizeAnimation(const std::filesystem::path& path,
                                      const std::filesystem::path& outputPath,
                                      const ExecutionMode mode) override {
        animationPath = path;
        animationMode = mode;
        ++animationOptimizations;
        savedAnimationPath = outputPath;
        if (animationSave) return animationSave(outputPath);
        if (mode == ExecutionMode::Apply && operationResult.succeeded())
            std::ofstream(outputPath) << "converted";
        return operationResult;
    }

    bool loadSucceeds{true};
    bool saveSucceeds{true};
    bool removeSucceeds{true};
    std::function<bool(const std::filesystem::path&)> textureSave;
    std::function<bool(const std::filesystem::path&)> textureRemove;
    std::function<bool(const std::filesystem::path&)> meshSave;
    std::string throwAt;
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

    std::function<OperationResult(const std::filesystem::path&)> animationSave;
    std::filesystem::path savedAnimationPath;
    int animationOptimizations{};
    std::filesystem::path animationPath;
    std::optional<ExecutionMode> animationMode;
};

/// Runs an isolated writer until the parent forcibly terminates it at a filesystem boundary.
int textureCrashWorker(const std::filesystem::path& root, const std::filesystem::path& checkpoint,
                       const QString& boundary) {
    const auto source = root / "textures" / "source.tga";
    const auto pauseForTermination = [&] {
        std::ofstream(checkpoint) << "ready";
        for (;;) QThread::msleep(10);
    };
    RecordingBackend backend;
    backend.textureSave = [&](const std::filesystem::path& path) {
        std::ofstream(path) << (boundary == "during-save" ? "partial" : "converted");
        if (boundary == "during-save") pauseForTermination();
        return true;
    };
    backend.textureRemove = [&](const std::filesystem::path& path) {
        if (boundary == "before-source-removal") pauseForTermination();
        const auto removed = std::filesystem::remove(path);
        if (boundary == "after-source-removal") pauseForTermination();
        return removed;
    };
    const auto result = AssetExecutor(backend).execute(
        routeAsset(ExecutionMode::Apply, {RequestedWork::ConvertibleTextureConversion}, source),
        root);
    return result.succeeded() ? 0 : 2;
}
}  // namespace

class AssetExecutionTests final : public QObject {
    Q_OBJECT

   private slots:
    /// Defines process-death windows before and after durable Texture destination commit.
    void interruptedTextureRecovery_data();
    /// Recovers a killed writer's staging while preserving originals and committed destinations.
    void interruptedTextureRecovery();
    /// A partially written failed save must preserve the original Texture bytes.
    void failedTextureSavePreservesOriginal();
    /// Covers retained and missing files after a backend reports source-removal failure.
    void textureSourceRemovalFailure_data();
    /// Reports committed output and permits continuation only while both conversion files survive.
    void textureSourceRemovalFailure();
    /// Exceptions during staged save are fatal but leave durable inputs untouched.
    void textureSaveException();

    /// Keeps a Texture optimizer's service failure separate from its human-readable explanation.
    void textureOperationFailureDetails();
    /// A writer receives an already reserved same-directory file owned by the supplied registry.
    void textureStagingIsRegisteredBeforeSave();
    /// Commit failure retains both original files and cleanup removes only the staged output.
    void textureCommitFailure();
    /// Cleanup evidence remains secondary to the original backend save failure.
    void textureCleanupFailurePreservesPrimaryFailure();
    /// Read-only backend exceptions are fatal and cannot create staged or durable output.
    void textureReadOnlyException_data();
    /// Retains exception boundary and safety evidence without mutating the original Texture.
    void textureReadOnlyException();
    /// Native replacement uses the same staged commit path and reports the durable mutation.
    void nativeTextureCommit();
    /// Defines Apply and Dry Run expectations for conversion-only Texture work.
    void conversionOnlyTextureExecution_data();

    /// Verifies conversion alone executes a convertible Texture without ordinary Texture
    /// optimization.
    void conversionOnlyTextureExecution();

    /// A partial Mesh save preserves the original and reports no durable mutation.
    void failedMeshSavePreservesOriginal();
    /// Exercises every Mesh backend exception boundary before durable commit.
    void meshBackendException_data();
    /// Reports fatal structured exceptions while preserving original bytes.
    void meshBackendException();
    /// Defines the ordinary Mesh backend failure boundaries.
    void meshOperationFailure_data();
    /// Ordinary failures preserve source bytes and identify the failing Mesh operation.
    void meshOperationFailure();
    /// Unchanged Apply and Dry Run work never creates staged output.
    void meshWithoutChanges_data();
    /// Keeps unchanged and preview operations read-only on disk.
    void meshWithoutChanges();
    /// A closed registry after save cannot erase already committed Mesh mutation evidence.
    void meshRegistrationFailureAfterCommit();
    /// A directory obstructing the staging area leaves the original Mesh intact.
    void meshStagingFailure();
    /// Failed destination replacement retains unowned contents and cleans only staging.
    void meshCommitFailure();

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
    /// Covers failed, invalid, unchanged, and throwing converter output before commit.
    void animationStagedOutcomes_data();
    /// Preserves original bytes and reports the stable failed boundary while cleaning staging.
    void animationStagedOutcomes();

    /// Verifies a reported backend failure cannot alter the earlier Routing Decision.
    void executionFailurePreservesRoutedDecision();

    /// Verifies Archive extraction is rejected by the loose-Asset execution seam.
    void archiveIsNotOwnedByAssetExecutor();
};

void AssetExecutionTests::interruptedTextureRecovery_data() {
    QTest::addColumn<QString>("boundary");
    QTest::newRow("partial staged save") << QStringLiteral("during-save");
    QTest::newRow("committed output retained source") << QStringLiteral("before-source-removal");
    QTest::newRow("committed conversion") << QStringLiteral("after-source-removal");
}

void AssetExecutionTests::interruptedTextureRecovery() {
    QFETCH(QString, boundary);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = std::filesystem::path(directory.path().toStdWString()) / "Mod";
    const auto checkpoint = root.parent_path() / "checkpoint";
    const auto source = root / "textures" / "source.tga";
    const auto destination = root / "textures" / "source.dds";
    std::filesystem::create_directories(source.parent_path());
    std::ofstream(source) << "original";
    std::ofstream(destination) << "old destination";
    QProcess child;
    child.start(QCoreApplication::applicationFilePath(),
                {QStringLiteral("--texture-crash-worker"), QString::fromStdWString(root.wstring()),
                 QString::fromStdWString(checkpoint.wstring()), boundary});
    QVERIFY(child.waitForStarted());
    QTRY_VERIFY_WITH_TIMEOUT(std::filesystem::exists(checkpoint), 15000);
    child.kill();
    QVERIFY(child.waitForFinished());
    QCOMPARE(child.exitStatus(), QProcess::CrashExit);

    cao::run::StagingRecovery recovery;
    const auto failure = recovery.recover(root);
    QVERIFY2(!failure, failure ? failure->detail().c_str() : "");
    QCOMPARE(readBytes(destination),
             boundary == "during-save" ? std::string("old destination") : std::string("converted"));
    if (boundary == "after-source-removal")
        QVERIFY(!std::filesystem::exists(source));
    else
        QCOMPARE(readBytes(source), std::string("original"));
    for (const auto& entry : std::filesystem::directory_iterator(root / ".cao-staging"))
        QVERIFY(entry.path().filename() == "owner.lock" ||
                entry.path().filename() == "ownership.manifest");
    QVERIFY(!std::filesystem::exists(source.parent_path() / ".cao-staging"));
}

void AssetExecutionTests::failedTextureSavePreservesOriginal() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto source = std::filesystem::path(directory.path().toStdWString()) / "native.dds";
    std::ofstream(source) << "original";
    RecordingBackend backend;
    backend.textureSave = [](const std::filesystem::path& path) {
        std::ofstream(path) << "partial output";
        return false;
    };
    const auto result = AssetExecutor(backend).execute(
        routeAsset(ExecutionMode::Apply, {RequestedWork::NativeTextureOptimization}, source));
    QVERIFY(!result.succeeded());
    QCOMPARE(result.failure().value(), AssetExecutionFailure::SaveFailed);
    QCOMPARE(readBytes(source), std::string("original"));
    QCOMPARE(backend.textureRemovals, 0);
    QCOMPARE(result.mutationState(), MutationState::None);
    QVERIFY(result.safeToContinue());
    QVERIFY(result.affectedPath() == source);
    QVERIFY(!std::filesystem::exists(backend.savedTexturePath));
}

void AssetExecutionTests::textureSourceRemovalFailure_data() {
    QTest::addColumn<int>("damage");
    QTest::addColumn<bool>("throws");
    QTest::newRow("both usable") << 0 << false;
    QTest::newRow("source missing") << 1 << false;
    QTest::newRow("output missing") << 2 << false;
    QTest::newRow("source empty") << 3 << false;
    QTest::newRow("source corrupted") << 4 << false;
    QTest::newRow("exception after commit") << 0 << true;
    QTest::newRow("exception after source loss") << 1 << true;
}

void AssetExecutionTests::textureSourceRemovalFailure() {
    QFETCH(int, damage);
    QFETCH(bool, throws);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto source = std::filesystem::path(directory.path().toStdWString()) / "source.tga";
    const auto output = source.parent_path() / "source.dds";
    std::ofstream(source) << "original";
    RecordingBackend backend;
    backend.textureSave = [](const std::filesystem::path& path) {
        std::ofstream(path) << "converted";
        return true;
    };
    backend.textureRemove = [&](const std::filesystem::path& path) {
        if (readBytes(output) != "converted") throw std::logic_error("Output not committed");
        if (damage == 1) std::filesystem::remove(path);
        if (damage == 2) std::filesystem::remove(output);
        if (damage == 3) std::ofstream(path).close();
        if (damage == 4) std::ofstream(path) << "damaged!";
        if (throws) throw std::runtime_error("removal backend threw");
        return false;
    };
    cao::run::TemporaryArtifactRegistry artifacts;
    const auto result = AssetExecutor(backend).execute(
        routeAsset(ExecutionMode::Apply, {RequestedWork::ConvertibleTextureConversion}, source),
        artifacts);
    QVERIFY(!result.succeeded());
    QCOMPARE(result.failure().value(), throws ? AssetExecutionFailure::BackendException
                                              : AssetExecutionFailure::SourceRemovalFailed);
    QCOMPARE(result.mutationState(),
             damage == 0 ? MutationState::Committed : MutationState::PartialOrUnknown);
    QCOMPARE(result.safeToContinue(), damage == 0 && !throws);
    QVERIFY(result.affectedPath() == source);
    QCOMPARE(result.operation(), std::string("remove_texture_source"));
    QVERIFY(artifacts.performSafetyCleanup().empty());
    if (damage != 2) QCOMPARE(readBytes(output), std::string("converted"));
    if (damage == 0) QCOMPARE(readBytes(source), std::string("original"));
}

void AssetExecutionTests::textureOperationFailureDetails() {
    RecordingBackend backend;
    backend.operationResult = OperationResult::failed("synthetic Texture service error");
    const auto result = AssetExecutor(backend).execute(routeAsset(
        ExecutionMode::DryRun, {RequestedWork::NativeTextureOptimization}, "fixture.dds"));
    QCOMPARE(result.message(), std::string("Failed to optimize Texture."));
    QCOMPARE(result.serviceDetail(), std::string("synthetic Texture service error"));
}

void AssetExecutionTests::textureSaveException() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto source = std::filesystem::path(directory.path().toStdWString()) / "native.dds";
    std::ofstream(source) << "original";
    RecordingBackend backend;
    backend.textureSave = [](const std::filesystem::path& path) -> bool {
        std::ofstream(path) << "partial output";
        throw std::runtime_error("save backend threw");
    };
    const auto result = AssetExecutor(backend).execute(
        routeAsset(ExecutionMode::Apply, {RequestedWork::NativeTextureOptimization}, source));
    QCOMPARE(result.failure().value(), AssetExecutionFailure::BackendException);
    QCOMPARE(result.mutationState(), MutationState::None);
    QVERIFY(!result.safeToContinue());
    QVERIFY(result.affectedPath() == source);
    QCOMPARE(result.operation(), std::string("save_texture"));
    QCOMPARE(result.message(), std::string("Texture backend raised an exception."));
    QCOMPARE(result.serviceDetail(), std::string("save backend threw"));
    QCOMPARE(readBytes(source), std::string("original"));
    QVERIFY(!std::filesystem::exists(backend.savedTexturePath));
}

void AssetExecutionTests::textureStagingIsRegisteredBeforeSave() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto source = std::filesystem::path(directory.path().toStdWString()) / "native.dds";
    std::ofstream(source) << "original";
    cao::run::TemporaryArtifactRegistry artifacts;
    RecordingBackend backend;
    bool reservedBeforeSave = false;
    backend.textureSave = [&](const std::filesystem::path& path) {
        reservedBeforeSave = std::filesystem::is_regular_file(path);
        std::ofstream(path) << "partial output";
        return false;
    };
    const auto result = AssetExecutor(backend).execute(
        routeAsset(ExecutionMode::Apply, {RequestedWork::NativeTextureOptimization}, source),
        artifacts);
    QVERIFY(reservedBeforeSave);
    QCOMPARE(result.failure().value(), AssetExecutionFailure::SaveFailed);
    QVERIFY(std::filesystem::exists(backend.savedTexturePath));
    QVERIFY(artifacts.performSafetyCleanup().empty());
    QVERIFY(!std::filesystem::exists(backend.savedTexturePath));
    QCOMPARE(readBytes(source), std::string("original"));
}

void AssetExecutionTests::textureCommitFailure() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto source = std::filesystem::path(directory.path().toStdWString()) / "source.tga";
    const auto destination = source.parent_path() / "source.dds";
    std::ofstream(source) << "original";
    std::filesystem::create_directory(destination);
    std::ofstream(destination / "unowned") << "keep";
    RecordingBackend backend;
    backend.textureSave = [](const std::filesystem::path& path) {
        std::ofstream(path) << "converted";
        return true;
    };
    const auto result = AssetExecutor(backend).execute(
        routeAsset(ExecutionMode::Apply, {RequestedWork::ConvertibleTextureConversion}, source));
    QCOMPARE(result.failure().value(), AssetExecutionFailure::CommitFailed);
    QCOMPARE(result.mutationState(), MutationState::None);
    QVERIFY(result.safeToContinue());
    QVERIFY(result.affectedPath() == destination);
    QCOMPARE(backend.textureRemovals, 0);
    QCOMPARE(readBytes(source), std::string("original"));
    QCOMPARE(readBytes(destination / "unowned"), std::string("keep"));
    QVERIFY(!std::filesystem::exists(backend.savedTexturePath));
}

void AssetExecutionTests::textureCleanupFailurePreservesPrimaryFailure() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto source = std::filesystem::path(directory.path().toStdWString()) / "native.dds";
    std::ofstream(source) << "original";
    RecordingBackend backend;
    backend.textureSave = [](const std::filesystem::path& path) {
        std::filesystem::remove(path);
        std::filesystem::create_directory(path);
        std::ofstream(path / "unregistered") << "keep";
        return false;
    };
    const auto result = AssetExecutor(backend).execute(
        routeAsset(ExecutionMode::Apply, {RequestedWork::NativeTextureOptimization}, source));
    QCOMPARE(result.failure().value(), AssetExecutionFailure::SaveFailed);
    // The unregistered contents replace the staged file, so cleanup preserves that evidence.
    QCOMPARE(result.cleanupFailures().size(), std::size_t{1});
    QCOMPARE(result.cleanupFailures().front().code(),
             cao::run::RunFailureCode::TemporaryArtifactCleanupFailed);
    QCOMPARE(readBytes(backend.savedTexturePath / "unregistered"), std::string("keep"));
    QCOMPARE(readBytes(source), std::string("original"));
}

void AssetExecutionTests::textureReadOnlyException_data() {
    QTest::addColumn<QString>("boundary");
    QTest::newRow("load standard exception") << QStringLiteral("load_texture");
    QTest::newRow("optimize unknown exception") << QStringLiteral("optimize_texture");
}

void AssetExecutionTests::textureReadOnlyException() {
    QFETCH(QString, boundary);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto source = std::filesystem::path(directory.path().toStdWString()) / "native.dds";
    std::ofstream(source) << "original";
    RecordingBackend backend;
    backend.throwAt = boundary.toStdString();
    const auto result = AssetExecutor(backend).execute(
        routeAsset(ExecutionMode::Apply, {RequestedWork::NativeTextureOptimization}, source));
    QCOMPARE(result.failure().value(), AssetExecutionFailure::BackendException);
    QCOMPARE(result.mutationState(), MutationState::None);
    QCOMPARE(result.failureCategory().value(), cao::execution::ExecutionFailureCategory::Contract);
    QCOMPARE(result.phase(), cao::run::RunPhase::ProcessingAssets);
    QCOMPARE(result.operation(), boundary.toStdString());
    QVERIFY(result.affectedPath() == source);
    QVERIFY(!result.safeToContinue());
    QCOMPARE(backend.textureSaves, 0);
    QCOMPARE(readBytes(source), std::string("original"));
}

void AssetExecutionTests::nativeTextureCommit() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto source = std::filesystem::path(directory.path().toStdWString()) / "native.dds";
    std::ofstream(source) << "original";
    RecordingBackend backend;
    backend.textureSave = [&](const std::filesystem::path& path) {
        if (readBytes(source) != "original" || path == source)
            throw std::runtime_error("Native save overwrote the original");
        std::ofstream(path) << "optimized";
        return true;
    };
    const auto result = AssetExecutor(backend).execute(
        routeAsset(ExecutionMode::Apply, {RequestedWork::NativeTextureOptimization}, source));
    QVERIFY(result.succeeded());
    QCOMPARE(result.mutationState(), MutationState::Committed);
    QCOMPARE(readBytes(source), std::string("optimized"));
    QCOMPARE(backend.textureRemovals, 0);
    QVERIFY(!std::filesystem::exists(backend.savedTexturePath));
}

void AssetExecutionTests::conversionOnlyTextureExecution_data() {
    QTest::addColumn<int>("mode");
    QTest::addColumn<int>("expectedSaveCount");
    QTest::addColumn<int>("expectedRemoveCount");

    QTest::newRow("Apply") << static_cast<int>(ExecutionMode::Apply) << 1 << 1;
    QTest::newRow("Dry Run") << static_cast<int>(ExecutionMode::DryRun) << 0 << 0;
}

void AssetExecutionTests::conversionOnlyTextureExecution() {
    QFETCH(int, mode);
    QFETCH(int, expectedSaveCount);
    QFETCH(int, expectedRemoveCount);

    const auto executionMode = static_cast<ExecutionMode>(mode);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto source = std::filesystem::path(directory.path().toStdWString()) / "Source.Name.TgA";
    const auto destination = source.parent_path() / "Source.Name.dds";
    std::ofstream(source) << "original";
    std::ofstream(destination) << "old destination";
    const auto asset =
        routeAsset(executionMode, {RequestedWork::ConvertibleTextureConversion}, source);
    RecordingBackend backend;
    backend.textureSave = [&](const std::filesystem::path& path) {
        if (path.parent_path() != destination.parent_path() || !cao::run::isStagingName(path) ||
            path == destination || readBytes(destination) != "old destination")
            throw std::runtime_error("Destination changed before staging completed");
        std::ofstream(path) << "converted";
        return true;
    };
    backend.textureRemove = [&](const std::filesystem::path& path) {
        if (readBytes(destination) != "converted")
            throw std::runtime_error("Source removal preceded destination commit");
        return std::filesystem::remove(path);
    };
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
        QCOMPARE(readBytes(destination), std::string("converted"));
        QVERIFY(!std::filesystem::exists(source));
        QVERIFY(!std::filesystem::exists(backend.savedTexturePath));
        QVERIFY(backend.removedTexturePath == asset.executionPath());
        QCOMPARE(result.mutationState(), MutationState::Committed);
    } else {
        QCOMPARE(readBytes(source), std::string("original"));
        QCOMPARE(readBytes(destination), std::string("old destination"));
        QCOMPARE(result.mutationState(), MutationState::None);
    }
}

void AssetExecutionTests::failedMeshSavePreservesOriginal() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto source = std::filesystem::path(directory.path().toStdWString()) / "actor.nif";
    std::ofstream(source) << "original";
    RecordingBackend backend;
    backend.meshSave = [](const std::filesystem::path& path) {
        std::ofstream(path) << "partial output";
        return false;
    };
    const auto result = AssetExecutor(backend).execute(
        routeAsset(ExecutionMode::Apply, {RequestedWork::StandardMeshOptimization}, source));
    QCOMPARE(readBytes(source), std::string("original"));
    QCOMPARE(result.failure().value(), AssetExecutionFailure::SaveFailed);
    QCOMPARE(result.mutationState(), MutationState::None);
    QVERIFY(result.safeToContinue());
    QVERIFY(result.affectedPath() == source);
    QCOMPARE(result.operation(), std::string("save_mesh"));
    QVERIFY(!std::filesystem::exists(backend.savedMeshPath));
}

void AssetExecutionTests::meshBackendException_data() {
    QTest::addColumn<QString>("boundary");
    QTest::newRow("load") << QStringLiteral("load_mesh");
    QTest::newRow("optimization") << QStringLiteral("optimize_mesh");
    QTest::newRow("maintenance") << QStringLiteral("maintain_mesh_references");
    QTest::newRow("partial save") << QStringLiteral("save_mesh");
}

void AssetExecutionTests::meshBackendException() {
    QFETCH(QString, boundary);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto source = std::filesystem::path(directory.path().toStdWString()) / "actor.nif";
    std::ofstream(source) << "original";
    RecordingBackend backend;
    backend.throwAt = boundary.toStdString();
    backend.meshSave = [](const std::filesystem::path& staged) -> bool {
        std::ofstream(staged) << "partial output";
        throw std::runtime_error("save backend threw");
    };
    const auto result = AssetExecutor(backend).execute(routeAsset(
        ExecutionMode::Apply,
        {RequestedWork::StandardMeshOptimization, RequestedWork::ConvertibleTextureConversion},
        source));
    QCOMPARE(result.failure().value(), AssetExecutionFailure::BackendException);
    QCOMPARE(result.failureCategory().value(), cao::execution::ExecutionFailureCategory::Contract);
    QCOMPARE(result.mutationState(), MutationState::None);
    QCOMPARE(result.phase(), cao::run::RunPhase::ProcessingAssets);
    QCOMPARE(result.operation(), boundary.toStdString());
    QVERIFY(result.affectedPath() == source);
    QVERIFY(!result.safeToContinue());
    QCOMPARE(readBytes(source), std::string("original"));
    if (boundary == "save_mesh") {
        QCOMPARE(result.serviceDetail(), std::string("save backend threw"));
        QVERIFY(!std::filesystem::exists(backend.savedMeshPath));
    } else {
        QCOMPARE(backend.meshSaves, 0);
    }
}

void AssetExecutionTests::meshWithoutChanges_data() {
    QTest::addColumn<int>("mode");
    QTest::addColumn<bool>("changed");
    QTest::newRow("unchanged Apply") << static_cast<int>(ExecutionMode::Apply) << false;
    QTest::newRow("changed Dry Run") << static_cast<int>(ExecutionMode::DryRun) << true;
}

void AssetExecutionTests::meshOperationFailure_data() {
    QTest::addColumn<QString>("boundary");
    QTest::newRow("load") << QStringLiteral("load_mesh");
    QTest::newRow("optimization") << QStringLiteral("optimize_mesh");
    QTest::newRow("maintenance") << QStringLiteral("maintain_mesh_references");
}

void AssetExecutionTests::meshOperationFailure() {
    QFETCH(QString, boundary);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto source = std::filesystem::path(directory.path().toStdWString()) / "actor.nif";
    std::ofstream(source) << "original";
    RecordingBackend backend;
    backend.loadSucceeds = boundary != "load_mesh";
    backend.operationResult = OperationResult::failed("synthetic Mesh service error");
    const auto request = boundary == "maintain_mesh_references"
                             ? RequestedWork::ConvertibleTextureConversion
                             : RequestedWork::StandardMeshOptimization;
    const auto result = AssetExecutor(backend).execute(
        routeAsset(ExecutionMode::Apply, {request}, source));
    QCOMPARE(result.failure().value(), boundary == "load_mesh" ? AssetExecutionFailure::LoadFailed
                                                               : AssetExecutionFailure::OperationFailed);
    QCOMPARE(result.operation(), boundary.toStdString());
    QCOMPARE(result.mutationState(), MutationState::None);
    QVERIFY(result.safeToContinue());
    QVERIFY(result.affectedPath() == source);
    if (boundary != "load_mesh")
        QCOMPARE(result.serviceDetail(), std::string("synthetic Mesh service error"));
    QCOMPARE(backend.meshSaves, 0);
    QCOMPARE(readBytes(source), std::string("original"));
}

void AssetExecutionTests::meshWithoutChanges() {
    QFETCH(int, mode);
    QFETCH(bool, changed);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = std::filesystem::path(directory.path().toStdWString());
    const auto source = root / "actor.nif";
    std::ofstream(source) << "original";
    RecordingBackend backend;
    backend.operationResult = changed ? OperationResult::changed() : OperationResult::unchanged();
    const auto result = AssetExecutor(backend).execute(routeAsset(
        static_cast<ExecutionMode>(mode),
        {RequestedWork::StandardMeshOptimization, RequestedWork::ConvertibleTextureConversion},
        source));
    QVERIFY(result.succeeded());
    QCOMPARE(result.mutationState(), MutationState::None);
    QCOMPARE(backend.meshOptimizations, 1);
    QCOMPARE(backend.meshMaintenances, 1);
    QCOMPARE(backend.meshSaves, 0);
    QCOMPARE(readBytes(source), std::string("original"));
    QCOMPARE(std::distance(std::filesystem::directory_iterator(root),
                           std::filesystem::directory_iterator()), std::ptrdiff_t{1});
}

void AssetExecutionTests::meshRegistrationFailureAfterCommit() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto source = std::filesystem::path(directory.path().toStdWString()) / "actor.nif";
    std::ofstream(source) << "original";
    cao::run::TemporaryArtifactRegistry artifacts;
    RecordingBackend backend;
    backend.meshSave = [&](const std::filesystem::path& staged) {
        // Close the registry to inject its documented terminal-registration contract failure.
        artifacts.performSafetyCleanup();
        std::ofstream(staged) << "saved mesh";
        return true;
    };
    const auto result = AssetExecutor(backend).execute(
        routeAsset(ExecutionMode::Apply, {RequestedWork::StandardMeshOptimization}, source),
        artifacts);
    QCOMPARE(result.failure().value(), AssetExecutionFailure::BackendException);
    QCOMPARE(result.mutationState(), MutationState::Committed);
    QVERIFY(!result.safeToContinue());
    QCOMPARE(result.operation(), std::string("commit_mesh"));
    QCOMPARE(readBytes(source), std::string("saved mesh"));
    QVERIFY(!std::filesystem::exists(backend.savedMeshPath));
}

void AssetExecutionTests::meshStagingFailure() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = std::filesystem::path(directory.path().toStdWString());
    const auto source = root / "actor.nif";
    std::ofstream(source) << "original";
    std::ofstream(root / ".cao-staging") << "unowned";
    RecordingBackend backend;
    const auto result = AssetExecutor(backend).execute(
        routeAsset(ExecutionMode::Apply, {RequestedWork::StandardMeshOptimization}, source));
    QCOMPARE(result.failure().value(), AssetExecutionFailure::StagingFailed);
    QCOMPARE(result.mutationState(), MutationState::None);
    // Recovery cannot prove ownership of the obstructing path, so the run must stop.
    QVERIFY(!result.safeToContinue());
    QCOMPARE(result.operation(), std::string("stage_mesh"));
    QCOMPARE(backend.meshSaves, 0);
    QCOMPARE(readBytes(source), std::string("original"));
    QCOMPARE(readBytes(root / ".cao-staging"), std::string("unowned"));
}

void AssetExecutionTests::meshCommitFailure() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto destination = std::filesystem::path(directory.path().toStdWString()) / "actor.nif";
    // The backend accepts the fixture, but the real filesystem must reject replacing a directory.
    std::filesystem::create_directory(destination);
    std::ofstream(destination / "unowned") << "keep";
    RecordingBackend backend;
    const auto result = AssetExecutor(backend).execute(
        routeAsset(ExecutionMode::Apply, {RequestedWork::StandardMeshOptimization}, destination));
    QCOMPARE(result.failure().value(), AssetExecutionFailure::CommitFailed);
    QCOMPARE(result.mutationState(), MutationState::None);
    QVERIFY(result.safeToContinue());
    QVERIFY(result.affectedPath() == destination);
    QCOMPARE(result.operation(), std::string("commit_mesh"));
    QCOMPARE(readBytes(destination / "unowned"), std::string("keep"));
    QVERIFY(!std::filesystem::exists(backend.savedMeshPath));
}

void AssetExecutionTests::meshVariantSelectsLoadMode_data() {
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

void AssetExecutionTests::meshVariantSelectsLoadMode() {
    QFETCH(QString, path);
    QFETCH(int, variant);
    QFETCH(int, request);

    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto source = std::filesystem::path(directory.path().toStdWString()) /
                        std::filesystem::path(path.toStdWString());
    std::filesystem::create_directories(source.parent_path());
    std::ofstream(source) << "original";
    const auto asset = routeAsset(ExecutionMode::Apply, {static_cast<RequestedWork>(request)}, source);
    RecordingBackend backend;
    const AssetExecutor executor(backend);

    const auto result = executor.execute(asset);

    QVERIFY(result.succeeded());
    QCOMPARE(backend.meshLoads, 1);
    QVERIFY(backend.meshPath == asset.executionPath());
    QCOMPARE(backend.meshVariant.value(), static_cast<MeshVariant>(variant));
}

void AssetExecutionTests::meshOperationsShareOneTransaction_data() {
    QTest::addColumn<bool>("optimize");
    QTest::addColumn<bool>("maintain");
    QTest::addColumn<QString>("path");

    QTest::newRow("optimization only") << true << false << QStringLiteral("Meshes/Actor.nif");
    QTest::newRow("maintenance only") << false << true << QStringLiteral("Meshes/Actor.nif");
    QTest::newRow("optimization and maintenance")
        << true << true << QStringLiteral("Meshes/Actor.nif");
}

void AssetExecutionTests::meshOperationsShareOneTransaction() {
    QFETCH(bool, optimize);
    QFETCH(bool, maintain);
    QFETCH(QString, path);

    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto executionPath = std::filesystem::path(directory.path().toStdWString()) /
                               std::filesystem::path(path.toStdWString());
    std::filesystem::create_directories(executionPath.parent_path());
    std::ofstream(executionPath) << "original";
    const auto asset =
        !optimize  ? routeAsset(ExecutionMode::Apply, {RequestedWork::ConvertibleTextureConversion},
                                executionPath)
        : maintain ? routeAsset(ExecutionMode::Apply,
                                {RequestedWork::StandardMeshOptimization,
                                 RequestedWork::ConvertibleTextureConversion},
                                executionPath)
                   : routeAsset(ExecutionMode::Apply, {RequestedWork::StandardMeshOptimization},
                                executionPath);
    RecordingBackend backend;
    bool reservedBeforeSave = false;
    backend.meshSave = [&](const std::filesystem::path& staged) {
        reservedBeforeSave = std::filesystem::is_regular_file(staged);
        if (staged == executionPath || staged.parent_path() != executionPath.parent_path() ||
            !cao::run::isStagingName(staged) || readBytes(executionPath) != "original")
            throw std::runtime_error("Mesh changed before staged save completed");
        std::ofstream(staged) << "saved mesh";
        return true;
    };
    const AssetExecutor executor(backend);

    const auto result = executor.execute(asset);

    QVERIFY(result.succeeded());
    QCOMPARE(backend.meshLoads, 1);
    QCOMPARE(backend.meshOptimizations, optimize ? 1 : 0);
    QCOMPARE(backend.meshMaintenances, maintain ? 1 : 0);
    QCOMPARE(backend.meshSaves, 1);
    QVERIFY(reservedBeforeSave);
    QCOMPARE(result.mutationState(), MutationState::Committed);
    QCOMPARE(readBytes(executionPath), std::string("saved mesh"));
    QVERIFY(!std::filesystem::exists(backend.savedMeshPath));
}

void AssetExecutionTests::dryRunMeshMaintenanceDoesNotMutate() {
    const auto asset =
        routeAsset(ExecutionMode::DryRun, {RequestedWork::ConvertibleTextureConversion},
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

void AssetExecutionTests::animationExecution_data() {
    QTest::addColumn<int>("mode");

    QTest::newRow("Apply") << static_cast<int>(ExecutionMode::Apply);
    QTest::newRow("Dry Run") << static_cast<int>(ExecutionMode::DryRun);
}

void AssetExecutionTests::animationExecution() {
    QFETCH(int, mode);

    const auto executionMode = static_cast<ExecutionMode>(mode);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto source = std::filesystem::path(directory.path().toStdWString()) / "Walk.hkx";
    std::ofstream(source) << "original";
    const auto asset = routeAsset(executionMode, {RequestedWork::AnimationOptimization}, source);
    RecordingBackend backend;
    const AssetExecutor executor(backend);

    const auto result = executor.execute(asset);

    QVERIFY(result.succeeded());
    QCOMPARE(result.mutationState(), executionMode == ExecutionMode::Apply
                                         ? MutationState::Committed : MutationState::None);
    QCOMPARE(readBytes(source), executionMode == ExecutionMode::Apply
                                    ? std::string("converted") : std::string("original"));
    QCOMPARE(backend.animationOptimizations, 1);
    QVERIFY(backend.animationPath == asset.executionPath());
    QCOMPARE(backend.animationMode.value(), executionMode);
}

void AssetExecutionTests::animationFailureIsReported() {
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto source = std::filesystem::path(directory.path().toStdWString()) / "Walk.hkx";
    std::ofstream(source) << "original";
    const auto asset = routeAsset(ExecutionMode::Apply, {RequestedWork::AnimationOptimization}, source);
    RecordingBackend backend;
    backend.operationResult = OperationResult::failed("synthetic animation failure");
    const AssetExecutor executor(backend);

    const auto result = executor.execute(asset);

    QVERIFY(!result.succeeded());
    QCOMPARE(result.failure().value(), AssetExecutionFailure::OperationFailed);
    QCOMPARE(result.serviceDetail(), std::string("synthetic animation failure"));
    QCOMPARE(result.mutationState(), MutationState::None);
    QVERIFY(result.safeToContinue());
    QCOMPARE(readBytes(source), std::string("original"));
}

void AssetExecutionTests::animationStagedOutcomes_data() {
    QTest::addColumn<int>("scenario");
    QTest::newRow("partial output failure") << 0;
    QTest::newRow("empty output success") << 1;
    QTest::newRow("standard exception") << 2;
    QTest::newRow("unknown exception") << 3;
    QTest::newRow("unchanged output") << 4;
}

void AssetExecutionTests::animationStagedOutcomes() {
    QFETCH(int, scenario);
    QTemporaryDir directory;
    QVERIFY(directory.isValid());
    const auto root = std::filesystem::path(directory.path().toStdWString());
    const auto source = root / "Walk.HKX";
    std::ofstream(source) << "original";
    RecordingBackend backend;
    backend.animationSave = [&](const std::filesystem::path& staged) -> OperationResult {
        if (scenario == 1) return OperationResult::changed();
        std::ofstream(staged) << "partial";
        if (scenario == 2) throw std::runtime_error("converter exception");
        if (scenario == 3) throw 42;
        if (scenario == 4) return OperationResult::unchanged();
        return OperationResult::failed("converter failed");
    };
    const auto result = AssetExecutor(backend).execute(
        routeAsset(ExecutionMode::Apply, {RequestedWork::AnimationOptimization}, source));
    QCOMPARE(readBytes(source), std::string("original"));
    QCOMPARE(result.mutationState(), MutationState::None);
    QCOMPARE(result.safeToContinue(), scenario != 2 && scenario != 3);
    QVERIFY(result.cleanupFailures().empty());
    QVERIFY(backend.savedAnimationPath.parent_path() == source.parent_path());
    QVERIFY(cao::run::isStagingName(backend.savedAnimationPath));
    QVERIFY(!std::filesystem::exists(backend.savedAnimationPath));
    if (scenario == 4) {
        QVERIFY(result.succeeded());
        return;
    }
    QCOMPARE(result.failure().value(), scenario == 0 ? AssetExecutionFailure::OperationFailed
                                        : scenario == 1 ? AssetExecutionFailure::SaveFailed
                                                        : AssetExecutionFailure::BackendException);
    QCOMPARE(result.failureCategory().value(),
             scenario == 0 ? cao::execution::ExecutionFailureCategory::Backend
             : scenario == 1 ? cao::execution::ExecutionFailureCategory::Filesystem
                             : cao::execution::ExecutionFailureCategory::Contract);
    QCOMPARE(result.operation(), std::string(scenario == 1 ? "save_animation" : "optimize_animation"));
    QVERIFY(result.affectedPath() == source);
    QCOMPARE(result.phase(), cao::run::RunPhase::ProcessingAssets);
    if (scenario == 2) QCOMPARE(result.serviceDetail(), std::string("converter exception"));
}

void AssetExecutionTests::executionFailurePreservesRoutedDecision() {
    const auto asset = routeAsset(ExecutionMode::Apply, {RequestedWork::StandardMeshOptimization},
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

void AssetExecutionTests::archiveIsNotOwnedByAssetExecutor() {
    const auto asset = routeAsset(ExecutionMode::Apply, {RequestedWork::ArchiveExtraction},
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

/// Dispatches isolated crash workers before normal Qt test argument parsing.
int main(int argc, char** argv) {
    QCoreApplication application(argc, argv);
    const auto arguments = application.arguments();
    if (arguments.size() == 5 && arguments.at(1) == "--texture-crash-worker")
        return textureCrashWorker(std::filesystem::path(arguments.at(2).toStdWString()),
                                  std::filesystem::path(arguments.at(3).toStdWString()),
                                  arguments.at(4));
    AssetExecutionTests tests;
    return QTest::qExec(&tests, argc, argv);
}

#include "AssetExecutionTests.moc"
