/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#include "MainOptimizer.h"
#include "PluginsOperations.h"
#include "Profiles.h"
#include "TexturesOptimizer.h"

MainOptimizer::MainOptimizer(const OptionsCAO &optOptions)
    : _optOptions(optOptions),
      _meshesOpt(MeshesOptimizer(_optOptions.bMeshesHeadparts,
                                 optOptions.iMeshesOptimizationLevel,
                                 optOptions.bMeshesResave)) {}

void handleBadFile(const QString &path) {
  if (QFile::rename(path, path + ".caobad")) {
    PLOG_ERROR << QString("%1 was renamed to %2").arg(path, path + ".caobad");
  } else {
    PLOG_ERROR << QString("Please remove %1").arg(path);
  }
}

void MainOptimizer::extractArchive(const ArchiveExtractionWorkItem &workItem) {
  if (_optOptions.bDryRun)
    return; // TODO if "dry run" run dry run on the assets in the BSA

  const QString &file = workItem.path;
  if (_optOptions.bBsaExtract && QFileInfo(file).isFile()) {
    PLOG_INFO << "BSA found ! Extracting...(this may take a long time, do not "
                 "force close the program): " +
                     file;
    _bsaOpt.extract(file, _optOptions.bBsaDeleteBackup);
  }

  // TODO if(options.bBsaOptimizeAssets)
}

void MainOptimizer::processLooseAsset(const LooseAssetWorkItem &workItem,
                                      const ModAssetMetadata &metadata) {
  try {
    switch (workItem.kind) {
    case LooseAssetKind::TextureDds:
      processTexture(workItem.path, TexturesOptimizer::DDS);
      break;
    case LooseAssetKind::TextureTga:
      processTexture(workItem.path, TexturesOptimizer::TGA);
      break;
    case LooseAssetKind::Mesh:
      processNif(workItem.path, metadata.isHeadpartMesh(workItem.path)
                                    ? MeshAssetRole::Headpart
                                    : MeshAssetRole::Regular);
      break;
    case LooseAssetKind::Animation:
      processHkx(workItem.path);
      break;
    }
  } catch (const std::exception &e) {
    PLOG_ERROR << "Cannot process: " + workItem.path
               << "\nAn exception occurred: " << e.what();
    handleBadFile(workItem.path);
  }
}

void MainOptimizer::packArchive(const ArchivePackingWorkItem &workItem) {
  const QString &folder = workItem.folder;
  if (_optOptions.bBsaCreate && QDir(folder).exists()) {
    PLOG_INFO << "Creating BSA...";
    _bsaOpt.packAll(folder, _optOptions);
  }
}

void MainOptimizer::processTexture(const QString &file,
                                   const TexturesOptimizer::TextureType &type) {
  const bool processTextures =
      _optOptions.bTexturesMipmaps || _optOptions.bTexturesCompress ||
      _optOptions.bTexturesNecessary || _optOptions.bTexturesResizeSize ||
      _optOptions.bTexturesResizeRatio;
  if (!processTextures)
    return;

  if (!_texturesOpt.open(file, type)) {
    PLOG_ERROR << "Failed to open: " << file;
    handleBadFile(file);
    return;
  }

  // Resizing
  std::optional<size_t> width;
  std::optional<size_t> height;

  if (_optOptions.bTexturesResizeRatio) {
    width =
        _texturesOpt.getInfo().width / _optOptions.iTexturesTargetWidthRatio;
    height =
        _texturesOpt.getInfo().height / _optOptions.iTexturesTargetHeightRatio;
  } else if (_optOptions.bTexturesResizeSize) {
    width = _optOptions.iTexturesTargetWidth;
    height = _optOptions.iTexturesTargetHeight;
  }

  if (_optOptions.bDryRun)
    _texturesOpt.dryOptimize(_optOptions.bTexturesNecessary,
                             _optOptions.bTexturesCompress,
                             _optOptions.bTexturesMipmaps, width, height);
  else {
    if (!_texturesOpt.optimize(_optOptions.bTexturesNecessary,
                               _optOptions.bTexturesCompress,
                               _optOptions.bTexturesMipmaps, width, height)) {
      PLOG_ERROR << "Failed to optimize: " + file;
      return;
    }

    if (type == TexturesOptimizer::DDS && !_texturesOpt.modifiedCurrentTexture)
      return; // Not saving if there wasn't any change

    // Saving to file
    QString newName = file;
    if (type == TexturesOptimizer::TGA)
      newName = newName.chopped(4) + ".dds";
    if (!_texturesOpt.saveToFile(newName)) {
      PLOG_ERROR << "Failed to optimize: " + file;
    } else if (type == TexturesOptimizer::TGA)
      QFile(file).remove();
  }
}

void MainOptimizer::processHkx(const QString &file) {
  if (!_optOptions.bAnimationsOptimization)
    return;

  if (_optOptions.bAnimationsOptimization && _optOptions.bDryRun)
    PLOG_INFO << file + " would be converted to the appropriate format.";
  else if (_optOptions.bAnimationsOptimization)
    _animOpt.convert(file);
}

void MainOptimizer::processNif(const QString &file, const MeshAssetRole role) {
  if (_optOptions.iMeshesOptimizationLevel == 0)
    return;

  if (_optOptions.iMeshesOptimizationLevel >= 1 && _optOptions.bDryRun)
    _meshesOpt.dryOptimize(file, role);
  else if (_optOptions.iMeshesOptimizationLevel >= 1 && !_optOptions.bDryRun)
    if (!_meshesOpt.optimize(file, role))
      handleBadFile(file);
}
