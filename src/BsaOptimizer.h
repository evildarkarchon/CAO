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

/*! \brief Durably brackets workspace creation with the owning Mod lock. */
class ArchiveTransactionBootstrap {
public:
  virtual ~ArchiveTransactionBootstrap() = default;

  /*! \brief Records intent before the reserved root or workspace is created. */
  virtual void begin(const QString &modPath, const QString &transactionId,
                     const QString &workspacePath) = 0;
  /*! \brief Clears intent only after the workspace manifest is durable. */
  virtual void complete(const QString &modPath,
                        const QString &transactionId) = 0;
};

/*!
 * \brief Executes complete staged archive extraction and packing transactions.
 *
 * Engine output is isolated in an owned in-Mod transaction workspace.
 * Durable replay restores runtime failures through the same path used after a
 * process restart.
 */
class BSAOptimizer final : public QObject {
  Q_DECLARE_TR_FUNCTIONS(BsaOptimizer)

public:
  /*! \brief Creates a production archive transaction module. */
  BSAOptimizer();

  /*! \brief Creates production archive transactions owned by held Mod locks. */
  explicit BSAOptimizer(ArchiveTransactionBootstrap &bootstrap);

  /*!
   * \brief Creates an archive transaction module using supplied test adapters.
   * \param engine Stages archive contents and outputs without live mutations.
   * \param files Publishes and rolls back staged filesystem changes.
   */
  BSAOptimizer(ArchiveEngine &engine, ArchiveFileOperations &files);

  /*! \brief Creates deterministic archive transactions with lock bootstrap. */
  BSAOptimizer(ArchiveEngine &engine, ArchiveFileOperations &files,
               ArchiveTransactionBootstrap &bootstrap);

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
  /*! \brief Creates missing destination parents through write-ahead records. */
  void ensureDestinationParent(const QString &destination,
                               const QString &liveRoot,
                               ArchiveTransactionWorkspace &workspace);
  /*! \brief Durably records and performs one no-overwrite Asset move. */
  void moveJournaled(const QString &source, const QString &destination,
                     ArchiveTransactionWorkspace &workspace);
  /*! \brief Creates a fully owned durable workspace for one transaction. */
  [[nodiscard]] ArchiveTransactionWorkspace
  createWorkspace(ArchiveTransactionKind kind, const QString &modPath,
                  const QString &anchorPath,
                  const QMap<QString, QString> &policyFacts = {});

  std::unique_ptr<ArchiveEngine> _ownedEngine;
  std::unique_ptr<ArchiveFileOperations> _ownedFiles;
  ArchiveEngine *_engine = nullptr;
  ArchiveFileOperations *_files = nullptr;
  ArchiveTransactionBootstrap *_bootstrap = nullptr;
};
