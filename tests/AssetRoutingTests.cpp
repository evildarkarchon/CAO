#include "AssetRouting/AssetRouter.h"

#include <QtTest>

#include <algorithm>
#include <filesystem>
#include <type_traits>
#include <variant>

using cao::routing::AssetRouter;
using cao::routing::AmbiguousArchiveExtension;
using cao::routing::AssetKind;
using cao::routing::AssetOperation;
using cao::routing::ExecutionMode;
using cao::routing::MalformedArchiveExtension;
using cao::routing::MalformedArchiveExtensionReason;
using cao::routing::MeshVariant;
using cao::routing::MissingArchiveExtension;
using cao::routing::ProfileCapability;
using cao::routing::ProfileCapabilities;
using cao::routing::RequestedWork;
using cao::routing::RoutedAsset;
using cao::routing::RoutingPolicy;
using cao::routing::RunRequest;
using cao::routing::TextureVariant;
using cao::routing::UnsupportedDerivedOperation;
using cao::routing::UnsupportedRequestedAssetKind;
using cao::routing::UnsupportedRequestedAssetVariant;
using cao::routing::UnsupportedDecision;

static_assert(!std::is_copy_assignable_v<RoutingPolicy>);
static_assert(!std::is_move_assignable_v<RoutingPolicy>);
static_assert(!std::is_default_constructible_v<RoutingPolicy>);

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
};

void AssetRoutingTests::compilesCompleteRoutingPolicy()
{
    const auto request = RunRequest::forWork(ExecutionMode::DryRun,
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
    const auto request = RunRequest::forWork(ExecutionMode::Apply,
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

    const auto request = RunRequest::optimizeNativeTextures();
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
    QTest::newRow("DDS only in parent segment") << QStringLiteral("mods/textures.dds/readme") << false;
    QTest::newRow("unknown terminal extension") << QStringLiteral("mods/Textures/readme.txt") << false;
}

void AssetRoutingTests::nativeTextureTracer()
{
    QFETCH(QString, executionPath);
    QFETCH(bool, shouldRoute);

    const auto request = RunRequest::optimizeNativeTextures();
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
    QVERIFY(routedAsset.texture().variant() == TextureVariant::Native);
}

QTEST_APPLESS_MAIN(AssetRoutingTests)

#include "AssetRoutingTests.moc"
