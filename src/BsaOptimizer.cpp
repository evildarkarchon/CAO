/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#include "BsaOptimizer.h"

#include <QDir>
#include <QFileInfo>

#include <algorithm>
#include <stdexcept>

namespace {
bool isSafeRelativePath(const QString &relativePath) {
  const QString clean = QDir::cleanPath(relativePath);
  return !relativePath.isEmpty() && !QDir::isAbsolutePath(relativePath) &&
         clean != ".." && !clean.startsWith("../") && !clean.startsWith("..\\");
}

QString normalizedAbsolutePath(const QString &path) {
  return QDir::cleanPath(QFileInfo(path).absoluteFilePath()).toLower();
}

bool isLoosePackableAsset(const QString &path) {
  const QString suffix = QFileInfo(path).suffix().toLower();
  return suffix != "bsa" && suffix != "ba2" && suffix != "esp" &&
         suffix != "esm" && suffix != "esl";
}

[[noreturn]] void throwFailure(ArchiveOperation operation,
                               ArchiveFailureStage stage,
                               const QString &assetPath,
                               const std::exception &error,
                               QStringList rollback = {}) {
  throw ArchiveExecutionError(operation, stage, assetPath,
                              QString::fromUtf8(error.what()),
                              std::move(rollback));
}
} // namespace

BSAOptimizer::BSAOptimizer()
    : _ownedEngine(std::make_unique<BtuArchiveEngine>()),
      _ownedFiles(std::make_unique<QtArchiveFileOperations>()),
      _engine(_ownedEngine.get()), _files(_ownedFiles.get()) {}

BSAOptimizer::BSAOptimizer(ArchiveEngine &engine, ArchiveFileOperations &files)
    : _engine(&engine), _files(&files) {}

void BSAOptimizer::extract(const QString &archivePath, const bool deleteBackup,
                           const bool dryRun) {
  if (dryRun) {
    PLOG_INFO << archivePath + " would be extracted from BSA.";
    return;
  }

  QString staging;
  QStringList published;
  QStringList createdDirectories;
  const QString liveRoot = QFileInfo(archivePath).absolutePath();

  try {
    staging = _files->createSiblingStagingDirectory(archivePath, "extract");
  } catch (const std::exception &error) {
    throwFailure(ArchiveOperation::Extraction, ArchiveFailureStage::Staging,
                 archivePath, error);
  }

  try {
    _engine->extractTo(archivePath, staging);
  } catch (const std::exception &error) {
    cleanupStaging(staging);
    throwFailure(ArchiveOperation::Extraction, ArchiveFailureStage::Engine,
                 archivePath, error);
  }

  QVector<ArchiveFileEntry> entries;
  try {
    entries = _files->listRecursively(staging);
    for (const auto &entry : entries) {
      if (entry.symbolicLink ||
          (entry.regularFile && !isSafeRelativePath(entry.relativePath))) {
        throw std::runtime_error(QString("Unsafe staged archive path: %1")
                                     .arg(entry.relativePath)
                                     .toStdString());
      }
    }
  } catch (const std::exception &error) {
    cleanupStaging(staging);
    throwFailure(ArchiveOperation::Extraction, ArchiveFailureStage::Validation,
                 archivePath, error);
  }

  std::sort(entries.begin(), entries.end(),
            [](const ArchiveFileEntry &left, const ArchiveFileEntry &right) {
              return left.relativePath.compare(right.relativePath,
                                               Qt::CaseInsensitive) < 0;
            });

  try {
    for (const auto &entry : entries) {
      if (!entry.regularFile)
        continue;

      const QString destination = QDir(liveRoot).filePath(entry.relativePath);
      // Existing loose Assets always win. This also makes the first extracted
      // archive win a collision with a later archive in the same execution.
      if (_files->exists(destination))
        continue;

      ensureDestinationParent(destination, liveRoot, createdDirectories);
      _files->move(entry.absolutePath, destination);
      published << destination;
    }

    if (deleteBackup) {
      _files->removeFile(archivePath);
    } else {
      _files->move(archivePath, uniqueBackupPath(archivePath));
    }
  } catch (const std::exception &error) {
    const QStringList rollback =
        rollbackPublished(published, createdDirectories);
    cleanupStaging(staging);
    throwFailure(ArchiveOperation::Extraction,
                 rollback.isEmpty() ? ArchiveFailureStage::Publishing
                                    : ArchiveFailureStage::Rollback,
                 archivePath, error, rollback);
  }

  cleanupStaging(staging);
  PLOG_INFO << "BSA successfully extracted: " + archivePath;
}

