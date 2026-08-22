/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#include "MainOptimizer.h"
#include "MeshReferenceMaintenance.h"
#include "Profiles.h"
#include "PluginsOperations.h"
#include "TexturesOptimizer.h"

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
    }
    return result;
}

void handleBadFile(const QString &path)
{
    if (QFile::rename(path, path + ".caobad")) {
        PLOG_ERROR << QString("%1 was renamed to %2").arg(path, path + ".caobad");
    } else {
        PLOG_ERROR << QString("Please remove %1").arg(path);
    }
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

void MainOptimizer::process(const QString &file)
{
    const auto u8BsaExt = btu::bsa::Settings::get(Profiles::bsaGame()).extension;
    const auto asciiBsaExt = btu::common::as_ascii(u8BsaExt);
    const auto bsaExt = QString::fromUtf8(asciiBsaExt.data(), static_cast<int>(asciiBsaExt.size()));
    const bool nif = file.endsWith(".nif", Qt::CaseInsensitive)
                     || file.endsWith(".btr", Qt::CaseInsensitive)
                     || file.endsWith(".bto", Qt::CaseInsensitive);
    try {
        if (file.endsWith(".dds", Qt::CaseInsensitive))
            processTexture(file, TexturesOptimizer::DDS);
        else if (nif)
            processNif(file);
        else if (file.endsWith(".tga", Qt::CaseInsensitive) && Profiles::texturesConvertTga())
            processTexture(file, TexturesOptimizer::TGA);
        else if (file.endsWith(bsaExt, Qt::CaseInsensitive))
            processBsa(file);
        else if (file.endsWith(".hkx", Qt::CaseInsensitive))
            processHkx(file);
        else
            PLOG_ERROR << "Cannot process: " + file;
    } catch (const std::exception &e) {
        PLOG_ERROR << "Cannot process: " + file
                   << "\nAn exception occurred: " << e.what();
        handleBadFile(file);
    }
}

void MainOptimizer::processBsa(const QString &file) const
{
    if (_optOptions.bDryRun)
        return; //TODO if "dry run" run dry run on the assets in the BSA

    if (_optOptions.bBsaExtract && QFileInfo(file).isFile())
    {
        PLOG_INFO << "BSA found ! Extracting...(this may take a long time, do not force close the program): " + file;
        _bsaOpt.extract(file, _optOptions.bBsaDeleteBackup);
    }

    //TODO if(options.bBsaOptimizeAssets)
}

void
MainOptimizer::packBsa(const QString& folder)
{
    if (_optOptions.bBsaCreate && QDir(folder).exists())
    {
        PLOG_INFO << "Creating BSA...";
        _bsaOpt.packAll(folder, _optOptions);
    }
}

void MainOptimizer::processTexture(const QString &file, const TexturesOptimizer::TextureType &type)
{
    const bool processTextures = _optOptions.bTexturesMipmaps || _optOptions.bTexturesCompress
                                 || _optOptions.bTexturesNecessary || _optOptions.bTexturesResizeSize
                                 || _optOptions.bTexturesResizeRatio;
    if (!processTextures)
        return;

    if (!_texturesOpt.open(file, type))
    {
        PLOG_ERROR << "Failed to open: " << file;
        handleBadFile(file);
        return;
    }

    //Resizing
    std::optional<size_t> width;
    std::optional<size_t> height;

    if (_optOptions.bTexturesResizeRatio)
    {
        width = _texturesOpt.getInfo().width / _optOptions.iTexturesTargetWidthRatio;
        height = _texturesOpt.getInfo().height / _optOptions.iTexturesTargetHeightRatio;
    }
    else if (_optOptions.bTexturesResizeSize)
    {
        width = _optOptions.iTexturesTargetWidth;
        height = _optOptions.iTexturesTargetHeight;
    }

    if (_optOptions.bDryRun)
        _texturesOpt.dryOptimize(_optOptions.bTexturesNecessary,
                                 _optOptions.bTexturesCompress,
                                 _optOptions.bTexturesMipmaps,
                                 width,
                                 height);
    else
    {
        if (!_texturesOpt.optimize(_optOptions.bTexturesNecessary,
                                   _optOptions.bTexturesCompress,
                                   _optOptions.bTexturesMipmaps,
                                   width,
                                   height))
        {
            PLOG_ERROR << "Failed to optimize: " + file;
            return;
        }

        if (type == TexturesOptimizer::DDS && !_texturesOpt.modifiedCurrentTexture)
            return; //Not saving if there wasn't any change

        //Saving to file
        QString newName = file;
        if (type == TexturesOptimizer::TGA)
            newName = newName.chopped(4) + ".dds";
        if (!_texturesOpt.saveToFile(newName))
        {
            PLOG_ERROR << "Failed to optimize: " + file;
        }
        else if (type == TexturesOptimizer::TGA)
            QFile(file).remove();
    }
}

void MainOptimizer::processHkx(const QString &file)
{
    if (!_optOptions.bAnimationsOptimization)
        return;

    if (_optOptions.bAnimationsOptimization && _optOptions.bDryRun)
        PLOG_INFO << file + " would be converted to the appropriate format.";
    else if (_optOptions.bAnimationsOptimization)
        static_cast<void>(_animOpt.convert(file));
}

void MainOptimizer::processNif(const QString &file)
{
    if (_optOptions.iMeshesOptimizationLevel == 0)
        return;

    if (_optOptions.iMeshesOptimizationLevel >= 1 && _optOptions.bDryRun)
        _meshesOpt.dryOptimize(file);
    else if (_optOptions.iMeshesOptimizationLevel >= 1 && !_optOptions.bDryRun)
        if (!_meshesOpt.optimize(file))
            handleBadFile(file);
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
