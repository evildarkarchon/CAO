#include "AssetWorkPolicyResolver.h"
#include "AssetWorkOptions.h"
#include "AssetWorkOptionsDraft.h"
#include "AssetWorkProfileSnapshot.h"

#include <catch2/catch_test_macros.hpp>

namespace {
AssetWorkProfileSnapshot profileWithSupport(const bool archivesEnabled = true,
                                            const bool meshesEnabled = true,
                                            const bool animationsEnabled = true,
                                            const bool texturesEnabled = true) {
  auto archiveSettings = btu::bsa::Settings::get(btu::Game::FO4);
  archiveSettings.max_size = 12345;

  AssetWorkProfileSnapshotInput input;
  input.archivesEnabled = archivesEnabled;
  input.archiveSettings = archiveSettings;
  input.filesToNotPack = {u8"meshes\\blocked"};
  input.meshesEnabled = meshesEnabled;
  input.meshFileVersion = nifly::V20_0_0_5;
  input.meshStream = 83;
  input.meshUser = 12;
  input.animationsEnabled = animationsEnabled;
  input.texturesEnabled = texturesEnabled;
  input.textureFormat = DXGI_FORMAT_BC7_UNORM;
  input.texturesCompressInterface = true;
  input.textureUnwantedFormats = {DXGI_FORMAT_BC1_UNORM, DXGI_FORMAT_BC3_UNORM};
  input.texturesConvertTga = true;

  const auto result = AssetWorkProfileSnapshot::create(std::move(input));
  REQUIRE(result.snapshot.has_value());
  return std::move(result.snapshot.value());
}
} // namespace

TEST_CASE(
    "Asset Work Policy Resolver produces aligned policies for supported work") {
  AssetWorkOptionsDraft draft;
  draft.bBsaExtract = true;
  draft.bBsaCreate = true;
  draft.bBsaDeleteBackup = true;
  draft.bBsaMergeIncomp = false;
  draft.bBsaMergeTexture = true;
  draft.bBsaCreateDummies = false;
  draft.bBsaCompress = false;
  draft.bBsaDeleteSource = false;
  draft.iMeshesOptimizationLevel = 2;
  draft.bMeshesHeadparts = false;
  draft.bMeshesResave = true;
  draft.bTexturesNecessary = false;
  draft.bTexturesCompress = true;
  draft.bTexturesMipmaps = true;
  draft.bTexturesResizeSize = true;
  draft.iTexturesTargetWidth = 1024;
  draft.iTexturesTargetHeight = 512;
  draft.bAnimationsOptimization = true;

  const auto optionsResult = AssetWorkOptions::create(draft);
  REQUIRE(optionsResult.options.has_value());

  const auto resolution = AssetWorkPolicyResolver::resolve(
      optionsResult.options.value(), profileWithSupport());

  REQUIRE(resolution.planning().allowsArchiveExtraction());
  REQUIRE(resolution.planning().allowsArchivePacking());
  REQUIRE(resolution.planning().allowsDdsTextureOptimization());
  REQUIRE(resolution.planning().allowsTgaTextureConversion());
  REQUIRE(resolution.planning().allowsMeshOptimization());
  REQUIRE(resolution.planning().allowsAnimationOptimization());

  REQUIRE(resolution.execution().archive().deleteBackup);
  REQUIRE_FALSE(resolution.execution().archive().mergeIncompressible);
  REQUIRE(resolution.execution().archive().mergeTextures);
  REQUIRE_FALSE(resolution.execution().archive().createDummies);
  REQUIRE_FALSE(resolution.execution().archive().compress);
  REQUIRE_FALSE(resolution.execution().archive().deleteSource);
  REQUIRE(resolution.execution().archive().settings.max_size == 12345);
  REQUIRE(resolution.execution().archive().filesToNotPack ==
          std::vector<std::u8string>{u8"meshes\\blocked"});
  REQUIRE(resolution.execution().texture().compress);
  REQUIRE(resolution.execution().texture().mipmaps);
  REQUIRE(resolution.execution().texture().resizeBySize);
  REQUIRE(resolution.execution().texture().targetWidth == 1024);
  REQUIRE(resolution.execution().texture().targetHeight == 512);
  REQUIRE(resolution.execution().texture().outputFormat ==
          DXGI_FORMAT_BC7_UNORM);
  REQUIRE(resolution.execution().texture().compressInterface);
  REQUIRE(resolution.execution().texture().unwantedFormats ==
          QList<DXGI_FORMAT>{DXGI_FORMAT_BC1_UNORM, DXGI_FORMAT_BC3_UNORM});
  REQUIRE_FALSE(resolution.execution().mesh().processHeadparts);
  REQUIRE(resolution.execution().mesh().resaveMeshes);
  REQUIRE(resolution.execution().mesh().optimizationLevel == 2);
  REQUIRE(resolution.execution().mesh().targetFileVersion == nifly::V20_0_0_5);
  REQUIRE(resolution.execution().mesh().targetStream == 83);
  REQUIRE(resolution.execution().mesh().targetUser == 12);
  REQUIRE(resolution.execution().mesh().renameTgaReferences);
  REQUIRE(resolution.notices().empty());
}

