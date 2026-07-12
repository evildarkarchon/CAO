/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

#include <QByteArray>
#include <QMap>
#include <QString>
#include <QStringList>
#include <QVector>

#include <memory>

class ArchiveFileOperations;

enum class ArchiveTransactionKind { Extraction, Packing };

enum class ArchiveTransactionRecordKind {
  Intent,
  MutationComplete,
  Commit,
  CleanupComplete
};

struct ArchiveTransactionManifest {
  static constexpr int CurrentSchemaVersion = 1;

  int schemaVersion = CurrentSchemaVersion;
  QString transactionId;
  ArchiveTransactionKind kind = ArchiveTransactionKind::Extraction;
  QString canonicalModPath;
  QString modIdentity;
  QString canonicalAnchorPath;
  QString anchorIdentity;
  QString volumeIdentity;
  // create() records this after the workspace exists; callers leave it empty.
  QString workspaceIdentity;
  QMap<QString, QString> policyFacts;
};

struct ArchiveTransactionJournalRecord {
  quint64 sequence = 0;
  ArchiveTransactionRecordKind kind = ArchiveTransactionRecordKind::Intent;
  QMap<QString, QString> fields;
};

/*!
 * \brief Durable storage operations required by an Archive Transaction
 * workspace.
 *
 * Each write operation returns only after the new bytes and relevant metadata
 * have been flushed. Implementations must never turn an append into a rewrite.
 */
class ArchiveTransactionDurability {
public:
  virtual ~ArchiveTransactionDurability() = default;

  /*! \brief Returns whether a filesystem entry exists at \p path. */
  [[nodiscard]] virtual bool exists(const QString &path) const = 0;
  /*! \brief Creates \p path; bootstrap ownership covers a partial creation. */
  virtual void createDirectory(const QString &path) = 0;
  /*! \brief Returns the stable filesystem identity of the entry at \p path. */
  [[nodiscard]] virtual QString identity(const QString &path) const = 0;
  /*! \brief Creates a new file and durably writes all \p contents. */
  virtual void writeNewDurably(const QString &path,
                               const QByteArray &contents) = 0;
  /*! \brief Appends all \p contents and durably flushes them before returning.
   */
  virtual void appendDurably(const QString &path,
                             const QByteArray &contents) = 0;
  /*! \brief Reads the complete file at \p path. */
  [[nodiscard]] virtual QByteArray readAll(const QString &path) const = 0;
  /*! \brief Proves the reserved root and workspace are owned, confined paths.
   *
   * Production implementations reject reparse points and volume changes. The
   * default preserves deterministic in-memory adapters whose paths have no
   * operating-system identity.
   */
  virtual void validateWorkspacePath(const QString &modPath,
                                     const QString &workspacePath) const;
  /*! \brief Proves a journal path resolves inside the owning Mod or workspace.
   *
   * Missing destinations are checked through their nearest existing ancestor
   * so a junction cannot redirect a later directory creation outside either
   * owned tree.
   */
  virtual void validateReplayPath(const QString &path, const QString &modPath,
                                  const QString &workspacePath) const;
  /*! \brief Rejects filesystems lacking required local durable semantics. */
  virtual void preflightTransaction(const QString &modPath) const;
  /*! \brief Removes an owned workspace tree if it still exists. */
  virtual void removeTree(const QString &path) = 0;
  /*! \brief Removes \p path if it exists and is empty. */
  virtual void removeEmptyDirectory(const QString &path) = 0;
};

/*!
 * \brief Owns the durable manifest and write-ahead journal for one Archive
 * Transaction.
 *
 * The object validates the complete durable history when reopened. A caller
 * cannot observe or advance a workspace whose ownership or journal history is
 * ambiguous.
 */
class ArchiveTransactionWorkspace {
public:
  static constexpr auto ReservedRootName = ".cao-transactions";

  /*! \brief Creates and durably initializes an owned in-Mod workspace.
   *  \param manifest Complete transaction ownership and recovery metadata.
   *  \param durability Durable filesystem implementation retained by the
   *  returned workspace.
   *  \throws std::runtime_error if metadata is incomplete or storage fails.
   */
  [[nodiscard]] static ArchiveTransactionWorkspace
  create(const ArchiveTransactionManifest &manifest,
         std::shared_ptr<ArchiveTransactionDurability> durability);

  /*! \brief Reopens and validates an existing workspace.
   *  \throws std::runtime_error for an unknown manifest, corrupt completed
   *  frame, invalid transition, or non-final torn data.
   */
  [[nodiscard]] static ArchiveTransactionWorkspace
  reopen(const QString &workspacePath,
         std::shared_ptr<ArchiveTransactionDurability> durability);

  /*! \brief Appends one durable journal record and returns its sequence. */
  quint64 append(ArchiveTransactionRecordKind kind,
                 const QMap<QString, QString> &fields = {});
  /*! \brief Durably marks published output as authoritative. */
  void commit(const QMap<QString, QString> &fields = {});
  /*! \brief Restores an incomplete transaction or finishes committed cleanup.
   *  \param files Filesystem adapter used by both normal publication and
   *  restart recovery.
   *  \return Diagnostics for replay actions that could not be completed.
   *
   *  Replay is idempotent: move intent state is inferred from the presence of
   *  its source and destination, including a crash between mutation and its
   *  completion record.
   */
  [[nodiscard]] QStringList replay(ArchiveFileOperations &files);
  /*! \brief Idempotently removes this workspace and then its empty root. */
  void cleanup();

  /*! \brief Returns the manifest owned for this workspace's lifetime. */
  [[nodiscard]] const ArchiveTransactionManifest &manifest() const noexcept;
  /*! \brief Returns parsed records owned until the next append or replay. */
  [[nodiscard]] const QVector<ArchiveTransactionJournalRecord> &
  records() const noexcept;
  /*! \brief Returns whether a durable commit record was parsed or appended. */
  [[nodiscard]] bool isCommitted() const noexcept;
  /*! \brief Returns the owned workspace path for this object's lifetime. */
  [[nodiscard]] const QString &path() const noexcept;

private:
  ArchiveTransactionWorkspace(
      QString workspacePath, ArchiveTransactionManifest manifest,
      QVector<ArchiveTransactionJournalRecord> records,
      std::shared_ptr<ArchiveTransactionDurability> durability);

  QString m_path;
  ArchiveTransactionManifest m_manifest;
  QVector<ArchiveTransactionJournalRecord> m_records;
  std::shared_ptr<ArchiveTransactionDurability> m_durability;
  bool m_committed = false;
};

/*!
 * \brief Production durability adapter backed by the local filesystem.
 *
 * File writes are flushed through the operating-system handle. Directory
 * creation is kept behind this adapter so stronger native directory flushing
 * can be added without changing Archive Transaction callers.
 */
class FileArchiveTransactionDurability final
    : public ArchiveTransactionDurability {
public:
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
  void removeTree(const QString &path) override;
  void removeEmptyDirectory(const QString &path) override;
};
