/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

#include "ArchiveExecutionError.h"

#include <QStringList>

#include <optional>

/*! \brief Read-only classification of candidate Mod directories. */
struct LegacyArchiveWorkspaceDiscoveryResult {
  QStringList candidateModPaths;
  QStringList legacyWorkspacePaths;
  QStringList ignoredLegacyWorkspacePaths;

  /*!
   * \brief Creates the typed failure used when a legacy path is in scope.
   * \return A manual-resolution failure containing every exact legacy path,
   * or no value when discovery found none.
   */
  [[nodiscard]] std::optional<ArchiveExecutionError>
  manualResolutionFailure() const;
};

/*!
 * \brief Recognizes unjournaled sibling workspaces created by older CAO
 * versions before Asset Work Items are planned.
 *
 * Discovery never adopts, deletes, or otherwise mutates a candidate. Callers
 * decide whether the reported paths intersect the requested scope; reported
 * paths outside it can therefore be omitted with a diagnostic and left
 * untouched.
 */
class LegacyArchiveWorkspaceDiscovery final {
public:
  /*!
   * \brief Classifies exact legacy archive workspace names among candidates.
   * \param candidateDirectories Immediate directories supplied by recovery
   * preflight in their stable reporting order.
   * \param ignoredMods Mod names whose exact legacy siblings stay untouched.
   * \return Ordinary Mod candidates and exact legacy workspace paths.
   */
  [[nodiscard]] static LegacyArchiveWorkspaceDiscoveryResult
  discover(const QStringList &candidateDirectories,
           const QStringList &ignoredMods = {});
};
