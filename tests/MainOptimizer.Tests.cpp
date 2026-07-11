#include "MainOptimizer.h"
#include "MainOptimizerInternal.h"

#include "AssetWorkOptions.h"
#include "AssetWorkOptionsDraft.h"
#include "AssetWorkPolicyResolver.h"
#include "AssetWorkProfileSnapshot.h"

#include <catch2/catch_test_macros.hpp>

#include <memory>

namespace {
AssetWorkExecutionPolicy executionPolicy(const bool dryRun) {
  AssetWorkOptionsDraft draft;
  draft.bDryRun = dryRun;
  draft.bTexturesNecessary = true;

  AssetWorkProfileSnapshotInput profileInput;
  profileInput.archivesEnabled = true;
  profileInput.archiveSettings = btu::bsa::Settings::get(btu::Game::FO4);
  profileInput.archiveSettings.extension = u8".bsa";
  profileInput.meshesEnabled = true;
  profileInput.animationsEnabled = true;
  profileInput.texturesEnabled = true;
  profileInput.textureFormat = DXGI_FORMAT_BC7_UNORM;
  profileInput.texturesConvertTga = true;

  const auto optionsResult = AssetWorkOptions::create(draft);
  REQUIRE(optionsResult.options.has_value());
  auto profileResult =
      AssetWorkProfileSnapshot::create(std::move(profileInput));
  REQUIRE(profileResult.snapshot.has_value());
  return AssetWorkPolicyResolver::resolve(optionsResult.options.value(),
                                          profileResult.snapshot.value())
      .execution();
}

class RecordedLooseAssetTransactions final : public LooseAssetTransactions {
public:
  AssetTransactionResult nextResult;
  QVector<LooseAssetWorkItem> workItems;
  bool throwUnknown = false;

  AssetTransactionResult execute(const LooseAssetWorkItem &workItem,
                                 const ModAssetMetadata &) override {
    workItems.push_back(workItem);
    if (throwUnknown)
      throw 1;
    return nextResult;
  }
};

class RecordedAssetQuarantine final : public AssetQuarantine {
public:
  QStringList paths;

  AssetQuarantineResult quarantine(const QString &assetPath) override {
    paths.push_back(assetPath);
    return {.quarantined = true};
  }
};
} // namespace

TEST_CASE("MainOptimizer quarantines a malformed loose Asset and queues its "
          "notices") {
  auto transactions = std::make_unique<RecordedLooseAssetTransactions>();
  transactions->nextResult = {AssetTransactionStatus::MalformedAsset,
                              {{AssetTransactionNoticeCode::MalformedAsset,
                                "textures/bad.dds", "", "Invalid DDS header"}}};
  auto quarantine = std::make_unique<RecordedAssetQuarantine>();
  auto *recordedQuarantine = quarantine.get();
  auto reports = std::make_shared<AssetTransactionReportQueue>();

  auto optimizer = MainOptimizerInternalFactory::create(
      executionPolicy(false), std::move(transactions), std::move(quarantine),
      reports);
  optimizer->processLooseAsset(
      LooseAssetWorkItem{"textures/bad.dds", LooseAssetKind::TextureDds},
      ModAssetMetadata());

  REQUIRE(recordedQuarantine->paths == QStringList{"textures/bad.dds"});
  const auto queuedReports = reports->drain();
  REQUIRE(queuedReports.size() == 1);
  REQUIRE(queuedReports.front().assetPath == "textures/bad.dds");
  REQUIRE(queuedReports.front().result.status ==
          AssetTransactionStatus::MalformedAsset);
  REQUIRE(queuedReports.front().result.notices.size() == 1);
  REQUIRE(queuedReports.front().result.notices.front().code ==
          AssetTransactionNoticeCode::MalformedAsset);
}

TEST_CASE(
    "MainOptimizer keeps malformed loose Assets unchanged during Dry Run") {
  auto transactions = std::make_unique<RecordedLooseAssetTransactions>();
  transactions->nextResult = {AssetTransactionStatus::MalformedAsset,
                              {{AssetTransactionNoticeCode::MalformedAsset,
                                "meshes/bad.nif",
                                {},
                                "Invalid mesh"}}};
  auto quarantine = std::make_unique<RecordedAssetQuarantine>();
  auto *recordedQuarantine = quarantine.get();
  auto reports = std::make_shared<AssetTransactionReportQueue>();
  auto optimizer = MainOptimizerInternalFactory::create(
      executionPolicy(true), std::move(transactions), std::move(quarantine),
      reports);

  optimizer->processLooseAsset(
      LooseAssetWorkItem{"meshes/bad.nif", LooseAssetKind::Mesh},
      ModAssetMetadata());

  REQUIRE(recordedQuarantine->paths.isEmpty());
  REQUIRE(reports->drain().size() == 1);
}

TEST_CASE("MainOptimizer reports operational loose Asset failure without "
          "quarantine") {
  auto transactions = std::make_unique<RecordedLooseAssetTransactions>();
  transactions->nextResult = {AssetTransactionStatus::OperationalFailure,
                              {{AssetTransactionNoticeCode::OperationalFailure,
                                "textures/good.dds",
                                {},
                                "Direct3D device unavailable"}}};
  auto quarantine = std::make_unique<RecordedAssetQuarantine>();
  auto *recordedQuarantine = quarantine.get();
  auto reports = std::make_shared<AssetTransactionReportQueue>();
  auto optimizer = MainOptimizerInternalFactory::create(
      executionPolicy(false), std::move(transactions), std::move(quarantine),
      reports);

  optimizer->processLooseAsset(
      LooseAssetWorkItem{"textures/good.dds", LooseAssetKind::TextureDds},
      ModAssetMetadata());

  REQUIRE(recordedQuarantine->paths.isEmpty());
  const auto queuedReports = reports->drain();
  REQUIRE(queuedReports.size() == 1);
  REQUIRE(queuedReports.front().result.status ==
          AssetTransactionStatus::OperationalFailure);
}

TEST_CASE("MainOptimizer classifies unknown loose Asset exceptions") {
  auto transactions = std::make_unique<RecordedLooseAssetTransactions>();
  transactions->throwUnknown = true;
  auto reports = std::make_shared<AssetTransactionReportQueue>();
  auto optimizer = MainOptimizerInternalFactory::create(
      executionPolicy(false), std::move(transactions),
      std::make_unique<RecordedAssetQuarantine>(), reports);

  optimizer->processLooseAsset(
      LooseAssetWorkItem{"textures/good.dds", LooseAssetKind::TextureDds},
      ModAssetMetadata());

  const auto queuedReports = reports->drain();
  REQUIRE(queuedReports.size() == 1);
  REQUIRE(queuedReports.front().result.status ==
          AssetTransactionStatus::OperationalFailure);
}
