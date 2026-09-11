#include "AssetRouting/AssetRouter.h"

#include <QtTest>

#include <algorithm>
#include <filesystem>
#include <span>
#include <type_traits>
#include <utility>
#include <variant>
#include <vector>

using cao::routing::AssetRouter;
using cao::routing::AmbiguousArchiveExtension;
using cao::routing::AssetKind;
using cao::routing::AnimationAsset;
using cao::routing::ArchiveAsset;
using cao::routing::AssetIdentity;
using cao::routing::AssetOperation;
using cao::routing::OptimizerTarget;
using cao::routing::ExecutionMode;
using cao::routing::MalformedArchiveExtension;
using cao::routing::MalformedArchiveExtensionReason;
using cao::routing::MeshAsset;
using cao::routing::MeshVariant;
using cao::routing::MissingArchiveExtension;
using cao::routing::ProfileCapability;
using cao::routing::ProfileCapabilities;
using cao::routing::RequestedWork;
using cao::routing::RoutedAsset;
using cao::routing::RoutedAssetReferences;
using cao::routing::RoutingPolicy;
using cao::routing::RoutingDecision;
using cao::routing::RoutingLedger;
using cao::routing::RoutedAssetPhase;
using cao::routing::RoutingPolicyRequest;
using cao::routing::SkipReason;
using cao::routing::SkippedAsset;
using cao::routing::TextureVariant;
using cao::routing::TextureAsset;
using cao::routing::UnsupportedDerivedOperation;
using cao::routing::UnsupportedRequestedAssetKind;
using cao::routing::UnsupportedRequestedAssetVariant;
using cao::routing::UnsupportedDecision;

static_assert(!std::is_copy_assignable_v<RoutingPolicy>);
static_assert(!std::is_move_assignable_v<RoutingPolicy>);
static_assert(!std::is_default_constructible_v<RoutingPolicy>);
static_assert(!std::is_constructible_v<TextureAsset, MeshVariant>);
static_assert(!std::is_constructible_v<MeshAsset, TextureVariant>);
static_assert(std::variant_size_v<AssetIdentity> == 4);
static_assert(std::variant_size_v<RoutingDecision> == 3);
static_assert(std::is_same_v<decltype(std::declval<const RoutingLedger &>().routedAssets()),
                             std::span<const RoutedAsset>>);
static_assert(std::is_same_v<RoutedAssetReferences::value_type,
                             std::reference_wrapper<const RoutedAsset>>);

template<typename Decision>
concept ExposesRoutedExecutionFacts = requires(const Decision &decision) {
    decision.phase();
    decision.target();
    decision.executionMode();
    decision.operations();
};

static_assert(ExposesRoutedExecutionFacts<RoutedAsset>);
static_assert(!ExposesRoutedExecutionFacts<SkippedAsset>);
static_assert(!ExposesRoutedExecutionFacts<UnsupportedDecision>);

enum class ExpectedIdentity
{
    NativeTexture,
    ConvertibleTexture,
    StandardMesh,
    TerrainMesh,
    Animation,
    Archive
};

enum class SkipCase
{
    DryRunArchive,
    DisabledTexture,
    ExcludedNativeTexture,
    ExcludedConvertibleTexture,
    ExcludedTerrainMesh,
    DisabledAnimation,
    DisabledArchive
};

/// Builds the shared all-work router used by recognition and carried-execution-fact matrices.
AssetRouter fullyEnabledRouter()
{
    const auto request = RoutingPolicyRequest::forWork(ExecutionMode::Apply,
                                             {RequestedWork::NativeTextureOptimization,
                                              RequestedWork::ConvertibleTextureConversion,
                                              RequestedWork::StandardMeshOptimization,
                                              RequestedWork::TerrainMeshOptimization,
                                              RequestedWork::AnimationOptimization,
                                              RequestedWork::ArchiveExtraction});
    const auto capabilities = ProfileCapabilities::define(
        ".ba2", {ProfileCapability::NativeTextureOptimization,
                  ProfileCapability::ConvertibleTextureConversion,
                  ProfileCapability::StandardMeshOptimization,
                  ProfileCapability::TerrainMeshOptimization,
                  ProfileCapability::AnimationOptimization,
                  ProfileCapability::ArchiveExtraction,
                  ProfileCapability::MeshReferenceMaintenance});
    const auto result = RoutingPolicy::compile(request, capabilities);
    if (!result.hasPolicy())
        qFatal("The known-valid complete Routing Policy failed to compile");
    return AssetRouter(*result.policy());
}

class AssetRoutingTests final : public QObject
{
    Q_OBJECT

private slots:
    /// Verifies valid dedicated request and Profile Capability values produce an immutable policy.
    void compilesCompleteRoutingPolicy();

    /// Verifies compilation returns every request, capability, and derived-work conflict together.
    void returnsAllPolicyValidationErrors();

    /// Defines missing, malformed, and ambiguous profile Archive extension cases.
    void profileArchiveExtensionValidation_data();

    /// Verifies invalid Archive definitions produce structured outcomes instead of policies or exceptions.
    void profileArchiveExtensionValidation();

    /// Defines paths that distinguish terminal, case-insensitive DDS matching from incidental suffix text.
    void nativeTextureTracer_data();

    /// Verifies the public router decision, native Texture identity, and exact execution-path ownership.
    void nativeTextureTracer();

