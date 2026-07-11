/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

#include "AssetWorkExecutionPolicy.h"
#include "AssetWorkOptions.h"
#include "AssetWorkPolicy.h"
#include "AssetWorkProfileSnapshot.h"

#include <vector>

enum class AssetWorkKind {
  ArchiveExtraction,
  ArchivePacking,
  MeshOptimization,
  TextureOptimization,
  AnimationOptimization
};

struct AssetWorkPolicyNotice {
  AssetWorkKind work = AssetWorkKind::TextureOptimization;

  bool operator==(const AssetWorkPolicyNotice &) const = default;
};

class AssetWorkPolicyResolution final {
public:
  [[nodiscard]] const AssetWorkPolicy &planning() const noexcept;
  [[nodiscard]] const AssetWorkExecutionPolicy &execution() const noexcept;
  [[nodiscard]] const std::vector<AssetWorkPolicyNotice> &
  notices() const noexcept;

private:
  friend class AssetWorkPolicyResolver;

  /*!
   * \brief Stores one resolver-produced pair and its presentation-neutral
   * notices.
   */
  AssetWorkPolicyResolution(AssetWorkPolicy planning,
                            AssetWorkExecutionPolicy execution,
                            std::vector<AssetWorkPolicyNotice> notices);

  AssetWorkPolicy _planning;
  AssetWorkExecutionPolicy _execution;
  std::vector<AssetWorkPolicyNotice> _notices;
};

class AssetWorkPolicyResolver final {
public:
  /*!
   * \brief Resolves planning and execution policy as one consistent result.
   * \param options Validated immutable Asset Work Options.
   * \param profile Validated immutable facts from the selected Profile.
   * \return Immutable sibling policies and structured notices. The result is
   * deterministic and performs no I/O.
   */
  [[nodiscard]] static AssetWorkPolicyResolution
  resolve(const AssetWorkOptions &options,
          const AssetWorkProfileSnapshot &profile);
};
