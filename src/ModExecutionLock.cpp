/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#include "ModExecutionLock.h"

#include <QCryptographicHash>
#include <QDateTime>
#include <QDir>
#include <QFileInfo>
#include <QJsonDocument>
#include <QJsonObject>

#include <Windows.h>

#include <algorithm>
#include <limits>
#include <stdexcept>
#include <utility>

namespace {
constexpr int LockRecordSchemaVersion = 1;

[[noreturn]] void fail(const QString &message) {
  throw std::runtime_error(message.toStdString());
}

QString windowsErrorMessage(const QString &operation, const QString &path,
                            const DWORD error) {
  return QString("%1 failed for %2 (Windows error %3)")
      .arg(operation, path)
      .arg(error);
}

QString pathFromHandle(const HANDLE handle, const QString &inputPath) {
  const DWORD required = GetFinalPathNameByHandleW(
      handle, nullptr, 0, FILE_NAME_NORMALIZED | VOLUME_NAME_DOS);
  if (required == 0)
    fail(windowsErrorMessage("Resolving canonical Mod path", inputPath,
                             GetLastError()));

  std::wstring buffer(required, L'\0');
  const DWORD written = GetFinalPathNameByHandleW(
      handle, buffer.data(), required, FILE_NAME_NORMALIZED | VOLUME_NAME_DOS);
  if (written == 0 || written >= required)
    fail(windowsErrorMessage("Resolving canonical Mod path", inputPath,
                             GetLastError()));
  buffer.resize(written);

  QString result = QString::fromStdWString(buffer);
  if (result.startsWith(QStringLiteral("\\\\?\\UNC\\"), Qt::CaseInsensitive)) {
    result = QStringLiteral("\\\\") + result.mid(8);
  } else if (result.startsWith(QStringLiteral("\\\\?\\"))) {
    result = result.mid(4);
  }
  return QDir::cleanPath(result);
}

struct IdentifiedMod {
  QString canonicalPath;
  quint32 volumeSerialNumber = 0;
  quint64 fileIndex = 0;
};

IdentifiedMod identifyMod(const QString &modPath) {
  const QString absolutePath = QFileInfo(modPath).absoluteFilePath();
  const HANDLE handle = CreateFileW(
      reinterpret_cast<LPCWSTR>(absolutePath.utf16()), FILE_READ_ATTRIBUTES,
      FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, nullptr,
      OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS, nullptr);
  if (handle == INVALID_HANDLE_VALUE)
    fail(windowsErrorMessage("Opening Mod directory", absolutePath,
                             GetLastError()));

  BY_HANDLE_FILE_INFORMATION information{};
  if (!GetFileInformationByHandle(handle, &information)) {
    const DWORD error = GetLastError();
    CloseHandle(handle);
    fail(windowsErrorMessage("Identifying Mod directory", absolutePath, error));
  }

  QString canonicalPath;
  try {
    canonicalPath = pathFromHandle(handle, absolutePath);
  } catch (...) {
    CloseHandle(handle);
    throw;
  }
  CloseHandle(handle);

  const quint64 fileIndex =
      (static_cast<quint64>(information.nFileIndexHigh) << 32U) |
      static_cast<quint64>(information.nFileIndexLow);
  return {canonicalPath, information.dwVolumeSerialNumber, fileIndex};
}

QString lockPathFor(const QString &canonicalModPath) {
  const QByteArray key =
      QDir::toNativeSeparators(canonicalModPath).toCaseFolded().toUtf8();
  const QString digest = QString::fromLatin1(
      QCryptographicHash::hash(key, QCryptographicHash::Sha256).toHex());
  const QFileInfo modInfo(canonicalModPath);
  return modInfo.dir().filePath(
      QStringLiteral(".cao-mod-lock-%1.lock").arg(digest.left(32)));
}

QJsonObject identityJson(const ModExecutionLockRecord &record) {
  QJsonObject identity;
  identity.insert(QStringLiteral("canonicalModPath"), record.canonicalModPath);
  identity.insert(QStringLiteral("volumeSerialNumber"),
                  QString::number(record.volumeSerialNumber));
  identity.insert(QStringLiteral("fileIndex"),
                  QString::number(record.fileIndex));
  return identity;
}

QByteArray serialize(const ModExecutionLockRecord &record) {
  QJsonObject root;
  root.insert(QStringLiteral("schemaVersion"), record.schemaVersion);
  root.insert(QStringLiteral("modIdentity"), identityJson(record));
  root.insert(QStringLiteral("processId"), QString::number(record.processId));
  root.insert(QStringLiteral("acquiredAtUtc"), record.acquiredAtUtc);
  if (record.bootstrap) {
    QJsonObject bootstrap;
    bootstrap.insert(QStringLiteral("transactionId"),
                     record.bootstrap->transactionId);
    bootstrap.insert(QStringLiteral("workspacePath"),
                     record.bootstrap->workspacePath);
    root.insert(QStringLiteral("bootstrap"), bootstrap);
  }
  return QJsonDocument(root).toJson(QJsonDocument::Compact) + '\n';
}

std::optional<ModExecutionLockRecord>
parsePrevious(const QByteArray &contents) {
  if (contents.trimmed().isEmpty())
    return std::nullopt;

  // Only newline-terminated records were fully written. Ignoring a torn final
  // append preserves the last durably flushed bootstrap intent after a crash.
  const QList<QByteArray> lines = contents.split('\n');
  std::optional<ModExecutionLockRecord> latest;
  for (int lineIndex = 0; lineIndex + 1 < lines.size(); ++lineIndex) {
    if (lines[lineIndex].trimmed().isEmpty())
      continue;
    QJsonParseError parseError;
    const QJsonDocument document =
        QJsonDocument::fromJson(lines[lineIndex], &parseError);
    if (parseError.error != QJsonParseError::NoError || !document.isObject())
      fail(QString("The stale Mod lock record is invalid: %1")
               .arg(parseError.errorString()));

    const QJsonObject root = document.object();
    if (root.value(QStringLiteral("schemaVersion")).toInt(-1) !=
        LockRecordSchemaVersion)
      fail("The stale Mod lock record has an unsupported schema version");

    const QJsonObject identity =
        root.value(QStringLiteral("modIdentity")).toObject();
    ModExecutionLockRecord record;
    record.schemaVersion = LockRecordSchemaVersion;
    record.canonicalModPath =
        identity.value(QStringLiteral("canonicalModPath")).toString();
    bool volumeOk = false;
    record.volumeSerialNumber =
        identity.value(QStringLiteral("volumeSerialNumber"))
            .toString()
            .toUInt(&volumeOk);
    bool fileIndexOk = false;
    record.fileIndex = identity.value(QStringLiteral("fileIndex"))
                           .toString()
                           .toULongLong(&fileIndexOk);
    bool processOk = false;
    record.processId =
        root.value(QStringLiteral("processId")).toString().toUInt(&processOk);
    record.acquiredAtUtc =
        root.value(QStringLiteral("acquiredAtUtc")).toString();
    if (record.canonicalModPath.isEmpty() || !volumeOk || !fileIndexOk ||
        !processOk || record.acquiredAtUtc.isEmpty())
      fail("The stale Mod lock record is missing required diagnostic fields");

    if (root.contains(QStringLiteral("bootstrap"))) {
      const QJsonObject bootstrap =
          root.value(QStringLiteral("bootstrap")).toObject();
      ModExecutionLockBootstrapRecord parsed{
          bootstrap.value(QStringLiteral("transactionId")).toString(),
          bootstrap.value(QStringLiteral("workspacePath")).toString()};
      if (parsed.transactionId.isEmpty() || parsed.workspacePath.isEmpty())
        fail("The stale Mod lock bootstrap record is incomplete");
      record.bootstrap = std::move(parsed);
    }
    latest = std::move(record);
  }
  return latest;
}

class WindowsHeldModExecutionLockFile final : public HeldModExecutionLockFile {
public:
  WindowsHeldModExecutionLockFile(HANDLE handle, QString path)
      : handle_(handle), path_(std::move(path)) {}

