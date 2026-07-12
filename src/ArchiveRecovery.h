/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

#include "ArchiveExecutionError.h"

#include <QString>
#include <QStringList>
#include <QVector>

class ArchiveFileOperations;

enum class ArchiveRecoveryAction { CommittedCleanup, IncompleteRollback };

enum class ArchiveRecoveryOutcome { Clean, Recovered, RecoveryRequired };

/*! \brief Describes one owned Archive Transaction found during recovery. */
struct ArchiveRecoveryWorkspace {
  QString path;
  ArchiveRecoveryAction action = ArchiveRecoveryAction::IncompleteRollback;
};

/*!
 * \brief Typed result of validating or performing Archive Recovery.
 *
 * RecoveryRequired is returned only for Dry Run and prohibits planning from
 * continuing until a normal execution restores consistency.
 */
struct ArchiveRecoveryResult {
  ArchiveRecoveryOutcome outcome = ArchiveRecoveryOutcome::Clean;
  QVector<ArchiveRecoveryWorkspace> workspaces;

  /*! \brief Returns whether Asset Work Item planning may safely continue. */
  [[nodiscard]] bool planningMayProceed() const noexcept {
    return outcome == ArchiveRecoveryOutcome::Clean ||
           outcome == ArchiveRecoveryOutcome::Recovered;
  }
};

/*!
 * \brief Discovers and validates owned Archive Transaction workspaces before
 * planning.
 *
 * The caller must hold exclusive execution locks for every supplied Mod for
 * the whole call. Recovery deliberately has no cancellation callback: once
 * entered, validation and committed cleanup form an uninterruptible
 * consistency phase.
 */
class ArchiveRecovery final {
public:
  /*!
   * \brief Recovers workspaces within exactly the already-selected locked
   * Mods.
   * \param lockedModPaths Canonical or stable paths whose locks are held by
   * the caller.
   * \param dryRun Validate and report required work without changing files.
   * \param files Durable filesystem adapter used to reopen and replay owned
   * workspaces. It must outlive this call.
   * \return A typed clean, recovered, or recovery-required outcome.
   * \throws ArchiveExecutionError before mutation for unowned, corrupt, or
   * ambiguous histories, or when rollback or committed cleanup fails.
   */
  [[nodiscard]] static ArchiveRecoveryResult
  recover(const QStringList &lockedModPaths, bool dryRun,
          ArchiveFileOperations &files);
};
