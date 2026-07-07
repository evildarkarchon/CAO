/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

#include <QString>

struct ProfilePlanningSnapshot
{
    bool bsaEnabled = false;
    bool meshesEnabled = false;
    bool animationsEnabled = false;
    bool texturesEnabled = false;
    bool texturesConvertTga = false;
    QString bsaExtension;
};

struct RequestedAssetWork
{
    bool extractArchives = false;
    bool packArchives = false;
    bool optimizeMeshes = false;
    bool optimizeTextures = false;
    bool optimizeAnimations = false;
};

class AssetWorkPolicy final
{
public:
    /*!
     * \brief Creates a policy that allows no Asset Work.
     *
     * The default policy is useful for value initialization and keeps
     * partially-initialized planning requests from accidentally enabling work.
     */
    AssetWorkPolicy() = default;

    /*!
     * \brief Resolves requested work against the selected Profile's capabilities.
     * \param requested The asset work requested by the user options.
     * \param profile The selected Profile capabilities relevant to Asset Work planning.
     * \return A policy exposing only the work that is both requested and supported.
     */
    [[nodiscard]] static AssetWorkPolicy resolve(const RequestedAssetWork &requested,
                                                 const ProfilePlanningSnapshot &profile);

    /*!
     * \brief Checks whether archive extraction is allowed at all.
     * \return True when archive extraction was requested, the selected Profile supports archives, and it has an archive extension.
     */
    [[nodiscard]] bool allowsArchiveExtraction() const;

    /*!
     * \brief Checks whether an archive should be extracted under this policy.
     * \param fileName The archive file name to compare against the Profile archive extension.
     * \return True when archive extraction is allowed and the file name matches the Profile extension.
     */
    [[nodiscard]] bool allowsArchiveExtractionFor(const QString &fileName) const;

    /*!
     * \brief Checks whether archive packing is allowed for selected Mods.
     * \return True when archive packing was requested and the selected Profile supports archives.
     */
    [[nodiscard]] bool allowsArchivePacking() const;

    /*!
     * \brief Checks whether DDS texture Asset Work is allowed.
     * \return True when texture work was requested and the selected Profile supports textures.
     */
    [[nodiscard]] bool allowsDdsTextureOptimization() const;

    /*!
     * \brief Checks whether TGA-to-DDS texture Asset Work is allowed.
     * \return True when texture work was requested and the selected Profile enables TGA conversion.
     */
    [[nodiscard]] bool allowsTgaTextureConversion() const;

    /*!
     * \brief Checks whether mesh Asset Work is allowed.
     * \return True when mesh work was requested and the selected Profile supports meshes.
     */
    [[nodiscard]] bool allowsMeshOptimization() const;

    /*!
     * \brief Checks whether animation Asset Work is allowed.
     * \return True when animation work was requested and the selected Profile supports animations.
     */
    [[nodiscard]] bool allowsAnimationOptimization() const;

private:
    AssetWorkPolicy(bool extractArchives,
                    bool packArchives,
                    bool optimizeMeshes,
                    bool optimizeDdsTextures,
                    bool convertTgaTextures,
                    bool optimizeAnimations,
                    QString archiveExtension);

    bool _extractArchives = false;
    bool _packArchives = false;
    bool _optimizeMeshes = false;
    bool _optimizeDdsTextures = false;
    bool _convertTgaTextures = false;
    bool _optimizeAnimations = false;
    QString _archiveExtension;
};
