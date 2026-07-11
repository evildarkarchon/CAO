/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#include "AssetWorkPolicyResolver.h"

#include "btu/common/string.hpp"

#include <utility>

AssetWorkPolicyResolution::AssetWorkPolicyResolution(
    AssetWorkPolicy planning, AssetWorkExecutionPolicy execution,
    std::vector<AssetWorkPolicyNotice> notices)
    : _planning(std::move(planning)), _execution(std::move(execution)),
      _notices(std::move(notices)) {}

const AssetWorkPolicy &AssetWorkPolicyResolution::planning() const noexcept {
  return _planning;
}

const AssetWorkExecutionPolicy &
AssetWorkPolicyResolution::execution() const noexcept {
  return _execution;
}

const std::vector<AssetWorkPolicyNotice> &
AssetWorkPolicyResolution::notices() const noexcept {
  return _notices;
}

AssetWorkPolicyResolution
AssetWorkPolicyResolver::resolve(const AssetWorkOptions &options,
                                 const AssetWorkProfileSnapshot &profile) {
  const auto &profileFacts = profile._input;
  const bool textureWorkRequested =
      options._necessaryTextureOptimization || options._compressTextures ||
      options._generateTextureMipmaps || options._resizeTexturesBySize ||
      options._resizeTexturesByRatio;
  const bool textureWorkSupported =
      textureWorkRequested && profileFacts.texturesEnabled;
  const bool tgaConversionAllowed =
      textureWorkSupported && profileFacts.texturesConvertTga;
  std::vector<AssetWorkPolicyNotice> notices;
  if (options._extractArchives && !profileFacts.archivesEnabled) {
    notices.emplace_back(AssetWorkKind::ArchiveExtraction);
  }
  if (options._packArchives && !profileFacts.archivesEnabled) {
    notices.emplace_back(AssetWorkKind::ArchivePacking);
  }
  if (options._meshOptimizationLevel > 0 && !profileFacts.meshesEnabled) {
    notices.emplace_back(AssetWorkKind::MeshOptimization);
  }
  if (textureWorkRequested && !profileFacts.texturesEnabled) {
    notices.emplace_back(AssetWorkKind::TextureOptimization);
  }
  if (options._optimizeAnimations && !profileFacts.animationsEnabled) {
    notices.emplace_back(AssetWorkKind::AnimationOptimization);
  }

  const auto archiveExtensionBytes =
      btu::common::as_ascii(profileFacts.archiveSettings.extension);
  const QString archiveExtension =
      QString::fromUtf8(archiveExtensionBytes.data(),
                        static_cast<int>(archiveExtensionBytes.size()));

  AssetWorkPolicy planning{
      options._extractArchives && profileFacts.archivesEnabled,
      options._packArchives && profileFacts.archivesEnabled,
      options._meshOptimizationLevel > 0 && profileFacts.meshesEnabled,
      textureWorkSupported,
      tgaConversionAllowed,
      options._optimizeAnimations && profileFacts.animationsEnabled,
      archiveExtension};

  AssetWorkExecutionPolicy execution(
      options._dryRun,
      ArchiveExecutionPolicy{
          options._deleteArchiveBackup, options._mergeIncompressibleArchives,
          options._mergeTextureArchives, options._createDummyPlugins,
          options._compressArchives, options._deletePackedSource,
          profileFacts.archiveSettings, profileFacts.filesToNotPack},
      TextureExecutionPolicy{
          profileFacts.texturesEnabled && options._necessaryTextureOptimization,
          profileFacts.texturesEnabled && options._compressTextures,
          profileFacts.texturesEnabled && options._generateTextureMipmaps,
          profileFacts.texturesEnabled && options._resizeTexturesBySize,
          options._textureTargetWidth, options._textureTargetHeight,
          profileFacts.texturesEnabled && options._resizeTexturesByRatio,
          options._textureTargetWidthRatio, options._textureTargetHeightRatio,
          profileFacts.textureFormat, profileFacts.texturesCompressInterface,
          profileFacts.textureUnwantedFormats},
      MeshExecutionPolicy{
          options._processHeadparts, options._resaveMeshes,
          profileFacts.meshesEnabled ? options._meshOptimizationLevel : 0,
          profileFacts.meshFileVersion, profileFacts.meshStream,
          profileFacts.meshUser, tgaConversionAllowed});

  return AssetWorkPolicyResolution(std::move(planning), std::move(execution),
                                   std::move(notices));
}
