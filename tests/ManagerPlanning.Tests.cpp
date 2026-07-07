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

ProfilePlanningSnapshot supportedProfile()
{
    return ProfilePlanningSnapshot{true, true, true, true, true, ".ba2"};
}
}

TEST_CASE("ManagerPlanning maps optimization mode and passthrough fields")
{
    OptionsCAO options;
    setDefaultOptions(options);
    options.bBsaExtract = true;
    options.bBsaCreate = true;

    const QStringList ignoredMods{"Nemesis", "BodySlide"};

    auto request = ManagerPlanning::createAssetWorkPlanRequest(options, ignoredMods, supportedProfile());

    REQUIRE(request.selectedPath == "D:/mods");
    REQUIRE(request.mode == AssetWorkMode::SingleMod);
    REQUIRE(request.ignoredMods == ignoredMods);
    REQUIRE(request.policy.allowsArchiveExtractionFor("Archive.ba2"));
    REQUIRE_FALSE(request.policy.allowsArchiveExtractionFor("Archive.bsa"));
    REQUIRE(request.policy.allowsArchivePacking());

    options.mode = OptionsCAO::SeveralMods;
    request = ManagerPlanning::createAssetWorkPlanRequest(options, ignoredMods, supportedProfile());

    REQUIRE(request.mode == AssetWorkMode::SeveralMods);
}

TEST_CASE("ManagerPlanning aggregates texture option flags into Asset Work Policy")
{
    OptionsCAO options;
    setDefaultOptions(options);

    SECTION("all disabled")
    {
        const auto request = ManagerPlanning::createAssetWorkPlanRequest(options, {}, supportedProfile());
        REQUIRE_FALSE(request.policy.allowsDdsTextureOptimization());
        REQUIRE_FALSE(request.policy.allowsTgaTextureConversion());
    }

    SECTION("necessary optimization")
    {
        options.bTexturesNecessary = true;
        const auto request = ManagerPlanning::createAssetWorkPlanRequest(options, {}, supportedProfile());
        REQUIRE(request.policy.allowsDdsTextureOptimization());
        REQUIRE(request.policy.allowsTgaTextureConversion());
    }

    SECTION("compression")
    {
        options.bTexturesCompress = true;
        const auto request = ManagerPlanning::createAssetWorkPlanRequest(options, {}, supportedProfile());
        REQUIRE(request.policy.allowsDdsTextureOptimization());
        REQUIRE(request.policy.allowsTgaTextureConversion());
    }

    SECTION("mipmaps")
    {
        options.bTexturesMipmaps = true;
        const auto request = ManagerPlanning::createAssetWorkPlanRequest(options, {}, supportedProfile());
        REQUIRE(request.policy.allowsDdsTextureOptimization());
        REQUIRE(request.policy.allowsTgaTextureConversion());
    }

    SECTION("resize by size")
    {
        options.bTexturesResizeSize = true;
        const auto request = ManagerPlanning::createAssetWorkPlanRequest(options, {}, supportedProfile());
        REQUIRE(request.policy.allowsDdsTextureOptimization());
        REQUIRE(request.policy.allowsTgaTextureConversion());
    }

    SECTION("resize by ratio")
    {
        options.bTexturesResizeRatio = true;
        const auto request = ManagerPlanning::createAssetWorkPlanRequest(options, {}, supportedProfile());
        REQUIRE(request.policy.allowsDdsTextureOptimization());
        REQUIRE(request.policy.allowsTgaTextureConversion());
    }
}

TEST_CASE("ManagerPlanning maps mesh and animation options into Asset Work Policy")
{
    OptionsCAO options;
    setDefaultOptions(options);

    SECTION("mesh level zero disables mesh work")
    {
        options.iMeshesOptimizationLevel = 0;
        const auto request = ManagerPlanning::createAssetWorkPlanRequest(options, {}, supportedProfile());
        REQUIRE_FALSE(request.policy.allowsMeshOptimization());
    }

    SECTION("positive mesh level enables mesh work")
    {
        options.iMeshesOptimizationLevel = 1;
        const auto request = ManagerPlanning::createAssetWorkPlanRequest(options, {}, supportedProfile());
        REQUIRE(request.policy.allowsMeshOptimization());
    }

    SECTION("animation flag enables animation work")
    {
        options.bAnimationsOptimization = true;
        const auto request = ManagerPlanning::createAssetWorkPlanRequest(options, {}, supportedProfile());
        REQUIRE(request.policy.allowsAnimationOptimization());
    }
}

TEST_CASE("ManagerPlanning maps BSA flags independently into Asset Work Policy")
{
    OptionsCAO options;
    setDefaultOptions(options);

    options.bBsaExtract = true;
    options.bBsaCreate = false;
    auto request = ManagerPlanning::createAssetWorkPlanRequest(options, {}, supportedProfile());

    REQUIRE(request.policy.allowsArchiveExtractionFor("Archive.ba2"));
    REQUIRE_FALSE(request.policy.allowsArchivePacking());

    options.bBsaExtract = false;
    options.bBsaCreate = true;
    request = ManagerPlanning::createAssetWorkPlanRequest(options, {}, supportedProfile());

    REQUIRE_FALSE(request.policy.allowsArchiveExtraction());
    REQUIRE(request.policy.allowsArchivePacking());
}
