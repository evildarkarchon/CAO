/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#include "MainOptimizer.h"
#include "MeshReferenceMaintenance.h"
#include "TexturesOptimizer.h"

#include <algorithm>
#include <string>

namespace {
/// Renames an unreadable optimizer input to a collision-safe path outside packable Asset
/// extensions.
void handleBadFile(const QString& path) {
    auto quarantinePath = path + ".caobad";
    for (quint64 suffix = 1; QFileInfo::exists(quarantinePath); ++suffix)
        quarantinePath = path + ".caobad." + QString::number(suffix);

    if (QFile::rename(path, quarantinePath)) {
        PLOG_ERROR << QString("%1 was renamed to %2").arg(path, quarantinePath);
    } else {
        PLOG_ERROR << QString("Please remove %1").arg(path);
    }
}

/// Folds a Texture path to the form used to compare Mesh references against execution paths: both
/// separator conventions become '/' and case is discarded. Mesh references carry Windows
/// separators regardless of the host, so this cannot defer to QDir::fromNativeSeparators.
QString normalizeTexturePath(const QString& path) {
    QString normalized = path;
    normalized.replace(QLatin1Char('\\'), QLatin1Char('/'));
    return normalized.toLower();
}

/// Reports whether a normalized execution path names the Asset a normalized Mesh reference points
/// at. Mesh references are relative to the game's Data directory while execution paths are rooted
/// in the scanned mod, so the reference has to match a whole trailing component sequence.
bool namesSameTexture(const QString& executionPath, const QString& reference) {
    if (reference.isEmpty() || !executionPath.endsWith(reference)) return false;
    return executionPath.size() == reference.size() ||
           executionPath.at(executionPath.size() - reference.size() - 1) == QLatin1Char('/');
}
}  // namespace

MainOptimizer::MainOptimizer(const OptionsCAO& optOptions)
    : _optOptions(optOptions),
      _meshesOpt(MeshesOptimizer(_optOptions.bMeshesHeadparts, optOptions.iMeshesOptimizationLevel,
                                 optOptions.bMeshesResave)),
      _assetExecutor(*this) {
    addHeadparts();
    addLandscapeTextures();
}

cao::execution::AssetExecutionResult MainOptimizer::process(
    const cao::routing::RoutedAsset& asset) {
    const auto result = _assetExecutor.execute(asset);
    if (!result.succeeded()) {
        PLOG_ERROR << "Cannot process Routed Asset: "
                   << QString::fromStdWString(asset.executionPath().wstring()) << "\n"
                   << result.message();

        // Mesh Reference Maintenance rewrites a referenced .tga name to .dds, so a failed
        // conversion would leave that reference pointing at a DDS that was never produced. The
        // failing Texture is recorded by identity rather than as a run-wide bit, because every
        // other TGA source in the same run was deleted once its DDS replacement was saved and its
        // references therefore still have to be rewritten. Asset Run always completes the Texture
        // target before the Mesh target, so the recorded set is definitive by the time any Mesh is
        // executed.
        if (asset.target() == cao::routing::OptimizerTarget::Texture &&
            asset.operations().contains(cao::routing::AssetOperation::Conversion)) {
            _failedTextureConversions.append(
                normalizeTexturePath(QString::fromStdWString(asset.executionPath().wstring())));
        }

        // Quarantine mutates the effective tree, so Dry Run only reports the load failure.
        if (asset.executionMode() == cao::routing::ExecutionMode::Apply &&
            result.failure() == cao::execution::AssetExecutionFailure::LoadFailed) {
            handleBadFile(QString::fromStdWString(asset.executionPath().wstring()));
        }
    }
    return result;
}

void MainOptimizer::addHeadparts() {
    _meshesOpt.listHeadparts(_optOptions.userPath);
    if (_optOptions.mode == OptionsCAO::SeveralMods) {
        const QDir dir(_optOptions.userPath);
        for (const auto& directory : dir.entryList(QDir::Dirs | QDir::NoDotAndDotDot))
            _meshesOpt.listHeadparts(dir.filePath(directory));
    }
}

void MainOptimizer::addLandscapeTextures() {
    _meshesOpt.listHeadparts(_optOptions.userPath);
    if (_optOptions.mode == OptionsCAO::SeveralMods) {
        const QDir dir(_optOptions.userPath);
        for (const auto& directory : dir.entryList(QDir::Dirs | QDir::NoDotAndDotDot))
            _meshesOpt.listHeadparts(dir.filePath(directory));
    }
}