    /// Defines every supported extension and its kind-specific Asset identity.
    void supportedAssetRecognition_data();

    /// Verifies supported terminal extensions map case-insensitively to the correct Asset identity.
    void supportedAssetRecognition();

    /// Defines the complete execution facts carried for every eligible Asset identity.
    void routedAssetFacts_data();

    /// Verifies Routed Assets carry phase, target, mode, and closed operations without policy reinterpretation.
    void routedAssetFacts();

    /// Defines recognized exclusions that exercise deterministic Skip Reason precedence.
    void skipReasonPrecedence_data();

    /// Verifies recognized Assets are skipped with the highest-precedence stable reason.
    void skipReasonPrecedence();

    /// Verifies conversion-only Texture work routes TGA and derives maintenance for both Mesh Variants.
    void conversionOnlyRoutesTextureAndMaintainsMeshes();

    /// Verifies Dry Run preserves eligible Loose Asset operations while excluding Archive extraction.
    void dryRunPreservesLooseAssetOperations();

    /// Verifies routing neither depends on nor mutates path existence and file contents.
    void routingIgnoresFilesystemState();

    /// Verifies batch routing owns Routed Assets after borrowed input expires while preserving order and duplicates.
    void batchRoutingOwnsRoutedAssetsInInputOrder();

    /// Verifies unsupported paths are omitted while every recognized exclusion is counted by stable Skip Reason.
    void batchRoutingOmitsUnsupportedPathsAndCountsSkips();

    /// Verifies phase and target queries expose const Routed Assets in original relative order.
    void ledgerQueriesRoutedAssetsByPhaseAndTarget();

    /// Verifies a Routed Asset contributes one work entry even when it carries multiple operations.
    void ledgerWorkTotalCountsRoutedAssetsNotOperations();

    /// Verifies batch aggregation matches single-path decisions for every Routing Disposition.
    void batchRoutingMatchesSingleAssetDecisions();

    /// Verifies an empty borrowed batch produces an empty ledger without exclusions.
    void emptyBatchProducesEmptyLedger();
};

void AssetRoutingTests::compilesCompleteRoutingPolicy()
{
    const auto request = RoutingPolicyRequest::forWork(ExecutionMode::DryRun,
                                             {RequestedWork::NativeTextureOptimization,
                                              RequestedWork::ConvertibleTextureConversion,
                                              RequestedWork::StandardMeshOptimization,
                                              RequestedWork::TerrainMeshOptimization,
                                              RequestedWork::AnimationOptimization,
                                              RequestedWork::ArchiveExtraction});
    const auto capabilities = ProfileCapabilities::define(
        ".Ba2", {ProfileCapability::NativeTextureOptimization,
                  ProfileCapability::ConvertibleTextureConversion,
                  ProfileCapability::StandardMeshOptimization,
                  ProfileCapability::TerrainMeshOptimization,
                  ProfileCapability::AnimationOptimization,
                  ProfileCapability::ArchiveExtraction,
                  ProfileCapability::MeshReferenceMaintenance});

    const auto result = RoutingPolicy::compile(request, capabilities);

    QVERIFY(result.hasPolicy());
    QCOMPARE(result.errors().size(), std::size_t{0});
    const auto *policy = result.policy();
    QVERIFY(policy != nullptr);
    QCOMPARE(policy->executionMode(), ExecutionMode::DryRun);
    for (const auto work : {RequestedWork::NativeTextureOptimization,
                            RequestedWork::ConvertibleTextureConversion,
                            RequestedWork::StandardMeshOptimization,
                            RequestedWork::TerrainMeshOptimization,
                            RequestedWork::AnimationOptimization,
                            RequestedWork::ArchiveExtraction}) {
        QVERIFY(policy->requests(work));
    }
    QVERIFY(policy->maintainsMeshReferences());
    QCOMPARE(policy->archiveExtension(), std::string(".ba2"));
}

