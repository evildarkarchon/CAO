/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

#include "AssetTransaction.h"

#include <QByteArray>

#include <memory>

enum class TextureSourceKind { Dds, Tga };

struct TextureTransformRequest {
  QString sourcePath;
  TextureSourceKind sourceKind = TextureSourceKind::Dds;
  bool dryRun = false;
};

struct TextureTransformResult {
  AssetTransactionStatus status = AssetTransactionStatus::Unchanged;
  QByteArray encodedDds;
  QVector<AssetTransactionNotice> notices;
};

class TextureTransformEngine {
public:
  virtual ~TextureTransformEngine() = default;

  /*!
   * \brief Inspects and transforms one texture without changing the filesystem.
   * \param request The source texture and Dry Run contract.
   * \return Encoded DDS output when a non-Dry-Run transformation completed.
   */
  [[nodiscard]] virtual TextureTransformResult
  transform(const TextureTransformRequest &request) = 0;
};

struct TextureOutputCommitRequest {
  QString sourcePath;
  QString targetPath;
  QByteArray encodedDds;
  bool removeSourceAfterCommit = false;
};

class TextureOutputCommitter {
public:
  virtual ~TextureOutputCommitter() = default;

  /*!
   * \brief Atomically publishes encoded DDS output and optional TGA cleanup.
   * \param request Source, destination, bytes, and cleanup requirement.
   * \return Completed only after publication and cleanup commit together.
   */
  [[nodiscard]] virtual AssetTransactionResult
  commit(const TextureOutputCommitRequest &request) = 0;
};

class AtomicTextureOutputCommitter final : public TextureOutputCommitter {
public:
  /*!
   * \brief Publishes DDS bytes with same-directory atomic replacement.
   * \param request Source, destination, bytes, and optional TGA cleanup.
   * \return OperationalFailure when publication or rollback cannot complete.
   */
  [[nodiscard]] AssetTransactionResult
  commit(const TextureOutputCommitRequest &request) override;
};

class TextureAssetTransaction final {
public:
  /*!
   * \brief Creates a complete texture transaction from transform and commit
   * adapters.
   * \param dryRun Whether filesystem mutations are forbidden.
   * \param engine Adapter that performs read-only inspection and
   * transformation.
   * \param committer Adapter that publishes transformed output atomically.
   */
  TextureAssetTransaction(bool dryRun,
                          std::unique_ptr<TextureTransformEngine> engine,
                          std::unique_ptr<TextureOutputCommitter> committer);

  /*!
   * \brief Completes inspection, transformation, and permitted persistence.
   * \param sourcePath The classified texture Asset path.
   * \param sourceKind Whether the source is DDS or TGA.
   * \return A classified result with structured notices.
   */
  [[nodiscard]] AssetTransactionResult execute(const QString &sourcePath,
                                               TextureSourceKind sourceKind);

private:
  bool _dryRun = false;
  std::unique_ptr<TextureTransformEngine> _engine;
  std::unique_ptr<TextureOutputCommitter> _committer;
};
