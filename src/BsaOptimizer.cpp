/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#include "BsaOptimizer.h"

#include "AssetPathVisibility.h"

#include <QDir>
#include <QFileInfo>
#include <QUuid>

#include <algorithm>
#include <optional>
#include <stdexcept>
#include <utility>

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

BSAOptimizer::BSAOptimizer(ArchiveTransactionBootstrap &bootstrap)
    : BSAOptimizer() {
  _bootstrap = &bootstrap;
}

BSAOptimizer::BSAOptimizer(ArchiveEngine &engine, ArchiveFileOperations &files)
    : _engine(&engine), _files(&files) {}

BSAOptimizer::BSAOptimizer(ArchiveEngine &engine, ArchiveFileOperations &files,
                           ArchiveTransactionBootstrap &bootstrap)
    : BSAOptimizer(engine, files) {
  _bootstrap = &bootstrap;
}

void BSAOptimizer::extract(const QString &archivePath, const bool deleteBackup,
                           const bool dryRun) {
  if (dryRun) {
    PLOG_INFO << archivePath + " would be extracted from BSA.";
    return;
  }

  const QString liveRoot = QFileInfo(archivePath).absolutePath();
  std::optional<ArchiveTransactionWorkspace> workspace;
  try {
    workspace.emplace(createWorkspace(
        ArchiveTransactionKind::Extraction, liveRoot, archivePath,
        {{QStringLiteral("deleteBackup"), deleteBackup ? "true" : "false"}}));
  } catch (const std::exception &error) {
    throwFailure(ArchiveOperation::Extraction, ArchiveFailureStage::Staging,
                 archivePath, error);
  }

  const QString staging = QDir(workspace->path()).filePath("staged");
  const QString rollbackRoot = QDir(workspace->path()).filePath("rollback");
  try {
    _files->createDirectories(staging);
    _files->createDirectories(rollbackRoot);
  } catch (const std::exception &error) {
    const QStringList rollback = workspace->replay(*_files);
    throwFailure(ArchiveOperation::Extraction,
                 rollback.isEmpty() ? ArchiveFailureStage::Staging
                                    : ArchiveFailureStage::Rollback,
                 archivePath, error, rollback);
  }

  try {
    _engine->extractTo(archivePath, staging);
  } catch (const std::exception &error) {
    const QStringList rollback = workspace->replay(*_files);
    if (!rollback.isEmpty())
      throwFailure(ArchiveOperation::Extraction, ArchiveFailureStage::Rollback,
                   archivePath, error, rollback);
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
      if (entry.regularFile)
        _files->flushFileDurably(entry.absolutePath);
    }
  } catch (const std::exception &error) {
    const QStringList rollback = workspace->replay(*_files);
    if (!rollback.isEmpty())
      throwFailure(ArchiveOperation::Extraction, ArchiveFailureStage::Rollback,
                   archivePath, error, rollback);
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

      ensureDestinationParent(destination, liveRoot, *workspace);
      moveJournaled(entry.absolutePath, destination, *workspace);
    }

    if (deleteBackup) {
      moveJournaled(archivePath, QDir(rollbackRoot).filePath("source-archive"),
                    *workspace);
    } else {
      // A retained extraction backup is a committed Asset, not rollback-only
      // transaction material, so committed recovery must leave it in place.
      moveJournaled(archivePath, uniqueBackupPath(archivePath), *workspace);
    }
    QMap<QString, QString> cleanup;
    if (deleteBackup)
      cleanup.insert(QStringLiteral("cleanup-file.0"),
                     QDir(rollbackRoot).filePath("source-archive"));
    workspace->commit(cleanup);
  } catch (const std::exception &error) {
    const QStringList rollback = workspace->replay(*_files);
    throwFailure(ArchiveOperation::Extraction,
                 rollback.isEmpty() ? ArchiveFailureStage::Publishing
                                    : ArchiveFailureStage::Rollback,
                 archivePath, error, rollback);
  }

  const QStringList cleanup = workspace->replay(*_files);
  if (!cleanup.isEmpty()) {
    throw ArchiveExecutionError(
        ArchiveOperation::Extraction, ArchiveFailureStage::SourceCleanup,
        archivePath,
        QString("Archive extraction committed, but deferred cleanup failed: %1")
            .arg(cleanup.join("; ")));
  }
  PLOG_INFO << "BSA successfully extracted: " + archivePath;
}

