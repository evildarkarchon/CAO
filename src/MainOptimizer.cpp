/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#include "MainOptimizer.h"

MainOptimizer::MainOptimizer(const AssetWorkExecutionPolicy &executionPolicy)
    : _executionPolicy(executionPolicy), _meshesOpt(_executionPolicy.mesh()),
      _texturesOpt(_executionPolicy.texture()) {}

/*!
 * \brief Quarantines a malformed Asset unless execution is a Dry Run.
 * \param path The Asset path that could not be processed.
 * \param dryRun Whether filesystem mutations are forbidden for this run.
 */
void handleBadFile(const QString &path, const bool dryRun) {
  // Quarantine is itself an Asset mutation, so Dry Run may only report it.
  if (dryRun) {
    PLOG_ERROR
        << QString("Dry Run left malformed Asset unchanged: %1").arg(path);
    return;
  }

  if (QFile::rename(path, path + ".caobad")) {
    PLOG_ERROR << QString("%1 was renamed to %2").arg(path, path + ".caobad");
  } else {
    PLOG_ERROR << QString("Please remove %1").arg(path);
  }
}

void MainOptimizer::extractArchive(const ArchiveExtractionWorkItem &workItem) {
  const QString &file = workItem.path;
  if (QFileInfo(file).isFile()) {
    if (_executionPolicy.dryRun()) {
      PLOG_INFO << file + " would be extracted from BSA.";
      return;
    }

    PLOG_INFO << "BSA found ! Extracting...(this may take a long time, do not "
                 "force close the program): " +
                     file;
    _bsaOpt.extract(file, _executionPolicy.archive().deleteBackup);
  }

  // TODO if BSA content optimization is added, route it through execution
  // policy rather than raw options.
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
    handleBadFile(workItem.path, _executionPolicy.dryRun());
  }
}

void MainOptimizer::packArchive(const ArchivePackingWorkItem &workItem) {
  const QString &folder = workItem.folder;
  if (!QDir(folder).exists())
    return;

  if (_executionPolicy.dryRun()) {
    PLOG_INFO << folder + " would be packed into BSA archives.";
    return;
  }

  PLOG_INFO << "Creating BSA...";
  _bsaOpt.packAll(folder, _executionPolicy.archive());
}

void MainOptimizer::processTexture(const QString &file,
                                   const TexturesOptimizer::TextureType &type) {
  if (!_texturesOpt.open(file, type)) {
    PLOG_ERROR << "Failed to open: " << file;
    handleBadFile(file, _executionPolicy.dryRun());
    return;
  }

  // Resizing
  std::optional<size_t> width;
  std::optional<size_t> height;

  if (_executionPolicy.texture().resizeByRatio) {
    if (_executionPolicy.texture().targetWidthRatio == 0 ||
        _executionPolicy.texture().targetHeightRatio == 0) {
      PLOG_ERROR << "Cannot resize texture by ratio because target ratios must "
                    "be greater than zero: " +
                        file;
      return;
    }

    width = _texturesOpt.getInfo().width /
            _executionPolicy.texture().targetWidthRatio;
    height = _texturesOpt.getInfo().height /
             _executionPolicy.texture().targetHeightRatio;
  } else if (_executionPolicy.texture().resizeBySize) {
    width = _executionPolicy.texture().targetWidth;
    height = _executionPolicy.texture().targetHeight;
  }

  if (_executionPolicy.dryRun())
    _texturesOpt.dryOptimize(width, height);
  else {
    if (!_texturesOpt.optimize(width, height)) {
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
  if (_executionPolicy.dryRun())
    PLOG_INFO << file + " would be converted to the appropriate format.";
  else
    _animOpt.convert(file);
}

void MainOptimizer::processNif(const QString &file, const MeshAssetRole role) {
  if (_executionPolicy.mesh().optimizationLevel == 0)
    return;

  if (_executionPolicy.dryRun())
    _meshesOpt.dryOptimize(file, role);
  else if (!_meshesOpt.optimize(file, role))
    handleBadFile(file, _executionPolicy.dryRun());
}