void BSAOptimizer::packAll(const QString &folderPath,
                           const ArchiveExecutionPolicy &policy,
                           const bool dryRun) {
  if (dryRun) {
    PLOG_INFO << folderPath + " would be packed into BSA archives.";
    return;
  }

  QString staging;
  try {
    staging = _files->createSiblingStagingDirectory(folderPath, "pack");
  } catch (const std::exception &error) {
    throwFailure(ArchiveOperation::Packing, ArchiveFailureStage::Staging,
                 folderPath, error);
  }

  QSet<QString> inputManifest;
  try {
    const auto inputs = _files->listRecursively(folderPath);
    for (const auto &input : inputs) {
      if (input.regularFile && !input.symbolicLink &&
          isSafeRelativePath(input.relativePath) &&
          isLoosePackableAsset(input.relativePath)) {
        inputManifest.insert(normalizedAbsolutePath(input.absolutePath));
      }
    }
  } catch (const std::exception &error) {
    cleanupStaging(staging);
    throwFailure(ArchiveOperation::Packing, ArchiveFailureStage::Validation,
                 folderPath, error);
  }

  StagedArchivePacking packing;
  try {
    packing = _engine->packTo(folderPath, staging, policy);
  } catch (const std::exception &error) {
    cleanupStaging(staging);
    throwFailure(ArchiveOperation::Packing, ArchiveFailureStage::Engine,
                 folderPath, error);
  }

  try {
    QSet<QString> destinations;
    const QDir stagingDir(staging);
    for (const auto &output : packing.outputs) {
      const QString stagedRelative =
          stagingDir.relativeFilePath(output.stagingPath);
      if (!_files->exists(output.stagingPath) ||
          !isSafeRelativePath(stagedRelative) ||
          !isSafeRelativePath(output.relativeDestination)) {
        throw std::runtime_error(QString("Invalid staged archive output: %1")
                                     .arg(output.stagingPath)
                                     .toStdString());
      }
      const QString normalized =
          QDir::cleanPath(output.relativeDestination).toLower();
      if (destinations.contains(normalized)) {
        throw std::runtime_error(
            QString("Duplicate staged archive destination: %1")
                .arg(output.relativeDestination)
                .toStdString());
      }
      destinations.insert(normalized);
    }
    for (const QString &source : packing.packedSourceAssets) {
      const QString relative = QDir(folderPath).relativeFilePath(source);
      if (!isSafeRelativePath(relative) || !isLoosePackableAsset(relative) ||
          !inputManifest.contains(normalizedAbsolutePath(source))) {
        throw std::runtime_error(
            QString("Archive engine returned an unsafe packed source: %1")
                .arg(source)
                .toStdString());
      }
    }
  } catch (const std::exception &error) {
    cleanupStaging(staging);
    throwFailure(ArchiveOperation::Packing, ArchiveFailureStage::Validation,
                 folderPath, error);
  }

  struct PublishedOutput {
    QString destination;
    QString backup;
    bool published = false;
  };
  QVector<PublishedOutput> journal;
  QStringList createdDirectories;

  try {
    for (const auto &output : packing.outputs) {
      const QString destination =
          QDir(folderPath).filePath(output.relativeDestination);

      if (output.kind == StagedArchiveOutputKind::DummyPlugin &&
          _files->exists(destination)) {
        // A generated dummy must never replace a real loose plugin.
        continue;
      }

      ensureDestinationParent(destination, folderPath, createdDirectories);
      PublishedOutput entry{destination, {}, false};
      if (_files->exists(destination)) {
        entry.backup = uniqueBackupPath(destination);
        _files->move(destination, entry.backup);
      }
      journal.push_back(entry);
      _files->move(output.stagingPath, destination);
      journal.back().published = true;
    }
  } catch (const std::exception &error) {
    QStringList rollback;
    for (auto it = journal.rbegin(); it != journal.rend(); ++it) {
      try {
        if (it->published)
          _files->removeFile(it->destination);
        if (!it->backup.isEmpty())
          _files->move(it->backup, it->destination);
      } catch (const std::exception &rollbackError) {
        rollback << QString::fromUtf8(rollbackError.what());
      }
    }
    rollback.append(rollbackPublished({}, createdDirectories));
    cleanupStaging(staging);
    throwFailure(ArchiveOperation::Packing,
                 rollback.isEmpty() ? ArchiveFailureStage::Publishing
                                    : ArchiveFailureStage::Rollback,
                 folderPath, error, rollback);
  }

  cleanupStaging(staging);

  if (policy.deleteSource) {
    QStringList failures;
    for (const QString &source : packing.packedSourceAssets) {
      try {
        // Source deletion is deliberately after the entire archive output set
        // commits. A failure leaves safe duplicates and retained backups.
        _files->removeFile(source);
      } catch (const std::exception &error) {
        failures << QString::fromUtf8(error.what());
      }
    }
    if (!failures.isEmpty()) {
      throw ArchiveExecutionError(
          ArchiveOperation::Packing, ArchiveFailureStage::SourceCleanup,
          folderPath,
          QString("Archive outputs committed, but loose source cleanup failed: "
                  "%1")
              .arg(failures.join("; ")));
    }
  }

  PLOG_INFO << "BSA archive set successfully packed: " + folderPath;
}

