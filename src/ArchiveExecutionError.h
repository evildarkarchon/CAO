/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

#include <QString>
#include <QStringList>

#include <stdexcept>

enum class ArchiveOperation { Extraction, Packing, Recovery };

enum class ArchiveFailureStage {
  Staging,
  Engine,
  Validation,
  Publishing,
  Rollback,
  SourceCleanup,
  RecoveryDiscovery,
  RecoveryValidation,
  RecoveryRollback,
  RecoveryCleanup
};

/*!
 * \brief Describes a terminal archive transaction failure.
 *
 * Archive failures cross Asset Work Plan Execution unchanged so the caller can
 * distinguish the failed operation and whether rollback also encountered an
 * error.
 */
class ArchiveExecutionError final : public std::runtime_error {
public:
  /*! \brief Creates a typed terminal archive failure.
   *  \param operation Archive transaction that failed.
   *  \param stage Transaction stage in which the failure occurred.
   *  \param assetPath Primary archive or Mod path involved in the failure.
   *  \param diagnostic Human-readable description of the primary failure.
   *  \param rollbackDiagnostics Additional rollback failures; empty means the
   *  transaction restored its pre-execution state.
   *  \param workspacePaths Owned or conflicting recovery workspaces involved
   *  in the failure; empty for ordinary extraction and packing failures.
   */
  ArchiveExecutionError(ArchiveOperation operation, ArchiveFailureStage stage,
                        QString assetPath, QString diagnostic,
                        QStringList rollbackDiagnostics = {},
                        QStringList workspacePaths = {})
      : std::runtime_error(diagnostic.toStdString()), _operation(operation),
        _stage(stage), _assetPath(std::move(assetPath)),
        _diagnostic(std::move(diagnostic)),
        _rollbackDiagnostics(std::move(rollbackDiagnostics)),
        _workspacePaths(std::move(workspacePaths)) {}

  [[nodiscard]] ArchiveOperation operation() const noexcept {
    return _operation;
  }
  [[nodiscard]] ArchiveFailureStage stage() const noexcept { return _stage; }
  [[nodiscard]] const QString &assetPath() const noexcept { return _assetPath; }
  [[nodiscard]] const QString &diagnostic() const noexcept {
    return _diagnostic;
  }
  [[nodiscard]] const QStringList &rollbackDiagnostics() const noexcept {
    return _rollbackDiagnostics;
  }
  [[nodiscard]] bool rollbackCompleted() const noexcept {
    return _rollbackDiagnostics.isEmpty();
  }
  /*! \brief Returns structured recovery workspace paths, if applicable. */
  [[nodiscard]] const QStringList &workspacePaths() const noexcept {
    return _workspacePaths;
  }

private:
  ArchiveOperation _operation;
  ArchiveFailureStage _stage;
  QString _assetPath;
  QString _diagnostic;
  QStringList _rollbackDiagnostics;
  QStringList _workspacePaths;
};
