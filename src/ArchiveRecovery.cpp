/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#include "ArchiveRecovery.h"

#include "ArchiveFileOperations.h"
#include "ArchiveTransactionWorkspace.h"

#include <QDir>
#include <QFileInfo>
#include <QFileInfoList>

#include <algorithm>
#include <memory>
#include <stdexcept>
#include <utility>
#include <vector>

namespace {
struct DiscoveredWorkspace {
  QString modPath;
  ArchiveTransactionWorkspace workspace;
};

[[noreturn]] void throwRecoveryFailure(const ArchiveFailureStage stage,
                                       const QString &path,
                                       const QString &message,
                                       QStringList workspacePaths = {}) {
  const QString diagnostic = message + QStringLiteral(": ") + path;
  if (workspacePaths.isEmpty() && !path.isEmpty())
    workspacePaths.push_back(path);
  throw ArchiveExecutionError(ArchiveOperation::Recovery, stage, path,
                              diagnostic, {}, std::move(workspacePaths));
}

QString normalizedPath(const QString &path) {
  return QDir::cleanPath(QFileInfo(path).absoluteFilePath());
}

bool samePath(const QString &left, const QString &right) {
  return normalizedPath(left).compare(normalizedPath(right),
                                      Qt::CaseInsensitive) == 0;
}

/*! \brief Resolves the durable anchor identity before or after its move. */
QString currentAnchorIdentity(const ArchiveTransactionWorkspace &workspace,
                              ArchiveFileOperations &files) {
  const QString &anchorPath = workspace.manifest().canonicalAnchorPath;
  if (files.exists(anchorPath))
    return files.identity(anchorPath);

  for (const auto &record : workspace.records()) {
    if (record.kind != ArchiveTransactionRecordKind::Intent ||
        record.fields.value(QStringLiteral("operation")) !=
            QStringLiteral("move") ||
        !samePath(record.fields.value(QStringLiteral("source")), anchorPath))
      continue;
    const QString destination =
        record.fields.value(QStringLiteral("destination"));
    if (files.exists(destination))
      return files.identity(destination);
  }
  throw std::runtime_error(
      "Archive Transaction anchor identity cannot be established");
}

/*! \brief Proves that a reopened workspace still belongs to the locked Mod. */
void validateOwnership(const ArchiveTransactionWorkspace &workspace,
                       const QString &modPath, ArchiveFileOperations &files) {
  const ArchiveTransactionManifest &manifest = workspace.manifest();
  if (!samePath(manifest.canonicalModPath, modPath))
    throw std::runtime_error("Archive Transaction belongs to a different Mod");
  const QString currentModIdentity = files.identity(modPath);
  if (currentModIdentity != manifest.modIdentity ||
      currentModIdentity != manifest.volumeIdentity)
    throw std::runtime_error("Archive Transaction Mod identity has changed");
  if (currentAnchorIdentity(workspace, files) != manifest.anchorIdentity)
    throw std::runtime_error("Archive Transaction anchor identity has changed");
}

/*! \brief Reopens every immediate owned workspace without mutating Assets. */
std::vector<DiscoveredWorkspace> discover(const QStringList &lockedModPaths,
                                          ArchiveFileOperations &files) {
  std::vector<DiscoveredWorkspace> discovered;
  auto durability = std::shared_ptr<ArchiveTransactionDurability>(
      &files, [](ArchiveTransactionDurability *) {});
  for (const QString &suppliedModPath : lockedModPaths) {
    const QString modPath = normalizedPath(suppliedModPath);
    const QString rootPath =
        QDir(modPath).filePath(ArchiveTransactionWorkspace::ReservedRootName);
    const QFileInfo root(rootPath);
    if (!root.exists())
      continue;

    if (!root.isDir() || root.isSymLink())
      throwRecoveryFailure(ArchiveFailureStage::RecoveryDiscovery, rootPath,
                           "Archive Recovery found an unowned reserved root");

    const QFileInfoList entries = QDir(rootPath).entryInfoList(
        QDir::AllEntries | QDir::NoDotAndDotDot, QDir::Name | QDir::IgnoreCase);
    // A valid successful cleanup removes the root. Its unexplained continued
    // existence therefore cannot be treated as proof of CAO ownership.
    if (entries.isEmpty())
      throwRecoveryFailure(ArchiveFailureStage::RecoveryDiscovery, rootPath,
                           "Archive Recovery found an unowned empty root");

    for (const QFileInfo &entry : entries) {
      if (!entry.isDir() || entry.isSymLink())
        throwRecoveryFailure(ArchiveFailureStage::RecoveryDiscovery,
                             normalizedPath(entry.absoluteFilePath()),
                             "Archive Recovery found an unowned reserved root");

      const QString workspacePath = normalizedPath(entry.absoluteFilePath());
      try {
        auto workspace =
            ArchiveTransactionWorkspace::reopen(workspacePath, durability);
        validateOwnership(workspace, modPath, files);
        discovered.push_back({modPath, std::move(workspace)});
      } catch (const ArchiveExecutionError &) {
        throw;
      } catch (const std::exception &error) {
        throwRecoveryFailure(ArchiveFailureStage::RecoveryValidation,
                             workspacePath, QString::fromUtf8(error.what()),
                             {workspacePath});
      }
    }
  }
  return discovered;
}

/*! \brief Rejects ambiguous per-Mod rollback histories before replay. */
void validateIncompleteHistoryCounts(
    const QStringList &lockedModPaths,
    const std::vector<DiscoveredWorkspace> &workspaces) {
  for (const QString &suppliedModPath : lockedModPaths) {
    const QString modPath = normalizedPath(suppliedModPath);
    const qsizetype incomplete = static_cast<qsizetype>(std::count_if(
        workspaces.cbegin(), workspaces.cend(), [&](const auto &candidate) {
          return samePath(candidate.modPath, modPath) &&
                 !candidate.workspace.isCommitted();
        }));
    if (incomplete > 1) {
      QStringList conflictingPaths;
      for (const auto &candidate : workspaces) {
        if (samePath(candidate.modPath, modPath) &&
            !candidate.workspace.isCommitted())
          conflictingPaths.push_back(candidate.workspace.path());
      }
      throwRecoveryFailure(
          ArchiveFailureStage::RecoveryValidation, modPath,
          "Archive Recovery found multiple incomplete transactions",
          std::move(conflictingPaths));
    }
  }
}

/*! \brief Converts validated state into caller-facing progress data. */
QVector<ArchiveRecoveryWorkspace>
describe(const std::vector<DiscoveredWorkspace> &workspaces) {
  QVector<ArchiveRecoveryWorkspace> result;
  result.reserve(static_cast<int>(workspaces.size()));
  for (const auto &candidate : workspaces) {
    result.push_back({candidate.workspace.path(),
                      candidate.workspace.isCommitted()
                          ? ArchiveRecoveryAction::CommittedCleanup
                          : ArchiveRecoveryAction::IncompleteRollback});
  }
  return result;
}
} // namespace

