/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

#include "AssetWorkPlan.h"

#include <QStringList>

/*!
 * \brief Computes normalized paths that one loose Asset transaction may write.
 * \param workItem The classified loose Asset Work Item.
 * \return Case-folded absolute paths suitable for conflict comparison.
 *
 * TGA conversion includes both its source and sibling DDS destination so it
 * cannot race with discovery of an existing DDS Asset on Windows.
 */
[[nodiscard]] QStringList
looseAssetWriteKeys(const LooseAssetWorkItem &workItem);

/*!
 * \brief Checks whether two loose Asset transactions may write the same path.
 * \param first Normalized write keys for the first transaction.
 * \param second Normalized write keys for the second transaction.
 * \return True when the transactions must be serialized.
 */
[[nodiscard]] bool looseAssetWriteKeysConflict(const QStringList &first,
                                               const QStringList &second);