void AssetRoutingTests::returnsAllPolicyValidationErrors()
{
    const auto request = RoutingPolicyRequest::forWork(ExecutionMode::Apply,
                                             {RequestedWork::ConvertibleTextureConversion,
                                              RequestedWork::TerrainMeshOptimization,
                                              RequestedWork::AnimationOptimization});
    const auto capabilities = ProfileCapabilities::withoutArchiveExtension(
        {ProfileCapability::NativeTextureOptimization,
         ProfileCapability::StandardMeshOptimization});

    const auto result = RoutingPolicy::compile(request, capabilities);

    QVERIFY(!result.hasPolicy());
    QVERIFY(result.policy() == nullptr);
    const auto errors = result.errors();
    QCOMPARE(errors.size(), std::size_t{5});
    QCOMPARE(std::count_if(errors.begin(), errors.end(), [](const auto &error) {
                 return std::holds_alternative<MissingArchiveExtension>(error);
             }),
             1);
    QCOMPARE(std::count_if(errors.begin(), errors.end(), [](const auto &error) {
                 return std::holds_alternative<UnsupportedRequestedAssetKind>(error);
             }),
             1);
    QCOMPARE(std::count_if(errors.begin(), errors.end(), [](const auto &error) {
                 return std::holds_alternative<UnsupportedRequestedAssetVariant>(error);
             }),
             2);
    QVERIFY(std::any_of(errors.begin(), errors.end(), [](const auto &error) {
        const auto *unsupported = std::get_if<UnsupportedRequestedAssetKind>(&error);
        return unsupported != nullptr
            && unsupported->request == RequestedWork::AnimationOptimization
            && unsupported->kind == AssetKind::Animation;
    }));
    QVERIFY(std::any_of(errors.begin(), errors.end(), [](const auto &error) {
        const auto *unsupported = std::get_if<UnsupportedRequestedAssetVariant>(&error);
        return unsupported != nullptr
            && unsupported->request == RequestedWork::ConvertibleTextureConversion
            && std::holds_alternative<TextureVariant>(unsupported->variant)
            && std::get<TextureVariant>(unsupported->variant) == TextureVariant::Convertible;
    }));
    QVERIFY(std::any_of(errors.begin(), errors.end(), [](const auto &error) {
        const auto *unsupported = std::get_if<UnsupportedRequestedAssetVariant>(&error);
        return unsupported != nullptr
            && unsupported->request == RequestedWork::TerrainMeshOptimization
            && std::holds_alternative<MeshVariant>(unsupported->variant)
            && std::get<MeshVariant>(unsupported->variant) == MeshVariant::Terrain;
    }));
    QCOMPARE(std::count_if(errors.begin(), errors.end(), [](const auto &error) {
                 const auto *unsupported = std::get_if<UnsupportedDerivedOperation>(&error);
                 return unsupported != nullptr
                     && unsupported->cause == RequestedWork::ConvertibleTextureConversion
                     && unsupported->operation == AssetOperation::MeshReferenceMaintenance;
             }),
             1);
}

void AssetRoutingTests::profileArchiveExtensionValidation_data()
{
    QTest::addColumn<QString>("archiveExtension");
    QTest::addColumn<int>("errorKind");
    QTest::addColumn<int>("malformedReason");

    constexpr int missing = 0;
    constexpr int malformed = 1;
    constexpr int ambiguous = 2;
    constexpr int noMalformedReason = -1;

    QTest::newRow("empty") << QString() << missing << noMalformedReason;
    QTest::newRow("missing leading period")
        << QStringLiteral("bsa") << malformed
        << static_cast<int>(MalformedArchiveExtensionReason::MissingLeadingPeriod);
    QTest::newRow("empty suffix")
        << QStringLiteral(".") << malformed
        << static_cast<int>(MalformedArchiveExtensionReason::EmptySuffix);
    QTest::newRow("path separator")
        << QStringLiteral(".bs/a") << malformed
        << static_cast<int>(MalformedArchiveExtensionReason::InvalidCharacter);
    QTest::newRow("multiple periods")
        << QStringLiteral(".bsa.backup") << malformed
        << static_cast<int>(MalformedArchiveExtensionReason::InvalidCharacter);
    QTest::newRow("case-insensitive built-in collision")
        << QStringLiteral(".DdS") << ambiguous << noMalformedReason;
}

void AssetRoutingTests::profileArchiveExtensionValidation()
{
    QFETCH(QString, archiveExtension);
    QFETCH(int, errorKind);
    QFETCH(int, malformedReason);

    const auto request = RoutingPolicyRequest::optimizeNativeTextures();
    const auto capabilities = ProfileCapabilities::define(
        archiveExtension.toStdString(), {ProfileCapability::NativeTextureOptimization});

    const auto result = RoutingPolicy::compile(request, capabilities);

    QVERIFY(!result.hasPolicy());
    QCOMPARE(result.errors().size(), std::size_t{1});
    const auto &error = result.errors().front();
    if (errorKind == 0) {
        QVERIFY(std::holds_alternative<MissingArchiveExtension>(error));
    } else if (errorKind == 1) {
        const auto *malformed = std::get_if<MalformedArchiveExtension>(&error);
        QVERIFY(malformed != nullptr);
        QCOMPARE(malformed->extension, archiveExtension.toStdString());
        QCOMPARE(static_cast<int>(malformed->reason), malformedReason);
    } else {
        const auto *ambiguous = std::get_if<AmbiguousArchiveExtension>(&error);
        QVERIFY(ambiguous != nullptr);
        QCOMPARE(ambiguous->extension, archiveExtension.toStdString());
        QCOMPARE(ambiguous->conflictingExtension, std::string(".dds"));
        QCOMPARE(ambiguous->conflictingKind, AssetKind::Texture);
    }
}

void AssetRoutingTests::nativeTextureTracer_data()
{
    QTest::addColumn<QString>("executionPath");
    QTest::addColumn<bool>("shouldRoute");

    QTest::newRow("mixed-case terminal DDS") << QStringLiteral("mods/Textures/Armor.DdS") << true;
    QTest::newRow("lexically unnormalized caller path")
        << QStringLiteral("mods/Textures/../Textures/Armor.DdS") << true;
    QTest::newRow("DDS only in parent segment") << QStringLiteral("mods/textures.dds/readme") << false;
    QTest::newRow("DDS before another terminal extension")
        << QStringLiteral("mods/Textures/Armor.dds.backup") << false;
    QTest::newRow("extension text without terminal period")
        << QStringLiteral("mods/Textures/DDS") << false;
    QTest::newRow("unknown terminal extension") << QStringLiteral("mods/Textures/readme.txt") << false;
}

