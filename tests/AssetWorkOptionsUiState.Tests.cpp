#include "AssetWorkOptionsUiState.h"

#include <catch2/catch_test_macros.hpp>

namespace {
AssetWorkOptionsUiContext allAvailableContext() {
  return AssetWorkOptionsUiContext{.bsaAvailable = true,
                                   .meshesAvailable = true,
                                   .animationsAvailable = true,
                                   .texturesAvailable = true,
                                   .advancedSettingsVisible = true,
                                   .advancedSettingsEditable = true};
}
} // namespace

TEST_CASE("Asset Work Options UI state normalizes dry-run archive controls "
          "during presentation") {
  OptionsCAO options;
  options.bDryRun = true;
  options.bBsaExtract = true;
  options.bBsaCreate = true;
  options.bBsaDeleteBackup = true;
  options.bBsaCreateDummies = true;

  const auto state =
      AssetWorkOptionsUi::present(options, allAvailableContext());

  REQUIRE(state.dryRun);
  REQUIRE_FALSE(state.archive.controlsEnabled);
  REQUIRE_FALSE(state.archive.extract);
  REQUIRE_FALSE(state.archive.create);
  REQUIRE_FALSE(state.archive.deleteBackup);
  REQUIRE(state.archive.createDummies);

  OptionsCAO captured;
  AssetWorkOptionsUi::capture(state, captured);

  REQUIRE(captured.bDryRun);
  REQUIRE_FALSE(captured.bBsaExtract);
  REQUIRE_FALSE(captured.bBsaCreate);
  REQUIRE_FALSE(captured.bBsaDeleteBackup);
  REQUIRE_FALSE(captured.bBsaCreateDummies);
}

TEST_CASE(
    "Asset Work Options UI state normalizes several-Mod mesh optimization") {
  OptionsCAO options;
  options.mode = OptionsCAO::SeveralMods;
  options.iMeshesOptimizationLevel = 3;

  const auto state =
      AssetWorkOptionsUi::present(options, allAvailableContext());

  REQUIRE(state.mode == OptionsCAO::SeveralMods);
  REQUIRE(state.meshes.optimizationEnabled);
  REQUIRE(state.meshes.optimizationLevel == 1);
  REQUIRE_FALSE(state.meshes.mediumAndFullOptimizationEnabled);

  OptionsCAO captured;
  AssetWorkOptionsUi::capture(state, captured);

  REQUIRE(captured.mode == OptionsCAO::SeveralMods);
  REQUIRE(captured.iMeshesOptimizationLevel == 1);
}

TEST_CASE("Asset Work Options UI state leaves disabled Profile asset kinds out "
          "of captured options") {
  OptionsCAO options;
  options.bTexturesNecessary = true;
  options.bTexturesCompress = true;
  options.bTexturesMipmaps = true;
  options.bTexturesResizeSize = true;
  options.bTexturesResizeRatio = true;
  options.iTexturesTargetWidth = 1024;
  options.iTexturesTargetHeight = 512;

  auto context = allAvailableContext();
  context.texturesAvailable = false;

  const auto state = AssetWorkOptionsUi::present(options, context);
  REQUIRE_FALSE(state.textures.tabEnabled);

  OptionsCAO captured;
  AssetWorkOptionsUi::capture(state, captured);

  REQUIRE_FALSE(captured.bTexturesNecessary);
  REQUIRE_FALSE(captured.bTexturesCompress);
  REQUIRE_FALSE(captured.bTexturesMipmaps);
  REQUIRE_FALSE(captured.bTexturesResizeSize);
  REQUIRE_FALSE(captured.bTexturesResizeRatio);
  REQUIRE(captured.iTexturesTargetWidth == 1024);
  REQUIRE(captured.iTexturesTargetHeight == 512);
}

TEST_CASE("Asset Work Options UI state carries advanced settings visibility "
          "and editability") {
  auto context = allAvailableContext();
  context.advancedSettingsVisible = true;
  context.advancedSettingsEditable = false;

  const OptionsCAO options;
  const auto state = AssetWorkOptionsUi::present(options, context);

  REQUIRE(state.advanced.visible);
  REQUIRE_FALSE(state.advanced.editable);
}