void BSAOptimizer::packAll(const QString &folderPath,
                           const ArchiveExecutionPolicy &policy,
                           const bool dryRun) {
  if (dryRun) {
    PLOG_INFO << folderPath + " would be packed into BSA archives.";
    return;
  }

  std::optional<ArchiveTransactionWorkspace> workspace;
  try {
    workspace.emplace(
        createWorkspace(ArchiveTransactionKind::Packing, folderPath, folderPath,
                        {{QStringLiteral("deleteSource"),
                          policy.deleteSource ? "true" : "false"}}));
  } catch (const std::exception &error) {
    throwFailure(ArchiveOperation::Packing, ArchiveFailureStage::Staging,
                 folderPath, error);
  }
  const QString staging = QDir(workspace->path()).filePath("staged");
  const QString rollbackRoot = QDir(workspace->path()).filePath("rollback");
  try {
    _files->createDirectories(staging);
    _files->createDirectories(rollbackRoot);
  } catch (const std::exception &error) {
    const QStringList rollback = workspace->replay(*_files);
    throwFailure(ArchiveOperation::Packing,
                 rollback.isEmpty() ? ArchiveFailureStage::Staging
                                    : ArchiveFailureStage::Rollback,
                 folderPath, error, rollback);
  }

  QSet<QString> inputManifest;
  try {
    const auto inputs = _files->listRecursively(folderPath);
    for (const auto &input : inputs) {
      if (input.regularFile && !input.symbolicLink &&
          isSafeRelativePath(input.relativePath) &&
          !AssetPathVisibility::isInternalPath(input.relativePath) &&
          isLoosePackableAsset(input.relativePath)) {
        inputManifest.insert(normalizedAbsolutePath(input.absolutePath));
      }
    }
  } catch (const std::exception &error) {
    const QStringList rollback = workspace->replay(*_files);
    if (!rollback.isEmpty())
      throwFailure(ArchiveOperation::Packing, ArchiveFailureStage::Rollback,
                   folderPath, error, rollback);
    throwFailure(ArchiveOperation::Packing, ArchiveFailureStage::Validation,
                 folderPath, error);
  }

  StagedArchivePacking packing;
  try {
    packing = _engine->packTo(folderPath, staging, policy);
  } catch (const std::exception &error) {
    const QStringList rollback = workspace->replay(*_files);
    if (!rollback.isEmpty())
      throwFailure(ArchiveOperation::Packing, ArchiveFailureStage::Rollback,
                   folderPath, error, rollback);
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
      _files->flushFileDurably(output.stagingPath);
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
    const QStringList rollback = workspace->replay(*_files);
    if (!rollback.isEmpty())
      throwFailure(ArchiveOperation::Packing, ArchiveFailureStage::Rollback,
                   folderPath, error, rollback);
    throwFailure(ArchiveOperation::Packing, ArchiveFailureStage::Validation,
                 folderPath, error);
  }

  try {
    int rollbackIndex = 0;
    QStringList rollbackOnlyFiles;
    bool publishedOutput = false;
    for (const auto &output : packing.outputs) {
      const QString destination =
          QDir(folderPath).filePath(output.relativeDestination);

      if (output.kind == StagedArchiveOutputKind::DummyPlugin &&
          _files->exists(destination)) {
        // A generated dummy must never replace a real loose plugin.
        continue;
      }

      ensureDestinationParent(destination, folderPath, *workspace);
      if (_files->exists(destination)) {
        const QString rollbackPath =
            QDir(rollbackRoot)
                .filePath(QString("output-%1").arg(rollbackIndex++));
        moveJournaled(destination, rollbackPath, *workspace);
        rollbackOnlyFiles << rollbackPath;
      }
      moveJournaled(output.stagingPath, destination, *workspace);
      publishedOutput = true;
    }

    if (!publishedOutput) {
      // A no-op pack has no journal intents and therefore cannot be committed.
      workspace->cleanup();
      return;
    }

    QMap<QString, QString> cleanup;
    int cleanupIndex = 0;
    for (const QString &rollbackPath : std::as_const(rollbackOnlyFiles))
      cleanup.insert(QString("cleanup-file.%1").arg(cleanupIndex++),
                     rollbackPath);
    if (policy.deleteSource) {
      int sourceIndex = 0;
      for (const QString &source : packing.packedSourceAssets) {
        const QString rollbackPath =
            QDir(rollbackRoot).filePath(QString("source-%1").arg(sourceIndex));
        moveJournaled(source, rollbackPath, *workspace);
        cleanup.insert(QString("cleanup-file.%1").arg(cleanupIndex++),
                       rollbackPath);
        ++sourceIndex;
      }
    }
    workspace->commit(cleanup);
  } catch (const std::exception &error) {
    const QStringList rollback = workspace->replay(*_files);
    throwFailure(ArchiveOperation::Packing,
                 rollback.isEmpty() ? ArchiveFailureStage::Publishing
                                    : ArchiveFailureStage::Rollback,
                 folderPath, error, rollback);
  }

  const QStringList cleanup = workspace->replay(*_files);
  if (!cleanup.isEmpty()) {
    throw ArchiveExecutionError(
        ArchiveOperation::Packing, ArchiveFailureStage::SourceCleanup,
        folderPath,
        QString("Archive outputs committed, but deferred cleanup failed: %1")
            .arg(cleanup.join("; ")));
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

void BSAOptimizer::ensureDestinationParent(
    const QString &destination, const QString &liveRoot,
    ArchiveTransactionWorkspace &workspace) {
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
    _files->validateReplayPath(path, workspace.manifest().canonicalModPath,
                               workspace.path());
    workspace.append(
        ArchiveTransactionRecordKind::Intent,
        {{QStringLiteral("operation"), QStringLiteral("create-directory")},
         {QStringLiteral("path"), path}});
    _files->createDirectories(path);
    workspace.append(ArchiveTransactionRecordKind::MutationComplete,
                     {{QStringLiteral("path"), path}});
  }
}

void BSAOptimizer::moveJournaled(const QString &source,
                                 const QString &destination,
                                 ArchiveTransactionWorkspace &workspace) {
  _files->validateReplayPath(source, workspace.manifest().canonicalModPath,
                             workspace.path());
  _files->validateReplayPath(destination, workspace.manifest().canonicalModPath,
                             workspace.path());
  workspace.append(ArchiveTransactionRecordKind::Intent,
                   {{QStringLiteral("operation"), QStringLiteral("move")},
                    {QStringLiteral("source"), source},
                    {QStringLiteral("destination"), destination}});
  _files->move(source, destination);
  workspace.append(ArchiveTransactionRecordKind::MutationComplete,
                   {{QStringLiteral("source"), source},
                    {QStringLiteral("destination"), destination}});
}

ArchiveTransactionWorkspace
BSAOptimizer::createWorkspace(const ArchiveTransactionKind kind,
                              const QString &modPath, const QString &anchorPath,
                              const QMap<QString, QString> &policyFacts) {
  const QString canonicalModPath = QFileInfo(modPath).absoluteFilePath();
  const QString canonicalAnchorPath = QFileInfo(anchorPath).absoluteFilePath();
  _files->preflightTransaction(canonicalModPath);
  ArchiveTransactionManifest manifest;
  manifest.transactionId = QUuid::createUuid().toString(QUuid::WithoutBraces);
  manifest.kind = kind;
  manifest.canonicalModPath = canonicalModPath;
  manifest.modIdentity = _files->identity(canonicalModPath);
  manifest.canonicalAnchorPath = canonicalAnchorPath;
  manifest.anchorIdentity = _files->identity(canonicalAnchorPath);
  // The durable adapter's stable identity includes its volume component. The
  // full Mod identity is retained here until the adapter exposes it separately.
  manifest.volumeIdentity = manifest.modIdentity;
  manifest.policyFacts = policyFacts;
  const QString workspacePath =
      QDir(QDir(canonicalModPath)
               .filePath(ArchiveTransactionWorkspace::ReservedRootName))
          .filePath(manifest.transactionId);
  if (_bootstrap)
    _bootstrap->begin(canonicalModPath, manifest.transactionId, workspacePath);
  auto durability = std::shared_ptr<ArchiveTransactionDurability>(
      _files, [](ArchiveTransactionDurability *) {});
  auto workspace =
      ArchiveTransactionWorkspace::create(manifest, std::move(durability));
  if (_bootstrap)
    _bootstrap->complete(canonicalModPath, manifest.transactionId);
  return workspace;
}
