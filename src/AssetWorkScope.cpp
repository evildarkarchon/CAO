/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#include "AssetWorkScope.h"

#include <QDir>

#include <utility>

AssetWorkScope AssetWorkScope::resolve(const QString &selectedPath,
                                       const AssetWorkMode mode,
                                       const QStringList &ignoredMods) {
  if (mode == AssetWorkMode::SingleMod)
    return AssetWorkScope(QStringList{selectedPath});

  QStringList selectedMods;
  const QDir directory(selectedPath);
  for (const auto &subDirectory :
       directory.entryList(QDir::Dirs | QDir::NoDotAndDotDot, QDir::Name)) {
    // Separators are empty directories used by Mod Organizer 2.
    if (!subDirectory.contains("separator", Qt::CaseInsensitive) &&
        !ignoredMods.contains(subDirectory, Qt::CaseInsensitive))
      selectedMods << directory.filePath(subDirectory);
  }

  return AssetWorkScope(std::move(selectedMods));
}

const QStringList &AssetWorkScope::selectedMods() const noexcept {
  return _selectedMods;
}

AssetWorkScope::AssetWorkScope(QStringList selectedMods)
    : _selectedMods(std::move(selectedMods)) {}
