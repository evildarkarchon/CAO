/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

#include "AssetWorkPlan.h"
#include "OptionsCAO.h"

namespace ManagerPlanning
{
/*!
 * \brief Builds the asset-work planner request without constructing a Manager.
 * \param options The UI or CLI options for the Asset Work Plan.
 * \param ignoredMods Mod names read from the active profile that should be skipped in several-mod mode.
 * \param profile The already-captured profile capability snapshot for planning decisions.
 * \return The request consumed by AssetWorkPlanner.
 */
[[nodiscard]] AssetWorkPlanRequest createAssetWorkPlanRequest(const OptionsCAO &options,
                                                              const QStringList &ignoredMods,
                                                              const ProfilePlanningSnapshot &profile);
}
