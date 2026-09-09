/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

#include "AssetExecution/AssetExecutor.h"
#include "AnimationsOptimizer.h"
#include "MeshesOptimizer.h"
#include "OptionsCAO.h"
#include "TexturesOptimizer.h"

/*!
 * \brief Coordinates all the subclasses in order to optimize BSAs, textures, meshes and animations
 */
class MainOptimizer final : public QObject, private cao::execution::AssetExecutionBackend {
    Q_DECLARE_TR_FUNCTIONS(MainOptimizer)

   public:
    explicit MainOptimizer(const OptionsCAO& optOptions);
    /// Borrows worker-owned options and gives each backend its own immutable profile facts.
    MainOptimizer(const OptionsCAO& optOptions, const OptimizerProfileSnapshot& profile);

    /// Executes one Routed Asset strictly from its carried path, identity, target, operations, and
    /// mode. Temporary ownership uses the configured Mod Root and is cleaned before returning;
    /// callers without a configured selection use the input's parent as an isolated root.
    [[nodiscard]] cao::execution::AssetExecutionResult process(
        const cao::routing::RoutedAsset& asset);

    /// Executes an Asset with the run's shared temporary ownership and its selected Mod Root.
    /// The caller retains the registry until all attempts and Safety Cleanup finish.
    [[nodiscard]] cao::execution::AssetExecutionResult process(
        const cao::routing::RoutedAsset& asset, cao::run::TemporaryArtifactRegistry& artifacts,
        const std::filesystem::path& modRoot);

   private:
    /// Records conversion failures and applies load-failure quarantine to one execution result.
    [[nodiscard]] cao::execution::AssetExecutionResult finishAttempt(
        const cao::routing::RoutedAsset& asset, cao::execution::AssetExecutionResult result);

    void addLandscapeTextures();
    void addHeadparts();

    /// Loads a Texture using the carried Variant rather than its execution-path extension.
    bool loadTexture(const std::filesystem::path& path,
                     cao::routing::TextureVariant variant) override;

    /// Applies or evaluates the independently carried Texture operations on the loaded Texture.
    cao::execution::OperationResult optimizeTexture(const cao::routing::AssetOperations& operations,
                                                    cao::routing::ExecutionMode mode) override;

    /// Saves the loaded Texture to the output path selected by Asset Executor.
    bool saveTexture(const std::filesystem::path& path) override;

    /// Removes a converted source Texture only after its DDS replacement is saved.
    bool removeTexture(const std::filesystem::path& path) override;

    /// Returns the last Texture adapter's service diagnostic, empty when none is available.
    std::string textureFailureDetail() const override;

    /// Loads a Mesh using the carried Standard or Terrain Variant.
    bool loadMesh(const std::filesystem::path& path, cao::routing::MeshVariant variant) override;

    /// Applies or evaluates ordinary optimization on the currently loaded Mesh.
    cao::execution::OperationResult optimizeMesh(const std::filesystem::path& path,
                                                 cao::routing::ExecutionMode mode) override;

    /// Applies or evaluates Mesh Reference Maintenance independently of ordinary optimization.
    cao::execution::OperationResult maintainMeshReferences(
        cao::routing::ExecutionMode mode) override;

    /// Saves the loaded Mesh once after all carried operations complete.
    bool saveMesh(const std::filesystem::path& path) override;

    /// Evaluates an Animation in Dry Run, or writes its conversion to registered staging in Apply.
    /// outputPath is empty for Dry Run; the Asset Executor owns commitment and cleanup in Apply.
    cao::execution::OperationResult optimizeAnimation(const std::filesystem::path& sourcePath,
                                                      const std::filesystem::path& outputPath,
                                                      cao::routing::ExecutionMode mode) override;

    const OptionsCAO& _optOptions;

    MeshesOptimizer _meshesOpt;
    AnimationsOptimizer _animOpt;
    TexturesOptimizer _texturesOpt;
    QString _textureFailureDetail;
    std::unique_ptr<nifly::NifFile> _loadedMesh;
    /// Normalized execution path of the currently loaded Mesh, so Mesh Reference Maintenance can
    /// tell a conversion failure in this Mesh's own Mod Root from an identically named one in a
    /// sibling Mod Root scanned by the same Several Mods run.
    QString _loadedMeshPath;
    /// Normalized execution paths of the convertible Textures whose conversion did not commit a
    /// usable DDS, so Mesh Reference Maintenance withholds only references those failures broke.
    QStringList _failedTextureConversions;
    cao::execution::AssetExecutor _assetExecutor;
};
