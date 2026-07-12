/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#include "AssetPathVisibility.h"

#include <QDir>
#include <QStringList>

#include <algorithm>

bool AssetPathVisibility::isInternalPath(const QString &path) noexcept {
  const QString normalized = QDir::fromNativeSeparators(QDir::cleanPath(path));
  const QStringList segments = normalized.split('/', Qt::SkipEmptyParts);
  return std::any_of(segments.cbegin(), segments.cend(),
                     [](const QString &segment) {
                       return segment.compare(ArchiveTransactionRootName,
                                              Qt::CaseInsensitive) == 0;
                     });
}
