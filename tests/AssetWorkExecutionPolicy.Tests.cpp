#include "AssetWorkExecutionPolicy.h"
#include "OptionsCAO.h"

#include <catch2/catch_test_macros.hpp>

namespace
{
ProfileExecutionSnapshot executionProfile()
{
    auto archiveSettings = btu::bsa::Settings::get(btu::Game::FO4);
    archiveSettings.max_size = 12345;

    return ProfileExecutionSnapshot{
        archiveSettings,
        std::vector<std::u8string>{u8"meshes\\blocked"},
        nifly::V20_0_0_5,
        83,
        12,
        DXGI_FORMAT_BC7_UNORM,
        true,
        QList<DXGI_FORMAT>{DXGI_FORMAT_BC1_UNORM, DXGI_FORMAT_BC3_UNORM},
        true};
}
}

TEST_CASE("Asset Work Execution Policy resolves execution options and Profile facts")
{
    OptionsCAO options;
    options.bDryRun = true;
    options.bBsaDeleteBackup = true;
    options.bBsaMergeIncomp = false;
    options.bBsaMergeTexture = true;
    options.bBsaCreateDummies = false;
    options.bBsaCompress = false;
    options.bBsaDeleteSource = false;
    options.bTexturesNecessary = false;
    options.bTexturesCompress = true;
    options.bTexturesMipmaps = true;
    options.bTexturesResizeSize = true;
    options.iTexturesTargetWidth = 1024;
    options.iTexturesTargetHeight = 512;
    options.bTexturesResizeRatio = true;
    options.iTexturesTargetWidthRatio = 4;
    options.iTexturesTargetHeightRatio = 2;
    options.bMeshesHeadparts = false;
    options.bMeshesResave = true;
    options.iMeshesOptimizationLevel = 2;

    const auto policy = AssetWorkExecutionPolicy::resolve(options, executionProfile());

    REQUIRE(policy.dryRun);
    REQUIRE(policy.archive.deleteBackup);
    REQUIRE_FALSE(policy.archive.mergeIncompressible);
    REQUIRE(policy.archive.mergeTextures);
    REQUIRE_FALSE(policy.archive.createDummies);
    REQUIRE_FALSE(policy.archive.compress);
    REQUIRE_FALSE(policy.archive.deleteSource);
    REQUIRE(policy.archive.settings.game == btu::Game::FO4);
    REQUIRE(policy.archive.settings.max_size == 12345);
    REQUIRE(policy.archive.filesToNotPack == std::vector<std::u8string>{u8"meshes\\blocked"});

    REQUIRE_FALSE(policy.texture.necessaryOptimization);
    REQUIRE(policy.texture.compress);
    REQUIRE(policy.texture.mipmaps);
    REQUIRE(policy.texture.resizeBySize);
    REQUIRE(policy.texture.targetWidth == 1024);
    REQUIRE(policy.texture.targetHeight == 512);
    REQUIRE(policy.texture.resizeByRatio);
    REQUIRE(policy.texture.targetWidthRatio == 4);
    REQUIRE(policy.texture.targetHeightRatio == 2);
    REQUIRE(policy.texture.outputFormat == DXGI_FORMAT_BC7_UNORM);
    REQUIRE(policy.texture.compressInterface);
    REQUIRE(policy.texture.unwantedFormats == QList<DXGI_FORMAT>{DXGI_FORMAT_BC1_UNORM, DXGI_FORMAT_BC3_UNORM});

    REQUIRE_FALSE(policy.mesh.processHeadparts);
    REQUIRE(policy.mesh.resaveMeshes);
    REQUIRE(policy.mesh.optimizationLevel == 2);
    REQUIRE(policy.mesh.targetFileVersion == nifly::V20_0_0_5);
    REQUIRE(policy.mesh.targetStream == 83);
    REQUIRE(policy.mesh.targetUser == 12);
    REQUIRE(policy.mesh.renameTgaReferences);
}
