#include "ManagerPlanning.h"

#include <catch2/catch_test_macros.hpp>

namespace
{
void setDefaultOptions(OptionsCAO &options)
{
    options.userPath = "D:/mods";
    options.mode = OptionsCAO::SingleMod;
    options.bTexturesNecessary = false;
}

ProfilePlanningSnapshot profileSnapshot()
{
    return ProfilePlanningSnapshot{true, false, true, false, true, ".ba2"};
}
}

TEST_CASE("ManagerPlanning maps optimization mode and passthrough fields")
{
    OptionsCAO options;
    setDefaultOptions(options);
    options.bBsaExtract = true;
    options.bBsaCreate = true;
    options.bAnimationsOptimization = true;

    const QStringList ignoredMods{"Nemesis", "BodySlide"};
    const auto profile = profileSnapshot();

    auto request = ManagerPlanning::createAssetWorkPlanRequest(options, ignoredMods, profile);

    REQUIRE(request.selectedPath == "D:/mods");
    REQUIRE(request.mode == AssetWorkMode::SingleMod);
    REQUIRE(request.ignoredMods == ignoredMods);
    REQUIRE(request.profile.bsaEnabled == profile.bsaEnabled);
    REQUIRE(request.profile.meshesEnabled == profile.meshesEnabled);
    REQUIRE(request.profile.animationsEnabled == profile.animationsEnabled);
    REQUIRE(request.profile.texturesEnabled == profile.texturesEnabled);
    REQUIRE(request.profile.texturesConvertTga == profile.texturesConvertTga);
    REQUIRE(request.profile.bsaExtension == profile.bsaExtension);
    REQUIRE(request.extractBsa);
    REQUIRE(request.createBsa);
    REQUIRE(request.optimizeAnimations);

    options.mode = OptionsCAO::SeveralMods;
    request = ManagerPlanning::createAssetWorkPlanRequest(options, ignoredMods, profile);

    REQUIRE(request.mode == AssetWorkMode::SeveralMods);
}

TEST_CASE("ManagerPlanning aggregates texture option flags")
{
    OptionsCAO options;
    setDefaultOptions(options);

    SECTION("all disabled")
    {
        const auto request = ManagerPlanning::createAssetWorkPlanRequest(options, {}, profileSnapshot());
        REQUIRE_FALSE(request.optimizeTextures);
    }

    SECTION("necessary optimization")
    {
        options.bTexturesNecessary = true;
        const auto request = ManagerPlanning::createAssetWorkPlanRequest(options, {}, profileSnapshot());
        REQUIRE(request.optimizeTextures);
    }

    SECTION("compression")
    {
        options.bTexturesCompress = true;
        const auto request = ManagerPlanning::createAssetWorkPlanRequest(options, {}, profileSnapshot());
        REQUIRE(request.optimizeTextures);
    }

    SECTION("mipmaps")
    {
        options.bTexturesMipmaps = true;
        const auto request = ManagerPlanning::createAssetWorkPlanRequest(options, {}, profileSnapshot());
        REQUIRE(request.optimizeTextures);
    }

    SECTION("resize by size")
    {
        options.bTexturesResizeSize = true;
        const auto request = ManagerPlanning::createAssetWorkPlanRequest(options, {}, profileSnapshot());
        REQUIRE(request.optimizeTextures);
    }

    SECTION("resize by ratio")
    {
        options.bTexturesResizeRatio = true;
        const auto request = ManagerPlanning::createAssetWorkPlanRequest(options, {}, profileSnapshot());
        REQUIRE(request.optimizeTextures);
    }
}

TEST_CASE("ManagerPlanning maps mesh and animation enablement from options")
{
    OptionsCAO options;
    setDefaultOptions(options);

    SECTION("mesh level zero disables mesh work")
    {
        options.iMeshesOptimizationLevel = 0;
        const auto request = ManagerPlanning::createAssetWorkPlanRequest(options, {}, profileSnapshot());
        REQUIRE_FALSE(request.optimizeMeshes);
    }

    SECTION("positive mesh level enables mesh work")
    {
        options.iMeshesOptimizationLevel = 1;
        const auto request = ManagerPlanning::createAssetWorkPlanRequest(options, {}, profileSnapshot());
        REQUIRE(request.optimizeMeshes);
    }

    SECTION("animation flag enables animation work")
    {
        options.bAnimationsOptimization = true;
        const auto request = ManagerPlanning::createAssetWorkPlanRequest(options, {}, profileSnapshot());
        REQUIRE(request.optimizeAnimations);
    }
}

TEST_CASE("ManagerPlanning maps BSA flags independently")
{
    OptionsCAO options;
    setDefaultOptions(options);

    options.bBsaExtract = true;
    options.bBsaCreate = false;
    auto request = ManagerPlanning::createAssetWorkPlanRequest(options, {}, profileSnapshot());

    REQUIRE(request.extractBsa);
    REQUIRE_FALSE(request.createBsa);

    options.bBsaExtract = false;
    options.bBsaCreate = true;
    request = ManagerPlanning::createAssetWorkPlanRequest(options, {}, profileSnapshot());

    REQUIRE_FALSE(request.extractBsa);
    REQUIRE(request.createBsa);
}
