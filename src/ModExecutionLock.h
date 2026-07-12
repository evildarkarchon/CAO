/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

#include <QByteArray>
#include <QString>
#include <QVector>

#include <memory>
#include <optional>
#include <utility>
#include <vector>

struct ModExecutionLockBootstrapRecord {
  QString transactionId;
  QString workspacePath;
};

struct ModExecutionLockRecord {
  int schemaVersion = 1;
  QString canonicalModPath;
  quint32 volumeSerialNumber = 0;
  quint64 fileIndex = 0;
  quint32 processId = 0;
  QString acquiredAtUtc;
  std::optional<ModExecutionLockBootstrapRecord> bootstrap;
};

/*! \brief An exclusively held lock-file handle.
 *
 * The handle lifetime, rather than the existence or contents of its pathname,
 * represents ownership. Implementations append and durably flush each complete
 * diagnostic record before returning from appendAndFlush(). The Windows
 * implementation uses CreateFile share mode zero rather than a PID check or
 * LockFileEx byte range; live contents are consequently unavailable to a
 * contender until the owning handle closes.
 */
class HeldModExecutionLockFile {
public:
  virtual ~HeldModExecutionLockFile() = default;

  /*! \brief Appends and durably flushes one complete lock-file record.
   *  \throws std::runtime_error if seeking, writing, or flushing fails.
   */
  virtual void appendAndFlush(const QByteArray &contents) = 0;

  /*! \brief Removes an unflushed torn tail and durably records the repair.
   *  \param size Byte length ending immediately after the last complete record.
   *  \throws std::runtime_error if truncating or flushing fails.
   */
  virtual void truncateAndFlush(qsizetype size) = 0;
};

/*! \brief Acquisition seam for production kernel locks and deterministic tests.
 */
class ModExecutionLockBackend {
public:
  virtual ~ModExecutionLockBackend() = default;

  /*! \brief Opens \p lockFilePath with exclusive sharing for its lifetime.
   *  \return The held file and any contents that existed before acquisition.
   *  \throws std::runtime_error when another owner holds the lock or opening
   *  the file fails.
   */
  [[nodiscard]] virtual std::pair<std::unique_ptr<HeldModExecutionLockFile>,
                                  QByteArray>
  acquire(const QString &lockFilePath) = 0;
};

/*! \brief Owns the exclusive execution lock and durable bootstrap state for one
 * Mod. */
class ModExecutionLock {
public:
  ModExecutionLock(ModExecutionLock &&) noexcept = default;
  ModExecutionLock &operator=(ModExecutionLock &&) noexcept = default;
  ModExecutionLock(const ModExecutionLock &) = delete;
  ModExecutionLock &operator=(const ModExecutionLock &) = delete;
  ~ModExecutionLock() = default;

  /*! \brief Acquires the production Windows lock for \p modPath.
   *  \throws std::runtime_error when the Mod cannot be identified, the record
   *  is inconsistent, or the lock cannot be exclusively acquired.
   */
  [[nodiscard]] static ModExecutionLock acquire(const QString &modPath);

  /*! \brief Acquires through \p backend for deterministic failure testing. */
  [[nodiscard]] static ModExecutionLock
  acquire(const QString &modPath, ModExecutionLockBackend &backend);

  /*! \brief Durably records workspace creation intent while retaining the lock.
   *  \param transactionId Stable identifier of the transaction being created.
   *  \param workspacePath Intended workspace below the owning Mod.
   *  \throws std::runtime_error for invalid input or a failed durable write.
   */
  void writeBootstrapRecord(const QString &transactionId,
                            const QString &workspacePath);

  /*! \brief Durably removes workspace creation intent after its manifest
   * exists.
   *  \throws std::runtime_error if the updated record cannot be flushed.
   */
  void clearBootstrapRecord();

  /*! \brief Returns the canonical path owned for this lock's lifetime. */
  [[nodiscard]] const QString &canonicalModPath() const noexcept;
  /*! \brief Returns the sibling diagnostic path owned by the held handle. */
  [[nodiscard]] const QString &lockFilePath() const noexcept;
  /*! \brief Returns the current record, owned until this lock is mutated. */
  [[nodiscard]] const ModExecutionLockRecord &record() const noexcept;
  /*! \brief Returns the record observed at acquisition for recovery. */
  [[nodiscard]] const std::optional<ModExecutionLockRecord> &
  previousRecord() const noexcept;

private:
  ModExecutionLock(QString canonicalModPath, QString lockFilePath,
                   ModExecutionLockRecord record,
                   std::optional<ModExecutionLockRecord> previousRecord,
                   std::unique_ptr<HeldModExecutionLockFile> heldFile);

  QString canonicalModPath_;
  QString lockFilePath_;
  ModExecutionLockRecord record_;
  std::optional<ModExecutionLockRecord> previousRecord_;
  std::unique_ptr<HeldModExecutionLockFile> heldFile_;
};

/*! \brief RAII collection that acquires every selected Mod lock or retains
 * none. */
class ModExecutionLockSet {
public:
  ModExecutionLockSet(ModExecutionLockSet &&) noexcept = default;
  ModExecutionLockSet &operator=(ModExecutionLockSet &&) noexcept = default;
  ModExecutionLockSet(const ModExecutionLockSet &) = delete;
  ModExecutionLockSet &operator=(const ModExecutionLockSet &) = delete;
  ~ModExecutionLockSet() = default;

  /*! \brief Acquires production locks in canonical case-insensitive path order.
   *  \throws std::runtime_error after releasing any locks already acquired.
   */
  [[nodiscard]] static ModExecutionLockSet
  acquire(const QVector<QString> &modPaths);

  /*! \brief Acquires a deterministic lock set through \p backend. */
  [[nodiscard]] static ModExecutionLockSet
  acquire(const QVector<QString> &modPaths, ModExecutionLockBackend &backend);

  /*! \brief Returns held locks in canonical acquisition order. */
  [[nodiscard]] const std::vector<ModExecutionLock> &locks() const noexcept;
  /*! \brief Returns mutable held locks for bootstrap record updates. */
  [[nodiscard]] std::vector<ModExecutionLock> &locks() noexcept;

private:
  explicit ModExecutionLockSet(std::vector<ModExecutionLock> locks);
  std::vector<ModExecutionLock> locks_;
};
