/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#include "ManagerPlanning.h"

namespace {
AssetWorkMode toAssetWorkMode(const OptionsCAO::OptimizationMode mode) {
  switch (mode) {
  case OptionsCAO::SingleMod:
    return AssetWorkMode::SingleMod;
  case OptionsCAO::SeveralMods:
    return AssetWorkMode::SeveralMods;
  }

  return AssetWorkMode::SingleMod;
}

bool texturesEnabledByOptions(const OptionsCAO &options) {
  return options.bTexturesMipmaps || options.bTexturesCompress ||
         options.bTexturesNecessary || options.bTexturesResizeSize ||
         options.bTexturesResizeRatio;
}

RequestedAssetWork requestedAssetWorkFromOptions(const OptionsCAO &options) {
  return RequestedAssetWork{
      .extractArchives = options.bBsaExtract,
      .packArchives = options.bBsaCreate,
      .optimizeMeshes = options.iMeshesOptimizationLevel > 0,
      .optimizeTextures = texturesEnabledByOptions(options),
      .optimizeAnimations = options.bAnimationsOptimization};
}
} // namespace

AssetWorkPlanRequest ManagerPlanning::createAssetWorkPlanRequest(
    const OptionsCAO &options, const QStringList &ignoredMods,
    const ProfilePlanningSnapshot &profile) {
  return AssetWorkPlanRequest{
      .selectedPath = options.userPath,
      .mode = toAssetWorkMode(options.mode),
      .ignoredMods = ignoredMods,
      .policy = AssetWorkPolicy::resolve(requestedAssetWorkFromOptions(options),
                                         profile)};
}
