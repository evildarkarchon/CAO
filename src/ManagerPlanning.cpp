/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#include "ManagerPlanning.h"

#include <utility>

AssetWorkPlanRequest ManagerPlanning::createAssetWorkPlanRequest(
    QString selectedPath, const AssetWorkMode mode, QStringList ignoredMods,
    AssetWorkPolicy policy) {
  return AssetWorkPlanRequest{std::move(selectedPath), mode,
                              std::move(ignoredMods), std::move(policy)};
}
