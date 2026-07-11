/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

#include "AssetWorkExecutionPolicy.h"

#include <QString>
#include <QStringList>
#include <QVector>

enum class StagedArchiveOutputKind { Archive, DummyPlugin };

struct StagedArchiveOutput {
  QString stagingPath;
  QString relativeDestination;
  StagedArchiveOutputKind kind = StagedArchiveOutputKind::Archive;
};

struct StagedArchivePacking {
  QVector<StagedArchiveOutput> outputs;
  QStringList packedSourceAssets;
};

/*!
 * \brief Stages archive engine output without mutating live Mod Assets.
 */
class ArchiveEngine {
public:
  virtual ~ArchiveEngine() = default;

  /*! \brief Extracts \p archivePath into the empty \p stagingRoot.
   *  \throws std::runtime_error when the engine cannot produce a complete
   *  staged extraction.
   */
  virtual void extractTo(const QString &archivePath,
                         const QString &stagingRoot) = 0;

  /*!
   * \brief Builds the complete archive output set in staging.
   * \param modPath Root containing the loose Assets to pack.
   * \param stagingRoot Empty transaction-owned output directory.
   * \param policy Resolved archive naming and packing rules.
   * \return Staged outputs and the exact loose source Assets included in them.
   * \throws std::runtime_error when the engine cannot produce the complete
   * staged output set.
   */
  [[nodiscard]] virtual StagedArchivePacking
  packTo(const QString &modPath, const QString &stagingRoot,
         const ArchiveExecutionPolicy &policy) = 0;
};

/*!
 * \brief Production adapter for bethutil archive extraction and packing.
 */
class BtuArchiveEngine final : public ArchiveEngine {
public:
  void extractTo(const QString &archivePath,
                 const QString &stagingRoot) override;
  [[nodiscard]] StagedArchivePacking
  packTo(const QString &modPath, const QString &stagingRoot,
         const ArchiveExecutionPolicy &policy) override;
};
