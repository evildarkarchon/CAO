/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#include "MeshesOptimizer.h"
#include "FilesystemOperations.h"

#include <QFile>
#include <QFileInfo>
#include <QSaveFile>
#include <QTemporaryFile>

using namespace nifly;

std::string to_string(const std::vector<std::string> &source) {
  std::string res = "[";
  bool first = true;
  for (const auto &s : source) {
    if (!first)
      res += ", ";
    res += s;
    first = false;
  }
  return res + "]";
}

std::string to_string(bool source) { return source ? "true" : "false"; }

MeshesOptimizer::MeshesOptimizer(MeshExecutionPolicy policy)
    : _policy(std::move(policy)) {}

AssetTransactionResult MeshesOptimizer::executeAsset(const QString &filePath,
                                                     const MeshAssetRole role,
                                                     const bool dryRun) {
  const QFileInfo source(filePath);
  if (!source.exists() || !source.isFile() || !source.isReadable()) {
    return {AssetTransactionStatus::OperationalFailure,
            {{AssetTransactionNoticeCode::OperationalFailure,
              filePath,
              {},
              "Mesh source is not readable"}}};
  }
  if (_policy.optimizationLevel == 0) {
    return {AssetTransactionStatus::Unchanged,
            {{AssetTransactionNoticeCode::UnchangedAsset,
              filePath,
              {},
              "Mesh optimization is disabled"}}};
  }

  auto loadAttempt = loadMesh(filePath);
  if (!std::get<0>(loadAttempt)) {
    return {AssetTransactionStatus::MalformedAsset,
            {{AssetTransactionNoticeCode::MalformedAsset,
              filePath,
              {},
              "Mesh data could not be decoded"}}};
  }

  if (dryRun) {
    auto nif = std::move(std::get<1>(loadAttempt));
    const ScanResult scanResult = scan(nif);
    if (scanResult == doNotProcess) {
      return {AssetTransactionStatus::Unchanged,
              {{AssetTransactionNoticeCode::UnchangedAsset,
                filePath,
                {},
                "Mesh is not eligible for optimization"}}};
    }
    const bool processedHeadpart = _policy.optimizationLevel >= 1 &&
                                   role == MeshAssetRole::Headpart &&
                                   _policy.processHeadparts;
    const bool modifiedMesh =
        _policy.resaveMeshes ||
        (_policy.optimizationLevel >= 1 && scanResult >= criticalIssue) ||
        _policy.optimizationLevel >= 2;
    const bool renamedReferencedTextures =
        _policy.renameTgaReferences && renameReferencedTexturesExtension(nif);
    if (!modifiedMesh && !renamedReferencedTextures && !processedHeadpart) {
      return {AssetTransactionStatus::Unchanged,
              {{AssetTransactionNoticeCode::UnchangedAsset,
                filePath,
                {},
                "No eligible mesh changes were found"}}};
    }
    dryOptimize(filePath, role);
    return {AssetTransactionStatus::Completed,
            {{AssetTransactionNoticeCode::IntendedAction, filePath, filePath,
              "Would apply eligible mesh work"}}};
  }

  const auto outcome = optimize(filePath, role);
  if (outcome == OptimizationOutcome::Failed) {
    return {AssetTransactionStatus::OperationalFailure,
            {{AssetTransactionNoticeCode::OperationalFailure, filePath,
              filePath, "Mesh transformation or commit failed"}}};
  }
  if (outcome == OptimizationOutcome::Unchanged) {
    return {AssetTransactionStatus::Unchanged,
            {{AssetTransactionNoticeCode::UnchangedAsset,
              filePath,
              {},
              "No eligible mesh changes were found"}}};
  }
  return {AssetTransactionStatus::Completed,
          {{AssetTransactionNoticeCode::CompletedAction, filePath, filePath,
            "Mesh transaction completed"}}};
}

