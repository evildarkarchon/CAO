/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

#include "AssetWorkMode.h"

#include <QString>
#include <QStringList>

class AssetWorkScope final {
public:
  /*!
   * \brief Resolves the selected Mod directories for one execution.
   * \param selectedPath The selected Mod or parent directory.
   * \param mode Whether the selected path names one Mod or contains several.
   * \param ignoredMods Case-insensitive Mod names excluded in Several Mods
   * mode.
   * \return A stable scope that can be shared by recovery and planning.
   */
  [[nodiscard]] static AssetWorkScope resolve(const QString &selectedPath,
                                              AssetWorkMode mode,
                                              const QStringList &ignoredMods);

  /*!
   * \brief Returns the resolved Mod directories in deterministic selection
   * order.
   * \return The exact directories in scope for this execution.
   */
  [[nodiscard]] const QStringList &selectedMods() const noexcept;

private:
  explicit AssetWorkScope(QStringList selectedMods);

  QStringList _selectedMods;
};
