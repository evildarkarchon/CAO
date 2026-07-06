#include "AssetWorkPlan.h"

#include <catch2/catch_test_macros.hpp>

#include <QDir>
#include <QFile>
#include <QTemporaryDir>

#include <algorithm>

namespace
{
void createFile(const QString &path)
{
    QFile file(path);
    REQUIRE(file.open(QIODevice::WriteOnly));
}

AssetWorkPlanRequest defaultRequest(const QString &selectedPath)
{
    return AssetWorkPlanRequest{selectedPath,
                                AssetWorkMode::SeveralMods,
                                {},
                                ProfilePlanningSnapshot{true, true, true, true, true, ".bsa"},
                                true,
                                true,
                                true,
                                true,
                                true};
}

bool containsLooseAsset(const AssetWorkPlan &plan, const QString &path, const LooseAssetKind kind)
{
    return std::any_of(plan.looseAssetsToOptimize.begin(),
                       plan.looseAssetsToOptimize.end(),
                       [&](const LooseAssetWorkItem &item) {
                           return item.path == path && item.kind == kind;
                       });
}
}

TEST_CASE("Asset work planner excludes ignored mods and Mod Organizer separators")
{
    QTemporaryDir tempDir;
    REQUIRE(tempDir.isValid());

    const QDir root(tempDir.path());
    REQUIRE(root.mkpath("Alpha"));
    REQUIRE(root.mkpath("Nemesis"));
    REQUIRE(root.mkpath("separator 1"));

    createFile(root.filePath("Alpha/Alpha.bsa"));
    createFile(root.filePath("Nemesis/Nemesis.bsa"));
    createFile(root.filePath("separator 1/Separator.bsa"));

    auto request = defaultRequest(tempDir.path());
    request.ignoredMods = QStringList{"Nemesis"};

    const AssetWorkPlanner planner(request);
    const auto plan = planner.planArchives();

    REQUIRE(plan.modsToProcess == QStringList{root.filePath("Alpha")});
    REQUIRE(plan.archivesToExtract.size() == 1);
    REQUIRE(plan.archivesToExtract[0].path == root.filePath("Alpha/Alpha.bsa"));
    REQUIRE(plan.archivesToPack.size() == 1);
    REQUIRE(plan.archivesToPack[0].folder == root.filePath("Alpha"));
}

TEST_CASE("Asset work planner filters loose assets by options and profile capabilities")
{
    QTemporaryDir tempDir;
    REQUIRE(tempDir.isValid());

    const QDir root(tempDir.path());
    REQUIRE(root.mkpath("Alpha/textures"));
    REQUIRE(root.mkpath("Alpha/meshes"));

    createFile(root.filePath("Alpha/textures/diffuse.dds"));
    createFile(root.filePath("Alpha/textures/source.tga"));
    createFile(root.filePath("Alpha/meshes/body.nif"));
    createFile(root.filePath("Alpha/anim.hkx"));

    auto request = defaultRequest(tempDir.path());
    request.profile.texturesConvertTga = false;
    request.optimizeAnimations = false;

    const AssetWorkPlanner planner(request);
    const auto archivePlan = planner.planArchives();
    const auto loosePlan = planner.planLooseAssets(archivePlan.modsToProcess);

    REQUIRE(loosePlan.looseAssetsToOptimize.size() == 2);
    REQUIRE(containsLooseAsset(loosePlan, root.filePath("Alpha/textures/diffuse.dds"), LooseAssetKind::TextureDds));
    REQUIRE(containsLooseAsset(loosePlan, root.filePath("Alpha/meshes/body.nif"), LooseAssetKind::Mesh));
}
