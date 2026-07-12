/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#include "AssetWorkPlan.h"
#include "AssetPathVisibility.h"

#include <QDir>

namespace {
template <typename Visitor>
void visitAssetFiles(const QString &rootPath, Visitor visitor) {
  const QDir root(rootPath);
  const QFileInfoList entries = root.entryInfoList(
      QDir::Dirs | QDir::Files | QDir::Hidden | QDir::System |
          QDir::NoDotAndDotDot,
      QDir::Name | QDir::IgnoreCase);
  for (const QFileInfo &entry : entries) {
    if (AssetPathVisibility::isInternalPath(entry.filePath()))
      continue;
    if (entry.isDir())
      visitAssetFiles(entry.filePath(), visitor);
    else
      visitor(entry);
  }
}
} // namespace

AssetWorkPlanner::AssetWorkPlanner(AssetWorkPlanRequest request,
                                   QStringList selectedMods)
    : _request(std::move(request)), _selectedMods(std::move(selectedMods)) {}

ArchiveAssetWorkPlan AssetWorkPlanner::planArchives() const {
  ArchiveAssetWorkPlan plan;
  plan.modsToProcess = _selectedMods;

  if (_request.policy.allowsArchivePacking()) {
    for (const auto &mod : plan.modsToProcess)
      plan.archivesToPack.push_back(ArchivePackingWorkItem{mod});
  }

  if (!_request.policy.allowsArchiveExtraction())
    return plan;

  for (const auto &mod : plan.modsToProcess) {
    visitAssetFiles(mod, [&](const QFileInfo &entry) {
      if (_request.policy.allowsArchiveExtractionFor(entry.fileName()))
        plan.archivesToExtract.push_back(
            ArchiveExtractionWorkItem{entry.filePath()});
    });
  }

  return plan;
}

LooseAssetWorkPlan AssetWorkPlanner::planLooseAssets() const {
  LooseAssetWorkPlan plan;
  plan.modsToProcess = _selectedMods;

  for (const auto &mod : plan.modsToProcess) {
    visitAssetFiles(mod, [&](const QFileInfo &entry) {
      const auto kind = _request.policy.classifyLooseAsset(entry.fileName());
      if (kind.has_value())
        plan.looseAssetsToOptimize.push_back(
            LooseAssetWorkItem{entry.filePath(), kind.value()});
    });
  }

  return plan;
}