void AssetRoutingTests::nativeTextureTracer()
{
    QFETCH(QString, executionPath);
    QFETCH(bool, shouldRoute);

    const auto request = RoutingPolicyRequest::optimizeNativeTextures();
    const auto capabilities = ProfileCapabilities::define(
        ".bsa", {ProfileCapability::NativeTextureOptimization});
    const auto result = RoutingPolicy::compile(request, capabilities);
    QVERIFY(result.hasPolicy());
    const AssetRouter router(*result.policy());
    const std::filesystem::path callerPath(executionPath.toStdWString());

    const auto decision = router.route(callerPath);

    if (!shouldRoute) {
        QVERIFY(std::holds_alternative<UnsupportedDecision>(decision));
        return;
    }

    QVERIFY(std::holds_alternative<RoutedAsset>(decision));
    const auto &routedAsset = std::get<RoutedAsset>(decision);
    QVERIFY(routedAsset.executionPath() == callerPath);
    QVERIFY(std::holds_alternative<TextureAsset>(routedAsset.identity()));
    QVERIFY(std::get<TextureAsset>(routedAsset.identity()).variant() == TextureVariant::Native);
}

void AssetRoutingTests::supportedAssetRecognition_data()
{
    QTest::addColumn<QString>("executionPath");
    QTest::addColumn<int>("expectedIdentity");

    QTest::newRow("native Texture")
        << QStringLiteral("Root/Textures/Native.DdS")
        << static_cast<int>(ExpectedIdentity::NativeTexture);
    QTest::newRow("convertible Texture")
        << QStringLiteral("Root/Textures/Convertible.TgA")
        << static_cast<int>(ExpectedIdentity::ConvertibleTexture);
    QTest::newRow("standard Mesh")
        << QStringLiteral("Root/Meshes/Standard.NiF")
        << static_cast<int>(ExpectedIdentity::StandardMesh);
    QTest::newRow("BTR terrain Mesh")
        << QStringLiteral("Root/Meshes/Terrain.BtR")
        << static_cast<int>(ExpectedIdentity::TerrainMesh);
    QTest::newRow("BTO terrain Mesh")
        << QStringLiteral("Root/Meshes/Terrain.BtO")
        << static_cast<int>(ExpectedIdentity::TerrainMesh);
    QTest::newRow("Animation")
        << QStringLiteral("Root/Animations/Behavior.HkX")
        << static_cast<int>(ExpectedIdentity::Animation);
    QTest::newRow("profile Archive")
        << QStringLiteral("Root/Archives/Assets.Ba2")
        << static_cast<int>(ExpectedIdentity::Archive);
}

void AssetRoutingTests::supportedAssetRecognition()
{
    QFETCH(QString, executionPath);
    QFETCH(int, expectedIdentity);

    const auto router = fullyEnabledRouter();
    const std::filesystem::path callerPath(executionPath.toStdWString());

    const auto decision = router.route(callerPath);

    QVERIFY(std::holds_alternative<RoutedAsset>(decision));
    const auto &routedAsset = std::get<RoutedAsset>(decision);
    QVERIFY(routedAsset.executionPath() == callerPath);
    const auto &identity = routedAsset.identity();
    switch (static_cast<ExpectedIdentity>(expectedIdentity)) {
    case ExpectedIdentity::NativeTexture:
        QCOMPARE(routedAsset.kind(), AssetKind::Texture);
        QVERIFY(std::holds_alternative<TextureAsset>(identity));
        QCOMPARE(std::get<TextureAsset>(identity).variant(), TextureVariant::Native);
        break;
    case ExpectedIdentity::ConvertibleTexture:
        QCOMPARE(routedAsset.kind(), AssetKind::Texture);
        QVERIFY(std::holds_alternative<TextureAsset>(identity));
        QCOMPARE(std::get<TextureAsset>(identity).variant(), TextureVariant::Convertible);
        break;
    case ExpectedIdentity::StandardMesh:
        QCOMPARE(routedAsset.kind(), AssetKind::Mesh);
        QVERIFY(std::holds_alternative<MeshAsset>(identity));
        QCOMPARE(std::get<MeshAsset>(identity).variant(), MeshVariant::Standard);
        break;
    case ExpectedIdentity::TerrainMesh:
        QCOMPARE(routedAsset.kind(), AssetKind::Mesh);
        QVERIFY(std::holds_alternative<MeshAsset>(identity));
        QCOMPARE(std::get<MeshAsset>(identity).variant(), MeshVariant::Terrain);
        break;
    case ExpectedIdentity::Animation:
        QCOMPARE(routedAsset.kind(), AssetKind::Animation);
        QVERIFY(std::holds_alternative<AnimationAsset>(identity));
        break;
    case ExpectedIdentity::Archive:
        QCOMPARE(routedAsset.kind(), AssetKind::Archive);
        QVERIFY(std::holds_alternative<ArchiveAsset>(identity));
        break;
    }
}

