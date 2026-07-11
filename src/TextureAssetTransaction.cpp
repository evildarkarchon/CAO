/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#include "TextureAssetTransaction.h"

#include <QFile>
#include <QSaveFile>

#include <utility>

namespace {
/*! \brief Selects a non-existing sibling rollback path without deleting data.
 */
QString rollbackPathFor(const QString &targetPath) {
  QString candidate = targetPath + ".caorollback";
  int suffix = 1;
  while (QFile::exists(candidate))
    candidate = targetPath + ".caorollback." + QString::number(suffix++);
  return candidate;
}

/*! \brief Builds an operational failure result for a commit diagnostic. */
AssetTransactionResult commitFailure(const TextureOutputCommitRequest &request,
                                     const QString &diagnostic) {
  return {AssetTransactionStatus::OperationalFailure,
          {{AssetTransactionNoticeCode::OperationalFailure, request.sourcePath,
            request.targetPath, diagnostic}}};
}
} // namespace

AssetTransactionResult AtomicTextureOutputCommitter::commit(
    const TextureOutputCommitRequest &request) {
  QString rollbackPath;
  const bool distinctExistingTarget =
      request.targetPath != request.sourcePath &&
      QFile::exists(request.targetPath);
  if (distinctExistingTarget) {
    rollbackPath = rollbackPathFor(request.targetPath);
    if (!QFile::rename(request.targetPath, rollbackPath))
      return commitFailure(request,
                           "Could not preserve the existing DDS output");
  }

  QSaveFile output(request.targetPath);
  // Direct-write fallback could truncate the live Asset before a failed commit.
  output.setDirectWriteFallback(false);
  if (!output.open(QIODevice::WriteOnly) ||
      output.write(request.encodedDds) != request.encodedDds.size() ||
      !output.commit()) {
    if (!rollbackPath.isEmpty())
      QFile::rename(rollbackPath, request.targetPath);
    return commitFailure(request,
                         "Could not atomically publish the DDS output");
  }

  if (request.removeSourceAfterCommit && !QFile::remove(request.sourcePath)) {
    // TGA conversion is one transaction: undo the new DDS if source cleanup
    // fails, then restore any DDS that existed before the transaction.
    const bool removedNewOutput = QFile::remove(request.targetPath);
    const bool restoredPreviousOutput =
        rollbackPath.isEmpty() ||
        QFile::rename(rollbackPath, request.targetPath);
    QString diagnostic = "Could not remove the converted TGA source";
    if (!removedNewOutput || !restoredPreviousOutput)
      diagnostic += "; rollback was incomplete";
    return commitFailure(request, diagnostic);
  }

  if (!rollbackPath.isEmpty() && !QFile::remove(rollbackPath))
    return commitFailure(request,
                         "DDS committed but rollback backup cleanup failed");

  return {AssetTransactionStatus::Completed,
          {{AssetTransactionNoticeCode::CompletedAction, request.sourcePath,
            request.targetPath, "Committed texture output"}}};
}

TextureAssetTransaction::TextureAssetTransaction(
    const bool dryRun, std::unique_ptr<TextureTransformEngine> engine,
    std::unique_ptr<TextureOutputCommitter> committer)
    : _dryRun(dryRun), _engine(std::move(engine)),
      _committer(std::move(committer)) {}

AssetTransactionResult
TextureAssetTransaction::execute(const QString &sourcePath,
                                 const TextureSourceKind sourceKind) {
  auto transformed = _engine->transform(
      TextureTransformRequest{sourcePath, sourceKind, _dryRun});
  AssetTransactionResult result{transformed.status,
                                std::move(transformed.notices)};
  if (_dryRun || transformed.status != AssetTransactionStatus::Completed)
    return result;

  QString targetPath = sourcePath;
  const bool replaceTga = sourceKind == TextureSourceKind::Tga;
  if (replaceTga)
    targetPath = sourcePath.chopped(4) + ".dds";

  auto committed = _committer->commit(TextureOutputCommitRequest{
      sourcePath, targetPath, std::move(transformed.encodedDds), replaceTga});
  result.status = committed.status;
  result.notices += committed.notices;
  return result;
}