TEST_CASE("Asset Work Policy Resolver normalizes Several-Mod mesh work") {
  AssetWorkOptionsDraft draft;
  draft.mode = AssetWorkMode::SeveralMods;
  draft.iMeshesOptimizationLevel = 3;

  const auto optionsResult = AssetWorkOptions::create(draft);
  REQUIRE(optionsResult.options.has_value());

  const auto resolution = AssetWorkPolicyResolver::resolve(
      optionsResult.options.value(), profileWithSupport());

  REQUIRE(resolution.planning().allowsMeshOptimization());
  REQUIRE(resolution.execution().mesh().optimizationLevel == 1);
  REQUIRE(resolution.notices().empty());
}

TEST_CASE(
    "Asset Work Policy Resolver reports Profile-unsupported texture work") {
  AssetWorkOptionsDraft draft;
  draft.bTexturesNecessary = true;

  const auto optionsResult = AssetWorkOptions::create(draft);
  REQUIRE(optionsResult.options.has_value());

  const auto resolution = AssetWorkPolicyResolver::resolve(
      optionsResult.options.value(),
      profileWithSupport(true, true, true, false));

  REQUIRE_FALSE(resolution.planning().allowsDdsTextureOptimization());
  REQUIRE_FALSE(resolution.planning().allowsTgaTextureConversion());
  REQUIRE_FALSE(resolution.execution().texture().necessaryOptimization);
  REQUIRE_FALSE(resolution.execution().texture().compress);
  REQUIRE_FALSE(resolution.execution().texture().mipmaps);
  REQUIRE(resolution.notices() ==
          std::vector<AssetWorkPolicyNotice>{
              AssetWorkPolicyNotice{AssetWorkKind::TextureOptimization}});
}

TEST_CASE("Asset Work Policy Resolver couples mesh TGA renaming to planned "
          "conversion") {
  AssetWorkOptionsDraft draft;
  draft.iMeshesOptimizationLevel = 1;
  draft.bTexturesNecessary = false;
  draft.bTexturesCompress = false;
  draft.bTexturesMipmaps = false;
  draft.bTexturesResizeSize = false;
  draft.bTexturesResizeRatio = false;

  const auto optionsResult = AssetWorkOptions::create(draft);
  REQUIRE(optionsResult.options.has_value());

  const auto resolution = AssetWorkPolicyResolver::resolve(
      optionsResult.options.value(), profileWithSupport());

  REQUIRE_FALSE(resolution.planning().allowsTgaTextureConversion());
  REQUIRE_FALSE(resolution.execution().mesh().renameTgaReferences);
}

TEST_CASE("Asset Work Policy Resolver preserves archive intent for Dry Run") {
  AssetWorkOptionsDraft draft;
  draft.bDryRun = true;
  draft.bBsaExtract = true;
  draft.bBsaCreate = true;

  const auto optionsResult = AssetWorkOptions::create(draft);
  REQUIRE(optionsResult.options.has_value());

  const auto resolution = AssetWorkPolicyResolver::resolve(
      optionsResult.options.value(), profileWithSupport());

  REQUIRE(resolution.planning().allowsArchiveExtraction());
  REQUIRE(resolution.planning().allowsArchivePacking());
  REQUIRE(resolution.execution().dryRun());
}

TEST_CASE("Asset Work Policy Resolver reports every unsupported requested work "
          "kind") {
  AssetWorkOptionsDraft draft;
  draft.bBsaExtract = true;
  draft.bBsaCreate = true;
  draft.iMeshesOptimizationLevel = 1;
  draft.bTexturesNecessary = true;
  draft.bAnimationsOptimization = true;

  const auto optionsResult = AssetWorkOptions::create(draft);
  REQUIRE(optionsResult.options.has_value());

  const auto resolution = AssetWorkPolicyResolver::resolve(
      optionsResult.options.value(),
      profileWithSupport(false, false, false, false));

  REQUIRE(resolution.notices() ==
          std::vector<AssetWorkPolicyNotice>{
              AssetWorkPolicyNotice{AssetWorkKind::ArchiveExtraction},
              AssetWorkPolicyNotice{AssetWorkKind::ArchivePacking},
              AssetWorkPolicyNotice{AssetWorkKind::MeshOptimization},
              AssetWorkPolicyNotice{AssetWorkKind::TextureOptimization},
              AssetWorkPolicyNotice{AssetWorkKind::AnimationOptimization}});
}

TEST_CASE(
    "Asset Work Policy Resolver does not report unrequested unsupported work") {
  AssetWorkOptionsDraft draft;
  draft.bTexturesNecessary = false;

  const auto optionsResult = AssetWorkOptions::create(draft);
  REQUIRE(optionsResult.options.has_value());

  const auto resolution = AssetWorkPolicyResolver::resolve(
      optionsResult.options.value(),
      profileWithSupport(false, false, false, false));

  REQUIRE(resolution.notices().empty());
}