void AssetRoutingTests::routedAssetFacts_data()
{
    QTest::addColumn<QString>("executionPath");
    QTest::addColumn<int>("phase");
    QTest::addColumn<int>("target");
    QTest::addColumn<bool>("extraction");
    QTest::addColumn<bool>("optimization");
    QTest::addColumn<bool>("conversion");
    QTest::addColumn<bool>("meshReferenceMaintenance");

    QTest::newRow("native Texture")
        << QStringLiteral("Textures/Native.dds")
        << static_cast<int>(RoutedAssetPhase::LooseAssetProcessing)
        << static_cast<int>(OptimizerTarget::Texture)
        << false << true << false << false;
    QTest::newRow("convertible Texture")
        << QStringLiteral("Textures/Convertible.tga")
        << static_cast<int>(RoutedAssetPhase::LooseAssetProcessing)
        << static_cast<int>(OptimizerTarget::Texture)
        << false << false << true << false;
    QTest::newRow("standard Mesh")
        << QStringLiteral("Meshes/Standard.nif")
        << static_cast<int>(RoutedAssetPhase::LooseAssetProcessing)
        << static_cast<int>(OptimizerTarget::Mesh)
        << false << true << false << true;
    QTest::newRow("terrain Mesh")
        << QStringLiteral("Meshes/Terrain.btr")
        << static_cast<int>(RoutedAssetPhase::LooseAssetProcessing)
        << static_cast<int>(OptimizerTarget::Mesh)
        << false << true << false << true;
    QTest::newRow("Animation")
        << QStringLiteral("Animations/Behavior.hkx")
        << static_cast<int>(RoutedAssetPhase::LooseAssetProcessing)
        << static_cast<int>(OptimizerTarget::Animation)
        << false << true << false << false;
    QTest::newRow("Archive")
        << QStringLiteral("Archives/Assets.ba2")
        << static_cast<int>(RoutedAssetPhase::ArchiveExtraction)
        << static_cast<int>(OptimizerTarget::Archive)
        << true << false << false << false;
}

void AssetRoutingTests::routedAssetFacts()
{
    QFETCH(QString, executionPath);
    QFETCH(int, phase);
    QFETCH(int, target);
    QFETCH(bool, extraction);
    QFETCH(bool, optimization);
    QFETCH(bool, conversion);
    QFETCH(bool, meshReferenceMaintenance);

    const auto router = fullyEnabledRouter();

    const auto decision = router.route(std::filesystem::path(executionPath.toStdWString()));

    QVERIFY(std::holds_alternative<RoutedAsset>(decision));
    const auto &routedAsset = std::get<RoutedAsset>(decision);
    QCOMPARE(static_cast<int>(routedAsset.phase()), phase);
    QCOMPARE(static_cast<int>(routedAsset.target()), target);
    QCOMPARE(routedAsset.executionMode(), ExecutionMode::Apply);
    QCOMPARE(routedAsset.operations().contains(AssetOperation::Extraction), extraction);
    QCOMPARE(routedAsset.operations().contains(AssetOperation::Optimization), optimization);
    QCOMPARE(routedAsset.operations().contains(AssetOperation::Conversion), conversion);
    QCOMPARE(routedAsset.operations().contains(AssetOperation::MeshReferenceMaintenance),
             meshReferenceMaintenance);
}

void AssetRoutingTests::skipReasonPrecedence_data()
{
    QTest::addColumn<int>("skipCaseValue");
    QTest::addColumn<QString>("executionPath");
    QTest::addColumn<int>("skipReason");

    QTest::newRow("disabled phase precedes disabled Archive kind")
        << static_cast<int>(SkipCase::DryRunArchive) << QStringLiteral("Archives/Assets.ba2")
        << static_cast<int>(SkipReason::DisabledPhase);
    QTest::newRow("disabled Texture kind")
        << static_cast<int>(SkipCase::DisabledTexture) << QStringLiteral("Textures/Native.dds")
        << static_cast<int>(SkipReason::DisabledAssetKind);
    QTest::newRow("excluded native Texture Variant")
        << static_cast<int>(SkipCase::ExcludedNativeTexture) << QStringLiteral("Textures/Native.dds")
        << static_cast<int>(SkipReason::ExcludedAssetVariant);
    QTest::newRow("excluded convertible Texture Variant")
        << static_cast<int>(SkipCase::ExcludedConvertibleTexture) << QStringLiteral("Textures/Convertible.tga")
        << static_cast<int>(SkipReason::ExcludedAssetVariant);
    QTest::newRow("excluded terrain Mesh Variant")
        << static_cast<int>(SkipCase::ExcludedTerrainMesh) << QStringLiteral("Meshes/Terrain.bto")
        << static_cast<int>(SkipReason::ExcludedAssetVariant);
    QTest::newRow("disabled Animation kind")
        << static_cast<int>(SkipCase::DisabledAnimation) << QStringLiteral("Animations/Behavior.hkx")
        << static_cast<int>(SkipReason::DisabledAssetKind);
    QTest::newRow("disabled Archive kind in Apply mode")
        << static_cast<int>(SkipCase::DisabledArchive) << QStringLiteral("Archives/Assets.ba2")
        << static_cast<int>(SkipReason::DisabledAssetKind);
}

