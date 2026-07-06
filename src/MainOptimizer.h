/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

#include "AnimationsOptimizer.h"
#include "AssetWorkPlan.h"
#include "BsaOptimizer.h"
#include "MeshesOptimizer.h"
#include "OptionsCAO.h"
#include "TexturesOptimizer.h"

/*!
 * \brief Coordinates all the subclasses in order to optimize BSAs, textures, meshes and animations
 */
class MainOptimizer final : public QObject
{
    Q_DECLARE_TR_FUNCTIONS(MainOptimizer)

public:
    explicit MainOptimizer(const OptionsCAO &optOptions);

    /*!
     * \brief Extracts one planned BSA archive work item.
     * \param workItem The archive extraction work item to execute.
     */
    void extractArchive(const ArchiveExtractionWorkItem &workItem);
    /*!
     * \brief Processes one planned loose asset work item.
     * \param workItem The classified loose asset work item to execute.
     */
    void processLooseAsset(const LooseAssetWorkItem &workItem);
    /*!
     * \brief Packs one planned archive target.
     * \param workItem The archive packing work item to execute.
     */
    void packArchive(const ArchivePackingWorkItem &workItem);

  private:
    void addLandscapeTextures();
    void addHeadparts();

    void processNif(const QString &file);
    void processTexture(const QString &file, const TexturesOptimizer::TextureType &type);
    void processHkx(const QString &file);

    const OptionsCAO& _optOptions;

    BSAOptimizer _bsaOpt;
    MeshesOptimizer _meshesOpt;
    AnimationsOptimizer _animOpt;
    TexturesOptimizer _texturesOpt;
};
