/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

#include "AssetTransaction.h"

#include <QByteArray>

#include <memory>

enum class AnimationToolStatus {
  Converted,
  AlreadyConverted,
  OperationalFailure
};

struct AnimationToolResult {
  AnimationToolStatus status = AnimationToolStatus::OperationalFailure;
  QByteArray convertedHkx;
  QString diagnostic;
};

class AnimationConversionTool {
public:
  virtual ~AnimationConversionTool() = default;

  /*!
   * \brief Converts one animation without changing the source Asset.
   * \param sourcePath The readable HKX Asset to inspect and convert.
   * \return Converted bytes, an unchanged classification, or a tool failure.
   */
  [[nodiscard]] virtual AnimationToolResult
  convert(const QString &sourcePath) = 0;
};

struct AnimationOutputCommitRequest {
  QString targetPath;
  QByteArray convertedHkx;
};

class AnimationOutputCommitter {
public:
  virtual ~AnimationOutputCommitter() = default;

  /*!
   * \brief Publishes a converted animation without exposing partial output.
   * \param request The original path and complete converted HKX bytes.
   * \return Completed only after the replacement has committed.
   */
  [[nodiscard]] virtual AssetTransactionResult
  commit(const AnimationOutputCommitRequest &request) = 0;
};

class AtomicAnimationOutputCommitter final : public AnimationOutputCommitter {
public:
  /*!
   * \brief Atomically replaces an animation using a sibling temporary file.
   * \param request The target path and complete converted HKX bytes.
   * \return OperationalFailure without replacing the original on save error.
   */
  [[nodiscard]] AssetTransactionResult
  commit(const AnimationOutputCommitRequest &request) override;
};

class AnimationAssetTransaction final {
public:
  /*!
   * \brief Creates an animation transaction from tool and persistence adapters.
   * \param dryRun Whether execution may inspect but not invoke or mutate.
   * \param tool Adapter for isolated HKX conversion.
   * \param committer Adapter for atomic publication.
   */
  AnimationAssetTransaction(
      bool dryRun, std::unique_ptr<AnimationConversionTool> tool,
      std::unique_ptr<AnimationOutputCommitter> committer);

  /*!
   * \brief Creates the production hkxcmd-backed animation transaction.
   * \param dryRun Whether execution may report intent only.
   * \return A complete transaction with unique staging and atomic persistence.
   */
  [[nodiscard]] static std::unique_ptr<AnimationAssetTransaction>
  createProduction(bool dryRun);

  /*!
   * \brief Completes inspection, conversion, and permitted persistence.
   * \param sourcePath The classified animation Asset path.
   * \return A structured completed, unchanged, or operational failure result.
   */
  [[nodiscard]] AssetTransactionResult execute(const QString &sourcePath);

private:
  bool _dryRun = false;
  std::unique_ptr<AnimationConversionTool> _tool;
  std::unique_ptr<AnimationOutputCommitter> _committer;
};
