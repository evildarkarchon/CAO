#include "AssetPathConflicts.h"

#include <catch2/catch_test_macros.hpp>

#include <QDir>

TEST_CASE("TGA conversion conflicts with its case-varied DDS destination") {
  const auto tgaKeys =
      looseAssetWriteKeys({QDir::temp().filePath("Textures/Stone.TGA"),
                           LooseAssetKind::TextureTga});
  const auto ddsKeys =
      looseAssetWriteKeys({QDir::temp().filePath("textures/stone.dds"),
                           LooseAssetKind::TextureDds});

  REQUIRE(looseAssetWriteKeysConflict(tgaKeys, ddsKeys));
}

TEST_CASE("unrelated loose Assets do not have path conflicts") {
  const auto first =
      looseAssetWriteKeys({QDir::temp().filePath("textures/stone.dds"),
                           LooseAssetKind::TextureDds});
  const auto second = looseAssetWriteKeys(
      {QDir::temp().filePath("meshes/statue.nif"), LooseAssetKind::Mesh});

  REQUIRE_FALSE(looseAssetWriteKeysConflict(first, second));
}
