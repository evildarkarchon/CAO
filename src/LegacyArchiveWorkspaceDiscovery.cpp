/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#include "LegacyArchiveWorkspaceDiscovery.h"

#include <QFileInfo>
#include <QUuid>

#include <algorithm>

namespace {
/*! \brief Matches the exact sibling naming scheme used by legacy archive
 * staging. */
std::optional<QString> legacyArchiveOwnerName(const QString &directoryName) {
  if (!directoryName.startsWith('.'))
    return std::nullopt;

  static const QStringList markers{QStringLiteral(".cao-pack-"),
                                   QStringLiteral(".cao-extract-")};
  for (const QString &marker : markers) {
    const qsizetype markerIndex =
        directoryName.lastIndexOf(marker, -1, Qt::CaseInsensitive);
    // A legacy name starts with a dot and has a non-empty anchor name before
    // the archive-operation marker.
    if (markerIndex <= 1)
      continue;

    const QString uuidText = directoryName.mid(markerIndex + marker.size());
    const QUuid uuid(uuidText);
    if (!uuid.isNull() && uuid.toString(QUuid::WithoutBraces)
                                  .compare(uuidText, Qt::CaseInsensitive) == 0)
      return directoryName.mid(1, markerIndex - 1);
  }
  return std::nullopt;
}

bool belongsToIgnoredMod(const QString &ownerName,
                         const QStringList &ignoredMods) {
  return std::any_of(
      ignoredMods.cbegin(), ignoredMods.cend(), [&](const QString &ignored) {
        return ownerName.compare(ignored, Qt::CaseInsensitive) == 0 ||
               ownerName.startsWith(ignored + QLatin1Char('.'),
                                    Qt::CaseInsensitive);
      });
}
} // namespace

std::optional<ArchiveExecutionError>
LegacyArchiveWorkspaceDiscoveryResult::manualResolutionFailure() const {
  if (legacyWorkspacePaths.isEmpty())
    return std::nullopt;

  const QString diagnostic =
      QStringLiteral("Legacy Archive Workspaces require manual resolution: ") +
      legacyWorkspacePaths.join(QStringLiteral("; "));
  return ArchiveExecutionError(
      ArchiveOperation::Recovery, ArchiveFailureStage::RecoveryDiscovery,
      legacyWorkspacePaths.front(), diagnostic, {}, legacyWorkspacePaths);
}

LegacyArchiveWorkspaceDiscoveryResult LegacyArchiveWorkspaceDiscovery::discover(
    const QStringList &candidateDirectories, const QStringList &ignoredMods) {
  LegacyArchiveWorkspaceDiscoveryResult result;
  for (const QString &path : candidateDirectories) {
    const auto owner = legacyArchiveOwnerName(QFileInfo(path).fileName());
    if (!owner) {
      result.candidateModPaths.push_back(path);
    } else if (belongsToIgnoredMod(*owner, ignoredMods)) {
      result.ignoredLegacyWorkspacePaths.push_back(path);
    } else {
      result.legacyWorkspacePaths.push_back(path);
    }
  }
  return result;
}
