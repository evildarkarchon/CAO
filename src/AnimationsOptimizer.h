/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

#include "AnimationAssetTransaction.h"
#include "pch.h"

/*!
 * \brief The AnimationsOptimizer class will handle all operations related to
 * animations (hkx files)
 */

class AnimationsOptimizer final : public QObject {
public:
  /*!
   * \brief Creates the production complete animation transaction.
   * \param dryRun Whether conversion must report intent without mutation.
   */
  explicit AnimationsOptimizer(bool dryRun = false);

  /*!
   * \brief Completes one animation Asset transaction.
   * \param filePath The animation Asset to inspect and possibly convert.
   * \return Structured completion, unchanged, or operational failure details.
   */
  AssetTransactionResult convert(const QString &filePath);

private:
  std::unique_ptr<AnimationAssetTransaction> _transaction;
};
