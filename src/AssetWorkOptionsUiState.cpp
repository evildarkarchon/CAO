/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#include "AssetWorkOptionsUiState.h"

namespace {
bool anyTextureWorkSelected(const OptionsCAO &options) {
  return options.bTexturesNecessary || options.bTexturesCompress ||
         options.bTexturesMipmaps;
}

bool anyTextureResizeSelected(const OptionsCAO &options) {
  return options.bTexturesResizeSize || options.bTexturesResizeRatio;
}
} // namespace

AssetWorkOptionsUiState
AssetWorkOptionsUi::present(const OptionsCAO &options,
                            const AssetWorkOptionsUiContext &context) {
  AssetWorkOptionsUiState state;
  state.dryRun = options.bDryRun;
  state.debugLog = options.bDebugLog;
  state.mode = options.mode;
  state.userPath = options.userPath;

  state.archive.tabEnabled = context.bsaAvailable;
  state.archive.controlsEnabled = context.bsaAvailable;
  state.archive.extract = options.bBsaExtract;
  state.archive.create = options.bBsaCreate;
  state.archive.deleteBackup = options.bBsaDeleteBackup;
  state.archive.mergeIncompressible = options.bBsaMergeIncomp;
  state.archive.mergeTextures = options.bBsaMergeTexture;
  state.archive.createDummies = options.bBsaCreateDummies;
  state.archive.compress = options.bBsaCompress;
  state.archive.deleteSource = options.bBsaDeleteSource;

  state.textures.tabEnabled = context.texturesAvailable;
  state.textures.enabled = anyTextureWorkSelected(options);
  state.textures.necessary = options.bTexturesNecessary;
  state.textures.compress = options.bTexturesCompress;
  state.textures.mipmaps = options.bTexturesMipmaps;
  state.textures.resizingEnabled = anyTextureResizeSelected(options);
  state.textures.resizeBySize = options.bTexturesResizeSize;
  state.textures.targetWidth = options.iTexturesTargetWidth;
  state.textures.targetHeight = options.iTexturesTargetHeight;
  state.textures.resizeByRatio = options.bTexturesResizeRatio;
  state.textures.targetWidthRatio = options.iTexturesTargetWidthRatio;
  state.textures.targetHeightRatio = options.iTexturesTargetHeightRatio;

  state.meshes.tabEnabled = context.meshesAvailable;
  state.meshes.optimizationEnabled = options.iMeshesOptimizationLevel > 0;
  state.meshes.optimizationLevel = options.iMeshesOptimizationLevel;
  state.meshes.processHeadparts = options.bMeshesHeadparts;
  state.meshes.resave = options.bMeshesResave;

  state.animations.tabEnabled = context.animationsAvailable;
  state.animations.optimize = options.bAnimationsOptimization;

  state.advanced.visible = context.advancedSettingsVisible;
  state.advanced.editable = context.advancedSettingsEditable;

  applyDryRun(state, state.dryRun);
  applyMode(state, state.mode);
  return state;
}

void AssetWorkOptionsUi::capture(const AssetWorkOptionsUiState &state,
                                 OptionsCAO &options) {
  options.bDryRun = state.dryRun;
  options.bDebugLog = state.debugLog;
  options.mode = state.mode;
  options.userPath = state.userPath;

  const bool archiveActive =
      state.archive.tabEnabled && state.archive.controlsEnabled;
  options.bBsaExtract = archiveActive && state.archive.extract;
  options.bBsaCreate = archiveActive && state.archive.create;
  options.bBsaDeleteBackup = archiveActive && state.archive.deleteBackup;
  options.bBsaMergeIncomp = archiveActive && state.archive.mergeIncompressible;
  options.bBsaMergeTexture = archiveActive && state.archive.mergeTextures;
  options.bBsaCreateDummies = archiveActive && state.archive.createDummies;
  options.bBsaCompress = archiveActive && state.archive.compress;
  options.bBsaDeleteSource = archiveActive && state.archive.deleteSource;

  const bool textureWorkActive =
      state.textures.tabEnabled && state.textures.enabled;
  options.bTexturesNecessary = textureWorkActive && state.textures.necessary;
  options.bTexturesCompress = textureWorkActive && state.textures.compress;
  options.bTexturesMipmaps = textureWorkActive && state.textures.mipmaps;

  const bool textureResizeActive =
      state.textures.tabEnabled && state.textures.resizingEnabled;
  options.bTexturesResizeSize =
      textureResizeActive && state.textures.resizeBySize;
  options.iTexturesTargetWidth = state.textures.targetWidth;
  options.iTexturesTargetHeight = state.textures.targetHeight;
  options.bTexturesResizeRatio =
      textureResizeActive && state.textures.resizeByRatio;
  options.iTexturesTargetWidthRatio = state.textures.targetWidthRatio;
  options.iTexturesTargetHeightRatio = state.textures.targetHeightRatio;

  const bool meshWorkActive =
      state.meshes.tabEnabled && state.meshes.optimizationEnabled;
  options.iMeshesOptimizationLevel =
      meshWorkActive ? state.meshes.optimizationLevel : 0;
  options.bMeshesHeadparts =
      state.meshes.tabEnabled && state.meshes.processHeadparts;
  options.bMeshesResave = state.meshes.tabEnabled && state.meshes.resave;

  options.bAnimationsOptimization =
      state.animations.tabEnabled && state.animations.optimize;
}

void AssetWorkOptionsUi::applyDryRun(AssetWorkOptionsUiState &state,
                                     const bool dryRun) {
  state.dryRun = dryRun;
  state.archive.controlsEnabled = state.archive.tabEnabled && !dryRun;

  if (!dryRun)
    return;

  // Dry-run intentionally suppresses archive mutation options; subordinate
  // archive packing options can remain visible but inactive while disabled.
  state.archive.extract = false;
  state.archive.create = false;
  state.archive.deleteBackup = false;
}

void AssetWorkOptionsUi::applyMode(AssetWorkOptionsUiState &state,
                                   const OptionsCAO::OptimizationMode mode) {
  state.mode = mode;
  state.meshes.mediumAndFullOptimizationEnabled =
      mode != OptionsCAO::SeveralMods;

  if (mode == OptionsCAO::SeveralMods && state.meshes.optimizationEnabled &&
      state.meshes.optimizationLevel > 0) {
    // Several-Mod mode only allows the necessary mesh optimization path.
    state.meshes.optimizationLevel = 1;
    state.meshes.optimizationEnabled = true;
  }
}
