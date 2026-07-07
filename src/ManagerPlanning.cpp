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
  return RequestedAssetWork{options.bBsaExtract, options.bBsaCreate,
                            options.iMeshesOptimizationLevel > 0,
                            texturesEnabledByOptions(options),
                            options.bAnimationsOptimization};
}
} // namespace

AssetWorkPlanRequest ManagerPlanning::createAssetWorkPlanRequest(
    const OptionsCAO &options, const QStringList &ignoredMods,
    const ProfilePlanningSnapshot &profile) {
  return AssetWorkPlanRequest{
      options.userPath, toAssetWorkMode(options.mode), ignoredMods,
      AssetWorkPolicy::resolve(requestedAssetWorkFromOptions(options),
                               profile)};
}
