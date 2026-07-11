/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

#include "pch.h"

#include "btu/bsa/settings.hpp"

#include <utility>
#include <vector>

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

class AssetWorkExecutionPolicy final {
public:
  [[nodiscard]] bool dryRun() const noexcept { return _dryRun; }
  [[nodiscard]] const ArchiveExecutionPolicy &archive() const noexcept {
    return _archive;
  }
  [[nodiscard]] const TextureExecutionPolicy &texture() const noexcept {
    return _texture;
  }
  [[nodiscard]] const MeshExecutionPolicy &mesh() const noexcept {
    return _mesh;
  }

private:
  friend class AssetWorkPolicyResolver;

  /*!
   * \brief Stores the execution settings produced by the combined resolver.
   * \param dryRun Whether execution must avoid filesystem mutations.
   * \param archive Resolved archive execution settings.
   * \param texture Resolved texture execution settings.
   * \param mesh Resolved mesh execution settings.
   */
  AssetWorkExecutionPolicy(bool dryRun, ArchiveExecutionPolicy archive,
                           TextureExecutionPolicy texture,
                           MeshExecutionPolicy mesh)
      : _dryRun(dryRun), _archive(std::move(archive)),
        _texture(std::move(texture)), _mesh(std::move(mesh)) {}

  bool _dryRun = false;
  ArchiveExecutionPolicy _archive;
  TextureExecutionPolicy _texture;
  MeshExecutionPolicy _mesh;
};