void AssetRoutingTests::skipReasonPrecedence()
{
    QFETCH(int, skipCaseValue);
    QFETCH(QString, executionPath);
    QFETCH(int, skipReason);

    const auto skipCase = static_cast<SkipCase>(skipCaseValue);
    const auto request = [skipCase] {
        if (skipCase == SkipCase::DryRunArchive)
            return RoutingPolicyRequest::forWork(ExecutionMode::DryRun, {});
        if (skipCase == SkipCase::ExcludedNativeTexture) {
            return RoutingPolicyRequest::forWork(ExecutionMode::Apply,
                                       {RequestedWork::ConvertibleTextureConversion});
        }
        if (skipCase == SkipCase::ExcludedConvertibleTexture) {
            return RoutingPolicyRequest::forWork(ExecutionMode::Apply,
                                       {RequestedWork::NativeTextureOptimization});
        }
        if (skipCase == SkipCase::ExcludedTerrainMesh) {
            return RoutingPolicyRequest::forWork(ExecutionMode::Apply,
                                       {RequestedWork::StandardMeshOptimization});
        }
        return RoutingPolicyRequest::forWork(ExecutionMode::Apply, {});
    }();
    const auto capabilities = ProfileCapabilities::define(
        ".ba2", {ProfileCapability::NativeTextureOptimization,
                  ProfileCapability::ConvertibleTextureConversion,
                  ProfileCapability::StandardMeshOptimization,
                  ProfileCapability::TerrainMeshOptimization,
                  ProfileCapability::AnimationOptimization,
                  ProfileCapability::ArchiveExtraction,
                  ProfileCapability::MeshReferenceMaintenance});
    const auto result = RoutingPolicy::compile(request, capabilities);
    QVERIFY(result.hasPolicy());
    const AssetRouter router(*result.policy());
    const std::filesystem::path callerPath(executionPath.toStdWString());

    const auto decision = router.route(callerPath);

    QVERIFY(std::holds_alternative<SkippedAsset>(decision));
    const auto &skippedAsset = std::get<SkippedAsset>(decision);
    QVERIFY(skippedAsset.executionPath() == callerPath);
    QCOMPARE(static_cast<int>(skippedAsset.reason()), skipReason);
}

void AssetRoutingTests::conversionOnlyRoutesTextureAndMaintainsMeshes()
{
    const auto request = RoutingPolicyRequest::forWork(
        ExecutionMode::Apply, {RequestedWork::ConvertibleTextureConversion});
    const auto capabilities = ProfileCapabilities::define(
        ".ba2", {ProfileCapability::ConvertibleTextureConversion,
                  ProfileCapability::MeshReferenceMaintenance});
    const auto result = RoutingPolicy::compile(request, capabilities);
    QVERIFY(result.hasPolicy());
    const AssetRouter router(*result.policy());

    for (const auto &path : {std::filesystem::path(L"Textures/Convertible.tga"),
                             std::filesystem::path(L"Meshes/Standard.nif"),
                             std::filesystem::path(L"Meshes/Terrain.btr"),
                             std::filesystem::path(L"Meshes/Terrain.bto")}) {
        const auto decision = router.route(path);
        QVERIFY(std::holds_alternative<RoutedAsset>(decision));
        const auto &routedAsset = std::get<RoutedAsset>(decision);
        QVERIFY(routedAsset.executionPath() == path);
        if (routedAsset.kind() == AssetKind::Texture) {
            QVERIFY(routedAsset.operations().contains(AssetOperation::Conversion));
            QVERIFY(!routedAsset.operations().contains(AssetOperation::Optimization));
            QVERIFY(!routedAsset.operations().contains(AssetOperation::MeshReferenceMaintenance));
        } else {
            QCOMPARE(routedAsset.kind(), AssetKind::Mesh);
            QVERIFY(routedAsset.operations().contains(AssetOperation::MeshReferenceMaintenance));
            QVERIFY(!routedAsset.operations().contains(AssetOperation::Optimization));
        }
    }
}

void AssetRoutingTests::dryRunPreservesLooseAssetOperations()
{
    const auto request = RoutingPolicyRequest::forWork(ExecutionMode::DryRun,
                                             {RequestedWork::ConvertibleTextureConversion,
                                              RequestedWork::StandardMeshOptimization,
                                              RequestedWork::ArchiveExtraction});
    const auto capabilities = ProfileCapabilities::define(
        ".ba2", {ProfileCapability::ConvertibleTextureConversion,
                  ProfileCapability::StandardMeshOptimization,
                  ProfileCapability::ArchiveExtraction,
                  ProfileCapability::MeshReferenceMaintenance});
    const auto result = RoutingPolicy::compile(request, capabilities);
    QVERIFY(result.hasPolicy());
    const AssetRouter router(*result.policy());

    const auto textureDecision = router.route(std::filesystem::path(L"Textures/Source.tga"));
    QVERIFY(std::holds_alternative<RoutedAsset>(textureDecision));
    const auto &texture = std::get<RoutedAsset>(textureDecision);
    QCOMPARE(texture.executionMode(), ExecutionMode::DryRun);
    QVERIFY(texture.operations().contains(AssetOperation::Conversion));

    const auto meshDecision = router.route(std::filesystem::path(L"Meshes/Model.nif"));
    QVERIFY(std::holds_alternative<RoutedAsset>(meshDecision));
    const auto &mesh = std::get<RoutedAsset>(meshDecision);
    QCOMPARE(mesh.executionMode(), ExecutionMode::DryRun);
    QVERIFY(mesh.operations().contains(AssetOperation::Optimization));
    QVERIFY(mesh.operations().contains(AssetOperation::MeshReferenceMaintenance));

    const auto archiveDecision = router.route(std::filesystem::path(L"Archives/Assets.ba2"));
    QVERIFY(std::holds_alternative<SkippedAsset>(archiveDecision));
    QCOMPARE(std::get<SkippedAsset>(archiveDecision).reason(), SkipReason::DisabledPhase);
}

