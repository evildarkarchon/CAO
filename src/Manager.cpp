/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#include "Manager.h"
#include "AssetWorkExecutionPolicy.h"
#include "AssetWorkPlanExecutor.h"
#include "ManagerPlanning.h"

namespace {
QString currentBsaExtension() {
  const auto u8BsaExt = btu::bsa::Settings::get(Profiles::bsaGame()).extension;
  const auto asciiBsaExt = btu::common::as_ascii(u8BsaExt);
  return QString::fromUtf8(asciiBsaExt.data(),
                           static_cast<int>(asciiBsaExt.size()));
}

ProfilePlanningSnapshot currentProfilePlanningSnapshot() {
  return ProfilePlanningSnapshot{
      Profiles::bsaEnabled(),         Profiles::meshesEnabled(),
      Profiles::animationsEnabled(),  Profiles::texturesEnabled(),
      Profiles::texturesConvertTga(), currentBsaExtension()};
}

class MainOptimizerExecutionAdapter final
    : public AssetWorkPlanExecutionAdapter {
public:
  /*!
   * \brief Creates the production adapter used for Asset Work Plan Execution.
   * \param executionPolicy Resolved policy consumed by MainOptimizer while
   * executing work items.
   */
  explicit MainOptimizerExecutionAdapter(
      const AssetWorkExecutionPolicy &executionPolicy)
      : _optimizer(executionPolicy) {}

  /*!
   * \brief Delegates archive extraction to MainOptimizer.
   * \param workItem The planned archive extraction work item.
   */
  void extractArchive(const ArchiveExtractionWorkItem &workItem) override {
    _optimizer.extractArchive(workItem);
  }

  /*!
   * \brief Delegates loose Asset processing to MainOptimizer.
   * \param workItem The planned loose Asset Work Item.
   * \param metadata Metadata derived from selected Mods for this execution.
   */
  void processLooseAsset(const LooseAssetWorkItem &workItem,
                         const ModAssetMetadata &metadata) override {
    _optimizer.processLooseAsset(workItem, metadata);
  }

  /*!
   * \brief Delegates archive packing to MainOptimizer.
   * \param workItem The planned archive packing work item.
   */
  void packArchive(const ArchivePackingWorkItem &workItem) override {
    _optimizer.packArchive(workItem);
  }

private:
  MainOptimizer _optimizer;
};
} // namespace

Manager::Manager(const OptionsCAO &opt)
    : _options(opt)

{
  init();
}

void Manager::init() {
  // Preparing logging
  initCustomLogger(Profiles::logPath(), _options.bDebugLog);

  PLOG_VERBOSE << "Checking settings...";
  const QString error = _options.isValid();
  if (!error.isEmpty()) {
    PLOG_FATAL << error;
    throw std::runtime_error("Options are not valid." + error.toStdString());
  }

  readIgnoredMods();
}

void Manager::printProgress(const int &total,
                            const QString &text = "Processing files") {
#ifndef GUI
  QTextStream(stdout) << "PROGRESS:|" << text << " - %v/%m - %p%|"
                      << _numberCompletedFiles << '|' << total << endl;
#endif
#ifdef GUI
  emit progressBarTextChanged(text + "- %v/%m - %p%", total,
                              _numberCompletedFiles);
#endif
}

void Manager::cancelProcess() { _isCancelled.store(true); }

void Manager::readIgnoredMods() {
  QFile &&ignoredModsFile = Profiles::getFile("ignoredMods.txt");
  _ignoredMods = FilesystemOperations::readFile(ignoredModsFile);

  if (_ignoredMods.isEmpty()) {
    PLOG_WARNING << "ignoredMods.txt not found. All mods will be processed, "
                    "including tools such as Nemesis or "
                    "Bodyslide studio.";
  }
}

AssetWorkPlanRequest Manager::createAssetWorkPlanRequest() const {
  return ManagerPlanning::createAssetWorkPlanRequest(
      _options, _ignoredMods, currentProfilePlanningSnapshot());
}

void Manager::runOptimization() {
  PLOG_DEBUG << "Game: " << Profiles::currentProfile();
  PLOG_INFO << "Processing: " + _options.userPath;
  PLOG_INFO << "Beginning...";

  const auto executionPolicy = AssetWorkExecutionPolicy::resolve(_options);
  MainOptimizerExecutionAdapter adapter(executionPolicy);
  ProfileFileAssetReferenceProvider profileReferences;
  PluginOperationsAssetReferenceReader pluginReferences;
  ModAssetMetadataBuilder metadataBuilder(profileReferences, pluginReferences);
  PLOG_INFO << "Listing files and directories...";
  AssetWorkPlanExecutor executor(createAssetWorkPlanRequest(), metadataBuilder,
                                 adapter);
  const auto result = executor.execute(AssetWorkPlanExecutionCallbacks{
      [this](const AssetWorkPlanProgress &progress) {
        _numberCompletedFiles = progress.completed;

        switch (progress.phase) {
        case AssetWorkPlanExecutionPhase::ArchiveExtraction:
          printProgress(progress.total, "Extracting BSAs");
          break;
        case AssetWorkPlanExecutionPhase::LooseAssetProcessing:
          printProgress(progress.total, "Processing files");
          break;
        case AssetWorkPlanExecutionPhase::ArchivePacking:
          if (progress.currentLabel.isEmpty())
            printProgress(progress.total, "Packing BSAs");
          else
            printProgress(progress.total,
                          "Packing BSAs - Folder:  " + progress.currentLabel);
          break;
        }
      },
      [this]() { return _isCancelled.load(); }});

  if (result == AssetWorkPlanExecutionResult::Cancelled)
    return;

  PLOG_INFO << "Process completed<br><br><br>";
  emit end();
}
