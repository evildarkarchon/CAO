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
/// extensions. Returns whether the rename committed a filesystem mutation.
bool handleBadFile(const QString& path) {
    auto quarantinePath = path + ".caobad";
    for (quint64 suffix = 1; QFileInfo::exists(quarantinePath); ++suffix)
        quarantinePath = path + ".caobad." + QString::number(suffix);

    if (QFile::rename(path, quarantinePath)) {
        PLOG_ERROR << QString("%1 was renamed to %2").arg(path, quarantinePath);
        return true;
    } else {
        PLOG_ERROR << QString("Please remove %1").arg(path);
        return false;
    }
}

/// Folds an Asset path to the form used to compare Mesh references against execution paths: both
/// separator conventions become '/' and case is discarded. Mesh references carry Windows
/// separators regardless of the host, so this cannot defer to QDir::fromNativeSeparators.
QString normalizeAssetPath(const QString& path) {
    QString normalized = path;
    normalized.replace(QLatin1Char('\\'), QLatin1Char('/'));
    return normalized.toLower();
}

/// Reports whether a normalized Texture execution path names the very Asset a normalized Mesh
/// reference points at, for a Mesh whose own normalized execution path is `meshPath`.
///
/// Mesh references are relative to the game's Data directory while execution paths are rooted in
/// the scanned mod, so the reference has to match a whole trailing component sequence. What
/// precedes that match is the Data directory the reference resolved against, and the referencing
/// Mesh has to live beneath it: Several Mods mode scans sibling Mod Roots that routinely hold
/// identically named Textures, so a suffix match alone would let one mod's failure withhold
/// another mod's reference to a Texture that converted and whose TGA source was then deleted.
bool namesSameTexture(const QString& executionPath, const QString& reference,
                      const QString& meshPath) {
    if (reference.isEmpty() || !executionPath.endsWith(reference)) return false;

    const auto dataRootSize = executionPath.size() - reference.size();
    if (dataRootSize != 0 && executionPath.at(dataRootSize - 1) != QLatin1Char('/')) return false;

    // Drop the separator the boundary check just consumed. An empty Data directory means the
    // reference matched the whole execution path, so both Assets are relative to the same
    // traversal origin and no further containment can be checked.
    const auto dataRoot = dataRootSize == 0 ? QString() : executionPath.left(dataRootSize - 1);
    return dataRoot.isEmpty() ||
           (meshPath.size() > dataRoot.size() && meshPath.startsWith(dataRoot) &&
            meshPath.at(dataRoot.size()) == QLatin1Char('/'));
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
    auto modRoot = asset.executionPath().parent_path();
    if (!_optOptions.userPath.isEmpty()) {
        modRoot =
            std::filesystem::absolute(std::filesystem::path(_optOptions.userPath.toStdWString()))
                .lexically_normal();
        if (_optOptions.mode == OptionsCAO::SeveralMods) {
            // Several Mods selects each immediate child, never a nested Texture directory.
            const auto relative = std::filesystem::absolute(asset.executionPath())
                                      .lexically_normal()
                                      .lexically_relative(modRoot);
            if (!relative.empty() && *relative.begin() != "..") modRoot /= *relative.begin();
        }
    }
    return finishAttempt(asset, _assetExecutor.execute(asset, modRoot));
}

cao::execution::AssetExecutionResult MainOptimizer::process(
    const cao::routing::RoutedAsset& asset, cao::run::TemporaryArtifactRegistry& artifacts,
    const std::filesystem::path& modRoot) {
    return finishAttempt(asset, _assetExecutor.execute(asset, artifacts, modRoot));
}

