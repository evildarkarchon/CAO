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

ProfilePlanningSnapshot defaultProfile()
{
    return ProfilePlanningSnapshot{true, true, true, true, true, ".bsa"};
}

RequestedAssetWork defaultRequestedWork()
{
    return RequestedAssetWork{true, true, true, true, true};
}

AssetWorkPolicy defaultPolicy()
{
    return AssetWorkPolicy::resolve(defaultRequestedWork(), defaultProfile());
}

void setPolicy(AssetWorkPlanRequest &request,
               const RequestedAssetWork &requested,
               const ProfilePlanningSnapshot &profile)
{
    request.policy = AssetWorkPolicy::resolve(requested, profile);
}

AssetWorkPlanRequest defaultRequest(const QString &selectedPath)
{
    return AssetWorkPlanRequest{selectedPath, AssetWorkMode::SeveralMods, {}, defaultPolicy()};
}

bool containsLooseAsset(const LooseAssetWorkPlan &plan, const QString &path, const LooseAssetKind kind)
{
    return std::any_of(plan.looseAssetsToOptimize.begin(),
                       plan.looseAssetsToOptimize.end(),
                       [&](const LooseAssetWorkItem &item) {
                           return item.path == path && item.kind == kind;
                       });
}

bool containsArchiveExtraction(const ArchiveAssetWorkPlan &plan, const QString &path)
{
    return std::any_of(plan.archivesToExtract.begin(),
                       plan.archivesToExtract.end(),
                       [&](const ArchiveExtractionWorkItem &item) {
                           return item.path == path;
                       });
}

}

