#include "AssetWorkOptions.h"
#include "AssetWorkOptionsDraft.h"

#include <catch2/catch_test_macros.hpp>

TEST_CASE("Asset Work Options reject simultaneous fixed-size and ratio resizing")
{
    AssetWorkOptionsDraft draft;
    draft.bTexturesResizeSize = true;
    draft.bTexturesResizeRatio = true;

    const auto result = AssetWorkOptions::create(draft);

    REQUIRE_FALSE(result.options.has_value());
    REQUIRE(result.error == "Choose either fixed-size or ratio texture resizing, not both");
}

TEST_CASE("Asset Work Options validate active texture resize parameters") {
  AssetWorkOptionsDraft draft;

  SECTION("fixed dimensions must be powers of two") {
    draft.bTexturesResizeSize = true;
    draft.iTexturesTargetWidth = 1000;

    const auto result = AssetWorkOptions::create(draft);

    REQUIRE_FALSE(result.options.has_value());
    REQUIRE(result.error ==
            "Texture target dimensions must be powers of two");
  }

  SECTION("ratios must be greater than zero") {
    draft.bTexturesResizeRatio = true;
    draft.iTexturesTargetWidthRatio = 0;

    const auto result = AssetWorkOptions::create(draft);

    REQUIRE_FALSE(result.options.has_value());
    REQUIRE(result.error == "Texture target ratios must be greater than zero");
  }
}

TEST_CASE("Asset Work Options reject invalid mode and mesh level values") {
  AssetWorkOptionsDraft draft;

  SECTION("mode") {
    draft.mode = static_cast<AssetWorkMode>(2);
    const auto result = AssetWorkOptions::create(draft);
    REQUIRE_FALSE(result.options.has_value());
  }

  SECTION("mesh level below range") {
    draft.iMeshesOptimizationLevel = -1;
    const auto result = AssetWorkOptions::create(draft);
    REQUIRE_FALSE(result.options.has_value());
  }

  SECTION("mesh level above range") {
    draft.iMeshesOptimizationLevel = 4;
    const auto result = AssetWorkOptions::create(draft);
    REQUIRE_FALSE(result.options.has_value());
  }
}

TEST_CASE("Asset Work Options ignore inactive texture resize parameters") {
  AssetWorkOptionsDraft draft;
  draft.iTexturesTargetWidth = 1000;
  draft.iTexturesTargetHeight = 500;
  draft.iTexturesTargetWidthRatio = 0;
  draft.iTexturesTargetHeightRatio = 0;

  const auto result = AssetWorkOptions::create(draft);

  REQUIRE(result.options.has_value());
}