  ~WindowsHeldModExecutionLockFile() override {
    if (handle_ != INVALID_HANDLE_VALUE)
      CloseHandle(handle_);
  }

  void appendAndFlush(const QByteArray &contents) override {
    LARGE_INTEGER end{};
    if (!SetFilePointerEx(handle_, end, nullptr, FILE_END))
      fail(windowsErrorMessage("Seeking to end of Mod lock record", path_,
                               GetLastError()));

    qsizetype offset = 0;
    while (offset < contents.size()) {
      DWORD written = 0;
      const DWORD remaining = static_cast<DWORD>(std::min<qsizetype>(
          contents.size() - offset, std::numeric_limits<DWORD>::max()));
      if (!WriteFile(handle_, contents.constData() + offset, remaining,
                     &written, nullptr) ||
          written == 0)
        fail(windowsErrorMessage("Writing Mod lock record", path_,
                                 GetLastError()));
      offset += static_cast<qsizetype>(written);
    }
    if (!FlushFileBuffers(handle_))
      fail(windowsErrorMessage("Flushing Mod lock record", path_,
                               GetLastError()));
  }

  void truncateAndFlush(const qsizetype size) override {
    LARGE_INTEGER end{};
    end.QuadPart = size;
    if (!SetFilePointerEx(handle_, end, nullptr, FILE_BEGIN) ||
        !SetEndOfFile(handle_))
      fail(windowsErrorMessage("Discarding torn Mod lock record", path_,
                               GetLastError()));
    if (!FlushFileBuffers(handle_))
      fail(windowsErrorMessage("Flushing repaired Mod lock record", path_,
                               GetLastError()));
  }

private:
  HANDLE handle_ = INVALID_HANDLE_VALUE;
  QString path_;
};

class WindowsModExecutionLockBackend final : public ModExecutionLockBackend {
public:
  std::pair<std::unique_ptr<HeldModExecutionLockFile>, QByteArray>
  acquire(const QString &lockFilePath) override {
    // Share mode zero is deliberate: neither PID state nor pathname existence
    // can confer or revoke ownership while this kernel handle remains open.
    const HANDLE handle =
        CreateFileW(reinterpret_cast<LPCWSTR>(lockFilePath.utf16()),
                    GENERIC_READ | GENERIC_WRITE, 0, nullptr, OPEN_ALWAYS,
                    FILE_ATTRIBUTE_HIDDEN, nullptr);
    if (handle == INVALID_HANDLE_VALUE)
      fail(windowsErrorMessage("Acquiring Mod execution lock", lockFilePath,
                               GetLastError()));

    LARGE_INTEGER size{};
    if (!GetFileSizeEx(handle, &size) || size.QuadPart < 0 ||
        size.QuadPart > std::numeric_limits<int>::max()) {
      const DWORD error = GetLastError();
      CloseHandle(handle);
      fail(windowsErrorMessage("Reading Mod lock record size", lockFilePath,
                               error));
    }
    QByteArray previous;
    previous.resize(static_cast<int>(size.QuadPart));
    qsizetype offset = 0;
    while (offset < previous.size()) {
      DWORD read = 0;
      const DWORD remaining = static_cast<DWORD>(previous.size() - offset);
      if (!ReadFile(handle, previous.data() + offset, remaining, &read,
                    nullptr) ||
          read == 0) {
        const DWORD error = GetLastError();
        CloseHandle(handle);
        fail(windowsErrorMessage("Reading Mod lock record", lockFilePath,
                                 error));
      }
      offset += static_cast<qsizetype>(read);
    }
    return {
        std::make_unique<WindowsHeldModExecutionLockFile>(handle, lockFilePath),
        previous};
  }
};

WindowsModExecutionLockBackend &productionBackend() {
  static WindowsModExecutionLockBackend backend;
  return backend;
}

void validatePreviousIdentity(
    const std::optional<ModExecutionLockRecord> &record,
    const IdentifiedMod &identified) {
  if (!record)
    return;
  if (record->canonicalModPath.compare(identified.canonicalPath,
                                       Qt::CaseInsensitive) != 0 ||
      record->volumeSerialNumber != identified.volumeSerialNumber ||
      record->fileIndex != identified.fileIndex)
    fail("The stale Mod lock record belongs to a different filesystem object");
  if (record->bootstrap) {
    const QString &transactionId = record->bootstrap->transactionId;
    if (transactionId.contains('/') || transactionId.contains('\\'))
      fail("The stale Mod lock bootstrap transaction ID is invalid");
    const QString expectedWorkspace =
        QDir(identified.canonicalPath)
            .filePath(
                QStringLiteral(".cao-transactions/%1").arg(transactionId));
    if (QDir::cleanPath(
            QFileInfo(record->bootstrap->workspacePath).absoluteFilePath())
            .compare(QDir::cleanPath(expectedWorkspace), Qt::CaseInsensitive) !=
        0)
      fail("The stale Mod lock bootstrap workspace is outside its owning Mod");
  }
}
} // namespace

