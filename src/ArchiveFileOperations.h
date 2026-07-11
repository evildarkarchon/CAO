/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

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
class ArchiveFileOperations {
public:
  virtual ~ArchiveFileOperations() = default;

  /*! \brief Creates an empty, unique staging directory beside \p anchor.
   *  \param anchor File or directory whose parent determines the staging area.
   *  \param purpose Human-readable suffix used to identify the transaction.
   *  \return Absolute path to the created directory.
   *  \throws std::runtime_error if the directory cannot be created.
   */
  [[nodiscard]] virtual QString
  createSiblingStagingDirectory(const QString &anchor,
                                const QString &purpose) = 0;
  /*! \brief Lists every entry below \p root with paths relative to that root.
   *  \throws std::runtime_error if the tree cannot be enumerated safely.
   */
  [[nodiscard]] virtual QVector<ArchiveFileEntry>
  listRecursively(const QString &root) const = 0;
  /*! \brief Returns whether a filesystem entry exists at \p path. */
  [[nodiscard]] virtual bool exists(const QString &path) const = 0;
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
  /*! \brief Removes the directory at \p path only when it is empty.
   *  \throws std::runtime_error when removal fails.
   */
  virtual void removeEmptyDirectory(const QString &path) = 0;
  /*! \brief Recursively removes the transaction-owned tree at \p path.
   *  \throws std::runtime_error when cleanup fails.
   */
  virtual void removeTree(const QString &path) = 0;
};

class QtArchiveFileOperations final : public ArchiveFileOperations {
public:
  [[nodiscard]] QString
  createSiblingStagingDirectory(const QString &anchor,
                                const QString &purpose) override;
  [[nodiscard]] QVector<ArchiveFileEntry>
  listRecursively(const QString &root) const override;
  [[nodiscard]] bool exists(const QString &path) const override;
  void createDirectories(const QString &path) override;
  void move(const QString &source, const QString &destination) override;
  void removeFile(const QString &path) override;
  void removeEmptyDirectory(const QString &path) override;
  void removeTree(const QString &path) override;
};
