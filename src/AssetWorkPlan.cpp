/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#include "AssetWorkPlan.h"

#include <QDir>
#include <QDirIterator>

AssetWorkPlanner::AssetWorkPlanner(AssetWorkPlanRequest request)
    : _request(std::move(request))
{}

AssetWorkPlan AssetWorkPlanner::planArchives() const
{
    AssetWorkPlan plan;
    plan.modsToProcess = selectMods();

    if (shouldPlanBsaPacking()) {
        for (const auto &mod : plan.modsToProcess)
            plan.archivesToPack.push_back(ArchivePackingWorkItem{mod});
    }

    if (!shouldPlanBsaArchives())
        return plan;

    for (const auto &mod : plan.modsToProcess) {
        QDirIterator it(mod, QDirIterator::Subdirectories);
        while (it.hasNext()) {
            it.next();
            if (it.fileInfo().isDir())
                continue;

            if (it.fileName().endsWith(_request.profile.bsaExtension, Qt::CaseInsensitive))
                plan.archivesToExtract.push_back(ArchiveExtractionWorkItem{it.filePath()});
        }
    }

    return plan;
}

AssetWorkPlan AssetWorkPlanner::planLooseAssets(const QStringList &modsToProcess) const
{
    AssetWorkPlan plan;
    plan.modsToProcess = modsToProcess;

    for (const auto &mod : plan.modsToProcess) {
        QDirIterator it(mod, QDirIterator::Subdirectories);
        while (it.hasNext()) {
            it.next();
            if (it.fileInfo().isDir())
                continue;

            const auto kind = classifyLooseAsset(it.fileName());
            if (kind.has_value())
                plan.looseAssetsToOptimize.push_back(LooseAssetWorkItem{it.filePath(), kind.value()});
        }
    }

    return plan;
}

QStringList AssetWorkPlanner::selectMods() const
{
    QStringList mods;

    if (_request.mode == AssetWorkMode::SingleMod) {
        mods << _request.selectedPath;
        return mods;
    }

    const QDir dir(_request.selectedPath);
    for (const auto &subDir : dir.entryList(QDir::Dirs | QDir::NoDotAndDotDot)) {
        // Separators are empty directories used by Mod Organizer 2.
        if (!subDir.contains("separator", Qt::CaseInsensitive) && !isIgnoredMod(subDir))
            mods << dir.filePath(subDir);
    }

    return mods;
}

bool AssetWorkPlanner::isIgnoredMod(const QString &modName) const
{
    return _request.ignoredMods.contains(modName, Qt::CaseInsensitive);
}

bool AssetWorkPlanner::shouldPlanBsaArchives() const
{
    return _request.extractBsa && _request.profile.bsaEnabled && !_request.profile.bsaExtension.isEmpty();
}

bool AssetWorkPlanner::shouldPlanBsaPacking() const
{
    return _request.createBsa && _request.profile.bsaEnabled;
}

bool AssetWorkPlanner::shouldPlanMeshes() const
{
    return _request.optimizeMeshes && _request.profile.meshesEnabled;
}

bool AssetWorkPlanner::shouldPlanTextures() const
{
    return _request.optimizeTextures && _request.profile.texturesEnabled;
}

bool AssetWorkPlanner::shouldPlanAnimations() const
{
    return _request.optimizeAnimations && _request.profile.animationsEnabled;
}

std::optional<LooseAssetKind> AssetWorkPlanner::classifyLooseAsset(const QString &fileName) const
{
    if (shouldPlanTextures() && fileName.endsWith(".dds", Qt::CaseInsensitive))
        return LooseAssetKind::TextureDds;

    if (shouldPlanMeshes()
        && (fileName.endsWith(".nif", Qt::CaseInsensitive)
            || fileName.endsWith(".btr", Qt::CaseInsensitive)
            || fileName.endsWith(".bto", Qt::CaseInsensitive)))
        return LooseAssetKind::Mesh;

    if (shouldPlanTextures() && _request.profile.texturesConvertTga
        && fileName.endsWith(".tga", Qt::CaseInsensitive))
        return LooseAssetKind::TextureTga;

    if (shouldPlanAnimations() && fileName.endsWith(".hkx", Qt::CaseInsensitive))
        return LooseAssetKind::Animation;

    return std::nullopt;
}
