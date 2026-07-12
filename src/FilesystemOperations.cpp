/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#include "FilesystemOperations.h"
#include "AssetPathVisibility.h"
#include "PluginsOperations.h"

namespace {
QMap<QString, QString> folderSnapshot(const QString &folder,
                                      const bool includeFileSize) {
  QMap<QString, QString> entries;
  const QDir root(folder);
  QDirIterator it(folder, QDirIterator::Subdirectories);

  while (it.hasNext()) {
    it.next();

    const QFileInfo entry = it.fileInfo();
    QString descriptor = entry.isDir() ? "dir" : "file";

    // Compare sizes from the iterator's absolute entry, not a relative path
    // that depends on the caller's current working directory.
    if (includeFileSize && entry.isFile())
      descriptor += ":" + QString::number(entry.size());

    entries.insert(root.relativeFilePath(entry.filePath()), descriptor);
  }

  return entries;
}

void deleteEmptyAssetDirectories(const QString &folderPath) {
  QDir folder(folderPath);
  const QFileInfoList children = folder.entryInfoList(
      QDir::Dirs | QDir::Hidden | QDir::System | QDir::NoDotAndDotDot,
      QDir::Name | QDir::IgnoreCase);
  for (const QFileInfo &child : children) {
    if (AssetPathVisibility::isInternalPath(child.filePath()))
      continue;
    deleteEmptyAssetDirectories(child.filePath());
    if (!child.filePath().contains("separator", Qt::CaseInsensitive))
      folder.rmdir(child.fileName());
  }
}
} // namespace

void FilesystemOperations::deleteEmptyDirectories(const QString &folderPath) {
  deleteEmptyAssetDirectories(folderPath);
}

bool FilesystemOperations::compareFolders(const QString &folder1,
                                          const QString &folder2,
                                          const bool &checkFileSize) {
  return folderSnapshot(folder1, checkFileSize) ==
         folderSnapshot(folder2, checkFileSize);
}

void FilesystemOperations::copyDir(const QString &source,
                                   const QString &destination,
                                   const bool overwriteExisting) {
  const QDir sourceDir(source);
  QDir destinationDir(destination);
  QDirIterator it(source, QDirIterator::Subdirectories);

  PLOG_VERBOSE << "Entering " + QString(__FUNCTION__) + " function";
  PLOG_DEBUG << "dest folder: " + destination + "\nsource folder: " + source;

  QStringList oldFiles;

  const QString currentDir = QDir::currentPath();
  QDir::setCurrent(destination);

  while (it.hasNext()) {
    it.next();
    if (!it.fileInfo().isDir()) // Skipping all directories
      oldFiles << it.filePath();
  }

  oldFiles.removeDuplicates();

  for (const auto &oldFile : oldFiles) {
    QString relativeFilename = sourceDir.relativeFilePath(oldFile);
    QString newFile =
        QDir::cleanPath(destination + QDir::separator() + relativeFilename);

    if (newFile.size() >= 255) {
      PLOG_ERROR << "An error occurred while moving files. Try reducing path "
                    "size (260 characters is the maximum)";
      return;
    }

    destinationDir.mkpath(QFileInfo(newFile).path());

    if (overwriteExisting)
      destinationDir.remove(newFile);

    QFile::copy(oldFile, newFile);
  }
  PLOG_VERBOSE << "Exiting moveFiles function";

  QDir::setCurrent(currentDir);
}

QStringList
FilesystemOperations::readFile(QFile &file,
                               std::function<void(QString &line)> function) {
  QStringList list;

  file.open(QFile::ReadOnly);
  if (!file.isOpen())
    return list;

  while (!file.atEnd()) {
    QString &&line = file.readLine().simplified();
    if (line.startsWith("#") || line.isEmpty())
      continue;

    function(line);
    list << line;
  }
  return list;
}

QStringList FilesystemOperations::readFile(QFile &file) {
  QStringList list;

  file.open(QFile::ReadOnly);
  if (!file.isOpen())
    return list;

  while (!file.atEnd()) {
    QString &&line = file.readLine().simplified();
    if (line.startsWith("#") || line.isEmpty())
      continue;

    list << line;
  }
  return list;
}

QStringList FilesystemOperations::listPlugins(QDirIterator &it) {
  QStringList plugins;
  const QRegularExpression pluginsExt(
      "\\.es[plm]$", QRegularExpression::CaseInsensitiveOption);
  while (it.hasNext()) {
    it.next();
    if (it.fileName().contains(pluginsExt) && !it.fileInfo().isDir())
      plugins << it.filePath();
  }

  return plugins;
}
