/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#include "AssetWorkPlanExecutor.h"
#include "FilesystemOperations.h"

#include <QDateTime>
#include <QFileInfo>

#include <utility>

AssetWorkPlanExecutor::AssetWorkPlanExecutor(AssetWorkPlanRequest request,
                                             const ModAssetMetadataProvider &metadataProvider,
                                             AssetWorkPlanExecutionAdapter &adapter)
    : _request(std::move(request))
    , _metadataProvider(metadataProvider)
    , _adapter(adapter)
{}

AssetWorkPlanExecutionResult AssetWorkPlanExecutor::execute(
    const AssetWorkPlanExecutionCallbacks &callbacks)
{
    const AssetWorkPlanner planner(_request);
    const auto archivePlan = planner.planArchives();

    int completedArchiveExtractions = 0;
    for (const auto &archive : archivePlan.archivesToExtract) {
        if (isCancelled(callbacks))
            return AssetWorkPlanExecutionResult::Cancelled;

        _adapter.extractArchive(archive);
        ++completedArchiveExtractions;
        reportProgress(callbacks,
                       AssetWorkPlanExecutionPhase::ArchiveExtraction,
                       completedArchiveExtractions,
                       static_cast<int>(archivePlan.archivesToExtract.size()));

        if (isCancelled(callbacks))
            return AssetWorkPlanExecutionResult::Cancelled;
    }

    if (isCancelled(callbacks))
        return AssetWorkPlanExecutionResult::Cancelled;

    const ModAssetMetadata metadata = _metadataProvider.buildForMods(archivePlan.modsToProcess);

    if (isCancelled(callbacks))
        return AssetWorkPlanExecutionResult::Cancelled;

    const auto loosePlan = planner.planLooseAssets(archivePlan.modsToProcess);
    reportProgress(callbacks,
                   AssetWorkPlanExecutionPhase::LooseAssetProcessing,
                   0,
                   static_cast<int>(loosePlan.looseAssetsToOptimize.size()));

    // Large Mods can contain many thousands of Assets, so preserve the existing
    // throttled loose Asset progress cadence instead of reporting every file.
    QDateTime lastLooseProgress = QDateTime::currentDateTime();
    int completedLooseAssets = 0;
    for (const auto &asset : loosePlan.looseAssetsToOptimize) {
        if (isCancelled(callbacks))
            return AssetWorkPlanExecutionResult::Cancelled;

        _adapter.processLooseAsset(asset, metadata);
        ++completedLooseAssets;

        if (isCancelled(callbacks))
            return AssetWorkPlanExecutionResult::Cancelled;

        const auto now = QDateTime::currentDateTime();
        if (now > lastLooseProgress.addMSecs(2000)) {
            reportProgress(callbacks,
                           AssetWorkPlanExecutionPhase::LooseAssetProcessing,
                           completedLooseAssets,
                           static_cast<int>(loosePlan.looseAssetsToOptimize.size()));
            lastLooseProgress = now;
        }
    }

    if (!archivePlan.archivesToPack.isEmpty()) {
        reportProgress(callbacks,
                       AssetWorkPlanExecutionPhase::ArchivePacking,
                       0,
                       static_cast<int>(archivePlan.archivesToPack.size()));
    }

    int completedArchivesToPack = 0;
    for (const auto &archive : archivePlan.archivesToPack) {
        if (isCancelled(callbacks))
            return AssetWorkPlanExecutionResult::Cancelled;

        _adapter.packArchive(archive);
        ++completedArchivesToPack;
        reportProgress(callbacks,
                       AssetWorkPlanExecutionPhase::ArchivePacking,
                       completedArchivesToPack,
                       static_cast<int>(archivePlan.archivesToPack.size()),
                       QFileInfo(archive.folder).fileName());
    }

    FilesystemOperations::deleteEmptyDirectories(_request.selectedPath);
    return AssetWorkPlanExecutionResult::Completed;
}

bool AssetWorkPlanExecutor::isCancelled(const AssetWorkPlanExecutionCallbacks &callbacks) const
{
    return callbacks.isCancelled && callbacks.isCancelled();
}

void AssetWorkPlanExecutor::reportProgress(const AssetWorkPlanExecutionCallbacks &callbacks,
                                           const AssetWorkPlanExecutionPhase phase,
                                           const int completed,
                                           const int total,
                                           const QString &currentLabel) const
{
    if (!callbacks.reportProgress)
        return;

    callbacks.reportProgress(AssetWorkPlanProgress{phase, completed, total, currentLabel});
}
