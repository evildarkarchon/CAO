/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

#include "AssetWorkMode.h"

#include <QString>

#include <cstddef>
#include <optional>

class AssetWorkOptionsDraft;
struct AssetWorkOptionsCreationResult;

class AssetWorkOptions final {
public:
  /*!
   * \brief Validates adapter input and creates immutable Asset Work Options.
   * \param draft Raw UI, CLI, or settings input to validate.
   * \return The immutable options, or a user-facing validation error.
   */
  [[nodiscard]] static AssetWorkOptionsCreationResult
  create(const AssetWorkOptionsDraft &draft);

  [[nodiscard]] AssetWorkMode mode() const noexcept;

private:
  friend class AssetWorkPolicyResolver;

  /*!
   * \brief Copies already-validated draft values into immutable storage.
   */
  explicit AssetWorkOptions(const AssetWorkOptionsDraft &draft);

  bool _extractArchives = false;
  bool _packArchives = false;
  bool _deleteArchiveBackup = false;
  bool _mergeIncompressibleArchives = true;
  bool _mergeTextureArchives = false;
  bool _createDummyPlugins = true;
  bool _compressArchives = true;
  bool _deletePackedSource = true;
  bool _optimizeAnimations = false;
  bool _dryRun = false;
  int _meshOptimizationLevel = 0;
  bool _processHeadparts = true;
  bool _resaveMeshes = false;
  bool _necessaryTextureOptimization = true;
  bool _compressTextures = false;
  bool _generateTextureMipmaps = false;
  bool _resizeTexturesBySize = false;
  size_t _textureTargetWidth = 2048;
  size_t _textureTargetHeight = 2048;
  bool _resizeTexturesByRatio = false;
  uint _textureTargetWidthRatio = 1;
  uint _textureTargetHeightRatio = 1;
  AssetWorkMode _mode = AssetWorkMode::SingleMod;
};

struct AssetWorkOptionsCreationResult {
  std::optional<AssetWorkOptions> options;
  QString error;
};