ArchiveRecoveryResult
ArchiveRecovery::recover(const QStringList &lockedModPaths, const bool dryRun,
                         ArchiveFileOperations &files) {
  std::vector<DiscoveredWorkspace> workspaces = discover(lockedModPaths, files);
  validateIncompleteHistoryCounts(lockedModPaths, workspaces);

  ArchiveRecoveryResult result;
  result.workspaces = describe(workspaces);
  if (workspaces.empty())
    return result;

  if (dryRun) {
    result.outcome = ArchiveRecoveryOutcome::RecoveryRequired;
    return result;
  }

  // Roll back incomplete histories before deleting authoritative committed
  // cleanup state, so a rollback failure preserves the maximum evidence.
  std::stable_sort(workspaces.begin(), workspaces.end(),
                   [](const auto &left, const auto &right) {
                     return !left.workspace.isCommitted() &&
                            right.workspace.isCommitted();
                   });
  for (auto &candidate : workspaces) {
    const QStringList failures = candidate.workspace.replay(files);
    if (!failures.isEmpty())
      throwRecoveryFailure(candidate.workspace.isCommitted()
                               ? ArchiveFailureStage::RecoveryCleanup
                               : ArchiveFailureStage::RecoveryRollback,
                           candidate.workspace.path(),
                           failures.join(QStringLiteral("; ")),
                           {candidate.workspace.path()});
  }
  result.outcome = ArchiveRecoveryOutcome::Recovered;
  return result;
}
