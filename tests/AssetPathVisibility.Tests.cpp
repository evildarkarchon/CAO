#include "AssetPathVisibility.h"

#include <catch2/catch_test_macros.hpp>

TEST_CASE(
    "Asset path visibility hides only the reserved Archive Transaction root") {
  REQUIRE(AssetPathVisibility::isInternalPath(
      "C:/Mods/Alpha/.cao-transactions/transaction/manifest.json"));
  REQUIRE(AssetPathVisibility::isInternalPath(
      R"(C:\Mods\Alpha\.CAO-TRANSACTIONS\transaction)"));
  REQUIRE_FALSE(AssetPathVisibility::isInternalPath(
      "C:/Mods/Alpha/.cao-transactions-backup/Asset.dds"));
  REQUIRE_FALSE(AssetPathVisibility::isInternalPath(
      "C:/Mods/Alpha/cao-transactions/Asset.dds"));
}
