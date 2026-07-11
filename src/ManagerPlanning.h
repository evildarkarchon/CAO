/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

#include "AssetWorkPlan.h"

namespace ManagerPlanning {
/*!
 * \brief Packages Manager-owned run context with an already-resolved policy.
 * \param selectedPath The selected Mod or parent path to process.
 * \param mode The validated Asset Work Mode.
 * \param ignoredMods Mod names excluded from Several-Mod work.
 * \param policy The planning policy from the combined resolver.
 * \return The request consumed by AssetWorkPlanner.
 */
[[nodiscard]] AssetWorkPlanRequest
createAssetWorkPlanRequest(QString selectedPath, AssetWorkMode mode,
                           QStringList ignoredMods, AssetWorkPolicy policy);
} // namespace ManagerPlanning
