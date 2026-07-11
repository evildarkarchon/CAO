/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#include "AssetWorkProfileSnapshot.h"

#include <utility>

AssetWorkProfileSnapshot::AssetWorkProfileSnapshot(
    AssetWorkProfileSnapshotInput input)
    : _input(std::move(input)) {}

AssetWorkProfileSnapshotCreationResult
AssetWorkProfileSnapshot::create(AssetWorkProfileSnapshotInput input) {
  if (input.archivesEnabled && input.archiveSettings.extension.empty()) {
    return {.snapshot = std::nullopt,
            .error = "An archive-enabled Profile must define an archive "
                     "extension"};
  }

  if (input.texturesEnabled && input.textureFormat == DXGI_FORMAT_UNKNOWN) {
    return {.snapshot = std::nullopt,
            .error = "A texture-enabled Profile must define an output "
                     "format"};
  }

  return {.snapshot = AssetWorkProfileSnapshot(std::move(input)), .error = {}};
}