cao::execution::AssetExecutionResult MainOptimizer::finishAttempt(
    const cao::routing::RoutedAsset& asset, cao::execution::AssetExecutionResult result) {
    if (!result.succeeded()) {
        PLOG_ERROR << "Cannot process Routed Asset: "
                   << QString::fromStdWString(asset.executionPath().wstring()) << "\n"
                   << result.message();
        if (!result.serviceDetail().empty()) PLOG_ERROR << result.serviceDetail();

        // Mesh Reference Maintenance rewrites a referenced .tga name to .dds, so a failed
        // conversion that did not commit a usable DDS would leave that reference pointing at an
        // absent output. A source-removal failure after safe commit still supplies the DDS, so its
        // references must be rewritten even though the original TGA remains. The
        // failing Texture is recorded by identity rather than as a run-wide bit, because every
        // other TGA source in the same run was deleted once its DDS replacement was saved and its
        // references therefore still have to be rewritten. Routed Asset execution always completes
        // the Texture target before the Mesh target, so the recorded set is definitive by the time
        // any Mesh is executed.
        if (asset.target() == cao::routing::OptimizerTarget::Texture &&
            asset.operations().contains(cao::routing::AssetOperation::Conversion) &&
            !(result.failure() == cao::execution::AssetExecutionFailure::SourceRemovalFailed &&
              result.mutationState() == cao::execution::MutationState::Committed &&
              result.safeToContinue())) {
            _failedTextureConversions.append(
                normalizeAssetPath(QString::fromStdWString(asset.executionPath().wstring())));
        }

        // Quarantine mutates the effective tree, so Dry Run only reports the load failure.
        if (asset.executionMode() == cao::routing::ExecutionMode::Apply &&
            result.failure() == cao::execution::AssetExecutionFailure::LoadFailed) {
            const bool quarantined =
                handleBadFile(QString::fromStdWString(asset.executionPath().wstring()));
            if (quarantined && (asset.target() == cao::routing::OptimizerTarget::Texture ||
                                asset.target() == cao::routing::OptimizerTarget::Mesh)) {
                // Loading failed before staging existed, but this adapter's successful quarantine
                // is itself a committed mutation and must be retained in the attempt evidence.
                result = cao::execution::AssetExecutionResult::failed(
                    *result.failure(), result.message(), cao::execution::MutationState::Committed,
                    result.safeToContinue(), result.affectedPath(), result.operation(),
                    result.serviceDetail());
            }
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
    _textureFailureDetail.clear();
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
    return _texturesOpt.saveToFile(QString::fromStdWString(path.wstring()), &_textureFailureDetail);
}

bool MainOptimizer::removeTexture(const std::filesystem::path& path) {
    _textureFailureDetail.clear();
    QFile source(QString::fromStdWString(path.wstring()));
    if (source.remove()) return true;
    _textureFailureDetail = source.errorString();
    return false;
}

std::string MainOptimizer::textureFailureDetail() const {
    return _textureFailureDetail.toStdString();
}

bool MainOptimizer::loadMesh(const std::filesystem::path& path,
                             const cao::routing::MeshVariant variant) {
    auto [loaded, mesh] = _meshesOpt.loadMesh(QString::fromStdWString(path.wstring()), variant);
    if (!loaded) {
        _loadedMesh.reset();
        _loadedMeshPath.clear();
        return false;
    }

    _loadedMesh = std::make_unique<nifly::NifFile>(std::move(mesh));
    // Mesh Reference Maintenance is told only the execution mode, so the Mesh's own location has
    // to be captured here for it to decide which Mod Root a recorded conversion failure belongs
    // to.
    _loadedMeshPath = normalizeAssetPath(QString::fromStdWString(path.wstring()));
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
        const auto normalizedReference = normalizeAssetPath(QString::fromStdString(reference));
        const auto failed = std::any_of(
            _failedTextureConversions.cbegin(), _failedTextureConversions.cend(),
            [&](const QString& failedTexture) {
                return namesSameTexture(failedTexture, normalizedReference, _loadedMeshPath);
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
    const std::filesystem::path& sourcePath, const std::filesystem::path& outputPath,
    const cao::routing::ExecutionMode mode) {
    const auto executionPath = QString::fromStdWString(sourcePath.wstring());
    if (mode == cao::routing::ExecutionMode::DryRun) {
        PLOG_INFO << executionPath + " would be converted to the appropriate format.";
        return cao::execution::OperationResult::changed();
    }
    return _animOpt.convert(executionPath, QString::fromStdWString(outputPath.wstring()))
               ? cao::execution::OperationResult::changed()
               : cao::execution::OperationResult::failed("Failed to optimize Animation.");
}