void AssetRoutingTests::routingIgnoresFilesystemState()
{
    const auto request = RoutingPolicyRequest::optimizeNativeTextures();
    const auto capabilities = ProfileCapabilities::define(
        ".ba2", {ProfileCapability::NativeTextureOptimization});
    const auto result = RoutingPolicy::compile(request, capabilities);
    QVERIFY(result.hasPolicy());
    const AssetRouter router(*result.policy());
    QTemporaryDir temporaryDirectory;
    QVERIFY(temporaryDirectory.isValid());
    const auto qPath = temporaryDirectory.filePath(QStringLiteral("NotReallyATexture.dds"));
    const std::filesystem::path executionPath(qPath.toStdWString());

    const auto missingDecision = router.route(executionPath);
    QVERIFY(std::holds_alternative<RoutedAsset>(missingDecision));

    QFile file(qPath);
    QVERIFY(file.open(QIODevice::WriteOnly));
    QCOMPARE(file.write("arbitrary non-DDS contents"), qint64{26});
    file.close();

    const auto existingDecision = router.route(executionPath);
    QVERIFY(std::holds_alternative<RoutedAsset>(existingDecision));
    const auto &missingAsset = std::get<RoutedAsset>(missingDecision);
    const auto &existingAsset = std::get<RoutedAsset>(existingDecision);
    QCOMPARE(missingAsset.kind(), existingAsset.kind());
    QCOMPARE(missingAsset.executionMode(), existingAsset.executionMode());
    QVERIFY(missingAsset.executionPath() == existingAsset.executionPath());
    QVERIFY(existingAsset.operations().contains(AssetOperation::Optimization));

    QVERIFY(file.open(QIODevice::ReadOnly));
    QCOMPARE(file.readAll(), QByteArray("arbitrary non-DDS contents"));
}

void AssetRoutingTests::batchRoutingOwnsRoutedAssetsInInputOrder()
{
    const auto router = fullyEnabledRouter();

    const RoutingLedger ledger = [&router] {
        const std::vector borrowedPaths{
            std::filesystem::path(L"Textures/First.dds"),
            std::filesystem::path(L"Meshes/Second.nif"),
            std::filesystem::path(L"Textures/First.dds")};
        return router.route(std::span<const std::filesystem::path>(borrowedPaths));
    }();

    const auto routedAssets = ledger.routedAssets();
    QCOMPARE(routedAssets.size(), std::size_t{3});
    QVERIFY(routedAssets[0].executionPath() == std::filesystem::path(L"Textures/First.dds"));
    QVERIFY(routedAssets[1].executionPath() == std::filesystem::path(L"Meshes/Second.nif"));
    QVERIFY(routedAssets[2].executionPath() == std::filesystem::path(L"Textures/First.dds"));
}

void AssetRoutingTests::batchRoutingOmitsUnsupportedPathsAndCountsSkips()
{
    const auto request = RoutingPolicyRequest::forWork(ExecutionMode::DryRun,
                                             {RequestedWork::ConvertibleTextureConversion,
                                              RequestedWork::ArchiveExtraction});
    const auto capabilities = ProfileCapabilities::define(
        ".ba2", {ProfileCapability::ConvertibleTextureConversion,
                  ProfileCapability::ArchiveExtraction,
                  ProfileCapability::MeshReferenceMaintenance});
    const auto result = RoutingPolicy::compile(request, capabilities);
    QVERIFY(result.hasPolicy());
    const AssetRouter router(*result.policy());
    const std::vector paths{
        std::filesystem::path(L"Textures/Routed.tga"),
        std::filesystem::path(L"Docs/Unsupported.txt"),
        std::filesystem::path(L"Textures/Excluded.dds"),
        std::filesystem::path(L"Animations/Disabled.hkx"),
        std::filesystem::path(L"Archives/Disabled.ba2"),
        std::filesystem::path(L"Textures/Routed.tga")};

    const auto ledger = router.route(std::span<const std::filesystem::path>(paths));

    const auto routedAssets = ledger.routedAssets();
    QCOMPARE(routedAssets.size(), std::size_t{2});
    QVERIFY(routedAssets[0].executionPath() == paths[0]);
    QVERIFY(routedAssets[1].executionPath() == paths[5]);
    QCOMPARE(ledger.skippedAssetCount(SkipReason::DisabledPhase), std::size_t{1});
    QCOMPARE(ledger.skippedAssetCount(SkipReason::DisabledAssetKind), std::size_t{1});
    QCOMPARE(ledger.skippedAssetCount(SkipReason::ExcludedAssetVariant), std::size_t{1});
}

