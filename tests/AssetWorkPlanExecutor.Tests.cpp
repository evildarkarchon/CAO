#include "AssetWorkPlanExecutor.h"
#include "AssetWorkOptions.h"
#include "AssetWorkOptionsDraft.h"
#include "AssetWorkPolicyResolver.h"
#include "AssetWorkProfileSnapshot.h"

#include <catch2/catch_test_macros.hpp>

#include <QDir>
#include <QFile>
#include <QTemporaryDir>

#include <vector>

namespace {
void createFile(const QString &path) {
  QFile file(path);
  REQUIRE(file.open(QIODevice::WriteOnly));
}

AssetWorkPolicy resolvedPolicy(const bool archives, const bool meshes,
                               const bool textures, const bool animations) {
  AssetWorkOptionsDraft draft;
  draft.bBsaExtract = archives;
  draft.bBsaCreate = archives;
  draft.iMeshesOptimizationLevel = meshes ? 1 : 0;
  draft.bTexturesNecessary = textures;
  draft.bAnimationsOptimization = animations;

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
      .planning();
}

AssetWorkPlanRequest defaultRequest(const QString &selectedPath) {
  return AssetWorkPlanRequest{selectedPath,
                              AssetWorkMode::SeveralMods,
                              {},
                              resolvedPolicy(true, true, true, true)};
}

struct RecordedWorkAdapter final : AssetWorkPlanExecutionAdapter {
  QStringList events;
  std::function<void(const ArchiveExtractionWorkItem &workItem)>
      onExtractArchive;

  void extractArchive(const ArchiveExtractionWorkItem &workItem) override {
    events << "extract:" + workItem.path;
    if (onExtractArchive)
      onExtractArchive(workItem);
  }

  void processLooseAsset(const LooseAssetWorkItem &workItem,
                         const ModAssetMetadata &metadata) override {
    events << "process:" + workItem.path;
    if (metadata.isHeadpartMesh(workItem.path))
      events << "headpart:" + workItem.path;
  }

  void packArchive(const ArchivePackingWorkItem &workItem) override {
    events << "pack:" + workItem.folder;
  }
};

struct RecordedMetadataProvider final : ModAssetMetadataProvider {
  mutable int buildCount = 0;
  mutable QStringList selectedMods;
  QStringList *events = nullptr;
  std::function<ModAssetMetadata(const QStringList &mods)> onBuild;

  ModAssetMetadata buildForMods(const QStringList &mods) const override {
    ++buildCount;
    selectedMods = mods;
    if (events)
      *events << "metadata:" + mods.join("|");
    if (onBuild)
      return onBuild(mods);
    return ModAssetMetadata();
  }
};
} // namespace

TEST_CASE("AssetWorkPlanExecutor runs extraction before post-extraction Loose "
          "Asset Discovery and packing") {
  QTemporaryDir tempDir;
  REQUIRE(tempDir.isValid());

  const QDir root(tempDir.path());
  REQUIRE(root.mkpath("Alpha/meshes"));
  REQUIRE(root.mkpath("Alpha/textures"));
  REQUIRE(root.mkpath("Alpha/empty/nested"));

  createFile(root.filePath("Alpha/Alpha.bsa"));
  createFile(root.filePath("Alpha/meshes/body.nif"));

  RecordedWorkAdapter adapter;
  RecordedMetadataProvider metadataProvider;
  DeterministicLooseAssetScheduler scheduler;
  metadataProvider.events = &adapter.events;
  adapter.onExtractArchive = [&](const ArchiveExtractionWorkItem &) {
    createFile(root.filePath("Alpha/textures/extracted.dds"));
  };

  AssetWorkPlanExecutor executor(defaultRequest(tempDir.path()),
                                 metadataProvider, adapter, scheduler);
  const auto result = executor.execute();

  const QString extractEvent = "extract:" + root.filePath("Alpha/Alpha.bsa");
  const QString metadataEvent = "metadata:" + root.filePath("Alpha");
  const QString existingAssetEvent =
      "process:" + root.filePath("Alpha/meshes/body.nif");
  const QString extractedAssetEvent =
      "process:" + root.filePath("Alpha/textures/extracted.dds");
  const QString packEvent = "pack:" + root.filePath("Alpha");

  REQUIRE(result == AssetWorkPlanExecutionResult::Completed);
  REQUIRE(adapter.events.first() == extractEvent);
  REQUIRE(metadataProvider.buildCount == 1);
  REQUIRE(metadataProvider.selectedMods == QStringList{root.filePath("Alpha")});
  REQUIRE(adapter.events.indexOf(metadataEvent) >
          adapter.events.indexOf(extractEvent));
  REQUIRE(adapter.events.indexOf(metadataEvent) <
          adapter.events.indexOf(existingAssetEvent));
  REQUIRE(adapter.events.contains(existingAssetEvent));
  REQUIRE(adapter.events.contains(extractedAssetEvent));
  REQUIRE(adapter.events.last() == packEvent);
  REQUIRE(adapter.events.indexOf(extractedAssetEvent) >
          adapter.events.indexOf(extractEvent));
  REQUIRE_FALSE(root.exists("Alpha/empty"));
}

