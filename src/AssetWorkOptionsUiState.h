/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

#include "AssetWorkOptionsDraft.h"

#include <QString>
#include <cstddef>

struct AssetWorkOptionsUiContext {
  bool bsaAvailable = false;
  bool meshesAvailable = false;
  bool animationsAvailable = false;
  bool texturesAvailable = false;
  bool advancedSettingsVisible = false;
  bool advancedSettingsEditable = false;
};

struct ArchiveOptionsUiState {
  bool tabEnabled = false;
  bool controlsEnabled = false;
  bool extract = false;
  bool create = false;
  bool deleteBackup = false;
  bool mergeIncompressible = true;
  bool mergeTextures = false;
  bool createDummies = true;
  bool compress = true;
  bool deleteSource = true;
};

struct TextureOptionsUiState {
  bool tabEnabled = false;
  bool enabled = false;
  bool necessary = true;
  bool compress = false;
  bool mipmaps = false;
  bool resizingEnabled = false;
  bool resizeBySize = false;
  size_t targetWidth = 2048;
  size_t targetHeight = 2048;
  bool resizeByRatio = false;
  uint targetWidthRatio = 1;
  uint targetHeightRatio = 1;
};

struct MeshOptionsUiState {
  bool tabEnabled = false;
  bool optimizationEnabled = false;
  int optimizationLevel = 0;
  bool mediumAndFullOptimizationEnabled = true;
  bool processHeadparts = true;
  bool resave = false;
};

struct AnimationOptionsUiState {
  bool tabEnabled = false;
  bool optimize = false;
};

struct AdvancedOptionsUiState {
  bool visible = false;
  bool editable = false;
};

struct AssetWorkOptionsUiState {
  bool dryRun = false;
  bool debugLog = false;
  AssetWorkMode mode = AssetWorkMode::SingleMod;
  QString userPath;
  ArchiveOptionsUiState archive;
  TextureOptionsUiState textures;
  MeshOptionsUiState meshes;
  AnimationOptionsUiState animations;
  AdvancedOptionsUiState advanced;
};

namespace AssetWorkOptionsUi {
/*!
 * \brief Presents an Asset Work Options draft as deterministic, widget-free
 * screen state.
 * \param options The raw adapter draft to present.
 * \param context Active Profile and user-interface context that affects which
 * options are available.
 * \return A screen-state value with Profile, dry-run, and mode rules applied.
 */
[[nodiscard]] AssetWorkOptionsUiState
present(const AssetWorkOptionsDraft &options,
        const AssetWorkOptionsUiContext &context);

/*!
 * \brief Captures an Asset Work Options draft from widget-free screen state.
 * \param state The current screen state after reducers and Profile
 * availability have been applied.
 * \param options The raw adapter draft to update in place.
 */
void capture(const AssetWorkOptionsUiState &state,
             AssetWorkOptionsDraft &options);

/*!
 * \brief Applies the dry-run reducer to screen state.
 * \param state The screen state to update.
 * \param dryRun Whether dry-run mode is active.
 */
void applyDryRun(AssetWorkOptionsUiState &state, bool dryRun);

/*!
 * \brief Applies the optimization mode reducer to screen state.
 * \param state The screen state to update.
 * \param mode The selected single-Mod or several-Mod optimization mode.
 */
void applyMode(AssetWorkOptionsUiState &state, AssetWorkMode mode);
} // namespace AssetWorkOptionsUi
