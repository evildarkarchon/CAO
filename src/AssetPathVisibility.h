/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

#include <QString>

namespace AssetPathVisibility {
inline constexpr auto ArchiveTransactionRootName = ".cao-transactions";

/*!
 * \brief Checks whether a path belongs to CAO's internal execution state.
 * \param path Absolute or relative filesystem path to classify.
 * \return True when an exact path component is the reserved Archive
 * Transaction root, case-insensitively.
 */
[[nodiscard]] bool isInternalPath(const QString &path) noexcept;
} // namespace AssetPathVisibility
