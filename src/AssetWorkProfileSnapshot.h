/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

#include "pch.h"

#include "btu/bsa/settings.hpp"

#include <optional>
#include <vector>

struct AssetWorkProfileSnapshotInput {
  bool archivesEnabled = false;
  btu::bsa::Settings archiveSettings;
  std::vector<std::u8string> filesToNotPack;
  bool meshesEnabled = false;
  nifly::NiFileVersion meshFileVersion = nifly::V20_2_0_7;
  uint meshStream = 0;
  uint meshUser = 0;
  bool animationsEnabled = false;
  bool texturesEnabled = false;
  DXGI_FORMAT textureFormat = DXGI_FORMAT_UNKNOWN;
  bool texturesCompressInterface = false;
  QList<DXGI_FORMAT> textureUnwantedFormats;
  bool texturesConvertTga = false;
};

struct AssetWorkProfileSnapshotCreationResult;

class AssetWorkProfileSnapshot final {
public:
  /*!
   * \brief Validates captured Profile facts and creates an immutable snapshot.
   * \param input The Profile capabilities and execution parameters captured by
   * the application composition root.
   * \return The snapshot, or a user-facing validation error for malformed
   * Profile data.
   */
  [[nodiscard]] static AssetWorkProfileSnapshotCreationResult
  create(AssetWorkProfileSnapshotInput input);

private:
  friend class AssetWorkPolicyResolver;

  /*!
   * \brief Stores Profile facts after create has validated their invariants.
   */
  explicit AssetWorkProfileSnapshot(AssetWorkProfileSnapshotInput input);

  AssetWorkProfileSnapshotInput _input;
};

struct AssetWorkProfileSnapshotCreationResult {
  std::optional<AssetWorkProfileSnapshot> snapshot;
  QString error;
};
