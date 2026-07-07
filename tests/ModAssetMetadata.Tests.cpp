#include "ModAssetMetadata.h"

#include <catch2/catch_test_macros.hpp>

#include <QDir>
#include <QFile>
#include <QHash>
#include <QTemporaryDir>

namespace
{
void createFile(const QString &path)
{
    QFile file(path);
    REQUIRE(file.open(QIODevice::WriteOnly));
}

struct FakeProfileAssetReferences final : ProfileAssetReferenceProvider
{
    QHash<QString, QStringList> lists;

    QStringList readReferenceList(const QString &fileName) const override
    {
        return lists.value(fileName);
    }
};

struct FakePluginAssetReferences final : PluginAssetReferenceReader
{
    QHash<QString, QStringList> headpartsByPlugin;
    mutable QStringList inspectedPlugins;

    QStringList listHeadparts(const QString &pluginPath) const override
    {
        inspectedPlugins << pluginPath;
        return headpartsByPlugin.value(pluginPath);
    }
};
}

TEST_CASE("ModAssetMetadata matches Headpart mesh paths case-insensitively")
{
    QTemporaryDir tempDir;
    REQUIRE(tempDir.isValid());

    const QDir root(tempDir.path());
    const QString relativeHeadpart = "meshes/actors/character/headparts/head.nif";
    const ModAssetMetadata metadata(QStringList{relativeHeadpart});

    REQUIRE(metadata.isHeadpartMesh(root.filePath("Alpha/MESHES/Actors/Character/Headparts/Head.nif")));

    QString nativePath = root.filePath("Alpha/meshes/actors/character/headparts/head.nif");
    nativePath.replace("/", "\\");
    REQUIRE(metadata.isHeadpartMesh(nativePath));

    REQUIRE_FALSE(metadata.isHeadpartMesh(root.filePath("Alpha/meshes/armor/helmet.nif")));
}

TEST_CASE("ModAssetMetadata treats FaceGen meshes as Headparts")
{
    const ModAssetMetadata metadata;

    REQUIRE(metadata.isHeadpartMesh("C:/mods/Alpha/meshes/actors/character/FaceGenData/face.nif"));
    REQUIRE_FALSE(metadata.isHeadpartMesh("C:/mods/Alpha/meshes/actors/character/body.nif"));
}

TEST_CASE("ModAssetMetadataBuilder combines Profile and selected Mod plugin Headparts")
{
    QTemporaryDir tempDir;
    REQUIRE(tempDir.isValid());

    const QDir root(tempDir.path());
    REQUIRE(root.mkpath("Alpha"));
    REQUIRE(root.mkpath("Beta"));

    const QString alphaPlugin = root.filePath("Alpha/Alpha.esp");
    const QString betaPlugin = root.filePath("Beta/Beta.esp");
    createFile(alphaPlugin);
    createFile(betaPlugin);

    FakeProfileAssetReferences profileReferences;
    profileReferences.lists.insert("customHeadparts.txt", QStringList{"meshes/profile/head.nif"});

    FakePluginAssetReferences pluginReferences;
    pluginReferences.headpartsByPlugin.insert(alphaPlugin, QStringList{"meshes/plugin/alpha-head.nif"});
    pluginReferences.headpartsByPlugin.insert(betaPlugin, QStringList{"meshes/plugin/beta-head.nif"});

    const ModAssetMetadataBuilder builder(profileReferences, pluginReferences);
    const ModAssetMetadata metadata = builder.buildForMods(QStringList{root.filePath("Alpha")});

    REQUIRE(metadata.isHeadpartMesh(root.filePath("Alpha/meshes/profile/head.nif")));
    REQUIRE(metadata.isHeadpartMesh(root.filePath("Alpha/meshes/plugin/alpha-head.nif")));
    REQUIRE_FALSE(metadata.isHeadpartMesh(root.filePath("Beta/meshes/plugin/beta-head.nif")));
    REQUIRE(pluginReferences.inspectedPlugins == QStringList{alphaPlugin});
}