QString BSAOptimizer::uniqueBackupPath(const QString &path) const {
  QString candidate = path + ".bak";
  int suffix = 1;
  while (_files->exists(candidate))
    candidate = QString("%1.bak.%2").arg(path).arg(suffix++);
  return candidate;
}

void BSAOptimizer::ensureDestinationParent(const QString &destination,
                                           const QString &liveRoot,
                                           QStringList &createdDirectories) {
  QString directory = QFileInfo(destination).absolutePath();
  QStringList missing;
  const QString normalizedRoot = QDir(liveRoot).absolutePath();
  while (directory != normalizedRoot && !_files->exists(directory)) {
    missing.prepend(directory);
    const QString parent = QFileInfo(directory).absolutePath();
    if (parent == directory)
      throw std::runtime_error("Destination escaped its live root");
    directory = parent;
  }
  for (const QString &path : missing) {
    _files->createDirectories(path);
    createdDirectories << path;
  }
}

QStringList
BSAOptimizer::rollbackPublished(const QStringList &published,
                                const QStringList &createdDirectories) {
  QStringList failures;
  for (auto it = published.crbegin(); it != published.crend(); ++it) {
    try {
      _files->removeFile(*it);
    } catch (const std::exception &error) {
      failures << QString::fromUtf8(error.what());
    }
  }
  for (auto it = createdDirectories.crbegin(); it != createdDirectories.crend();
       ++it) {
    try {
      _files->removeEmptyDirectory(*it);
    } catch (const std::exception &error) {
      failures << QString::fromUtf8(error.what());
    }
  }
  return failures;
}

void BSAOptimizer::cleanupStaging(const QString &stagingPath) noexcept {
  if (stagingPath.isEmpty())
    return;
  try {
    _files->removeTree(stagingPath);
  } catch (const std::exception &error) {
    // A committed transaction remains valid if only its private staging
    // cleanup fails. Keep the diagnostic without undoing published Assets.
    PLOG_WARNING << "Failed to clean archive staging: " << error.what();
  }
}
