/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

#include "ArchiveEngine.h"
#include "ArchiveExecutionError.h"
#include "ArchiveFileOperations.h"
#include "AssetWorkExecutionPolicy.h"
#include "pch.h"

#include <memory>

/*!
 * \brief Executes complete staged archive extraction and packing transactions.
 *
 * Engine output is isolated in a sibling staging directory. Publishing is
 * journaled so runtime failures restore the prior live Mod state.
 */
class BSAOptimizer final : public QObject {
  Q_DECLARE_TR_FUNCTIONS(BsaOptimizer)

public:
  /*! \brief Creates a production archive transaction module. */
  BSAOptimizer();

  /*!
   * \brief Creates an archive transaction module using supplied test adapters.
   * \param engine Stages archive contents and outputs without live mutations.
   * \param files Publishes and rolls back staged filesystem changes.
   */
  BSAOptimizer(ArchiveEngine &engine, ArchiveFileOperations &files);

  /*!
   * \brief Extracts and publishes one archive transaction.
   * \param archivePath Source archive Asset.
   * \param deleteBackup Remove the source after commit instead of retaining a
   * uniquely named backup.
   * \param dryRun Report intent without staging, engine, or filesystem work.
   * \throws ArchiveExecutionError when staging, engine, publishing, or rollback
   * fails.
   */
  void extract(const QString &archivePath, bool deleteBackup,
               bool dryRun = false);

  /*!
   * \brief Stages and publishes the complete archive set for one Mod.
   * \param folderPath Mod whose loose Assets are packed.
   * \param policy Resolved archive execution rules.
   * \param dryRun Report intent without staging, engine, or filesystem work.
   * \throws ArchiveExecutionError when staging, engine, publishing, rollback,
   * or post-commit source cleanup fails.
   */
  void packAll(const QString &folderPath, const ArchiveExecutionPolicy &policy,
               bool dryRun = false);

private:
  /*! \brief Selects a retained backup name without replacing prior backups. */
  [[nodiscard]] QString uniqueBackupPath(const QString &path) const;
  /*! \brief Creates missing destination parents and journals their paths. */
  void ensureDestinationParent(const QString &destination,
                               const QString &liveRoot,
                               QStringList &createdDirectories);
  /*! \brief Reverses newly published Assets and created directories. */
  [[nodiscard]] QStringList
  rollbackPublished(const QStringList &published,
                    const QStringList &createdDirectories);
  /*! \brief Best-effort cleanup that cannot invalidate a committed result. */
  void cleanupStaging(const QString &stagingPath) noexcept;

  std::unique_ptr<ArchiveEngine> _ownedEngine;
  std::unique_ptr<ArchiveFileOperations> _ownedFiles;
  ArchiveEngine *_engine = nullptr;
  ArchiveFileOperations *_files = nullptr;
};
