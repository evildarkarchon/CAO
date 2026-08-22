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
class MainOptimizer final : public QObject, private cao::execution::AssetExecutionBackend
{
    Q_DECLARE_TR_FUNCTIONS(MainOptimizer)

public:
    explicit MainOptimizer(const OptionsCAO &optOptions);

    /// Executes one Routed Asset strictly from its carried path, identity, target, operations, and mode.
    [[nodiscard]] cao::execution::AssetExecutionResult process(
        const cao::routing::RoutedAsset &asset);

  private:
    void addLandscapeTextures();
    void addHeadparts();

    /// Loads a Texture using the carried Variant rather than its execution-path extension.
    bool loadTexture(const std::filesystem::path &path,
                     cao::routing::TextureVariant variant) override;

    /// Applies or evaluates the independently carried Texture operations on the loaded Texture.
    cao::execution::OperationResult optimizeTexture(
        const cao::routing::AssetOperations &operations,
        cao::routing::ExecutionMode mode) override;

    /// Saves the loaded Texture to the output path selected by Asset Executor.
    bool saveTexture(const std::filesystem::path &path) override;

    /// Removes a converted source Texture only after its DDS replacement is saved.
    bool removeTexture(const std::filesystem::path &path) override;

    /// Loads a Mesh using the carried Standard or Terrain Variant.
    bool loadMesh(const std::filesystem::path &path,
                  cao::routing::MeshVariant variant) override;

    /// Applies or evaluates ordinary optimization on the currently loaded Mesh.
    cao::execution::OperationResult optimizeMesh(const std::filesystem::path &path,
                                                 cao::routing::ExecutionMode mode) override;

    /// Applies or evaluates Mesh Reference Maintenance independently of ordinary optimization.
    cao::execution::OperationResult maintainMeshReferences(
        cao::routing::ExecutionMode mode) override;

    /// Saves the loaded Mesh once after all carried operations complete.
    bool saveMesh(const std::filesystem::path &path) override;

    /// Applies or evaluates Animation optimization according to the carried execution mode.
    cao::execution::OperationResult optimizeAnimation(
        const std::filesystem::path &path,
        cao::routing::ExecutionMode mode) override;

    const OptionsCAO& _optOptions;

    MeshesOptimizer _meshesOpt;
    AnimationsOptimizer _animOpt;
    TexturesOptimizer _texturesOpt;
    std::unique_ptr<nifly::NifFile> _loadedMesh;
    cao::execution::AssetExecutor _assetExecutor;
};