ScanResult MeshesOptimizer::scan(NifFile &nif) const {
  if (!nif.IsValid())
    return doNotProcess;

  NiVersion version;
  version.SetFile(_policy.targetFileVersion);
  version.SetStream(_policy.targetStream);
  version.SetUser(_policy.targetUser);

  if (!nif.IsSSECompatible() || version.IsSK())
    return criticalIssue;
  else
    return good;
}

MeshesOptimizer::OptimizationOutcome
MeshesOptimizer::optimize(const QString &filepath, const MeshAssetRole role)
// Optimize the selected mesh
{
  auto [loadResult, nif] = loadMesh(filepath);
  if (!loadResult)
    return OptimizationOutcome::Failed;

  OptOptions options;
  options.targetVersion.SetFile(_policy.targetFileVersion);
  options.targetVersion.SetStream(_policy.targetStream);
  options.targetVersion.SetUser(_policy.targetUser);
  options.removeParallax = false;

  const ScanResult scanResult = scan(nif);

  auto print_res = [](nifly::OptResult res) {
    std::string str = "Details of mesh optimization:";
#define PRINT(x) str += "\n" #x ": " + to_string(x);
    PRINT(res.dupesRenamed);
    PRINT(res.shapesNormalsRemoved);
    PRINT(res.shapesParallaxRemoved);
    PRINT(res.shapesVColorsRemoved);
    PRINT(res.shapesPartTriangulated);
    PRINT(res.shapesTangentsAdded);
    PLOGV << str;
#undef PRINT
  };

  bool processedHeadpart = false;
  // Headparts have to get a special optimization
  if (_policy.optimizationLevel >= 1 && role == MeshAssetRole::Headpart) {
    if (_policy.processHeadparts) {
      options.headParts = true;
      PLOG_INFO << "Optimizing: " + filepath +
                       " as an headpart due to necessary optimization";
      print_res(nif.OptimizeFor(options));
      processedHeadpart = true;
    } else
      PLOG_VERBOSE << "Headpart mesh ignored: " + filepath;
  } else {
    switch (scanResult) {
    case doNotProcess:
      return OptimizationOutcome::Unchanged;
    case good:
    case lightIssue:
      if (_policy.optimizationLevel >= 3) {
        PLOG_INFO << "Optimizing: " + filepath + " due to full optimization";
        print_res(nif.OptimizeFor(options));
      }
      break;
    case criticalIssue:
      if (_policy.optimizationLevel >= 1) {
        PLOG_INFO << "Optimizing: " + filepath +
                         " due to necessary optimization";
        print_res(nif.OptimizeFor(options));
      }
      break;
    }
  }

  const auto modifiedMesh =
      _policy.resaveMeshes ||
      (_policy.optimizationLevel >= 1 && scanResult >= criticalIssue) ||
      _policy.optimizationLevel >= 2;

  // Renaming textures referenced in mesh if TGA were converted to DDS
  const auto renamedReferencedTextures =
      _policy.renameTgaReferences && renameReferencedTexturesExtension(nif);
  PLOG_VERBOSE_IF(renamedReferencedTextures)
      << "Renamed referenced textures from TGA to DDS in " + filepath;

  if (!modifiedMesh && !renamedReferencedTextures && !processedHeadpart)
    return OptimizationOutcome::Unchanged;
  if (!saveMesh(nif, filepath))
    return OptimizationOutcome::Failed;
  PLOG_VERBOSE << "Closing mesh: " + filepath;
  return OptimizationOutcome::Completed;
}