TEST_CASE(
    "AssetWorkPlanExecutor skips metadata scans for non-mesh loose work") {
  QTemporaryDir tempDir;
  REQUIRE(tempDir.isValid());

  const QDir root(tempDir.path());
  REQUIRE(root.mkpath("Alpha/animations"));
  REQUIRE(root.mkpath("Alpha/textures"));

  createFile(root.filePath("Alpha/animations/idle.hkx"));
  createFile(root.filePath("Alpha/textures/diffuse.dds"));

  const auto request =
      AssetWorkPlanRequest{tempDir.path(),
                           AssetWorkMode::SeveralMods,
                           {},
                           resolvedPolicy(false, false, true, true)};
  RecordedWorkAdapter adapter;
  RecordedMetadataProvider metadataProvider;
  DeterministicLooseAssetScheduler scheduler;

  AssetWorkPlanExecutor executor(request, metadataProvider, adapter, scheduler);
  const auto result = executor.execute();

  REQUIRE(result == AssetWorkPlanExecutionResult::Completed);
  REQUIRE(metadataProvider.buildCount == 0);
  REQUIRE(adapter.events.contains("process:" +
                                  root.filePath("Alpha/animations/idle.hkx")));
  REQUIRE(adapter.events.contains("process:" +
                                  root.filePath("Alpha/textures/diffuse.dds")));
}

TEST_CASE("AssetWorkPlanExecutor reports semantic progress") {
  QTemporaryDir tempDir;
  REQUIRE(tempDir.isValid());

  const QDir root(tempDir.path());
  REQUIRE(root.mkpath("Alpha"));
  createFile(root.filePath("Alpha/Alpha.bsa"));

  RecordedWorkAdapter adapter;
  RecordedMetadataProvider metadataProvider;
  DeterministicLooseAssetScheduler scheduler;
  std::vector<AssetWorkPlanProgress> progress;

  AssetWorkPlanExecutor executor(defaultRequest(tempDir.path()),
                                 metadataProvider, adapter, scheduler);
  const auto result = executor.execute(AssetWorkPlanExecutionCallbacks{
      [&](const AssetWorkPlanProgress &entry) { progress.push_back(entry); },
      {}});

  REQUIRE(result == AssetWorkPlanExecutionResult::Completed);
  REQUIRE(metadataProvider.buildCount == 0);
  REQUIRE(progress.size() == 5);
  REQUIRE(progress[0].phase == AssetWorkPlanExecutionPhase::ArchiveExtraction);
  REQUIRE(progress[0].completed == 0);
  REQUIRE(progress[0].total == 1);
  REQUIRE(progress[1].phase == AssetWorkPlanExecutionPhase::ArchiveExtraction);
  REQUIRE(progress[1].completed == 1);
  REQUIRE(progress[1].total == 1);
  REQUIRE(progress[2].phase ==
          AssetWorkPlanExecutionPhase::LooseAssetProcessing);
  REQUIRE(progress[2].completed == 0);
  REQUIRE(progress[2].total == 0);
  REQUIRE(progress[3].phase == AssetWorkPlanExecutionPhase::ArchivePacking);
  REQUIRE(progress[3].completed == 0);
  REQUIRE(progress[3].total == 1);
  REQUIRE(progress[4].phase == AssetWorkPlanExecutionPhase::ArchivePacking);
  REQUIRE(progress[4].completed == 1);
  REQUIRE(progress[4].total == 1);
  REQUIRE(progress[4].currentLabel == "Alpha");
}

