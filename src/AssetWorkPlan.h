/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

#include "AssetWorkMode.h"
#include "AssetWorkPolicy.h"

#include <QString>
#include <QStringList>
#include <QVector>

struct AssetWorkPlanRequest {
  QString selectedPath;
  AssetWorkMode mode = AssetWorkMode::SingleMod;
  QStringList ignoredMods;
  AssetWorkPolicy policy;
  // Dry Run and recovery-first callers disable generic filesystem cleanup;
  // archive transaction cleanup remains owned by Archive Recovery.
  bool cleanupEmptyDirectories = true;
  // Recovery-first composition supplies the exact locked scope so planning
  // cannot drift if the selected parent changes after lock acquisition.
  QStringList resolvedSelectedMods;
};

struct ArchiveExtractionWorkItem {
  QString path;
};

struct LooseAssetWorkItem {
  QString path;
  LooseAssetKind kind = LooseAssetKind::TextureDds;
};

struct ArchivePackingWorkItem {
  QString folder;
};

struct ArchiveAssetWorkPlan {
  QStringList modsToProcess;
  QVector<ArchiveExtractionWorkItem> archivesToExtract;
  QVector<ArchivePackingWorkItem> archivesToPack;
};

struct LooseAssetWorkPlan {
  QStringList modsToProcess;
  QVector<LooseAssetWorkItem> looseAssetsToOptimize;
};

class AssetWorkPlanner final {
public:
  /*!
   * \brief Creates a planner for one optimization request.
   * \param request The selected path, profile capabilities, ignored mods, and
   * enabled work categories.
   * \param selectedMods The already-resolved Mod directories that planning is
   * allowed to inspect.
   */
  AssetWorkPlanner(AssetWorkPlanRequest request, QStringList selectedMods);

  /*!
   * \brief Plans archive extraction and packing targets before archive
   * extraction mutates the filesystem.
   * \return An Archive Asset Work Plan containing selected mods, BSA archives
   * to extract, and folders to pack.
   */
  [[nodiscard]] ArchiveAssetWorkPlan planArchives() const;

  /*!
   * \brief Plans loose assets after archive extraction has had a chance to add
   * files.
   * \return A Loose Asset Work Plan containing loose assets to optimize,
   * preserving filesystem traversal order.
   */
  [[nodiscard]] LooseAssetWorkPlan planLooseAssets() const;

private:
  AssetWorkPlanRequest _request;
  QStringList _selectedMods;
};