void MeshesOptimizer::dryOptimize(const QString &filepath,
                                  const MeshAssetRole role) const {
  auto [loadResult, nif] = loadMesh(filepath);
  if (!loadResult)
    return;

  const ScanResult scanResult = scan(nif);

  // Headparts have to get a special optimization
  if (_policy.optimizationLevel >= 1 && role == MeshAssetRole::Headpart) {
    if (_policy.processHeadparts)
      PLOG_INFO << filepath + " would be optimized as an headpart due to "
                              "necessary optimization";
    else
      PLOG_VERBOSE << "Headpart mesh ignored: " + filepath;
  } else {
    switch (scanResult) {
    case doNotProcess:
      return;
    case good:
    case lightIssue:
      if (_policy.optimizationLevel >= 3)
        PLOG_INFO << filepath + " would be optimized due to full optimization";

      else if (_policy.optimizationLevel >= 2) {
        PLOG_INFO << filepath +
                         " would be optimized due to medium optimization";
      }
      break;
    case criticalIssue:
      if (_policy.optimizationLevel >= 1)
        PLOG_INFO << filepath +
                         " would be optimized due to necessary optimization";
      break;
    }
  }
}

bool MeshesOptimizer::renameReferencedTexturesExtension(NifFile &file) {
  bool meshChanged = false;
  for (auto shape : file.GetShapes()) {
    for (auto tex : file.GetTexturePathRefs(shape)) {
      if (tex.get().empty())
        continue;

      QString qsTex = QString::fromStdString(tex.get());
      if (qsTex.contains(".tga", Qt::CaseInsensitive)) {
        qsTex.replace(".tga", ".dds", Qt::CaseInsensitive);
        tex.get() = qsTex.toStdString();
        meshChanged = true;
      }
    }
  }
  return meshChanged;
}

std::tuple<bool, NifFile>
MeshesOptimizer::loadMesh(const QString &filepath) const {
  PLOG_VERBOSE << "Loading mesh: " + filepath;

  NifLoadOptions loadOptions;
  loadOptions.isTerrain = (filepath.endsWith("btr", Qt::CaseInsensitive) ||
                           filepath.endsWith("bto", Qt::CaseInsensitive));

  NifFile nif;
  if (nif.Load(filepath.toStdU16String(), loadOptions)) {
    PLOG_ERROR << "Cannot load mesh: " + filepath;
    return std::make_tuple(false, std::move(nif));
  }
  return std::make_tuple(true, std::move(nif));
}

bool MeshesOptimizer::saveMesh(NifFile &nif, const QString &filepath) const {
  PLOG_VERBOSE << "Saving mesh: " + filepath;
  const QFileInfo destination(filepath);
  QTemporaryFile staged(destination.dir().filePath(".cao-mesh-XXXXXX.tmp"));
  staged.setAutoRemove(true);
  if (!staged.open()) {
    PLOG_ERROR << "Cannot stage mesh: " + filepath;
    return false;
  }
  const QString stagedPath = staged.fileName();
  staged.close();
  // nifly requires ownership of the output path, while QSaveFile owns the
  // eventual atomic replacement of the live Asset.
  QFile::remove(stagedPath);
  if (nif.Save(stagedPath.toStdU16String())) {
    PLOG_ERROR << "Cannot save mesh: " + filepath;
    return false;
  }

  NifFile validation;
  NifLoadOptions validationOptions;
  validationOptions.isTerrain = filepath.endsWith("btr", Qt::CaseInsensitive) ||
                                filepath.endsWith("bto", Qt::CaseInsensitive);
  if (validation.Load(stagedPath.toStdU16String(), validationOptions)) {
    PLOG_ERROR << "Cannot validate staged mesh: " + filepath;
    return false;
  }

  QFile stagedOutput(stagedPath);
  if (!stagedOutput.open(QIODevice::ReadOnly))
    return false;
  const QByteArray stagedBytes = stagedOutput.readAll();
  QSaveFile liveOutput(filepath);
  liveOutput.setDirectWriteFallback(false);
  if (!liveOutput.open(QIODevice::WriteOnly) ||
      liveOutput.write(stagedBytes) != stagedBytes.size() ||
      !liveOutput.commit()) {
    PLOG_ERROR << "Cannot atomically commit mesh: " + filepath;
    return false;
  }
  return true;
}
