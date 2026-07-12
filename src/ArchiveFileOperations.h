/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

#include "ArchiveTransactionWorkspace.h"

#include <QByteArray>
#include <QString>
#include <QVector>

struct ArchiveFileEntry {
  QString absolutePath;
  QString relativePath;
  bool regularFile = false;
  bool symbolicLink = false;
};

/*!
 * \brief Filesystem operations required to publish and roll back archives.
 *
 * move() never overwrites. Implementations throw std::runtime_error for
 * ordinary operation failures so the archive transaction can retain context.
 */
class ArchiveFileOperations : public ArchiveTransactionDurability {
public:
  virtual ~ArchiveFileOperations() = default;

  /*! \brief Lists every entry below \p root with paths relative to that root.
   *  \throws std::runtime_error if the tree cannot be enumerated safely.
   */
  [[nodiscard]] virtual QVector<ArchiveFileEntry>
  listRecursively(const QString &root) const = 0;
  /*! \brief Returns whether a filesystem entry exists at \p path. */
  [[nodiscard]] virtual bool exists(const QString &path) const = 0;
  /*! \brief Creates exactly one durable transaction directory path. */
  virtual void createDirectory(const QString &path) override = 0;
  /*! \brief Returns the stable identity used by transaction ownership. */
  [[nodiscard]] virtual QString
  identity(const QString &path) const override = 0;
  /*! \brief Creates and durably flushes a transaction metadata file. */
  virtual void writeNewDurably(const QString &path,
                               const QByteArray &contents) override = 0;
  /*! \brief Appends and durably flushes transaction journal bytes. */
  virtual void appendDurably(const QString &path,
                             const QByteArray &contents) override = 0;
  /*! \brief Reads a complete transaction metadata file. */
  [[nodiscard]] virtual QByteArray
  readAll(const QString &path) const override = 0;
  /*! \brief Flushes already-written staged file bytes before publication. */
  virtual void flushFileDurably(const QString &path) = 0;
  /*! \brief Creates \p path and any missing parents.
   *  \throws std::runtime_error when creation fails.
   */
  virtual void createDirectories(const QString &path) = 0;
  /*! \brief Moves \p source to the non-existent \p destination.
   *  \throws std::runtime_error when the source is absent, the destination
   *  exists, or the move fails.
   */
  virtual void move(const QString &source, const QString &destination) = 0;
  /*! \brief Removes the regular file at \p path if present.
   *  \throws std::runtime_error when removal fails.
   */
  virtual void removeFile(const QString &path) = 0;
  /*! \brief Removes \p path when empty and leaves a non-empty path unchanged.
   *  \throws std::runtime_error when removal of an empty directory fails.
   */
  virtual void removeEmptyDirectory(const QString &path) = 0;
  /*! \brief Recursively removes the transaction-owned tree at \p path.
   *  \throws std::runtime_error when cleanup fails.
   */
  virtual void removeTree(const QString &path) = 0;
};

class QtArchiveFileOperations final : public ArchiveFileOperations {
public:
  [[nodiscard]] QVector<ArchiveFileEntry>
  listRecursively(const QString &root) const override;
  [[nodiscard]] bool exists(const QString &path) const override;
  void createDirectory(const QString &path) override;
  [[nodiscard]] QString identity(const QString &path) const override;
  void writeNewDurably(const QString &path,
                       const QByteArray &contents) override;
  void appendDurably(const QString &path, const QByteArray &contents) override;
  [[nodiscard]] QByteArray readAll(const QString &path) const override;
  void validateWorkspacePath(const QString &modPath,
                             const QString &workspacePath) const override;
  void validateReplayPath(const QString &path, const QString &modPath,
                          const QString &workspacePath) const override;
  void preflightTransaction(const QString &modPath) const override;
  void flushFileDurably(const QString &path) override;
  void createDirectories(const QString &path) override;
  void move(const QString &source, const QString &destination) override;
  void removeFile(const QString &path) override;
  void removeEmptyDirectory(const QString &path) override;
  void removeTree(const QString &path) override;
};
