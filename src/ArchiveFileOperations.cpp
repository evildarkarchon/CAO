/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#include "ArchiveFileOperations.h"

#include <QDir>
#include <QDirIterator>
#include <QFile>
#include <QFileInfo>
#include <QUuid>

#include <stdexcept>

namespace {
[[noreturn]] void fail(const QString &operation, const QString &path) {
  throw std::runtime_error(
      QString("%1 failed for %2").arg(operation, path).toStdString());
}
} // namespace

QString
QtArchiveFileOperations::createSiblingStagingDirectory(const QString &anchor,
                                                       const QString &purpose) {
  const QFileInfo anchorInfo(anchor);
  const QString parent =
      anchorInfo.isDir()
          ? anchorInfo.absolutePath()
          : QFileInfo(anchorInfo.dir().absolutePath()).dir().absolutePath();
  const QString name =
      QString(".%1.cao-%2-%3")
          .arg(anchorInfo.fileName(), purpose,
               QUuid::createUuid().toString(QUuid::WithoutBraces));
  const QString path = QDir(parent).filePath(name);
  if (!QDir().mkpath(path))
    fail("Creating archive staging directory", path);
  return path;
}

QVector<ArchiveFileEntry>
QtArchiveFileOperations::listRecursively(const QString &root) const {
  QVector<ArchiveFileEntry> result;
  const QDir rootDir(root);
  QDirIterator iterator(root, QDir::AllEntries | QDir::NoDotAndDotDot,
                        QDirIterator::Subdirectories);
  while (iterator.hasNext()) {
    const QString path = iterator.next();
    const QFileInfo info = iterator.fileInfo();
    result.push_back({path, rootDir.relativeFilePath(path), info.isFile(),
                      info.isSymLink()});
  }
  return result;
}

bool QtArchiveFileOperations::exists(const QString &path) const {
  return QFileInfo::exists(path);
}

void QtArchiveFileOperations::createDirectories(const QString &path) {
  if (!QDir().mkpath(path))
    fail("Creating directory", path);
}

void QtArchiveFileOperations::move(const QString &source,
                                   const QString &destination) {
  if (QFileInfo::exists(destination) || !QFile::rename(source, destination))
    fail("Moving Asset to " + destination, source);
}

void QtArchiveFileOperations::removeFile(const QString &path) {
  if (QFileInfo::exists(path) && !QFile::remove(path))
    fail("Removing Asset", path);
}

void QtArchiveFileOperations::removeEmptyDirectory(const QString &path) {
  if (QFileInfo::exists(path) && !QDir().rmdir(path))
    fail("Removing empty directory", path);
}

void QtArchiveFileOperations::removeTree(const QString &path) {
  if (QFileInfo::exists(path) && !QDir(path).removeRecursively())
    fail("Removing archive staging directory", path);
}
