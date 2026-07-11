/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#include "AssetPathConflicts.h"

#include <QDir>
#include <QFileInfo>
#include <QSet>

namespace {
QString normalizedWritePath(const QString &path) {
  const QFileInfo info(path);
  const QFileInfo parent(info.absolutePath());
  const QString canonicalParent = parent.canonicalFilePath();
  const QString resolvedParent =
      canonicalParent.isEmpty() ? parent.absoluteFilePath() : canonicalParent;
  return QDir::cleanPath(QDir(resolvedParent).filePath(info.fileName()))
      .toCaseFolded();
}
} // namespace

QStringList looseAssetWriteKeys(const LooseAssetWorkItem &workItem) {
  QStringList keys{normalizedWritePath(workItem.path)};
  if (workItem.kind == LooseAssetKind::TextureTga) {
    QString ddsPath = workItem.path;
    if (ddsPath.endsWith(".tga", Qt::CaseInsensitive))
      ddsPath.chop(4);
    ddsPath += ".dds";
    keys.push_back(normalizedWritePath(ddsPath));
  }
  keys.removeDuplicates();
  return keys;
}

bool looseAssetWriteKeysConflict(const QStringList &first,
                                 const QStringList &second) {
  const QSet<QString> firstSet(first.begin(), first.end());
  for (const auto &key : second) {
    if (firstSet.contains(key))
      return true;
  }
  return false;
}
