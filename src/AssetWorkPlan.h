/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

#include "AssetWorkPolicy.h"

#include <QString>
#include <QStringList>
#include <QVector>

enum class AssetWorkMode { SingleMod, SeveralMods };

struct AssetWorkPlanRequest {
  QString selectedPath;
  AssetWorkMode mode = AssetWorkMode::SingleMod;
  QStringList ignoredMods;
  AssetWorkPolicy policy;
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
   */
  explicit AssetWorkPlanner(AssetWorkPlanRequest request);

  /*!
   * \brief Plans archive extraction and packing targets before archive
   * extraction mutates the filesystem.
   * \return An Archive Asset Work Plan containing selected mods, BSA archives to
   * extract, and folders to pack.
   */
  [[nodiscard]] ArchiveAssetWorkPlan planArchives() const;

  /*!
   * \brief Plans loose assets after archive extraction has had a chance to add
   * files.
   * \param modsToProcess The selected mods from the archive plan.
   * \return A Loose Asset Work Plan containing loose assets to optimize,
   * preserving filesystem traversal order.
   */
  [[nodiscard]] LooseAssetWorkPlan
  planLooseAssets(const QStringList &modsToProcess) const;

private:
  [[nodiscard]] QStringList selectMods() const;
  [[nodiscard]] bool isIgnoredMod(const QString &modName) const;

  AssetWorkPlanRequest _request;
};
