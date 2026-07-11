#include "ManagerPlanning.h"
#include "AssetWorkOptions.h"
#include "AssetWorkOptionsDraft.h"
#include "AssetWorkPolicyResolver.h"
#include "AssetWorkProfileSnapshot.h"

#include <catch2/catch_test_macros.hpp>

TEST_CASE("Manager planning packages run context without re-resolving policy") {
  AssetWorkOptionsDraft draft;
  draft.bBsaExtract = true;
  draft.iMeshesOptimizationLevel = 1;
  const auto optionsResult = AssetWorkOptions::create(draft);
  REQUIRE(optionsResult.options.has_value());

  AssetWorkProfileSnapshotInput profileInput;
  profileInput.archivesEnabled = true;
  profileInput.archiveSettings = btu::bsa::Settings::get(btu::Game::FO4);
  profileInput.archiveSettings.extension = u8".ba2";
  profileInput.meshesEnabled = true;
  const auto profileResult =
      AssetWorkProfileSnapshot::create(std::move(profileInput));
  REQUIRE(profileResult.snapshot.has_value());

  const auto policy =
      AssetWorkPolicyResolver::resolve(optionsResult.options.value(),
                                       profileResult.snapshot.value())
          .planning();
  const QStringList ignoredMods{"Nemesis", "BodySlide"};

  const auto request = ManagerPlanning::createAssetWorkPlanRequest(
      "D:/mods", AssetWorkMode::SeveralMods, ignoredMods, policy);

  REQUIRE(request.selectedPath == "D:/mods");
  REQUIRE(request.mode == AssetWorkMode::SeveralMods);
  REQUIRE(request.ignoredMods == ignoredMods);
  REQUIRE(request.policy.allowsArchiveExtractionFor("Archive.ba2"));
  REQUIRE_FALSE(request.policy.allowsArchivePacking());
  REQUIRE(request.policy.allowsMeshOptimization());
  REQUIRE_FALSE(request.policy.allowsAnimationOptimization());
}
