#include "AssetWorkOptions.h"
#include "AssetWorkOptionsDraft.h"
#include "AssetWorkPolicyResolver.h"
#include "AssetWorkProfileSnapshot.h"

#include <catch2/catch_test_macros.hpp>

namespace {
AssetWorkProfileSnapshotInput supportedProfileInput() {
  AssetWorkProfileSnapshotInput input;
  input.archivesEnabled = true;
  input.archiveSettings = btu::bsa::Settings::get(btu::Game::FO4);
  input.archiveSettings.extension = u8".bsa";
  input.meshesEnabled = true;
  input.animationsEnabled = true;
  input.texturesEnabled = true;
  input.textureFormat = DXGI_FORMAT_BC7_UNORM;
  input.texturesConvertTga = true;
  return input;
}

AssetWorkPolicy resolvePlanning(
    const AssetWorkOptionsDraft &draft,
    AssetWorkProfileSnapshotInput profileInput = supportedProfileInput()) {
  const auto optionsResult = AssetWorkOptions::create(draft);
  REQUIRE(optionsResult.options.has_value());
  auto profileResult =
      AssetWorkProfileSnapshot::create(std::move(profileInput));
  REQUIRE(profileResult.snapshot.has_value());

  return AssetWorkPolicyResolver::resolve(
             optionsResult.options.value(), profileResult.snapshot.value())
      .planning();
}

AssetWorkOptionsDraft allWorkDraft() {
  AssetWorkOptionsDraft draft;
  draft.bBsaExtract = true;
  draft.bBsaCreate = true;
  draft.iMeshesOptimizationLevel = 1;
  draft.bTexturesNecessary = true;
  draft.bAnimationsOptimization = true;
  return draft;
}
} // namespace

TEST_CASE("Asset Work Policy allows requested Profile-supported work") {
  const auto policy = resolvePlanning(allWorkDraft());

  REQUIRE(policy.allowsArchiveExtraction());
  REQUIRE(policy.allowsArchivePacking());
  REQUIRE(policy.allowsDdsTextureOptimization());
  REQUIRE(policy.allowsTgaTextureConversion());
  REQUIRE(policy.allowsMeshOptimization());
  REQUIRE(policy.allowsAnimationOptimization());
}

TEST_CASE("Asset Work Policy suppresses Profile-unsupported work") {
  auto profile = supportedProfileInput();
  profile.archivesEnabled = false;
  profile.meshesEnabled = false;
  profile.animationsEnabled = false;
  profile.texturesEnabled = false;

  const auto policy = resolvePlanning(allWorkDraft(), std::move(profile));

  REQUIRE_FALSE(policy.allowsArchiveExtraction());
  REQUIRE_FALSE(policy.allowsArchivePacking());
  REQUIRE_FALSE(policy.allowsDdsTextureOptimization());
  REQUIRE_FALSE(policy.allowsTgaTextureConversion());
  REQUIRE_FALSE(policy.allowsMeshOptimization());
  REQUIRE_FALSE(policy.allowsAnimationOptimization());
}

TEST_CASE("Asset Work Policy matches archive extensions case-insensitively") {
  const auto policy = resolvePlanning(allWorkDraft());

  REQUIRE(policy.allowsArchiveExtractionFor("Archive.BSA"));
  REQUIRE_FALSE(policy.allowsArchiveExtractionFor("Archive.ba2"));
}

TEST_CASE("Asset Work Policy classifies supported loose Asset extensions") {
  const auto policy = resolvePlanning(allWorkDraft());

  REQUIRE(policy.classifyLooseAsset("diffuse.dds") ==
          LooseAssetKind::TextureDds);
  REQUIRE(policy.classifyLooseAsset("source.tga") ==
          LooseAssetKind::TextureTga);
  REQUIRE(policy.classifyLooseAsset("body.nif") == LooseAssetKind::Mesh);
  REQUIRE(policy.classifyLooseAsset("terrain.btr") == LooseAssetKind::Mesh);
  REQUIRE(policy.classifyLooseAsset("object.bto") == LooseAssetKind::Mesh);
  REQUIRE(policy.classifyLooseAsset("animation.hkx") ==
          LooseAssetKind::Animation);
  REQUIRE_FALSE(policy.classifyLooseAsset("readme.txt").has_value());
}

TEST_CASE("Asset Work Policy classifies loose Asset extensions case-insensitively") {
  const auto policy = resolvePlanning(allWorkDraft());

  REQUIRE(policy.classifyLooseAsset("DIFFUSE.DDS") ==
          LooseAssetKind::TextureDds);
  REQUIRE(policy.classifyLooseAsset("SOURCE.TGA") ==
          LooseAssetKind::TextureTga);
  REQUIRE(policy.classifyLooseAsset("BODY.NIF") == LooseAssetKind::Mesh);
  REQUIRE(policy.classifyLooseAsset("ANIMATION.HKX") ==
          LooseAssetKind::Animation);
}

TEST_CASE("Asset Work Policy omits unrequested loose Asset kinds") {
  AssetWorkOptionsDraft draft;
  draft.bTexturesNecessary = false;

  const auto policy = resolvePlanning(draft);

  REQUIRE_FALSE(policy.classifyLooseAsset("diffuse.dds").has_value());
  REQUIRE_FALSE(policy.classifyLooseAsset("source.tga").has_value());
  REQUIRE_FALSE(policy.classifyLooseAsset("body.nif").has_value());
  REQUIRE_FALSE(policy.classifyLooseAsset("animation.hkx").has_value());
}
