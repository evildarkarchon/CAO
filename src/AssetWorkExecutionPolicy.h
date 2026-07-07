/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

#include "pch.h"

#include "btu/bsa/settings.hpp"

#include <vector>

class OptionsCAO;

struct ArchiveExecutionPolicy {
  bool deleteBackup = false;
  bool mergeIncompressible = true;
  bool mergeTextures = false;
  bool createDummies = true;
  bool compress = true;
  bool deleteSource = true;
  btu::bsa::Settings settings;
  std::vector<std::u8string> filesToNotPack;
};

struct TextureExecutionPolicy {
  bool necessaryOptimization = true;
  bool compress = false;
  bool mipmaps = false;
  bool resizeBySize = false;
  size_t targetWidth = 2048;
  size_t targetHeight = 2048;
  bool resizeByRatio = false;
  uint targetWidthRatio = 1;
  uint targetHeightRatio = 1;
  DXGI_FORMAT outputFormat = DXGI_FORMAT_UNKNOWN;
  bool compressInterface = false;
  QList<DXGI_FORMAT> unwantedFormats;
};

struct MeshExecutionPolicy {
  bool processHeadparts = true;
  bool resaveMeshes = false;
  int optimizationLevel = 0;
  nifly::NiFileVersion targetFileVersion = nifly::V20_2_0_7;
  uint targetStream = 0;
  uint targetUser = 0;
  bool renameTgaReferences = false;
};

struct ProfileExecutionSnapshot {
  btu::bsa::Settings archiveSettings;
  std::vector<std::u8string> filesToNotPack;
  nifly::NiFileVersion meshFileVersion = nifly::V20_2_0_7;
  uint meshStream = 0;
  uint meshUser = 0;
  DXGI_FORMAT textureFormat = DXGI_FORMAT_UNKNOWN;
  bool texturesCompressInterface = false;
  QList<DXGI_FORMAT> textureUnwantedFormats;
  bool texturesConvertTga = false;
};

struct AssetWorkExecutionPolicy {
  bool dryRun = false;
  ArchiveExecutionPolicy archive;
  TextureExecutionPolicy texture;
  MeshExecutionPolicy mesh;

  /*!
   * \brief Resolves Asset Work Execution Policy from requested options and the
   * active Profile.
   * \param options The requested execution behavior captured from UI or CLI.
   * \return The value policy used by Asset Work Plan Execution adapters.
   */
  [[nodiscard]] static AssetWorkExecutionPolicy
  resolve(const OptionsCAO &options);

  /*!
   * \brief Resolves Asset Work Execution Policy from requested options and a
   * Profile snapshot.
   * \param options The requested execution behavior captured from UI or CLI.
   * \param profile The Profile facts needed while executing planned Asset Work
   * Items.
   * \return The value policy used by Asset Work Plan Execution adapters.
   */
  [[nodiscard]] static AssetWorkExecutionPolicy
  resolve(const OptionsCAO &options, const ProfileExecutionSnapshot &profile);
};