TEST_CASE("AssetWorkPlanExecutor stops before Loose Asset Discovery when "
          "cancellation follows extraction") {
  QTemporaryDir tempDir;
  REQUIRE(tempDir.isValid());

  const QDir root(tempDir.path());
  REQUIRE(root.mkpath("Alpha/empty/nested"));
  createFile(root.filePath("Alpha/Alpha.bsa"));

  RecordedWorkAdapter adapter;
  RecordedMetadataProvider metadataProvider;
  DeterministicLooseAssetScheduler scheduler;
  const QString extractEvent = "extract:" + root.filePath("Alpha/Alpha.bsa");

  AssetWorkPlanExecutor executor(defaultRequest(tempDir.path()),
                                 metadataProvider, adapter, scheduler);
  const auto result = executor.execute(AssetWorkPlanExecutionCallbacks{
      {}, [&]() { return adapter.events.contains(extractEvent); }});

  REQUIRE(result == AssetWorkPlanExecutionResult::Cancelled);
  REQUIRE(adapter.events == QStringList{extractEvent});
  REQUIRE(metadataProvider.buildCount == 0);
  REQUIRE(root.exists("Alpha/empty"));
}

TEST_CASE("AssetWorkPlanExecutor stops before loose asset processing when "
          "cancellation follows metadata build") {
  QTemporaryDir tempDir;
  REQUIRE(tempDir.isValid());

  const QDir root(tempDir.path());
  REQUIRE(root.mkpath("Alpha/empty/nested"));
  REQUIRE(root.mkpath("Alpha/meshes"));
  createFile(root.filePath("Alpha/meshes/body.nif"));

  RecordedWorkAdapter adapter;
  RecordedMetadataProvider metadataProvider;
  DeterministicLooseAssetScheduler scheduler;
  metadataProvider.events = &adapter.events;
  const QString metadataEvent = "metadata:" + root.filePath("Alpha");

  AssetWorkPlanExecutor executor(defaultRequest(tempDir.path()),
                                 metadataProvider, adapter, scheduler);
  const auto result = executor.execute(AssetWorkPlanExecutionCallbacks{
      {}, [&]() { return adapter.events.contains(metadataEvent); }});

  REQUIRE(result == AssetWorkPlanExecutionResult::Cancelled);
  REQUIRE(adapter.events == QStringList{metadataEvent});
  REQUIRE(metadataProvider.buildCount == 1);
  REQUIRE(root.exists("Alpha/empty"));
}

TEST_CASE("AssetWorkPlanExecutor drains loose Asset scheduling before "
          "returning cancellation") {
  QTemporaryDir tempDir;
  REQUIRE(tempDir.isValid());

  const QDir root(tempDir.path());
  REQUIRE(root.mkpath("Alpha/textures"));
  REQUIRE(root.mkpath("Alpha/empty/nested"));
  createFile(root.filePath("Alpha/textures/first.dds"));
  createFile(root.filePath("Alpha/textures/second.dds"));

  RecordedWorkAdapter adapter;
  RecordedMetadataProvider metadataProvider;
  DeterministicLooseAssetScheduler scheduler;
  AssetWorkPlanExecutor executor(defaultRequest(tempDir.path()),
                                 metadataProvider, adapter, scheduler);
  const auto result = executor.execute(AssetWorkPlanExecutionCallbacks{
      {}, [&] { return !adapter.events.filter("process:").isEmpty(); }});

  REQUIRE(result == AssetWorkPlanExecutionResult::Cancelled);
  REQUIRE(adapter.events.size() == 1);
  REQUIRE(adapter.events.first().startsWith("process:"));
  REQUIRE(root.exists("Alpha/empty"));
}
