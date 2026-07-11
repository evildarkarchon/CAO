/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#include "AssetWorkOptions.h"

#include "AssetWorkOptionsDraft.h"

namespace {
bool isPowerOfTwo(const size_t value) {
  return value != 0 && (value & (value - 1)) == 0;
}

int normalizedMeshOptimizationLevel(const AssetWorkOptionsDraft &draft) {
  // Several-Mod execution intentionally limits mesh work to the necessary
  // path, regardless of whether the draft came from UI, CLI, or settings.
  if (draft.mode == AssetWorkMode::SeveralMods &&
      draft.iMeshesOptimizationLevel > 1) {
    return 1;
  }
  return draft.iMeshesOptimizationLevel;
}
} // namespace

AssetWorkOptions::AssetWorkOptions(const AssetWorkOptionsDraft &draft)
    : _extractArchives(draft.bBsaExtract), _packArchives(draft.bBsaCreate),
      _deleteArchiveBackup(draft.bBsaDeleteBackup),
      _mergeIncompressibleArchives(draft.bBsaMergeIncomp),
      _mergeTextureArchives(draft.bBsaMergeTexture),
      _createDummyPlugins(draft.bBsaCreateDummies),
      _compressArchives(draft.bBsaCompress),
      _deletePackedSource(draft.bBsaDeleteSource),
      _optimizeAnimations(draft.bAnimationsOptimization),
      _dryRun(draft.bDryRun),
      _meshOptimizationLevel(normalizedMeshOptimizationLevel(draft)),
      _processHeadparts(draft.bMeshesHeadparts),
      _resaveMeshes(draft.bMeshesResave),
      _necessaryTextureOptimization(draft.bTexturesNecessary),
      _compressTextures(draft.bTexturesCompress),
      _generateTextureMipmaps(draft.bTexturesMipmaps),
      _resizeTexturesBySize(draft.bTexturesResizeSize),
      _textureTargetWidth(draft.iTexturesTargetWidth),
      _textureTargetHeight(draft.iTexturesTargetHeight),
      _resizeTexturesByRatio(draft.bTexturesResizeRatio),
      _textureTargetWidthRatio(draft.iTexturesTargetWidthRatio),
      _textureTargetHeightRatio(draft.iTexturesTargetHeightRatio),
      _mode(draft.mode == AssetWorkMode::SeveralMods
                ? AssetWorkMode::SeveralMods
                : AssetWorkMode::SingleMod) {}

AssetWorkMode AssetWorkOptions::mode() const noexcept { return _mode; }

AssetWorkOptionsCreationResult
AssetWorkOptions::create(const AssetWorkOptionsDraft &draft) {
  if (draft.mode != AssetWorkMode::SingleMod &&
      draft.mode != AssetWorkMode::SeveralMods) {
    return {.options = std::nullopt,
            .error = "This Asset Work mode does not exist"};
  }

  if (draft.iMeshesOptimizationLevel < 0 ||
      draft.iMeshesOptimizationLevel > 3) {
    return {.options = std::nullopt,
            .error = "Mesh optimization level must be between 0 and 3"};
  }

  if (draft.bTexturesResizeSize && draft.bTexturesResizeRatio) {
    return {.options = std::nullopt,
            .error = "Choose either fixed-size or ratio texture resizing, "
                     "not both"};
  }

  if (draft.bTexturesResizeSize &&
      (!isPowerOfTwo(draft.iTexturesTargetWidth) ||
       !isPowerOfTwo(draft.iTexturesTargetHeight))) {
    return {.options = std::nullopt,
            .error = "Texture target dimensions must be powers of two"};
  }

  if (draft.bTexturesResizeRatio && (draft.iTexturesTargetWidthRatio == 0 ||
                                     draft.iTexturesTargetHeightRatio == 0)) {
    return {.options = std::nullopt,
            .error = "Texture target ratios must be greater than zero"};
  }

  return {.options = AssetWorkOptions(draft), .error = {}};
}
