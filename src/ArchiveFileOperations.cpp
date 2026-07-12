/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#include "ArchiveFileOperations.h"

#include <QDir>
#include <QDirIterator>
#include <QFile>
#include <QFileInfo>

#include <stdexcept>

#ifdef Q_OS_WIN
#include <Windows.h>
#include <io.h>
#else
#include <unistd.h>
#endif

namespace {
[[noreturn]] void fail(const QString &operation, const QString &path) {
  throw std::runtime_error(
      QString("%1 failed for %2").arg(operation, path).toStdString());
}

#ifdef Q_OS_WIN
class NativeHandle final {
public:
  explicit NativeHandle(const HANDLE handle) : m_handle(handle) {}
  ~NativeHandle() {
    if (m_handle != INVALID_HANDLE_VALUE)
      CloseHandle(m_handle);
  }
  NativeHandle(const NativeHandle &) = delete;
  NativeHandle &operator=(const NativeHandle &) = delete;
  [[nodiscard]] HANDLE get() const noexcept { return m_handle; }

private:
  HANDLE m_handle = INVALID_HANDLE_VALUE;
};

DWORD volumeSerialForExistingPath(const QString &path,
                                  const bool rejectReparsePoint) {
  DWORD flags = FILE_FLAG_BACKUP_SEMANTICS;
  if (rejectReparsePoint)
    flags |= FILE_FLAG_OPEN_REPARSE_POINT;
  NativeHandle handle(CreateFileW(
      reinterpret_cast<LPCWSTR>(QDir::toNativeSeparators(path).utf16()),
      FILE_READ_ATTRIBUTES,
      FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, nullptr,
      OPEN_EXISTING, flags, nullptr));
  if (handle.get() == INVALID_HANDLE_VALUE)
    fail("Opening Archive Asset for move preflight", path);
  BY_HANDLE_FILE_INFORMATION information{};
  if (!GetFileInformationByHandle(handle.get(), &information))
    fail("Reading Archive Asset volume identity", path);
  if (rejectReparsePoint &&
      (information.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT) != 0)
    fail("Moving a reparse-point Archive Asset", path);
  return information.dwVolumeSerialNumber;
}

QString nearestExistingAncestor(QString path) {
  path = QDir::cleanPath(path);
  while (!QFileInfo::exists(path)) {
    const QString parent = QFileInfo(path).absolutePath();
    if (parent == path)
      fail("Finding destination volume for Archive Asset move", path);
    path = parent;
  }
  return path;
}
#endif
} // namespace

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

void QtArchiveFileOperations::createDirectory(const QString &path) {
  FileArchiveTransactionDurability durability;
  durability.createDirectory(path);
}

QString QtArchiveFileOperations::identity(const QString &path) const {
  FileArchiveTransactionDurability durability;
  return durability.identity(path);
}

void QtArchiveFileOperations::writeNewDurably(const QString &path,
                                              const QByteArray &contents) {
  FileArchiveTransactionDurability durability;
  durability.writeNewDurably(path, contents);
}

void QtArchiveFileOperations::appendDurably(const QString &path,
                                            const QByteArray &contents) {
  FileArchiveTransactionDurability durability;
  durability.appendDurably(path, contents);
}

QByteArray QtArchiveFileOperations::readAll(const QString &path) const {
  FileArchiveTransactionDurability durability;
  return durability.readAll(path);
}

void QtArchiveFileOperations::validateWorkspacePath(
    const QString &modPath, const QString &workspacePath) const {
  FileArchiveTransactionDurability durability;
  durability.validateWorkspacePath(modPath, workspacePath);
}

void QtArchiveFileOperations::validateReplayPath(
    const QString &path, const QString &modPath,
    const QString &workspacePath) const {
  FileArchiveTransactionDurability durability;
  durability.validateReplayPath(path, modPath, workspacePath);
}

void QtArchiveFileOperations::preflightTransaction(
    const QString &modPath) const {
  FileArchiveTransactionDurability durability;
  durability.preflightTransaction(modPath);
}

void QtArchiveFileOperations::flushFileDurably(const QString &path) {
  QFile file(path);
  if (!file.open(QIODevice::ReadWrite) || !file.flush())
    fail("Opening staged Archive Asset for durable flush", path);
#ifdef Q_OS_WIN
  const intptr_t nativeHandle = _get_osfhandle(file.handle());
  if (nativeHandle == -1 ||
      !FlushFileBuffers(reinterpret_cast<HANDLE>(nativeHandle)))
    fail("Durably flushing staged Archive Asset", path);
#else
  if (::fsync(file.handle()) != 0)
    fail("Durably flushing staged Archive Asset", path);
#endif
}

void QtArchiveFileOperations::createDirectories(const QString &path) {
  if (!QDir().mkpath(path))
    fail("Creating directory", path);
}

void QtArchiveFileOperations::move(const QString &source,
                                   const QString &destination) {
#ifdef Q_OS_WIN
  if (QFileInfo::exists(destination))
    fail("Moving Asset to " + destination, source);
  const DWORD sourceVolume = volumeSerialForExistingPath(source, true);
  const DWORD destinationVolume =
      volumeSerialForExistingPath(nearestExistingAncestor(destination), false);
  if (sourceVolume != destinationVolume)
    fail("Moving Archive Asset across filesystems to " + destination, source);

  // MOVEFILE_WRITE_THROUGH makes publication durable before journal progress
  // can advance. Omitting COPY_ALLOWED and REPLACE_EXISTING is intentional:
  // an Archive Transaction must be an atomic, same-volume, no-overwrite move.
  if (!MoveFileExW(
          reinterpret_cast<LPCWSTR>(QDir::toNativeSeparators(source).utf16()),
          reinterpret_cast<LPCWSTR>(
              QDir::toNativeSeparators(destination).utf16()),
          MOVEFILE_WRITE_THROUGH))
    fail("Moving Asset to " + destination, source);
#else
  if (QFileInfo::exists(destination) || !QFile::rename(source, destination))
    fail("Moving Asset to " + destination, source);
#endif
}

void QtArchiveFileOperations::removeFile(const QString &path) {
  if (QFileInfo::exists(path) && !QFile::remove(path))
    fail("Removing Asset", path);
}

void QtArchiveFileOperations::removeEmptyDirectory(const QString &path) {
  if (!QFileInfo::exists(path))
    return;
  const QDir directory(path);
  if (!directory.entryList(QDir::AllEntries | QDir::NoDotAndDotDot).isEmpty())
    return;
  if (!QDir().rmdir(path))
    fail("Removing empty directory", path);
}

void QtArchiveFileOperations::removeTree(const QString &path) {
  if (QFileInfo::exists(path) && !QDir(path).removeRecursively())
    fail("Removing archive staging directory", path);
}
