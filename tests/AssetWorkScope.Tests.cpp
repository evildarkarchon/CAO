#include "AssetWorkScope.h"

#include <catch2/catch_test_macros.hpp>

#include <QDir>
#include <QTemporaryDir>

TEST_CASE(
    "Asset Work Scope single-Mod mode selects exactly the requested Mod") {
  QTemporaryDir tempDirectory;
  REQUIRE(tempDirectory.isValid());

  const QDir root(tempDirectory.path());
  REQUIRE(root.mkpath("Chosen"));
  REQUIRE(root.mkpath("Other"));

  const auto scope = AssetWorkScope::resolve(
      root.filePath("Chosen"), AssetWorkMode::SingleMod, QStringList{"Chosen"});

  REQUIRE(scope.selectedMods() == QStringList{root.filePath("Chosen")});
}

TEST_CASE(
    "Asset Work Scope several-Mod mode excludes separators and ignored Mods") {
  QTemporaryDir tempDirectory;
  REQUIRE(tempDirectory.isValid());

  const QDir root(tempDirectory.path());
  REQUIRE(root.mkpath("Beta"));
  REQUIRE(root.mkpath("Alpha"));
  REQUIRE(root.mkpath("NeMeSiS"));
  REQUIRE(root.mkpath("separator 1"));

  const auto scope = AssetWorkScope::resolve(
      root.path(), AssetWorkMode::SeveralMods, QStringList{"nemesis"});

  REQUIRE(scope.selectedMods() ==
          QStringList{root.filePath("Alpha"), root.filePath("Beta")});
}