void AssetRoutingTests::ledgerQueriesRoutedAssetsByPhaseAndTarget()
{
    const auto router = fullyEnabledRouter();
    const std::vector paths{
        std::filesystem::path(L"Archives/First.ba2"),
        std::filesystem::path(L"Textures/Repeated.dds"),
        std::filesystem::path(L"Meshes/Middle.nif"),
        std::filesystem::path(L"Textures/Repeated.dds"),
        std::filesystem::path(L"Archives/Last.ba2")};
    const auto ledger = router.route(std::span<const std::filesystem::path>(paths));

    const auto looseAssets = ledger.routedAssets(RoutedAssetPhase::LooseAssetProcessing);
    QCOMPARE(looseAssets.size(), std::size_t{3});
    QVERIFY(looseAssets[0].get().executionPath() == paths[1]);
    QVERIFY(looseAssets[1].get().executionPath() == paths[2]);
    QVERIFY(looseAssets[2].get().executionPath() == paths[3]);

    const auto archiveAssets = ledger.routedAssets(RoutedAssetPhase::ArchiveExtraction);
    QCOMPARE(archiveAssets.size(), std::size_t{2});
    QVERIFY(archiveAssets[0].get().executionPath() == paths[0]);
    QVERIFY(archiveAssets[1].get().executionPath() == paths[4]);

    const auto textureAssets = ledger.routedAssets(OptimizerTarget::Texture);
    QCOMPARE(textureAssets.size(), std::size_t{2});
    QVERIFY(textureAssets[0].get().executionPath() == paths[1]);
    QVERIFY(textureAssets[1].get().executionPath() == paths[3]);

    const auto meshAssets = ledger.routedAssets(OptimizerTarget::Mesh);
    QCOMPARE(meshAssets.size(), std::size_t{1});
    QVERIFY(meshAssets[0].get().executionPath() == paths[2]);
    QVERIFY(ledger.routedAssets(OptimizerTarget::Animation).empty());
}

void AssetRoutingTests::ledgerWorkTotalCountsRoutedAssetsNotOperations()
{
    const auto request = RoutingPolicyRequest::forWork(ExecutionMode::Apply,
                                             {RequestedWork::ConvertibleTextureConversion,
                                              RequestedWork::StandardMeshOptimization});
    const auto capabilities = ProfileCapabilities::define(
        ".ba2", {ProfileCapability::ConvertibleTextureConversion,
                  ProfileCapability::StandardMeshOptimization,
                  ProfileCapability::MeshReferenceMaintenance});
    const auto result = RoutingPolicy::compile(request, capabilities);
    QVERIFY(result.hasPolicy());
    const AssetRouter router(*result.policy());
    const std::array paths{std::filesystem::path(L"Meshes/BothOperations.nif")};

    const auto ledger = router.route(std::span<const std::filesystem::path>(paths));

    const auto routedAssets = ledger.routedAssets();
    QCOMPARE(routedAssets.size(), std::size_t{1});
    QVERIFY(routedAssets[0].operations().contains(AssetOperation::Optimization));
    QVERIFY(routedAssets[0].operations().contains(AssetOperation::MeshReferenceMaintenance));
}

void AssetRoutingTests::batchRoutingMatchesSingleAssetDecisions()
{
    const auto request = RoutingPolicyRequest::forWork(
        ExecutionMode::Apply, {RequestedWork::ConvertibleTextureConversion});
    const auto capabilities = ProfileCapabilities::define(
        ".ba2", {ProfileCapability::ConvertibleTextureConversion,
                  ProfileCapability::MeshReferenceMaintenance});
    const auto result = RoutingPolicy::compile(request, capabilities);
    QVERIFY(result.hasPolicy());
    const AssetRouter router(*result.policy());
    const std::array paths{
        std::filesystem::path(L"Textures/Routed.tga"),
        std::filesystem::path(L"Textures/Skipped.dds"),
        std::filesystem::path(L"Docs/Unsupported.txt")};

    const auto routedDecision = router.route(paths[0]);
    const auto skippedDecision = router.route(paths[1]);
    const auto unsupportedDecision = router.route(paths[2]);
    const auto ledger = router.route(std::span<const std::filesystem::path>(paths));

    QVERIFY(std::holds_alternative<RoutedAsset>(routedDecision));
    QVERIFY(std::holds_alternative<SkippedAsset>(skippedDecision));
    QVERIFY(std::holds_alternative<UnsupportedDecision>(unsupportedDecision));
    const auto &singleRoutedAsset = std::get<RoutedAsset>(routedDecision);
    const auto routedAssets = ledger.routedAssets();
    QCOMPARE(routedAssets.size(), std::size_t{1});
    QVERIFY(routedAssets[0].executionPath() == singleRoutedAsset.executionPath());
    QCOMPARE(routedAssets[0].kind(), singleRoutedAsset.kind());
    QCOMPARE(routedAssets[0].phase(), singleRoutedAsset.phase());
    QCOMPARE(routedAssets[0].target(), singleRoutedAsset.target());
    QCOMPARE(routedAssets[0].executionMode(), singleRoutedAsset.executionMode());
    QCOMPARE(routedAssets[0].operations().contains(AssetOperation::Conversion),
             singleRoutedAsset.operations().contains(AssetOperation::Conversion));
    const auto reason = std::get<SkippedAsset>(skippedDecision).reason();
    QCOMPARE(ledger.skippedAssetCount(reason), std::size_t{1});
}

void AssetRoutingTests::emptyBatchProducesEmptyLedger()
{
    const auto router = fullyEnabledRouter();

    const auto ledger = router.route(std::span<const std::filesystem::path>{});

    QVERIFY(ledger.routedAssets().empty());
    QCOMPARE(ledger.skippedAssetCount(SkipReason::DisabledPhase), std::size_t{0});
    QCOMPARE(ledger.skippedAssetCount(SkipReason::DisabledAssetKind), std::size_t{0});
    QCOMPARE(ledger.skippedAssetCount(SkipReason::ExcludedAssetVariant), std::size_t{0});
}

QTEST_APPLESS_MAIN(AssetRoutingTests)

#include "AssetRoutingTests.moc"
