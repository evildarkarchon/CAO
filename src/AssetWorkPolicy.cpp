/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#include "AssetWorkPolicy.h"

#include <utility>

AssetWorkPolicy::AssetWorkPolicy(const bool extractArchives,
                                 const bool packArchives,
                                 const bool optimizeMeshes,
                                 const bool optimizeDdsTextures,
                                 const bool convertTgaTextures,
                                 const bool optimizeAnimations,
                                 QString archiveExtension)
    : _extractArchives(extractArchives)
    , _packArchives(packArchives)
    , _optimizeMeshes(optimizeMeshes)
    , _optimizeDdsTextures(optimizeDdsTextures)
    , _convertTgaTextures(convertTgaTextures)
    , _optimizeAnimations(optimizeAnimations)
    , _archiveExtension(std::move(archiveExtension))
{}

AssetWorkPolicy AssetWorkPolicy::resolve(const RequestedAssetWork &requested,
                                         const ProfilePlanningSnapshot &profile)
{
    const bool archiveWorkSupported = profile.bsaEnabled;
    const bool textureWorkSupported = requested.optimizeTextures && profile.texturesEnabled;

    return AssetWorkPolicy{requested.extractArchives && archiveWorkSupported && !profile.bsaExtension.isEmpty(),
                           requested.packArchives && archiveWorkSupported,
                           requested.optimizeMeshes && profile.meshesEnabled,
                           textureWorkSupported,
                           textureWorkSupported && profile.texturesConvertTga,
                           requested.optimizeAnimations && profile.animationsEnabled,
                           profile.bsaExtension};
}

bool AssetWorkPolicy::allowsArchiveExtractionFor(const QString &fileName) const
{
    return _extractArchives && fileName.endsWith(_archiveExtension, Qt::CaseInsensitive);
}

bool AssetWorkPolicy::allowsArchiveExtraction() const
{
    return _extractArchives;
}

bool AssetWorkPolicy::allowsArchivePacking() const
{
    return _packArchives;
}

bool AssetWorkPolicy::allowsDdsTextureOptimization() const
{
    return _optimizeDdsTextures;
}

bool AssetWorkPolicy::allowsTgaTextureConversion() const
{
    return _convertTgaTextures;
}

bool AssetWorkPolicy::allowsMeshOptimization() const
{
    return _optimizeMeshes;
}

bool AssetWorkPolicy::allowsAnimationOptimization() const
{
    return _optimizeAnimations;
}
