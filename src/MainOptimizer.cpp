/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#include "MainOptimizer.h"
#include "MeshReferenceMaintenance.h"
#include "TexturesOptimizer.h"

namespace
{
/// Renames an unreadable optimizer input to a collision-safe path outside packable Asset extensions.
void handleBadFile(const QString &path)
{
    auto quarantinePath = path + ".caobad";
    for (quint64 suffix = 1; QFileInfo::exists(quarantinePath); ++suffix)
        quarantinePath = path + ".caobad." + QString::number(suffix);

    if (QFile::rename(path, quarantinePath)) {
        PLOG_ERROR << QString("%1 was renamed to %2").arg(path, quarantinePath);
    } else {
        PLOG_ERROR << QString("Please remove %1").arg(path);
    }
}
}

MainOptimizer::MainOptimizer(const OptionsCAO &optOptions)
    : _optOptions(optOptions)
    , _meshesOpt(
          MeshesOptimizer(_optOptions.bMeshesHeadparts, optOptions.iMeshesOptimizationLevel, optOptions.bMeshesResave))
    , _assetExecutor(*this)
{
    addHeadparts();
    addLandscapeTextures();
}

cao::execution::AssetExecutionResult MainOptimizer::process(
    const cao::routing::RoutedAsset &asset)
{
    const auto result = _assetExecutor.execute(asset);
    if (!result.succeeded()) {
        PLOG_ERROR << "Cannot process Routed Asset: "
                   << QString::fromStdWString(asset.executionPath().wstring())
                   << "\n" << result.message();

        // Quarantine mutates the effective tree, so Dry Run only reports the load failure.
        if (asset.executionMode() == cao::routing::ExecutionMode::Apply
            && result.failure() == cao::execution::AssetExecutionFailure::LoadFailed) {
            handleBadFile(QString::fromStdWString(asset.executionPath().wstring()));
        }
    }
    return result;
}

void MainOptimizer::addHeadparts()
{
    _meshesOpt.listHeadparts(_optOptions.userPath);
    if (_optOptions.mode == OptionsCAO::SeveralMods)
    {
        const QDir dir(_optOptions.userPath);
        for (const auto &directory : dir.entryList(QDir::Dirs | QDir::NoDotAndDotDot))
            _meshesOpt.listHeadparts(dir.filePath(directory));
    }
}

void MainOptimizer::addLandscapeTextures()
{
    _meshesOpt.listHeadparts(_optOptions.userPath);
    if (_optOptions.mode == OptionsCAO::SeveralMods)
    {
        const QDir dir(_optOptions.userPath);
        for (const auto &directory : dir.entryList(QDir::Dirs | QDir::NoDotAndDotDot))
            _meshesOpt.listHeadparts(dir.filePath(directory));
    }
}

bool MainOptimizer::loadTexture(const std::filesystem::path &path,
                                const cao::routing::TextureVariant variant)
{
    const auto type = variant == cao::routing::TextureVariant::Native
                          ? TexturesOptimizer::DDS
                          : TexturesOptimizer::TGA;
    return _texturesOpt.open(QString::fromStdWString(path.wstring()), type);
}

cao::execution::OperationResult MainOptimizer::optimizeTexture(
    const cao::routing::AssetOperations &operations,
    const cao::routing::ExecutionMode mode)
{
    const bool optimize = operations.contains(cao::routing::AssetOperation::Optimization);
    const bool convert = operations.contains(cao::routing::AssetOperation::Conversion);
    std::optional<size_t> width;
    std::optional<size_t> height;
    if (optimize && _optOptions.bTexturesResizeRatio) {
        width = _texturesOpt.getInfo().width / _optOptions.iTexturesTargetWidthRatio;
        height = _texturesOpt.getInfo().height / _optOptions.iTexturesTargetHeightRatio;
    } else if (optimize && _optOptions.bTexturesResizeSize) {
        width = _optOptions.iTexturesTargetWidth;
        height = _optOptions.iTexturesTargetHeight;
    }

    const bool necessary = convert || (optimize && _optOptions.bTexturesNecessary);
    const bool compress = optimize && _optOptions.bTexturesCompress;
    const bool mipmaps = optimize && _optOptions.bTexturesMipmaps;
    if (mode == cao::routing::ExecutionMode::DryRun) {
        _texturesOpt.dryOptimize(necessary, compress, mipmaps, width, height);
        return cao::execution::OperationResult::changed();
    }

    if (!_texturesOpt.optimize(necessary, compress, mipmaps, width, height))
        return cao::execution::OperationResult::failed("Failed to optimize Texture.");
    if (convert || _texturesOpt.modifiedCurrentTexture)
        return cao::execution::OperationResult::changed();
    return cao::execution::OperationResult::unchanged();
}

bool MainOptimizer::saveTexture(const std::filesystem::path &path)
{
    return _texturesOpt.saveToFile(QString::fromStdWString(path.wstring()));
}

bool MainOptimizer::removeTexture(const std::filesystem::path &path)
{
    return QFile(QString::fromStdWString(path.wstring())).remove();
}

bool MainOptimizer::loadMesh(const std::filesystem::path &path,
                             const cao::routing::MeshVariant variant)
{
    auto [loaded, mesh] = _meshesOpt.loadMesh(QString::fromStdWString(path.wstring()), variant);
    if (!loaded) {
        _loadedMesh.reset();
        return false;
    }

    _loadedMesh = std::make_unique<nifly::NifFile>(std::move(mesh));
    return true;
}

cao::execution::OperationResult MainOptimizer::optimizeMesh(
    const std::filesystem::path &path,
    const cao::routing::ExecutionMode mode)
{
    if (!_loadedMesh)
        return cao::execution::OperationResult::failed("No Mesh is loaded.");
    return _meshesOpt.optimize(*_loadedMesh,
                              QString::fromStdWString(path.wstring()),
                              mode);
}

cao::execution::OperationResult MainOptimizer::maintainMeshReferences(
    const cao::routing::ExecutionMode mode)
{
    if (!_loadedMesh)
        return cao::execution::OperationResult::failed("No Mesh is loaded.");

    if (mode == cao::routing::ExecutionMode::DryRun) {
        const bool wouldChange = cao::execution::hasReferencedTgaTexture(*_loadedMesh);
        PLOG_INFO_IF(wouldChange)
            << "Referenced TGA Texture names would be replaced with DDS.";
        return wouldChange ? cao::execution::OperationResult::changed()
                           : cao::execution::OperationResult::unchanged();
    }

    const bool changed = cao::execution::replaceReferencedTgaTextureNames(*_loadedMesh);
    PLOG_VERBOSE_IF(changed) << "Replaced referenced TGA Texture names with DDS.";
    return changed ? cao::execution::OperationResult::changed()
                   : cao::execution::OperationResult::unchanged();
}

bool MainOptimizer::saveMesh(const std::filesystem::path &path)
{
    return _loadedMesh
           && _meshesOpt.saveMesh(*_loadedMesh, QString::fromStdWString(path.wstring()));
}

cao::execution::OperationResult MainOptimizer::optimizeAnimation(
    const std::filesystem::path &path,
    const cao::routing::ExecutionMode mode)
{
    const auto executionPath = QString::fromStdWString(path.wstring());
    if (mode == cao::routing::ExecutionMode::DryRun) {
        PLOG_INFO << executionPath + " would be converted to the appropriate format.";
        return cao::execution::OperationResult::changed();
    }
    return _animOpt.convert(executionPath)
               ? cao::execution::OperationResult::changed()
               : cao::execution::OperationResult::failed("Failed to optimize Animation.");
}
