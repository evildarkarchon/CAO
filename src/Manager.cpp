/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#include "Manager.h"
#include "ArchiveExecutionError.h"
#include "AssetWorkPlanExecution.h"
#include "AssetWorkPolicyResolver.h"
#include "AssetWorkProfileSnapshot.h"
#include "FilesystemOperations.h"
#include "Profiles.h"

#include "btu/common/string.hpp"

#include <utility>

namespace {
constexpr auto FilesToNotPackFile = "FilesToNotPack.txt";

btu::bsa::Settings currentArchiveSettings() {
  auto settings = btu::bsa::Settings::get(Profiles::bsaGame());
  if (Profiles::maxBsaUncompressedSize() > settings.max_size) {
    settings.max_size =
        static_cast<std::uintmax_t>(Profiles::maxBsaUncompressedSize());
  }
  return settings;
}

std::vector<std::u8string> currentFilesToNotPack() {
  QFile filesToNotPackFile = Profiles::getFile(FilesToNotPackFile);
  if (!filesToNotPackFile.exists()) {
    PLOG_ERROR << "FilesToNotPack.txt not found. Archive packing can include "
                  "Assets that must remain loose.";
    return {};
  }

  auto lines =
      FilesystemOperations::readFile(filesToNotPackFile, [](QString &line) {
        line = QDir::toNativeSeparators(line);
      });

  std::vector<std::u8string> filesToNotPack;
  filesToNotPack.reserve(static_cast<size_t>(lines.size()));
  for (auto &&line : lines) {
    filesToNotPack.emplace_back(
        btu::common::as_utf8_string(std::move(line).toStdString()));
  }
  if (filesToNotPack.empty()) {
    PLOG_ERROR << "FilesToNotPack.txt is empty or contains only comments. "
                  "Archive packing can include Assets that must remain loose.";
  }
  return filesToNotPack;
}

AssetWorkProfileSnapshot currentProfileSnapshot() {
  AssetWorkProfileSnapshotInput input;
  input.archivesEnabled = Profiles::bsaEnabled();
  input.archiveSettings = currentArchiveSettings();
  input.filesToNotPack = currentFilesToNotPack();
  input.meshesEnabled = Profiles::meshesEnabled();
  input.meshFileVersion = Profiles::meshesFileVersion();
  input.meshStream = Profiles::meshesStream();
  input.meshUser = Profiles::meshesUser();
  input.animationsEnabled = Profiles::animationsEnabled();
  input.texturesEnabled = Profiles::texturesEnabled();
  input.textureFormat = Profiles::texturesFormat();
  input.texturesCompressInterface = Profiles::texturesCompressInterface();
  input.textureUnwantedFormats = Profiles::texturesUnwantedFormats();
  input.texturesConvertTga = Profiles::texturesConvertTga();

  auto result = AssetWorkProfileSnapshot::create(std::move(input));
  if (!result.snapshot.has_value()) {
    throw std::runtime_error(result.error.toStdString());
  }
  return std::move(result.snapshot.value());
}

QString workKindName(const AssetWorkKind work) {
  switch (work) {
  case AssetWorkKind::ArchiveExtraction:
    return "archive extraction";
  case AssetWorkKind::ArchivePacking:
    return "archive packing";
  case AssetWorkKind::MeshOptimization:
    return "mesh optimization";
  case AssetWorkKind::TextureOptimization:
    return "texture optimization";
  case AssetWorkKind::AnimationOptimization:
    return "animation optimization";
  }
  return "unknown Asset work";
}

void logPolicyNotices(const std::vector<AssetWorkPolicyNotice> &notices) {
  for (const auto &notice : notices) {
    PLOG_WARNING << "Requested " + workKindName(notice.work) +
                        " is not supported by the selected Profile.";
  }
}

} // namespace

Manager::Manager(AssetWorkOptions options, QString selectedPath,
                 const bool debugLog)
    : _options(std::move(options)), _selectedPath(std::move(selectedPath)),
      _debugLog(debugLog) {
  init();
}

void Manager::init() {
  // Preparing logging
  initCustomLogger(Profiles::logPath(), _debugLog);

  PLOG_VERBOSE << "Checking settings...";
  if (!QDir(_selectedPath).exists() || _selectedPath.size() < 5) {
    const QString error =
        "This path does not exist or is shorter than 5 characters. Path: '" +
        _selectedPath + "'";
    PLOG_FATAL << error;
    throw std::runtime_error(error.toStdString());
  }

  _profileSnapshot.emplace(currentProfileSnapshot());
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

void Manager::runOptimization() {
  PLOG_DEBUG << "Game: " << Profiles::currentProfile();
  PLOG_INFO << "Processing: " + _selectedPath;
  PLOG_INFO << "Beginning...";

  const auto resolution =
      AssetWorkPolicyResolver::resolve(_options, _profileSnapshot.value());
  logPolicyNotices(resolution.notices());
  PLOG_INFO << "Listing files and directories...";
  try {
    const auto result = AssetWorkPlanExecution::execute(
        AssetWorkPlanExecutionRequest{_selectedPath, _options.mode(),
                                      _ignoredMods, resolution.planning(),
                                      resolution.execution()},
        AssetWorkPlanExecutionCallbacks{
            [this](const AssetWorkPlanProgress &progress) {
              _numberCompletedFiles = progress.completed;

              switch (progress.phase) {
              case AssetWorkPlanExecutionPhase::ArchiveRecovery:
                printProgress(progress.total, progress.currentLabel);
                break;
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
                  printProgress(
                      progress.total,
                      "Packing BSAs - Folder:  " + progress.currentLabel);
                break;
              }
            },
            [this]() { return _isCancelled.load(); },
            [](const AssetTransactionReport &report) {
              for (const auto &notice : report.result.notices) {
                if (notice.code == AssetTransactionNoticeCode::MalformedAsset ||
                    notice.code ==
                        AssetTransactionNoticeCode::OperationalFailure ||
                    notice.code ==
                        AssetTransactionNoticeCode::QuarantineFailure)
                  PLOG_ERROR << notice.diagnostic;
                else
                  PLOG_INFO << notice.diagnostic;
              }
            }});

    if (result == AssetWorkPlanExecutionResult::Cancelled)
      return;
  } catch (const ArchiveExecutionError &error) {
    PLOG_FATAL << QString("Archive transaction failed for %1: %2")
                      .arg(error.assetPath(), error.diagnostic());
    for (const auto &rollbackError : error.rollbackDiagnostics())
      PLOG_ERROR << "Archive rollback failure: " + rollbackError;
    throw;
  }

  PLOG_INFO << "Process completed<br><br><br>";
}
