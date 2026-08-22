#include "AssetRouting/AssetRouter.h"

#include <QtTest>

#include <filesystem>
#include <type_traits>
#include <variant>

using cao::routing::AssetRouter;
using cao::routing::ProfileCapabilities;
using cao::routing::RoutedAsset;
using cao::routing::RoutingPolicy;
using cao::routing::RunRequest;
using cao::routing::TextureVariant;
using cao::routing::UnsupportedDecision;

static_assert(!std::is_copy_assignable_v<RoutingPolicy>);

class AssetRoutingTests final : public QObject
{
    Q_OBJECT

private slots:
    /// Defines paths that distinguish terminal, case-insensitive DDS matching from incidental suffix text.
    void nativeTextureTracer_data();

    /// Verifies the public router decision, native Texture identity, and exact execution-path ownership.
    void nativeTextureTracer();
};

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
    const auto capabilities = ProfileCapabilities::withNativeTextures();
    const auto policy = RoutingPolicy::compile(request, capabilities);
    const AssetRouter router(policy);
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