ModExecutionLock::ModExecutionLock(
    QString canonicalModPath, QString lockFilePath,
    ModExecutionLockRecord record,
    std::optional<ModExecutionLockRecord> previousRecord,
    std::unique_ptr<HeldModExecutionLockFile> heldFile)
    : canonicalModPath_(std::move(canonicalModPath)),
      lockFilePath_(std::move(lockFilePath)), record_(std::move(record)),
      previousRecord_(std::move(previousRecord)),
      heldFile_(std::move(heldFile)) {}

ModExecutionLock ModExecutionLock::acquire(const QString &modPath) {
  return acquire(modPath, productionBackend());
}

ModExecutionLock ModExecutionLock::acquire(const QString &modPath,
                                           ModExecutionLockBackend &backend) {
  const IdentifiedMod identified = identifyMod(modPath);
  const QString lockPath = lockPathFor(identified.canonicalPath);
  auto [heldFile, previousContents] = backend.acquire(lockPath);
  const auto previous = parsePrevious(previousContents);
  validatePreviousIdentity(previous, identified);
  if (!previousContents.isEmpty() && !previousContents.endsWith('\n')) {
    // A later append must not turn the ignored torn tail into an earlier,
    // blocking corrupt record. Remove only bytes after the final delimiter.
    heldFile->truncateAndFlush(previousContents.lastIndexOf('\n') + 1);
  }

  ModExecutionLockRecord record;
  record.schemaVersion = LockRecordSchemaVersion;
  record.canonicalModPath = identified.canonicalPath;
  record.volumeSerialNumber = identified.volumeSerialNumber;
  record.fileIndex = identified.fileIndex;
  record.processId = GetCurrentProcessId();
  record.acquiredAtUtc =
      QDateTime::currentDateTimeUtc().toString(Qt::ISODateWithMs);

  // Persist owner diagnostics before returning the lock so every successful
  // acquisition leaves a complete record for contention and crash recovery.
  // Carry an unfinished bootstrap forward before recovery examines it. A
  // second crash must not erase the first process's only ownership proof.
  if (previous && previous->bootstrap)
    record.bootstrap = previous->bootstrap;
  heldFile->appendAndFlush(serialize(record));
  return ModExecutionLock(identified.canonicalPath, lockPath, std::move(record),
                          previous, std::move(heldFile));
}

