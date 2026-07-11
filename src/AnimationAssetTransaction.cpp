/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#include "AnimationAssetTransaction.h"

#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QProcess>
#include <QSaveFile>
#include <QUuid>

#include <exception>
#include <utility>

namespace {
constexpr auto hkxcmdPath = "bin/hkxcmd.exe";

/*! \brief Removes transaction-owned staging files on every return path. */
class AnimationStagingCleanup final {
public:
  QString inputPath;
  QString outputPath;

  ~AnimationStagingCleanup() {
    // hkxcmd cannot write to a caller-provided handle, so its unique sibling
    // files must be removed even when the process or output validation fails.
    if (!inputPath.isEmpty())
      QFile::remove(inputPath);
    if (!outputPath.isEmpty())
      QFile::remove(outputPath);
  }
};

/*! \brief Builds a structured operational failure for one animation Asset. */
AssetTransactionResult operationalFailure(const QString &path,
                                          const QString &diagnostic) {
  return {AssetTransactionStatus::OperationalFailure,
          {{AssetTransactionNoticeCode::OperationalFailure, path, path,
            diagnostic}}};
}

/*! \brief Runs hkxcmd against unique sibling staging files. */
class HkxcmdAnimationConversionTool final : public AnimationConversionTool {
public:
  [[nodiscard]] AnimationToolResult
  convert(const QString &sourcePath) override {
    if (!QFileInfo::exists(hkxcmdPath))
      return {AnimationToolStatus::OperationalFailure,
              {},
              "hkxcmd executable was not found"};

    const QFileInfo sourceInfo(sourcePath);
    const QString uniquePart =
        QUuid::createUuid().toString(QUuid::WithoutBraces);
    AnimationStagingCleanup staging;
    staging.inputPath =
        sourceInfo.dir().filePath("." + sourceInfo.completeBaseName() +
                                  ".caoanim." + uniquePart + ".hkx");
    staging.outputPath = staging.inputPath.chopped(4) + "-out.hkx";

    if (!QFile::copy(sourcePath, staging.inputPath))
      return {AnimationToolStatus::OperationalFailure,
              {},
              "Could not stage the animation for hkxcmd"};

    QProcess process;
    const QString stagedNativePath = QDir::toNativeSeparators(
        QFileInfo(staging.inputPath).absoluteFilePath());
    process.start(hkxcmdPath, {"convert", stagedNativePath, "-v", "AMD64"});
    if (!process.waitForStarted())
      return {AnimationToolStatus::OperationalFailure,
              {},
              "hkxcmd could not start"};
    if (!process.waitForFinished(-1))
      return {
          AnimationToolStatus::OperationalFailure, {}, "hkxcmd did not finish"};

    const QString output = QString::fromLocal8Bit(
        process.readAllStandardOutput() + process.readAllStandardError());
    if (output.contains("not loadable", Qt::CaseInsensitive))
      return {AnimationToolStatus::AlreadyConverted,
              {},
              "Animation is already converted"};
    if (process.exitStatus() != QProcess::NormalExit || process.exitCode() != 0)
      return {AnimationToolStatus::OperationalFailure,
              {},
              QString("hkxcmd failed with exit code %1: %2")
                  .arg(process.exitCode())
                  .arg(output)};

    QFile converted(staging.outputPath);
    if (!converted.open(QIODevice::ReadOnly))
      return {AnimationToolStatus::OperationalFailure,
              {},
              "hkxcmd did not produce readable output"};
    const QByteArray bytes = converted.readAll();
    if (bytes.isEmpty())
      return {AnimationToolStatus::OperationalFailure,
              {},
              "hkxcmd produced empty output"};

    return {AnimationToolStatus::Converted, bytes, {}};
  }
};
} // namespace

AssetTransactionResult AtomicAnimationOutputCommitter::commit(
    const AnimationOutputCommitRequest &request) {
  QSaveFile output(request.targetPath);
  // Direct-write fallback could truncate the live Asset before a failed save.
  output.setDirectWriteFallback(false);
  if (!output.open(QIODevice::WriteOnly) ||
      output.write(request.convertedHkx) != request.convertedHkx.size() ||
      !output.commit()) {
    return operationalFailure(request.targetPath,
                              "Could not atomically save converted animation");
  }

  return {AssetTransactionStatus::Completed,
          {{AssetTransactionNoticeCode::CompletedAction, request.targetPath,
            request.targetPath, "Converted animation"}}};
}

AnimationAssetTransaction::AnimationAssetTransaction(
    const bool dryRun, std::unique_ptr<AnimationConversionTool> tool,
    std::unique_ptr<AnimationOutputCommitter> committer)
    : _dryRun(dryRun), _tool(std::move(tool)),
      _committer(std::move(committer)) {}

std::unique_ptr<AnimationAssetTransaction>
AnimationAssetTransaction::createProduction(const bool dryRun) {
  return std::make_unique<AnimationAssetTransaction>(
      dryRun, std::make_unique<HkxcmdAnimationConversionTool>(),
      std::make_unique<AtomicAnimationOutputCommitter>());
}

AssetTransactionResult
AnimationAssetTransaction::execute(const QString &sourcePath) {
  QFile source(sourcePath);
  if (!QFileInfo(sourcePath).isFile() || !source.open(QIODevice::ReadOnly))
    return operationalFailure(sourcePath,
                              "Animation source is missing or unreadable");
  source.close();

  if (_dryRun) {
    return {
        AssetTransactionStatus::Completed,
        {{AssetTransactionNoticeCode::IntendedAction, sourcePath, sourcePath,
          "Would convert animation to the appropriate format"}}};
  }

  try {
    auto converted = _tool->convert(sourcePath);
    switch (converted.status) {
    case AnimationToolStatus::AlreadyConverted:
      return {AssetTransactionStatus::Unchanged,
              {{AssetTransactionNoticeCode::UnchangedAsset, sourcePath,
                sourcePath, converted.diagnostic}}};
    case AnimationToolStatus::OperationalFailure:
      return operationalFailure(sourcePath, converted.diagnostic);
    case AnimationToolStatus::Converted:
      if (converted.convertedHkx.isEmpty())
        return operationalFailure(sourcePath,
                                  "Animation tool returned empty output");
      return _committer->commit(AnimationOutputCommitRequest{
          sourcePath, std::move(converted.convertedHkx)});
    }
  } catch (const std::exception &error) {
    return operationalFailure(
        sourcePath, QString("Animation transaction failed unexpectedly: %1")
                        .arg(error.what()));
  } catch (...) {
    return operationalFailure(sourcePath,
                              "Animation transaction failed unexpectedly");
  }

  return operationalFailure(sourcePath, "Animation tool returned no status");
}
