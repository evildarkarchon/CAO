/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

#include "AnimationsOptimizer.h"
#include "AssetWorkExecutionPolicy.h"
#include "AssetWorkPlan.h"
#include "BsaOptimizer.h"
#include "MeshesOptimizer.h"
#include "ModAssetMetadata.h"
#include "TexturesOptimizer.h"

/*!
 * \brief Coordinates all the subclasses in order to optimize BSAs, textures,
 * meshes and animations
 */
class MainOptimizer final : public QObject {
  Q_DECLARE_TR_FUNCTIONS(MainOptimizer)

public:
  /*!
   * \brief Creates an optimizer from resolved Asset Work Execution Policy.
   * \param executionPolicy The already-resolved rules used while carrying out
   * planned Asset Work Items.
   */
  explicit MainOptimizer(const AssetWorkExecutionPolicy &executionPolicy);

  /*!
   * \brief Extracts one planned BSA archive work item.
   * \param workItem The archive extraction work item to execute.
   */
  void extractArchive(const ArchiveExtractionWorkItem &workItem);
  /*!
   * \brief Processes one planned loose asset work item.
   * \param workItem The classified loose asset work item to execute.
   * \param metadata Metadata derived from the selected Mods and active Profile.
   */
  void processLooseAsset(const LooseAssetWorkItem &workItem,
                         const ModAssetMetadata &metadata);
  /*!
   * \brief Packs one planned archive target.
   * \param workItem The archive packing work item to execute.
   */
  void packArchive(const ArchivePackingWorkItem &workItem);

private:
  void processNif(const QString &file, MeshAssetRole role);
  void processTexture(const QString &file,
                      const TexturesOptimizer::TextureType &type);
  void processHkx(const QString &file);

  AssetWorkExecutionPolicy _executionPolicy;

  BSAOptimizer _bsaOpt;
  MeshesOptimizer _meshesOpt;
  AnimationsOptimizer _animOpt;
  TexturesOptimizer _texturesOpt;
};
