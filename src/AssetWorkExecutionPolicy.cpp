/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#include "AssetWorkExecutionPolicy.h"

#include "FilesystemOperations.h"
#include "OptionsCAO.h"
#include "Profiles.h"

#include "btu/common/string.hpp"

#include <utility>

namespace {
constexpr auto FilesToNotPackFile = "FilesToNotPack.txt";

btu::bsa::Settings currentArchiveSettings() {
  auto settings = btu::bsa::Settings::get(Profiles::bsaGame());
  if (Profiles::maxBsaUncompressedSize() > settings.max_size)
    settings.max_size =
        static_cast<std::uintmax_t>(Profiles::maxBsaUncompressedSize());
  return settings;
}

std::vector<std::u8string> currentFilesToNotPack() {
  QFile &&filesToNotPackFile = Profiles::getFile(FilesToNotPackFile);

  auto lines =
      FilesystemOperations::readFile(filesToNotPackFile, [](QString &line) {
        line = QDir::toNativeSeparators(line);
      });

  std::vector<std::u8string> filesToNotPack;
  filesToNotPack.reserve(static_cast<size_t>(lines.size()));
  for (auto &&line : lines)
    filesToNotPack.emplace_back(
        btu::common::as_utf8_string(std::move(line).toStdString()));

  if (filesToNotPack.empty()) {
    PLOG_ERROR << "FilesToNotPack.txt not found. This can cause a number of "
                  "issues. For "
                  "example, for Skyrim, "
                  "animations will be packed to BSA, preventing them from "
                  "being detected "
                  "by FNIS and Nemesis.";
  }

  return filesToNotPack;
}

ProfileExecutionSnapshot currentProfileExecutionSnapshot() {
  return ProfileExecutionSnapshot{
      currentArchiveSettings(),
      currentFilesToNotPack(),
      Profiles::meshesFileVersion(),
      Profiles::meshesStream(),
      Profiles::meshesUser(),
      Profiles::texturesFormat(),
      Profiles::texturesCompressInterface(),
      Profiles::texturesUnwantedFormats(),
      Profiles::texturesConvertTga()};
}
} // namespace

AssetWorkExecutionPolicy
AssetWorkExecutionPolicy::resolve(const OptionsCAO &options) {
  return resolve(options, currentProfileExecutionSnapshot());
}

AssetWorkExecutionPolicy AssetWorkExecutionPolicy::resolve(
    const OptionsCAO &options, const ProfileExecutionSnapshot &profile) {
  return AssetWorkExecutionPolicy{
      options.bDryRun,
      ArchiveExecutionPolicy{options.bBsaDeleteBackup,
                             options.bBsaMergeIncomp,
                             options.bBsaMergeTexture,
                             options.bBsaCreateDummies,
                             options.bBsaCompress,
                             options.bBsaDeleteSource,
                             profile.archiveSettings,
                             profile.filesToNotPack},
      TextureExecutionPolicy{options.bTexturesNecessary,
                             options.bTexturesCompress,
                             options.bTexturesMipmaps,
                             options.bTexturesResizeSize,
                             options.iTexturesTargetWidth,
                             options.iTexturesTargetHeight,
                             options.bTexturesResizeRatio,
                             options.iTexturesTargetWidthRatio,
                             options.iTexturesTargetHeightRatio,
                             profile.textureFormat,
                             profile.texturesCompressInterface,
                             profile.textureUnwantedFormats},
      MeshExecutionPolicy{options.bMeshesHeadparts,
                          options.bMeshesResave,
                          options.iMeshesOptimizationLevel,
                          profile.meshFileVersion,
                          profile.meshStream,
                          profile.meshUser,
                          profile.texturesConvertTga}};
}