TEST_CASE("Asset work planner single-mod mode selects the chosen mod directly")
{
    QTemporaryDir tempDir;
    REQUIRE(tempDir.isValid());

    const QDir root(tempDir.path());
    REQUIRE(root.mkpath("Chosen"));
    REQUIRE(root.mkpath("Ignored"));

    auto request = defaultRequest(root.filePath("Chosen"));
    request.mode = AssetWorkMode::SingleMod;
    request.ignoredMods = QStringList{"Chosen"};

    const AssetWorkPlanner planner(request);
    const auto plan = planner.planArchives();

    REQUIRE(plan.modsToProcess == QStringList{root.filePath("Chosen")});
    REQUIRE(plan.archivesToPack.size() == 1);
    REQUIRE(plan.archivesToPack[0].folder == root.filePath("Chosen"));
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

TEST_CASE("Asset work planner excludes ignored mods case-insensitively in several-mod mode")
{
    QTemporaryDir tempDir;
    REQUIRE(tempDir.isValid());

    const QDir root(tempDir.path());
    REQUIRE(root.mkpath("Alpha"));
    REQUIRE(root.mkpath("NeMeSiS"));

    auto request = defaultRequest(tempDir.path());
    request.ignoredMods = QStringList{"nemesis"};

    const AssetWorkPlanner planner(request);
    const auto plan = planner.planArchives();

    REQUIRE(plan.modsToProcess == QStringList{root.filePath("Alpha")});
}

TEST_CASE("Asset work planner matches archive extensions case-insensitively")
{
    QTemporaryDir tempDir;
    REQUIRE(tempDir.isValid());

    const QDir root(tempDir.path());
    REQUIRE(root.mkpath("Alpha"));
    createFile(root.filePath("Alpha/ALPHA.BSA"));

    const AssetWorkPlanner planner(defaultRequest(tempDir.path()));
    const auto plan = planner.planArchives();

    REQUIRE(plan.archivesToExtract.size() == 1);
    REQUIRE(containsArchiveExtraction(plan, root.filePath("Alpha/ALPHA.BSA")));
}

TEST_CASE("Asset work planner keeps several-mod archive work in selected mod order")
{
    QTemporaryDir tempDir;
    REQUIRE(tempDir.isValid());

    const QDir root(tempDir.path());
    REQUIRE(root.mkpath("Beta"));
    REQUIRE(root.mkpath("Alpha/nested"));
    createFile(root.filePath("Beta/Beta.bsa"));
    createFile(root.filePath("Alpha/nested/Alpha.bsa"));

    const AssetWorkPlanner planner(defaultRequest(tempDir.path()));
    const auto plan = planner.planArchives();

    const QStringList expectedMods{root.filePath("Alpha"), root.filePath("Beta")};
    REQUIRE(plan.modsToProcess == expectedMods);
    REQUIRE(plan.archivesToPack.size() == expectedMods.size());
    REQUIRE(plan.archivesToPack[0].folder == expectedMods[0]);
    REQUIRE(plan.archivesToPack[1].folder == expectedMods[1]);
    REQUIRE(containsArchiveExtraction(plan, root.filePath("Alpha/nested/Alpha.bsa")));
    REQUIRE(containsArchiveExtraction(plan, root.filePath("Beta/Beta.bsa")));
}

TEST_CASE("Asset work planner requires a profile archive extension for extraction but not packing")
{
    QTemporaryDir tempDir;
    REQUIRE(tempDir.isValid());

    const QDir root(tempDir.path());
    REQUIRE(root.mkpath("Alpha"));
    createFile(root.filePath("Alpha/Alpha.bsa"));

    auto request = defaultRequest(tempDir.path());
    auto profile = defaultProfile();
    profile.bsaExtension.clear();
    setPolicy(request, defaultRequestedWork(), profile);

    const AssetWorkPlanner planner(request);
    const auto plan = planner.planArchives();

    REQUIRE(plan.archivesToExtract.isEmpty());
    REQUIRE(plan.archivesToPack.size() == 1);
    REQUIRE(plan.archivesToPack[0].folder == root.filePath("Alpha"));
}

TEST_CASE("Asset work planner loose asset discovery only scans supplied mods")
{
    QTemporaryDir tempDir;
    REQUIRE(tempDir.isValid());

    const QDir root(tempDir.path());
    REQUIRE(root.mkpath("Alpha/textures"));
    REQUIRE(root.mkpath("Beta/meshes"));

    createFile(root.filePath("Alpha/textures/diffuse.dds"));
    createFile(root.filePath("Beta/meshes/body.nif"));

    const AssetWorkPlanner planner(defaultRequest(tempDir.path()));
    const auto plan = planner.planLooseAssets(QStringList{root.filePath("Beta")});

    REQUIRE(plan.modsToProcess == QStringList{root.filePath("Beta")});
    REQUIRE(plan.looseAssetsToOptimize.size() == 1);
    REQUIRE(containsLooseAsset(plan, root.filePath("Beta/meshes/body.nif"), LooseAssetKind::Mesh));
    REQUIRE_FALSE(containsLooseAsset(plan, root.filePath("Alpha/textures/diffuse.dds"), LooseAssetKind::TextureDds));
}

TEST_CASE("Asset work planner BSA profile support disables archive work")
{
    QTemporaryDir tempDir;
    REQUIRE(tempDir.isValid());

    const QDir root(tempDir.path());
    REQUIRE(root.mkpath("Alpha"));

    createFile(root.filePath("Alpha/Alpha.bsa"));

    auto request = defaultRequest(tempDir.path());
    auto profile = defaultProfile();
    profile.bsaEnabled = false;
    setPolicy(request, defaultRequestedWork(), profile);

    const AssetWorkPlanner planner(request);
    const auto archivePlan = planner.planArchives();

    REQUIRE(archivePlan.modsToProcess == QStringList{root.filePath("Alpha")});
    REQUIRE(archivePlan.archivesToExtract.isEmpty());
    REQUIRE(archivePlan.archivesToPack.isEmpty());
}

TEST_CASE("Asset work planner archive option flags suppress archive work")
{
    QTemporaryDir tempDir;
    REQUIRE(tempDir.isValid());

    const QDir root(tempDir.path());
    REQUIRE(root.mkpath("Alpha"));

    createFile(root.filePath("Alpha/Alpha.bsa"));

    auto request = defaultRequest(tempDir.path());

    SECTION("archive extraction option disables extraction only")
    {
        auto requested = defaultRequestedWork();
        requested.extractArchives = false;
        setPolicy(request, requested, defaultProfile());

        const AssetWorkPlanner planner(request);
        const auto archivePlan = planner.planArchives();

        REQUIRE(archivePlan.modsToProcess == QStringList{root.filePath("Alpha")});
        REQUIRE(archivePlan.archivesToExtract.isEmpty());
        REQUIRE(archivePlan.archivesToPack.size() == 1);
    }

    SECTION("archive creation option disables packing only")
    {
        auto requested = defaultRequestedWork();
        requested.packArchives = false;
        setPolicy(request, requested, defaultProfile());

        const AssetWorkPlanner planner(request);
        const auto archivePlan = planner.planArchives();

        REQUIRE(archivePlan.modsToProcess == QStringList{root.filePath("Alpha")});
        REQUIRE(archivePlan.archivesToExtract.size() == 1);
        REQUIRE(archivePlan.archivesToPack.isEmpty());
    }
}

TEST_CASE("Asset work planner turns policy-classified files into loose Asset Work Items")
{
    QTemporaryDir tempDir;
    REQUIRE(tempDir.isValid());

    const QDir root(tempDir.path());
    REQUIRE(root.mkpath("Alpha/textures"));
    REQUIRE(root.mkpath("Alpha/meshes"));

    createFile(root.filePath("Alpha/textures/diffuse.dds"));
    createFile(root.filePath("Alpha/textures/source.tga"));
    createFile(root.filePath("Alpha/meshes/body.nif"));
    createFile(root.filePath("Alpha/animation.hkx"));
    createFile(root.filePath("Alpha/readme.txt"));

    const AssetWorkPlanner planner(defaultRequest(tempDir.path()));
    const auto archivePlan = planner.planArchives();
    const auto loosePlan = planner.planLooseAssets(archivePlan.modsToProcess);

    REQUIRE(loosePlan.looseAssetsToOptimize.size() == 4);
    REQUIRE(containsLooseAsset(loosePlan, root.filePath("Alpha/textures/diffuse.dds"), LooseAssetKind::TextureDds));
    REQUIRE(containsLooseAsset(loosePlan, root.filePath("Alpha/textures/source.tga"), LooseAssetKind::TextureTga));
    REQUIRE(containsLooseAsset(loosePlan, root.filePath("Alpha/meshes/body.nif"), LooseAssetKind::Mesh));
    REQUIRE(containsLooseAsset(loosePlan, root.filePath("Alpha/animation.hkx"), LooseAssetKind::Animation));
    REQUIRE_FALSE(containsLooseAsset(loosePlan, root.filePath("Alpha/readme.txt"), LooseAssetKind::TextureDds));
}

TEST_CASE("Asset work planner characterizes packing-only and extraction-only archive requests")
{
    QTemporaryDir tempDir;
    REQUIRE(tempDir.isValid());

    const QDir root(tempDir.path());
    REQUIRE(root.mkpath("Alpha"));
    createFile(root.filePath("Alpha/Alpha.bsa"));

    auto packingOnly = defaultRequest(tempDir.path());
    auto requestedPackingOnly = defaultRequestedWork();
    requestedPackingOnly.extractArchives = false;
    setPolicy(packingOnly, requestedPackingOnly, defaultProfile());

    const AssetWorkPlanner packingPlanner(packingOnly);
    const auto packingPlan = packingPlanner.planArchives();

    REQUIRE(packingPlan.archivesToExtract.isEmpty());
    REQUIRE(packingPlan.archivesToPack.size() == 1);
    REQUIRE(packingPlan.archivesToPack[0].folder == root.filePath("Alpha"));

    auto extractionOnly = defaultRequest(tempDir.path());
    auto requestedExtractionOnly = defaultRequestedWork();
    requestedExtractionOnly.packArchives = false;
    setPolicy(extractionOnly, requestedExtractionOnly, defaultProfile());

    const AssetWorkPlanner extractionPlanner(extractionOnly);
    const auto extractionPlan = extractionPlanner.planArchives();

    REQUIRE(extractionPlan.archivesToPack.isEmpty());
    REQUIRE(extractionPlan.archivesToExtract.size() == 1);
    REQUIRE(extractionPlan.archivesToExtract[0].path == root.filePath("Alpha/Alpha.bsa"));
}
