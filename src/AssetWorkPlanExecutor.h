/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#pragma once

#include "AssetWorkPlan.h"
#include "ModAssetMetadata.h"

#include <functional>

enum class AssetWorkPlanExecutionPhase
{
    ArchiveExtraction,
    LooseAssetProcessing,
    ArchivePacking
};

enum class AssetWorkPlanExecutionResult
{
    Completed,
    Cancelled
};

struct AssetWorkPlanProgress
{
    AssetWorkPlanExecutionPhase phase = AssetWorkPlanExecutionPhase::LooseAssetProcessing;
    int completed = 0;
    int total = 0;
    QString currentLabel;
};

struct AssetWorkPlanExecutionCallbacks
{
    std::function<void(const AssetWorkPlanProgress &progress)> reportProgress;
    std::function<bool()> isCancelled;
};

class AssetWorkPlanExecutionAdapter
{
public:
    virtual ~AssetWorkPlanExecutionAdapter() = default;

    /*!
     * \brief Executes one archive extraction Asset Work Item.
     * \param workItem The archive extraction work item from the Asset Work Plan.
     */
    virtual void extractArchive(const ArchiveExtractionWorkItem &workItem) = 0;
    /*!
     * \brief Executes one loose Asset Work Item.
     * \param workItem The classified loose Asset Work Item from Loose Asset Discovery.
     * \param metadata Metadata derived from the selected Mods and active Profile.
     */
    virtual void processLooseAsset(const LooseAssetWorkItem &workItem,
                                   const ModAssetMetadata &metadata) = 0;
    /*!
     * \brief Executes one archive packing Asset Work Item.
     * \param workItem The archive packing work item from the Asset Work Plan.
     */
    virtual void packArchive(const ArchivePackingWorkItem &workItem) = 0;
};

class AssetWorkPlanExecutor final
{
public:
    /*!
     * \brief Creates an executor for one Asset Work Plan request.
     * \param request The selected Mod or Mods and profile/options snapshot used for planning.
     * \param metadataProvider Builds Mod Asset Metadata after archive extraction has completed.
     * \param adapter The adapter that carries out archive, loose Asset, and archive packing work.
     */
    AssetWorkPlanExecutor(AssetWorkPlanRequest request,
                          const ModAssetMetadataProvider &metadataProvider,
                          AssetWorkPlanExecutionAdapter &adapter);

    /*!
     * \brief Carries out Asset Work Plan Execution from planning through cleanup.
     * \param callbacks Optional progress and cancellation callbacks owned by the caller.
     * \return Completed when all phases and cleanup run; Cancelled when cancellation stops execution early.
     */
    [[nodiscard]] AssetWorkPlanExecutionResult execute(const AssetWorkPlanExecutionCallbacks &callbacks = {});

private:
    [[nodiscard]] bool isCancelled(const AssetWorkPlanExecutionCallbacks &callbacks) const;
    void reportProgress(const AssetWorkPlanExecutionCallbacks &callbacks,
                        AssetWorkPlanExecutionPhase phase,
                        int completed,
                        int total,
                        const QString &currentLabel = {}) const;

    AssetWorkPlanRequest _request;
    const ModAssetMetadataProvider &_metadataProvider;
    AssetWorkPlanExecutionAdapter &_adapter;
};
