/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#include "ModAssetMetadata.h"
#include "AssetPathVisibility.h"
#include "FilesystemOperations.h"
#include "PluginsOperations.h"
#include "Profiles.h"

#include <QDir>
#include <QDirIterator>

namespace {
constexpr auto CustomHeadpartsFile = "customHeadparts.txt";

QString normalizeAssetPath(QString path) {
  path = QDir::fromNativeSeparators(QDir::cleanPath(path));

  // Metadata sources store game-relative paths, while Asset Work Items carry
  // absolute filesystem paths. Normalize both to the relative mesh Asset path.
  const QString lowerPath = path.toLower();
  const QString meshesMarker = "/meshes/";
  const int meshesMarkerIndex = lowerPath.indexOf(meshesMarker);
  if (meshesMarkerIndex >= 0)
    path = path.mid(meshesMarkerIndex + 1);

  return path.toLower();
}

QStringList listPluginsOutsideTransactions(const QString &rootPath) {
  static const QRegularExpression pluginExtension(
      QStringLiteral("\\.es[plm]$"),
      QRegularExpression::CaseInsensitiveOption);
  QStringList plugins;
  const QDir root(rootPath);
  const QFileInfoList entries = root.entryInfoList(
      QDir::Dirs | QDir::Files | QDir::Hidden | QDir::System |
          QDir::NoDotAndDotDot,
      QDir::Name | QDir::IgnoreCase);
  for (const QFileInfo &entry : entries) {
    if (AssetPathVisibility::isInternalPath(entry.filePath()))
      continue;
    if (entry.isDir())
      plugins += listPluginsOutsideTransactions(entry.filePath());
    else if (entry.fileName().contains(pluginExtension))
      plugins << entry.filePath();
  }
  return plugins;
}
} // namespace

ModAssetMetadata::ModAssetMetadata(const QStringList &headpartMeshPaths) {
  for (const auto &path : headpartMeshPaths)
    _headpartMeshPaths.insert(normalizeAssetPath(path));
}

bool ModAssetMetadata::isHeadpartMesh(const QString &assetPath) const {
  const QString normalizedPath = normalizeAssetPath(assetPath);
  return normalizedPath.contains("facegen") ||
         _headpartMeshPaths.contains(normalizedPath);
}

ModAssetMetadataBuilder::ModAssetMetadataBuilder(
    const ProfileAssetReferenceProvider &profileReferences,
    const PluginAssetReferenceReader &pluginReferences)
    : _profileReferences(profileReferences),
      _pluginReferences(pluginReferences) {}

ModAssetMetadata
ModAssetMetadataBuilder::buildForMods(const QStringList &selectedMods) const {
  QStringList headpartMeshPaths =
      _profileReferences.readReferenceList(CustomHeadpartsFile);
  if (headpartMeshPaths.isEmpty()) {
    PLOG_ERROR << "customHeadparts.txt not found. This can cause issue when "
                  "optimizing meshes, "
                  "as some headparts won't be detected.";
  }

  for (const auto &mod : selectedMods) {
    for (const auto &plugin : listPluginsOutsideTransactions(mod))
      headpartMeshPaths += _pluginReferences.listHeadparts(plugin);
  }

  headpartMeshPaths.removeDuplicates();
  return ModAssetMetadata(headpartMeshPaths);
}

QStringList ProfileFileAssetReferenceProvider::readReferenceList(
    const QString &fileName) const {
  QFile file = Profiles::getFile(fileName);
  return FilesystemOperations::readFile(
      file, [](QString &line) { line = QDir::cleanPath(line); });
}

QStringList PluginOperationsAssetReferenceReader::listHeadparts(
    const QString &pluginPath) const {
  return PluginsOperations::listHeadparts(pluginPath);
}
