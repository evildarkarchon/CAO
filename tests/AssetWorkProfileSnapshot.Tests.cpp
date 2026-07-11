#include "AssetWorkProfileSnapshot.h"

#include <catch2/catch_test_macros.hpp>

TEST_CASE("Asset Work Profile snapshot rejects malformed enabled capabilities") {
  SECTION("archive-enabled Profile requires an extension") {
    AssetWorkProfileSnapshotInput input;
    input.archivesEnabled = true;

    const auto result = AssetWorkProfileSnapshot::create(std::move(input));

    REQUIRE_FALSE(result.snapshot.has_value());
    REQUIRE(result.error ==
            "An archive-enabled Profile must define an archive extension");
  }

  SECTION("texture-enabled Profile requires an output format") {
    AssetWorkProfileSnapshotInput input;
    input.texturesEnabled = true;

    const auto result = AssetWorkProfileSnapshot::create(std::move(input));

    REQUIRE_FALSE(result.snapshot.has_value());
    REQUIRE(result.error ==
            "A texture-enabled Profile must define an output format");
  }
}
