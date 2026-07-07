#include "AssetWorkPolicy.h"

#include <catch2/catch_test_macros.hpp>

namespace
{
ProfilePlanningSnapshot supportedProfile()
{
    return ProfilePlanningSnapshot{true, true, true, true, true, ".bsa"};
}

RequestedAssetWork requestedWork()
{
    return RequestedAssetWork{true, true, true, true, true};
}
}

TEST_CASE("Asset Work Policy allows only work that is requested and Profile-supported")
{
    const auto policy = AssetWorkPolicy::resolve(requestedWork(), supportedProfile());

    REQUIRE(policy.allowsArchiveExtraction());
    REQUIRE(policy.allowsArchiveExtractionFor("Archive.bsa"));
    REQUIRE(policy.allowsArchivePacking());
    REQUIRE(policy.allowsDdsTextureOptimization());
    REQUIRE(policy.allowsTgaTextureConversion());
    REQUIRE(policy.allowsMeshOptimization());
    REQUIRE(policy.allowsAnimationOptimization());
}

TEST_CASE("Asset Work Policy suppresses work disabled by the Profile")
{
    auto profile = supportedProfile();

    SECTION("archives")
    {
        profile.bsaEnabled = false;
        const auto policy = AssetWorkPolicy::resolve(requestedWork(), profile);

        REQUIRE_FALSE(policy.allowsArchiveExtraction());
        REQUIRE_FALSE(policy.allowsArchiveExtractionFor("Archive.bsa"));
        REQUIRE_FALSE(policy.allowsArchivePacking());
    }

    SECTION("meshes")
    {
        profile.meshesEnabled = false;
        const auto policy = AssetWorkPolicy::resolve(requestedWork(), profile);

        REQUIRE_FALSE(policy.allowsMeshOptimization());
    }

    SECTION("textures")
    {
        profile.texturesEnabled = false;
        const auto policy = AssetWorkPolicy::resolve(requestedWork(), profile);

        REQUIRE_FALSE(policy.allowsDdsTextureOptimization());
        REQUIRE_FALSE(policy.allowsTgaTextureConversion());
    }

    SECTION("TGA conversion")
    {
        profile.texturesConvertTga = false;
        const auto policy = AssetWorkPolicy::resolve(requestedWork(), profile);

        REQUIRE(policy.allowsDdsTextureOptimization());
        REQUIRE_FALSE(policy.allowsTgaTextureConversion());
    }

    SECTION("animations")
    {
        profile.animationsEnabled = false;
        const auto policy = AssetWorkPolicy::resolve(requestedWork(), profile);

        REQUIRE_FALSE(policy.allowsAnimationOptimization());
    }
}

TEST_CASE("Asset Work Policy suppresses work that was not requested")
{
    auto requested = requestedWork();

    SECTION("archive extraction")
    {
        requested.extractArchives = false;
        const auto policy = AssetWorkPolicy::resolve(requested, supportedProfile());

        REQUIRE_FALSE(policy.allowsArchiveExtraction());
        REQUIRE_FALSE(policy.allowsArchiveExtractionFor("Archive.bsa"));
        REQUIRE(policy.allowsArchivePacking());
    }

    SECTION("archive packing")
    {
        requested.packArchives = false;
        const auto policy = AssetWorkPolicy::resolve(requested, supportedProfile());

        REQUIRE(policy.allowsArchiveExtraction());
        REQUIRE_FALSE(policy.allowsArchivePacking());
    }

    SECTION("meshes")
    {
        requested.optimizeMeshes = false;
        const auto policy = AssetWorkPolicy::resolve(requested, supportedProfile());

        REQUIRE_FALSE(policy.allowsMeshOptimization());
    }

    SECTION("textures")
    {
        requested.optimizeTextures = false;
        const auto policy = AssetWorkPolicy::resolve(requested, supportedProfile());

        REQUIRE_FALSE(policy.allowsDdsTextureOptimization());
        REQUIRE_FALSE(policy.allowsTgaTextureConversion());
    }

    SECTION("animations")
    {
        requested.optimizeAnimations = false;
        const auto policy = AssetWorkPolicy::resolve(requested, supportedProfile());

        REQUIRE_FALSE(policy.allowsAnimationOptimization());
    }
}

TEST_CASE("Asset Work Policy matches archive extensions case-insensitively")
{
    const auto policy = AssetWorkPolicy::resolve(requestedWork(), supportedProfile());

    REQUIRE(policy.allowsArchiveExtractionFor("Archive.BSA"));
    REQUIRE_FALSE(policy.allowsArchiveExtractionFor("Archive.ba2"));
}

