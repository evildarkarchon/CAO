/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#include "ArchiveTransactionWorkspace.h"

#include "ArchiveFileOperations.h"

#include <QCryptographicHash>
#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QJsonDocument>
#include <QJsonObject>
#include <QRegularExpression>
#include <QSet>
#include <QStringList>
#include <QUuid>

#include <algorithm>
#include <cwchar>
#include <limits>
#include <stdexcept>
#include <utility>

#ifdef Q_OS_WIN
#include <Windows.h>
#include <io.h>
#else
#include <sys/stat.h>
#include <unistd.h>
#endif

namespace {
constexpr auto ManifestFileName = "manifest.json";
constexpr auto JournalFileName = "journal.log";
constexpr auto JournalPreamble = "CAO-ARCHIVE-JOURNAL schema=1\n";
constexpr qsizetype JournalTerminatorSize = 6;

[[noreturn]] void fail(const QString &message, const QString &path = {}) {
  const QString detail = path.isEmpty() ? message : message + ": " + path;
  throw std::runtime_error(detail.toStdString());
}

bool isWithin(const QString &path, const QString &root) {
  const QString cleanPath = QDir::cleanPath(path);
  const QString cleanRoot = QDir::cleanPath(root);
#ifdef Q_OS_WIN
  constexpr auto sensitivity = Qt::CaseInsensitive;
#else
  constexpr auto sensitivity = Qt::CaseSensitive;
#endif
  return cleanPath.compare(cleanRoot, sensitivity) == 0 ||
         cleanPath.startsWith(cleanRoot + QLatin1Char('/'), sensitivity);
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

struct NativePathInformation {
  QString finalPath;
  DWORD volumeSerial = 0;
  DWORD attributes = 0;
};

QString removeNativePrefix(QString path) {
  if (path.startsWith(QStringLiteral("\\\\?\\UNC\\"), Qt::CaseInsensitive))
    return QStringLiteral("\\\\") + path.mid(8);
  if (path.startsWith(QStringLiteral("\\\\?\\")))
    return path.mid(4);
  return path;
}

NativePathInformation nativePathInformation(const QString &path,
                                            const bool openReparsePoint) {
  DWORD flags = FILE_FLAG_BACKUP_SEMANTICS;
  if (openReparsePoint)
    flags |= FILE_FLAG_OPEN_REPARSE_POINT;
  NativeHandle handle(CreateFileW(
      reinterpret_cast<LPCWSTR>(QDir::toNativeSeparators(path).utf16()),
      FILE_READ_ATTRIBUTES,
      FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, nullptr,
      OPEN_EXISTING, flags, nullptr));
  if (handle.get() == INVALID_HANDLE_VALUE)
    fail("Opening Archive Transaction path for validation failed", path);

  BY_HANDLE_FILE_INFORMATION identity{};
  if (!GetFileInformationByHandle(handle.get(), &identity))
    fail("Reading Archive Transaction path identity failed", path);
  FILE_ATTRIBUTE_TAG_INFO tag{};
  if (!GetFileInformationByHandleEx(handle.get(), FileAttributeTagInfo, &tag,
                                    sizeof(tag)))
    fail("Reading Archive Transaction path attributes failed", path);

  const DWORD required = GetFinalPathNameByHandleW(
      handle.get(), nullptr, 0, FILE_NAME_NORMALIZED | VOLUME_NAME_DOS);
  if (required == 0)
    fail("Resolving Archive Transaction path failed", path);
  std::wstring buffer(required, L'\0');
  const DWORD written =
      GetFinalPathNameByHandleW(handle.get(), buffer.data(), required,
                                FILE_NAME_NORMALIZED | VOLUME_NAME_DOS);
  if (written == 0 || written >= required)
    fail("Resolving Archive Transaction path failed", path);
  buffer.resize(written);
  return {QDir::cleanPath(removeNativePrefix(QString::fromStdWString(buffer))),
          identity.dwVolumeSerialNumber, tag.FileAttributes};
}

void validateAbsoluteFilesystemPath(const QString &path) {
  const QString native = QDir::toNativeSeparators(path);
  if (!QDir::isAbsolutePath(path) ||
      native.startsWith(QStringLiteral("\\\\")) ||
      native.startsWith(QStringLiteral("\\\\?\\")) ||
      native.startsWith(QStringLiteral("\\\\.\\")))
    fail("Archive Transaction path is not a local absolute path", path);
  // A colon after the drive designator names an alternate data stream. Such a
  // stream does not have an independent filesystem identity for recovery.
  if (native.indexOf(QLatin1Char(':'), 2) >= 0)
    fail("Archive Transaction path contains an alternate data stream", path);
}

QString nearestExistingAncestor(QString path) {
  path = QDir::cleanPath(path);
  while (!QFileInfo::exists(path)) {
    const QString parent = QFileInfo(path).absolutePath();
    if (parent == path)
      fail("Archive Transaction path has no existing ancestor", path);
    path = parent;
  }
  return path;
}
#endif

QString kindName(const ArchiveTransactionKind kind) {
  switch (kind) {
  case ArchiveTransactionKind::Extraction:
    return QStringLiteral("extraction");
  case ArchiveTransactionKind::Packing:
    return QStringLiteral("packing");
  }
  fail("Unknown Archive Transaction kind");
}

ArchiveTransactionKind parseKind(const QString &value) {
  if (value == QStringLiteral("extraction"))
    return ArchiveTransactionKind::Extraction;
  if (value == QStringLiteral("packing"))
    return ArchiveTransactionKind::Packing;
  fail("Unknown Archive Transaction kind");
}

QString recordKindName(const ArchiveTransactionRecordKind kind) {
  switch (kind) {
  case ArchiveTransactionRecordKind::Intent:
    return QStringLiteral("intent");
  case ArchiveTransactionRecordKind::MutationComplete:
    return QStringLiteral("mutation-complete");
  case ArchiveTransactionRecordKind::Commit:
    return QStringLiteral("commit");
  case ArchiveTransactionRecordKind::CleanupComplete:
    return QStringLiteral("cleanup-complete");
  }
  fail("Unknown Archive Transaction record kind");
}

ArchiveTransactionRecordKind parseRecordKind(const QString &value) {
  if (value == QStringLiteral("intent"))
    return ArchiveTransactionRecordKind::Intent;
  if (value == QStringLiteral("mutation-complete"))
    return ArchiveTransactionRecordKind::MutationComplete;
  if (value == QStringLiteral("commit"))
    return ArchiveTransactionRecordKind::Commit;
  if (value == QStringLiteral("cleanup-complete"))
    return ArchiveTransactionRecordKind::CleanupComplete;
  fail("Unknown Archive Transaction journal record");
}

QJsonObject stringMapToJson(const QMap<QString, QString> &values) {
  QJsonObject object;
  for (auto it = values.cbegin(); it != values.cend(); ++it)
    object.insert(it.key(), it.value());
  return object;
}

QMap<QString, QString> parseStringMap(const QJsonValue &value,
                                      const QString &description) {
  if (!value.isObject())
    fail(description + " must be an object");
  QMap<QString, QString> result;
  const QJsonObject object = value.toObject();
  for (auto it = object.constBegin(); it != object.constEnd(); ++it) {
    if (!it.value().isString())
      fail(description + " values must be strings");
    result.insert(it.key(), it.value().toString());
  }
  return result;
}

QString requiredString(const QJsonObject &object, const QString &name) {
  const QJsonValue value = object.value(name);
  if (!value.isString() || value.toString().isEmpty())
    fail("Archive Transaction manifest is missing " + name);
  return value.toString();
}

void validateManifest(const ArchiveTransactionManifest &manifest,
                      const bool requireWorkspaceIdentity = true) {
  if (manifest.schemaVersion !=
      ArchiveTransactionManifest::CurrentSchemaVersion)
    fail("Unsupported Archive Transaction manifest schema");

  const QUuid id(manifest.transactionId);
  if (id.isNull() ||
      id.toString(QUuid::WithoutBraces)
              .compare(manifest.transactionId, Qt::CaseInsensitive) != 0)
    fail("Archive Transaction ID must be a UUID without braces");

  QStringList required = {manifest.canonicalModPath, manifest.modIdentity,
                          manifest.canonicalAnchorPath, manifest.anchorIdentity,
                          manifest.volumeIdentity};
  if (requireWorkspaceIdentity)
    required.push_back(manifest.workspaceIdentity);
  if (std::any_of(required.cbegin(), required.cend(),
                  [](const QString &value) { return value.isEmpty(); }))
    fail("Archive Transaction manifest ownership is incomplete");
}

QByteArray serializeManifest(const ArchiveTransactionManifest &manifest) {
  QJsonObject object;
  object.insert("schemaVersion", manifest.schemaVersion);
  object.insert("transactionId", manifest.transactionId);
  object.insert("kind", kindName(manifest.kind));
  object.insert("canonicalModPath", manifest.canonicalModPath);
  object.insert("modIdentity", manifest.modIdentity);
  object.insert("canonicalAnchorPath", manifest.canonicalAnchorPath);
  object.insert("anchorIdentity", manifest.anchorIdentity);
  object.insert("volumeIdentity", manifest.volumeIdentity);
  object.insert("workspaceIdentity", manifest.workspaceIdentity);
  object.insert("policyFacts", stringMapToJson(manifest.policyFacts));
  return QJsonDocument(object).toJson(QJsonDocument::Indented);
}

ArchiveTransactionManifest parseManifest(const QByteArray &contents) {
  QJsonParseError error;
  const QJsonDocument document = QJsonDocument::fromJson(contents, &error);
  if (error.error != QJsonParseError::NoError || !document.isObject())
    fail("Archive Transaction manifest is not valid JSON");
  const QJsonObject object = document.object();
  const QJsonValue schema = object.value("schemaVersion");
  if (!schema.isDouble() || schema.toInt() != schema.toDouble())
    fail("Archive Transaction manifest has no integral schema version");

  ArchiveTransactionManifest manifest;
  manifest.schemaVersion = schema.toInt();
  manifest.transactionId = requiredString(object, "transactionId");
  manifest.kind = parseKind(requiredString(object, "kind"));
  manifest.canonicalModPath = requiredString(object, "canonicalModPath");
  manifest.modIdentity = requiredString(object, "modIdentity");
  manifest.canonicalAnchorPath = requiredString(object, "canonicalAnchorPath");
  manifest.anchorIdentity = requiredString(object, "anchorIdentity");
  manifest.volumeIdentity = requiredString(object, "volumeIdentity");
  manifest.workspaceIdentity = requiredString(object, "workspaceIdentity");
  manifest.policyFacts =
      parseStringMap(object.value("policyFacts"), "policyFacts");
  validateManifest(manifest);
  return manifest;
}

QByteArray serializePayload(const ArchiveTransactionRecordKind kind,
                            const QMap<QString, QString> &fields) {
  QJsonObject object;
  object.insert("fields", stringMapToJson(fields));
  object.insert("type", recordKindName(kind));
  return QJsonDocument(object).toJson(QJsonDocument::Compact);
}

QByteArray frame(const quint64 sequence, const QByteArray &payload) {
  const QByteArray checksum =
      QCryptographicHash::hash(payload, QCryptographicHash::Sha256).toHex();
  return QByteArray("@frame sequence=") + QByteArray::number(sequence) +
         " length=" + QByteArray::number(payload.size()) +
         " checksum=" + checksum + "\n" + payload + "\n@end\n";
}

void validateTransition(const QVector<ArchiveTransactionJournalRecord> &records,
                        const ArchiveTransactionRecordKind next) {
  const bool committed =
      std::any_of(records.cbegin(), records.cend(), [](const auto &record) {
        return record.kind == ArchiveTransactionRecordKind::Commit;
      });
  if (next == ArchiveTransactionRecordKind::Commit && committed)
    fail("Archive Transaction journal contains a duplicate commit");
  if (next == ArchiveTransactionRecordKind::Commit) {
    const auto count = [&](const ArchiveTransactionRecordKind kind) {
      return std::count_if(
          records.cbegin(), records.cend(),
          [&](const auto &record) { return record.kind == kind; });
    };
    if (count(ArchiveTransactionRecordKind::Intent) == 0 ||
        count(ArchiveTransactionRecordKind::Intent) !=
            count(ArchiveTransactionRecordKind::MutationComplete))
      fail("Archive Transaction commit appears before all intents complete");
  }
  if (committed && next != ArchiveTransactionRecordKind::CleanupComplete)
    fail("Archive Transaction journal mutates after commit");
  if (!committed && next == ArchiveTransactionRecordKind::CleanupComplete)
    fail("Archive Transaction cleanup appears before commit");
  if (next == ArchiveTransactionRecordKind::MutationComplete) {
    const auto intentCount =
        std::count_if(records.cbegin(), records.cend(), [](const auto &record) {
          return record.kind == ArchiveTransactionRecordKind::Intent;
        });
    const auto completedCount =
        std::count_if(records.cbegin(), records.cend(), [](const auto &record) {
          return record.kind == ArchiveTransactionRecordKind::MutationComplete;
        });
    if (completedCount >= intentCount)
      fail("Archive Transaction mutation has no unmatched prior intent");
  }
}

QVector<ArchiveTransactionJournalRecord>
parseJournal(const QByteArray &contents) {
  if (!contents.startsWith(JournalPreamble))
    fail("Unknown Archive Transaction journal schema");

  static const QRegularExpression headerExpression(
      QStringLiteral("^@frame sequence=([0-9]+) length=([0-9]+) "
                     "checksum=([0-9a-f]{64})$"));
  QVector<ArchiveTransactionJournalRecord> records;
  qsizetype cursor = QByteArray(JournalPreamble).size();
  while (cursor < contents.size()) {
    const qsizetype headerEnd = contents.indexOf('\n', cursor);
    if (headerEnd < 0)
      break; // A partial final header has no durable semantic meaning.

    const QString header =
        QString::fromLatin1(contents.mid(cursor, headerEnd - cursor));
    const QRegularExpressionMatch match = headerExpression.match(header);
    if (!match.hasMatch())
      fail("Malformed Archive Transaction journal frame header");

    bool sequenceOk = false;
    bool lengthOk = false;
    const quint64 sequence = match.captured(1).toULongLong(&sequenceOk);
    const qulonglong unsignedLength = match.captured(2).toULongLong(&lengthOk);
    if (!sequenceOk || !lengthOk ||
        unsignedLength >
            static_cast<qulonglong>((std::numeric_limits<int>::max)()))
      fail("Archive Transaction journal frame bounds are invalid");
    const qsizetype length = static_cast<qsizetype>(unsignedLength);
    const qsizetype payloadStart = headerEnd + 1;
    const qsizetype remaining = contents.size() - payloadStart;
    if (length > remaining || remaining - length < JournalTerminatorSize) {
      // A footer proves the frame append completed and its declared length was
      // corrupted; only an append without a footer is a torn final frame.
      if (contents.indexOf("\n@end\n", payloadStart) >= 0)
        fail("Archive Transaction journal has a corrupt completed frame");
      break;
    }
    const qsizetype frameEnd = payloadStart + length + JournalTerminatorSize;
    const QByteArray payload = contents.mid(payloadStart, length);
    if (contents.mid(payloadStart + length, JournalTerminatorSize) !=
        QByteArray("\n@end\n"))
      fail("Archive Transaction journal frame terminator is invalid");
    const QByteArray actualChecksum =
        QCryptographicHash::hash(payload, QCryptographicHash::Sha256).toHex();
    if (actualChecksum != match.captured(3).toLatin1())
      fail("Archive Transaction journal frame checksum does not match");
    if (sequence != static_cast<quint64>(records.size() + 1))
      fail("Archive Transaction journal sequence is not contiguous");

    QJsonParseError error;
    const QJsonDocument document = QJsonDocument::fromJson(payload, &error);
    if (error.error != QJsonParseError::NoError || !document.isObject())
      fail("Archive Transaction journal payload is not valid JSON");
    const QJsonObject object = document.object();
    const QJsonValue type = object.value("type");
    if (!type.isString())
      fail("Archive Transaction journal payload has no record type");
    const ArchiveTransactionRecordKind kind = parseRecordKind(type.toString());
    validateTransition(records, kind);
    records.push_back(
        {sequence, kind, parseStringMap(object.value("fields"), "fields")});
    cursor = frameEnd;
  }
  return records;
}

QString manifestPath(const QString &workspacePath) {
  return QDir(workspacePath).filePath(ManifestFileName);
}

QString journalPath(const QString &workspacePath) {
  return QDir(workspacePath).filePath(JournalFileName);
}

void flushFile(QFile &file, const QString &path) {
  if (!file.flush())
    fail("Flushing Archive Transaction file failed", path);
#ifdef Q_OS_WIN
  const intptr_t nativeHandle = _get_osfhandle(file.handle());
  if (nativeHandle == -1 ||
      !FlushFileBuffers(reinterpret_cast<HANDLE>(nativeHandle)))
    fail("Durably flushing Archive Transaction file failed", path);
#else
  if (::fsync(file.handle()) != 0)
    fail("Durably flushing Archive Transaction file failed", path);
#endif
}
} // namespace

void ArchiveTransactionDurability::validateWorkspacePath(
    const QString &, const QString &) const {}

void ArchiveTransactionDurability::validateReplayPath(const QString &,
                                                      const QString &,
                                                      const QString &) const {}

void ArchiveTransactionDurability::preflightTransaction(const QString &) const {
}

ArchiveTransactionWorkspace ArchiveTransactionWorkspace::create(
    const ArchiveTransactionManifest &manifest,
    std::shared_ptr<ArchiveTransactionDurability> durability) {
  validateManifest(manifest, false);
  if (!durability)
    fail("Archive Transaction durability adapter is required");

  const QString root =
      QDir(manifest.canonicalModPath).filePath(ReservedRootName);
  const QString workspacePath = QDir(root).filePath(manifest.transactionId);
  if (durability->exists(workspacePath))
    fail("Archive Transaction workspace already exists", workspacePath);

  // Validate before mkdir so a reserved-root junction cannot redirect the
  // first durable manifest outside the Mod.
  durability->validateWorkspacePath(manifest.canonicalModPath, workspacePath);
  durability->createDirectory(root);
  durability->createDirectory(workspacePath);
  durability->validateWorkspacePath(manifest.canonicalModPath, workspacePath);
  ArchiveTransactionManifest ownedManifest = manifest;
  ownedManifest.workspaceIdentity = durability->identity(workspacePath);
  validateManifest(ownedManifest);
  durability->writeNewDurably(manifestPath(workspacePath),
                              serializeManifest(ownedManifest));
  durability->writeNewDurably(journalPath(workspacePath), JournalPreamble);
  return ArchiveTransactionWorkspace(workspacePath, ownedManifest, {},
                                     std::move(durability));
}

ArchiveTransactionWorkspace ArchiveTransactionWorkspace::reopen(
    const QString &workspacePath,
    std::shared_ptr<ArchiveTransactionDurability> durability) {
  if (!durability)
    fail("Archive Transaction durability adapter is required");
  const QString inferredModPath =
      QFileInfo(QFileInfo(workspacePath).absolutePath()).absolutePath();
  // Ownership metadata itself is untrusted until the physical root and
  // workspace have been proven not to be reparse points.
  durability->validateWorkspacePath(inferredModPath, workspacePath);
  const ArchiveTransactionManifest manifest =
      parseManifest(durability->readAll(manifestPath(workspacePath)));
  const QString expectedPath =
      QDir(QDir(manifest.canonicalModPath).filePath(ReservedRootName))
          .filePath(manifest.transactionId);
  if (QDir::cleanPath(expectedPath)
          .compare(QDir::cleanPath(workspacePath), Qt::CaseInsensitive) != 0)
    fail("Archive Transaction manifest does not own its workspace",
         workspacePath);
  if (durability->identity(workspacePath) != manifest.workspaceIdentity)
    fail("Archive Transaction workspace identity has changed", workspacePath);
  durability->validateWorkspacePath(manifest.canonicalModPath, workspacePath);
  QVector<ArchiveTransactionJournalRecord> records =
      parseJournal(durability->readAll(journalPath(workspacePath)));
  return ArchiveTransactionWorkspace(workspacePath, manifest,
                                     std::move(records), std::move(durability));
}

ArchiveTransactionWorkspace::ArchiveTransactionWorkspace(
    QString workspacePath, ArchiveTransactionManifest manifest,
    QVector<ArchiveTransactionJournalRecord> records,
    std::shared_ptr<ArchiveTransactionDurability> durability)
    : m_path(std::move(workspacePath)), m_manifest(std::move(manifest)),
      m_records(std::move(records)), m_durability(std::move(durability)),
      m_committed(std::any_of(
          m_records.cbegin(), m_records.cend(), [](const auto &record) {
            return record.kind == ArchiveTransactionRecordKind::Commit;
          })) {}

quint64
ArchiveTransactionWorkspace::append(const ArchiveTransactionRecordKind kind,
                                    const QMap<QString, QString> &fields) {
  validateTransition(m_records, kind);
  QMap<QString, QString> durableFields = fields;
  if (kind == ArchiveTransactionRecordKind::Intent &&
      durableFields.value(QStringLiteral("operation")) ==
          QStringLiteral("move")) {
    const QString source = durableFields.value(QStringLiteral("source"));
    if (!source.isEmpty() && m_durability->exists(source)) {
      // Recovery must distinguish an interrupted move from an attacker or
      // external tool replacing the pathname after the intent became durable.
      durableFields.insert(QStringLiteral("sourceIdentity"),
                           m_durability->identity(source));
    }
  }
  const quint64 sequence = static_cast<quint64>(m_records.size() + 1);
  m_durability->appendDurably(
      journalPath(m_path),
      frame(sequence, serializePayload(kind, durableFields)));
  m_records.push_back({sequence, kind, durableFields});
  if (kind == ArchiveTransactionRecordKind::Commit)
    m_committed = true;
  return sequence;
}

void ArchiveTransactionWorkspace::commit(const QMap<QString, QString> &fields) {
  append(ArchiveTransactionRecordKind::Commit, fields);
}

QStringList ArchiveTransactionWorkspace::replay(ArchiveFileOperations &files) {
  QStringList failures;
  const auto recordFailure = [&](const std::exception &error) {
    failures << QString::fromUtf8(error.what());
  };

  try {
    m_durability->validateWorkspacePath(m_manifest.canonicalModPath, m_path);
    for (const auto &record : std::as_const(m_records)) {
      if (!m_committed && record.kind == ArchiveTransactionRecordKind::Intent) {
        const QString operation =
            record.fields.value(QStringLiteral("operation"));
        if (operation == QStringLiteral("move")) {
          const QString source = record.fields.value(QStringLiteral("source"));
          const QString destination =
              record.fields.value(QStringLiteral("destination"));
          m_durability->validateReplayPath(source, m_manifest.canonicalModPath,
                                           m_path);
          m_durability->validateReplayPath(destination,
                                           m_manifest.canonicalModPath, m_path);
          const QString expectedIdentity =
              record.fields.value(QStringLiteral("sourceIdentity"));
          if (!expectedIdentity.isEmpty()) {
            const bool sourceMatches =
                m_durability->exists(source) &&
                m_durability->identity(source) == expectedIdentity;
            const bool destinationMatches =
                m_durability->exists(destination) &&
                m_durability->identity(destination) == expectedIdentity;
            // A later journaled move may currently occupy this intent's
            // source path. Reverse replay removes that later publication
            // before restoring the identity retained at this destination.
            if (!sourceMatches && !destinationMatches) {
              const QString currentPath =
                  m_durability->exists(source) ? source : destination;
              fail("Archive Transaction move entry identity has changed",
                   currentPath);
            }
          }
        } else if (operation == QStringLiteral("create-directory")) {
          m_durability->validateReplayPath(
              record.fields.value(QStringLiteral("path")),
              m_manifest.canonicalModPath, m_path);
        } else {
          fail("Unknown Archive Transaction replay operation", operation);
        }
      } else if (record.kind == ArchiveTransactionRecordKind::Commit) {
        for (auto field = record.fields.cbegin(); field != record.fields.cend();
             ++field) {
          if (field.key().startsWith(QStringLiteral("cleanup-file.")))
            m_durability->validateReplayPath(
                field.value(), m_manifest.canonicalModPath, m_path);
        }
      }
    }
  } catch (const std::exception &error) {
    // Validation is deliberately completed before replay's first mutation.
    // Returning the diagnostic preserves the existing typed rollback seam.
    recordFailure(error);
    return failures;
  }

  if (m_committed) {
    const auto commitRecord = std::find_if(
        m_records.cbegin(), m_records.cend(), [](const auto &record) {
          return record.kind == ArchiveTransactionRecordKind::Commit;
        });
    const QMap<QString, QString> commitFields = commitRecord->fields;
    QSet<QString> completed;
    for (const auto &record : std::as_const(m_records)) {
      if (record.kind == ArchiveTransactionRecordKind::CleanupComplete)
        completed.insert(record.fields.value(QStringLiteral("path")));
    }
    for (auto it = commitFields.cbegin(); it != commitFields.cend(); ++it) {
      if (!it.key().startsWith(QStringLiteral("cleanup-file.")) ||
          completed.contains(it.value()))
        continue;
      try {
        files.removeFile(it.value());
        append(ArchiveTransactionRecordKind::CleanupComplete,
               {{QStringLiteral("path"), it.value()}});
      } catch (const std::exception &error) {
        recordFailure(error);
      }
    }
    if (failures.isEmpty()) {
      try {
        cleanup();
      } catch (const std::exception &error) {
        recordFailure(error);
      }
    }
    return failures;
  }

  QVector<ArchiveTransactionJournalRecord> intents;
  for (const auto &record : std::as_const(m_records)) {
    if (record.kind == ArchiveTransactionRecordKind::Intent)
      intents.push_back(record);
  }
  for (auto it = intents.crbegin(); it != intents.crend(); ++it) {
    const QString operation = it->fields.value(QStringLiteral("operation"));
    try {
      if (operation == QStringLiteral("move")) {
        const QString source = it->fields.value(QStringLiteral("source"));
        const QString destination =
            it->fields.value(QStringLiteral("destination"));
        const bool sourceExists = files.exists(source);
        const bool destinationExists = files.exists(destination);
        if (!sourceExists && destinationExists)
          files.move(destination, source);
        else if (sourceExists && destinationExists)
          throw std::runtime_error(
              QString("Archive Transaction move state is ambiguous: %1 and %2")
                  .arg(source, destination)
                  .toStdString());
        else if (!sourceExists && !destinationExists)
          throw std::runtime_error(
              QString("Archive Transaction move state is missing both paths: "
                      "%1 and %2")
                  .arg(source, destination)
                  .toStdString());
      } else if (operation == QStringLiteral("create-directory")) {
        files.removeEmptyDirectory(it->fields.value(QStringLiteral("path")));
      } else {
        throw std::runtime_error(
            QString("Unknown Archive Transaction replay operation: %1")
                .arg(operation)
                .toStdString());
      }
    } catch (const std::exception &error) {
      recordFailure(error);
    }
  }
  if (failures.isEmpty()) {
    try {
      if (m_durability->exists(m_path))
        m_durability->removeTree(m_path);
      const QString root = QFileInfo(m_path).absolutePath();
      if (m_durability->exists(root))
        m_durability->removeEmptyDirectory(root);
    } catch (const std::exception &error) {
      recordFailure(error);
    }
  }
  return failures;
}

void ArchiveTransactionWorkspace::cleanup() {
  if (!m_committed && !m_records.isEmpty())
    fail("Cannot clean an incomplete Archive Transaction workspace", m_path);
  if (m_durability->exists(m_path))
    m_durability->removeTree(m_path);
  const QString root = QFileInfo(m_path).absolutePath();
  if (m_durability->exists(root))
    m_durability->removeEmptyDirectory(root);
}

const ArchiveTransactionManifest &
ArchiveTransactionWorkspace::manifest() const noexcept {
  return m_manifest;
}

const QVector<ArchiveTransactionJournalRecord> &
ArchiveTransactionWorkspace::records() const noexcept {
  return m_records;
}

bool ArchiveTransactionWorkspace::isCommitted() const noexcept {
  return m_committed;
}

const QString &ArchiveTransactionWorkspace::path() const noexcept {
  return m_path;
}

bool FileArchiveTransactionDurability::exists(const QString &path) const {
  return QFileInfo::exists(path);
}

void FileArchiveTransactionDurability::createDirectory(const QString &path) {
  if (!QDir().mkpath(path))
    fail("Creating Archive Transaction directory failed", path);
  // Directory creation need not be the commit point: the flushed lock-file
  // bootstrap makes either persistence outcome attributable and recoverable,
  // while only the subsequently flushed manifest and journal grant ownership.
}

QString FileArchiveTransactionDurability::identity(const QString &path) const {
#ifdef Q_OS_WIN
  const HANDLE handle =
      CreateFileW(reinterpret_cast<LPCWSTR>(path.utf16()), FILE_READ_ATTRIBUTES,
                  FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                  nullptr, OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS, nullptr);
  if (handle == INVALID_HANDLE_VALUE)
    fail("Opening Archive Transaction entry for identity failed", path);
  BY_HANDLE_FILE_INFORMATION information{};
  const BOOL read = GetFileInformationByHandle(handle, &information);
  CloseHandle(handle);
  if (!read)
    fail("Reading Archive Transaction entry identity failed", path);
  const quint64 fileIndex =
      (static_cast<quint64>(information.nFileIndexHigh) << 32U) |
      information.nFileIndexLow;
  return QStringLiteral("%1:%2")
      .arg(static_cast<qulonglong>(information.dwVolumeSerialNumber), 0, 16)
      .arg(fileIndex, 0, 16);
#else
  struct stat information{};
  const QByteArray nativePath = QFile::encodeName(path);
  if (::stat(nativePath.constData(), &information) != 0)
    fail("Reading Archive Transaction entry identity failed", path);
  return QStringLiteral("%1:%2")
      .arg(static_cast<qulonglong>(information.st_dev), 0, 16)
      .arg(static_cast<qulonglong>(information.st_ino), 0, 16);
#endif
}

void FileArchiveTransactionDurability::writeNewDurably(
    const QString &path, const QByteArray &contents) {
  QFile file(path);
  if (!file.open(QIODevice::WriteOnly | QIODevice::NewOnly) ||
      file.write(contents) != contents.size())
    fail("Writing new Archive Transaction file failed", path);
  flushFile(file, path);
}

void FileArchiveTransactionDurability::appendDurably(
    const QString &path, const QByteArray &contents) {
  QFile file(path);
  if (!file.open(QIODevice::WriteOnly | QIODevice::Append) ||
      file.write(contents) != contents.size())
    fail("Appending Archive Transaction journal failed", path);
  flushFile(file, path);
}

QByteArray
FileArchiveTransactionDurability::readAll(const QString &path) const {
  QFile file(path);
  if (!file.open(QIODevice::ReadOnly))
    fail("Reading Archive Transaction file failed", path);
  return file.readAll();
}

void FileArchiveTransactionDurability::validateWorkspacePath(
    const QString &modPath, const QString &workspacePath) const {
#ifdef Q_OS_WIN
  validateAbsoluteFilesystemPath(modPath);
  validateAbsoluteFilesystemPath(workspacePath);
  const QString expectedRoot =
      QDir(modPath).filePath(ArchiveTransactionWorkspace::ReservedRootName);
  if (!isWithin(workspacePath, expectedRoot) ||
      QFileInfo(workspacePath)
              .absolutePath()
              .compare(QDir::cleanPath(expectedRoot), Qt::CaseInsensitive) != 0)
    fail("Archive Transaction workspace escapes its reserved root",
         workspacePath);

  const NativePathInformation mod = nativePathInformation(modPath, false);
  const auto rejectReparsePoint = [&](const QString &path,
                                      const QString &description) {
    const DWORD attributes = GetFileAttributesW(
        reinterpret_cast<LPCWSTR>(QDir::toNativeSeparators(path).utf16()));
    if (attributes == INVALID_FILE_ATTRIBUTES &&
        (GetLastError() == ERROR_FILE_NOT_FOUND ||
         GetLastError() == ERROR_PATH_NOT_FOUND))
      return;
    if (attributes == INVALID_FILE_ATTRIBUTES)
      fail("Inspecting Archive Transaction path failed", path);
    const NativePathInformation entry = nativePathInformation(path, true);
    if ((entry.attributes & FILE_ATTRIBUTE_REPARSE_POINT) != 0)
      fail(description, path);
    if (entry.volumeSerial != mod.volumeSerial)
      fail("Archive Transaction workspace is on a different volume", path);
  };
  rejectReparsePoint(expectedRoot,
                     "Archive Transaction reserved root is a reparse point");
  rejectReparsePoint(workspacePath,
                     "Archive Transaction workspace is a reparse point");

  const NativePathInformation ancestor =
      nativePathInformation(nearestExistingAncestor(workspacePath), false);
  if (ancestor.volumeSerial != mod.volumeSerial ||
      !isWithin(ancestor.finalPath, mod.finalPath))
    fail("Archive Transaction workspace resolves outside its Mod",
         workspacePath);
#else
  const QString root =
      QDir(modPath).filePath(ArchiveTransactionWorkspace::ReservedRootName);
  if (!QDir::isAbsolutePath(modPath) || !isWithin(workspacePath, root))
    fail("Archive Transaction workspace escapes its reserved root",
         workspacePath);
#endif
}

void FileArchiveTransactionDurability::validateReplayPath(
    const QString &path, const QString &modPath,
    const QString &workspacePath) const {
#ifdef Q_OS_WIN
  validateAbsoluteFilesystemPath(path);
  const QString cleanPath = QDir::cleanPath(path);
  if (!isWithin(cleanPath, modPath) && !isWithin(cleanPath, workspacePath))
    fail("Archive Transaction journal path escapes owned storage", path);

  const NativePathInformation mod = nativePathInformation(modPath, false);
  const NativePathInformation workspace =
      nativePathInformation(workspacePath, false);
  const NativePathInformation ancestor =
      nativePathInformation(nearestExistingAncestor(cleanPath), false);
  if (workspace.volumeSerial != mod.volumeSerial ||
      ancestor.volumeSerial != mod.volumeSerial)
    fail("Archive Transaction journal path crosses a volume", path);
  if (!isWithin(ancestor.finalPath, mod.finalPath) &&
      !isWithin(ancestor.finalPath, workspace.finalPath))
    fail("Archive Transaction journal path resolves outside owned storage",
         path);
#else
  if (!QDir::isAbsolutePath(path) ||
      (!isWithin(path, modPath) && !isWithin(path, workspacePath)))
    fail("Archive Transaction journal path escapes owned storage", path);
#endif
}

void FileArchiveTransactionDurability::preflightTransaction(
    const QString &modPath) const {
#ifdef Q_OS_WIN
  validateAbsoluteFilesystemPath(modPath);
  const QString native = QDir::toNativeSeparators(modPath);
  std::wstring volumePath(MAX_PATH, L'\0');
  if (!GetVolumePathNameW(reinterpret_cast<LPCWSTR>(native.utf16()),
                          volumePath.data(),
                          static_cast<DWORD>(volumePath.size())))
    fail("Resolving Archive Transaction filesystem failed", modPath);
  volumePath.resize(std::wcslen(volumePath.c_str()));
  std::wstring filesystemName(MAX_PATH, L'\0');
  if (!GetVolumeInformationW(volumePath.c_str(), nullptr, 0, nullptr, nullptr,
                             nullptr, filesystemName.data(),
                             static_cast<DWORD>(filesystemName.size())))
    fail("Reading Archive Transaction filesystem capabilities failed", modPath);
  filesystemName.resize(std::wcslen(filesystemName.c_str()));
  const QString filesystem = QString::fromStdWString(filesystemName);
  if (filesystem.compare(QStringLiteral("NTFS"), Qt::CaseInsensitive) != 0 &&
      filesystem.compare(QStringLiteral("ReFS"), Qt::CaseInsensitive) != 0)
    fail("Archive Transactions require NTFS or ReFS durable semantics",
         modPath);
#else
  Q_UNUSED(modPath)
  fail("Archive Transactions require a qualified Windows filesystem");
#endif
}

void FileArchiveTransactionDurability::removeTree(const QString &path) {
  if (QFileInfo::exists(path) && !QDir(path).removeRecursively())
    fail("Removing Archive Transaction workspace failed", path);
}

void FileArchiveTransactionDurability::removeEmptyDirectory(
    const QString &path) {
  if (!QFileInfo::exists(path))
    return;
  const QDir directory(path);
  if (!directory.entryList(QDir::AllEntries | QDir::NoDotAndDotDot).isEmpty())
    return;
  if (!QDir().rmdir(path))
    fail("Removing empty Archive Transaction root failed", path);
}