void ModExecutionLock::writeBootstrapRecord(const QString &transactionId,
                                            const QString &workspacePath) {
  if (transactionId.trimmed().isEmpty() || workspacePath.trimmed().isEmpty())
    fail("A Mod lock bootstrap record requires a transaction and workspace");
  if (transactionId.contains('/') || transactionId.contains('\\'))
    fail("A Mod lock transaction ID cannot contain path separators");
  const QString expectedWorkspace =
      QDir(canonicalModPath_)
          .filePath(QStringLiteral(".cao-transactions/%1").arg(transactionId));
  if (QDir::cleanPath(QFileInfo(workspacePath).absoluteFilePath())
          .compare(QDir::cleanPath(expectedWorkspace), Qt::CaseInsensitive) !=
      0)
    fail("A Mod lock bootstrap workspace must be the transaction's owned path");
  ModExecutionLockRecord updated = record_;
  updated.bootstrap =
      ModExecutionLockBootstrapRecord{transactionId, workspacePath};
  heldFile_->appendAndFlush(serialize(updated));
  record_ = std::move(updated);
}

void ModExecutionLock::clearBootstrapRecord() {
  ModExecutionLockRecord updated = record_;
  updated.bootstrap.reset();
  heldFile_->appendAndFlush(serialize(updated));
  record_ = std::move(updated);
}

const QString &ModExecutionLock::canonicalModPath() const noexcept {
  return canonicalModPath_;
}

const QString &ModExecutionLock::lockFilePath() const noexcept {
  return lockFilePath_;
}

const ModExecutionLockRecord &ModExecutionLock::record() const noexcept {
  return record_;
}

const std::optional<ModExecutionLockRecord> &
ModExecutionLock::previousRecord() const noexcept {
  return previousRecord_;
}

ModExecutionLockSet::ModExecutionLockSet(std::vector<ModExecutionLock> locks)
    : locks_(std::move(locks)) {}

ModExecutionLockSet
ModExecutionLockSet::acquire(const QVector<QString> &modPaths) {
  return acquire(modPaths, productionBackend());
}

ModExecutionLockSet
ModExecutionLockSet::acquire(const QVector<QString> &modPaths,
                             ModExecutionLockBackend &backend) {
  struct Candidate {
    QString inputPath;
    QString canonicalPath;
  };
  QVector<Candidate> candidates;
  candidates.reserve(modPaths.size());
  for (const QString &path : modPaths) {
    const IdentifiedMod identified = identifyMod(path);
    candidates.push_back({path, identified.canonicalPath});
  }
  std::sort(candidates.begin(), candidates.end(),
            [](const Candidate &left, const Candidate &right) {
              return left.canonicalPath.compare(right.canonicalPath,
                                                Qt::CaseInsensitive) < 0;
            });
  for (qsizetype index = 1; index < candidates.size(); ++index) {
    if (candidates[index - 1].canonicalPath.compare(
            candidates[index].canonicalPath, Qt::CaseInsensitive) == 0)
      fail(QString("The selected Mod scope contains the same Mod twice: %1")
               .arg(candidates[index].canonicalPath));
  }

  std::vector<ModExecutionLock> locks;
  locks.reserve(candidates.size());
  // The local vector owns each successful handle. Stack unwinding therefore
  // releases the whole prefix if any later acquisition or flush fails.
  for (const Candidate &candidate : candidates)
    locks.push_back(ModExecutionLock::acquire(candidate.inputPath, backend));
  return ModExecutionLockSet(std::move(locks));
}

const std::vector<ModExecutionLock> &
ModExecutionLockSet::locks() const noexcept {
  return locks_;
}

std::vector<ModExecutionLock> &ModExecutionLockSet::locks() noexcept {
  return locks_;
}