TEST_CASE("Asset Work Policy classifies supported loose Asset extensions")
{
    const auto policy = AssetWorkPolicy::resolve(requestedWork(), supportedProfile());

    REQUIRE(policy.classifyLooseAsset("diffuse.dds") == LooseAssetKind::TextureDds);
    REQUIRE(policy.classifyLooseAsset("source.tga") == LooseAssetKind::TextureTga);
    REQUIRE(policy.classifyLooseAsset("body.nif") == LooseAssetKind::Mesh);
    REQUIRE(policy.classifyLooseAsset("terrain.btr") == LooseAssetKind::Mesh);
    REQUIRE(policy.classifyLooseAsset("object.bto") == LooseAssetKind::Mesh);
    REQUIRE(policy.classifyLooseAsset("animation.hkx") == LooseAssetKind::Animation);
    REQUIRE_FALSE(policy.classifyLooseAsset("readme.txt").has_value());
}

TEST_CASE("Asset Work Policy classifies loose Asset extensions case-insensitively")
{
    const auto policy = AssetWorkPolicy::resolve(requestedWork(), supportedProfile());

    REQUIRE(policy.classifyLooseAsset("DIFFUSE.DDS") == LooseAssetKind::TextureDds);
    REQUIRE(policy.classifyLooseAsset("SOURCE.TGA") == LooseAssetKind::TextureTga);
    REQUIRE(policy.classifyLooseAsset("BODY.NIF") == LooseAssetKind::Mesh);
    REQUIRE(policy.classifyLooseAsset("ANIMATION.HKX") == LooseAssetKind::Animation);
}

TEST_CASE("Asset Work Policy suppresses loose Asset classification disabled by the Profile")
{
    auto profile = supportedProfile();

    SECTION("meshes")
    {
        profile.meshesEnabled = false;
        const auto policy = AssetWorkPolicy::resolve(requestedWork(), profile);

        REQUIRE_FALSE(policy.classifyLooseAsset("body.nif").has_value());
    }

    SECTION("textures")
    {
        profile.texturesEnabled = false;
        const auto policy = AssetWorkPolicy::resolve(requestedWork(), profile);

        REQUIRE_FALSE(policy.classifyLooseAsset("diffuse.dds").has_value());
        REQUIRE_FALSE(policy.classifyLooseAsset("source.tga").has_value());
    }

    SECTION("TGA conversion")
    {
        profile.texturesConvertTga = false;
        const auto policy = AssetWorkPolicy::resolve(requestedWork(), profile);

        REQUIRE(policy.classifyLooseAsset("diffuse.dds") == LooseAssetKind::TextureDds);
        REQUIRE_FALSE(policy.classifyLooseAsset("source.tga").has_value());
    }

    SECTION("animations")
    {
        profile.animationsEnabled = false;
        const auto policy = AssetWorkPolicy::resolve(requestedWork(), profile);

        REQUIRE_FALSE(policy.classifyLooseAsset("animation.hkx").has_value());
    }
}

TEST_CASE("Asset Work Policy suppresses loose Asset classification that was not requested")
{
    auto requested = requestedWork();

    SECTION("meshes")
    {
        requested.optimizeMeshes = false;
        const auto policy = AssetWorkPolicy::resolve(requested, supportedProfile());

        REQUIRE_FALSE(policy.classifyLooseAsset("body.nif").has_value());
    }

    SECTION("textures")
    {
        requested.optimizeTextures = false;
        const auto policy = AssetWorkPolicy::resolve(requested, supportedProfile());

        REQUIRE_FALSE(policy.classifyLooseAsset("diffuse.dds").has_value());
        REQUIRE_FALSE(policy.classifyLooseAsset("source.tga").has_value());
    }

    SECTION("animations")
    {
        requested.optimizeAnimations = false;
        const auto policy = AssetWorkPolicy::resolve(requested, supportedProfile());

        REQUIRE_FALSE(policy.classifyLooseAsset("animation.hkx").has_value());
    }
}

TEST_CASE("Asset Work Policy requires a Profile archive extension for extraction")
{
    auto profile = supportedProfile();
    profile.bsaExtension.clear();

    const auto policy = AssetWorkPolicy::resolve(requestedWork(), profile);

    REQUIRE_FALSE(policy.allowsArchiveExtraction());
    REQUIRE_FALSE(policy.allowsArchiveExtractionFor("Archive.bsa"));
    REQUIRE(policy.allowsArchivePacking());
}
