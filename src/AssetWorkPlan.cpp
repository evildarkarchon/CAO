/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#include "AssetWorkPlan.h"

#include <QDir>
#include <QDirIterator>

AssetWorkPlanner::AssetWorkPlanner(AssetWorkPlanRequest request)
    : _request(std::move(request)) {}

ArchiveAssetWorkPlan AssetWorkPlanner::planArchives() const {
  ArchiveAssetWorkPlan plan;
  plan.modsToProcess = selectMods();

  if (_request.policy.allowsArchivePacking()) {
    for (const auto &mod : plan.modsToProcess)
      plan.archivesToPack.push_back(ArchivePackingWorkItem{mod});
  }

  if (!_request.policy.allowsArchiveExtraction())
    return plan;

  for (const auto &mod : plan.modsToProcess) {
    QDirIterator it(mod, QDirIterator::Subdirectories);
    while (it.hasNext()) {
      it.next();
      if (it.fileInfo().isDir())
        continue;

      if (_request.policy.allowsArchiveExtractionFor(it.fileName()))
        plan.archivesToExtract.push_back(
            ArchiveExtractionWorkItem{it.filePath()});
    }
  }

  return plan;
}

LooseAssetWorkPlan
AssetWorkPlanner::planLooseAssets(const QStringList &modsToProcess) const {
  LooseAssetWorkPlan plan;
  plan.modsToProcess = modsToProcess;

  for (const auto &mod : plan.modsToProcess) {
    QDirIterator it(mod, QDirIterator::Subdirectories);
    while (it.hasNext()) {
      it.next();
      if (it.fileInfo().isDir())
        continue;

      const auto kind = classifyLooseAsset(it.fileName());
      if (kind.has_value())
        plan.looseAssetsToOptimize.push_back(
            LooseAssetWorkItem{it.filePath(), kind.value()});
    }
  }

  return plan;
}

QStringList AssetWorkPlanner::selectMods() const {
  QStringList mods;

  if (_request.mode == AssetWorkMode::SingleMod) {
    mods << _request.selectedPath;
    return mods;
  }

  const QDir dir(_request.selectedPath);
  for (const auto &subDir :
       dir.entryList(QDir::Dirs | QDir::NoDotAndDotDot, QDir::Name)) {
    // Separators are empty directories used by Mod Organizer 2.
    if (!subDir.contains("separator", Qt::CaseInsensitive) &&
        !isIgnoredMod(subDir))
      mods << dir.filePath(subDir);
  }

  return mods;
}

bool AssetWorkPlanner::isIgnoredMod(const QString &modName) const {
  return _request.ignoredMods.contains(modName, Qt::CaseInsensitive);
}

std::optional<LooseAssetKind>
AssetWorkPlanner::classifyLooseAsset(const QString &fileName) const {
  if (_request.policy.allowsDdsTextureOptimization() &&
      fileName.endsWith(".dds", Qt::CaseInsensitive))
    return LooseAssetKind::TextureDds;

  if (_request.policy.allowsMeshOptimization() &&
      (fileName.endsWith(".nif", Qt::CaseInsensitive) ||
       fileName.endsWith(".btr", Qt::CaseInsensitive) ||
       fileName.endsWith(".bto", Qt::CaseInsensitive)))
    return LooseAssetKind::Mesh;

  if (_request.policy.allowsTgaTextureConversion() &&
      fileName.endsWith(".tga", Qt::CaseInsensitive))
    return LooseAssetKind::TextureTga;

  if (_request.policy.allowsAnimationOptimization() &&
      fileName.endsWith(".hkx", Qt::CaseInsensitive))
    return LooseAssetKind::Animation;

  return std::nullopt;
}