bool MainOptimizer::loadTexture(const std::filesystem::path& path,
                                const cao::routing::TextureVariant variant) {
    const auto type = variant == cao::routing::TextureVariant::Native ? TexturesOptimizer::DDS
                                                                      : TexturesOptimizer::TGA;
    return _texturesOpt.open(QString::fromStdWString(path.wstring()), type);
}

cao::execution::OperationResult MainOptimizer::optimizeTexture(
    const cao::routing::AssetOperations& operations, const cao::routing::ExecutionMode mode) {
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

bool MainOptimizer::saveTexture(const std::filesystem::path& path) {
    return _texturesOpt.saveToFile(QString::fromStdWString(path.wstring()));
}

bool MainOptimizer::removeTexture(const std::filesystem::path& path) {
    return QFile(QString::fromStdWString(path.wstring())).remove();
}

bool MainOptimizer::loadMesh(const std::filesystem::path& path,
                             const cao::routing::MeshVariant variant) {
    auto [loaded, mesh] = _meshesOpt.loadMesh(QString::fromStdWString(path.wstring()), variant);
    if (!loaded) {
        _loadedMesh.reset();
        return false;
    }

    _loadedMesh = std::make_unique<nifly::NifFile>(std::move(mesh));
    return true;
}

cao::execution::OperationResult MainOptimizer::optimizeMesh(
    const std::filesystem::path& path, const cao::routing::ExecutionMode mode) {
    if (!_loadedMesh) return cao::execution::OperationResult::failed("No Mesh is loaded.");
    return _meshesOpt.optimize(*_loadedMesh, QString::fromStdWString(path.wstring()), mode);
}

cao::execution::OperationResult MainOptimizer::maintainMeshReferences(
    const cao::routing::ExecutionMode mode) {
    if (!_loadedMesh) return cao::execution::OperationResult::failed("No Mesh is loaded.");

    // A reference is withheld only when that specific Texture's own conversion failed, since only
    // then is the DDS the rewrite would name absent. Withholding never fails the Mesh: reporting
    // unchanged keeps any ordinary optimization on the same Mesh saved.
    bool withheldReference = false;
    const auto isEligible = [&](const std::string& reference) {
        const auto normalizedReference = normalizeTexturePath(QString::fromStdString(reference));
        const auto failed =
            std::any_of(_failedTextureConversions.cbegin(), _failedTextureConversions.cend(),
                        [&](const QString& failedTexture) {
                            return namesSameTexture(failedTexture, normalizedReference);
                        });
        withheldReference = withheldReference || failed;
        return !failed;
    };

    if (mode == cao::routing::ExecutionMode::DryRun) {
        const bool wouldChange = cao::execution::hasReferencedTgaTexture(*_loadedMesh, isEligible);
        // Detection stops at the first eligible reference, so this reports that at least one
        // rewrite is withheld rather than a complete count.
        PLOG_WARNING_IF(withheldReference)
            << "At least one referenced TGA Texture would keep its name because its own conversion "
               "failed during this run.";
        PLOG_INFO_IF(wouldChange) << "Referenced TGA Texture names would be replaced with DDS.";
        return wouldChange ? cao::execution::OperationResult::changed()
                           : cao::execution::OperationResult::unchanged();
    }

    const bool changed = cao::execution::replaceReferencedTgaTextureNames(*_loadedMesh, isEligible);
    PLOG_WARNING_IF(withheldReference)
        << "Kept a referenced TGA Texture name because its own conversion failed during this run.";
    PLOG_VERBOSE_IF(changed) << "Replaced referenced TGA Texture names with DDS.";
    return changed ? cao::execution::OperationResult::changed()
                   : cao::execution::OperationResult::unchanged();
}

bool MainOptimizer::saveMesh(const std::filesystem::path& path) {
    return _loadedMesh &&
           _meshesOpt.saveMesh(*_loadedMesh, QString::fromStdWString(path.wstring()));
}

cao::execution::OperationResult MainOptimizer::optimizeAnimation(
    const std::filesystem::path& path, const cao::routing::ExecutionMode mode) {
    const auto executionPath = QString::fromStdWString(path.wstring());
    if (mode == cao::routing::ExecutionMode::DryRun) {
        PLOG_INFO << executionPath + " would be converted to the appropriate format.";
        return cao::execution::OperationResult::changed();
    }
    return _animOpt.convert(executionPath)
               ? cao::execution::OperationResult::changed()
               : cao::execution::OperationResult::failed("Failed to optimize Animation.");
}
