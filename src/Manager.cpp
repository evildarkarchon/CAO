/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#include "Manager.h"
#include "ManagerPlanning.h"

namespace
{
QString currentBsaExtension()
{
    const auto u8BsaExt = btu::bsa::Settings::get(Profiles::bsaGame()).extension;
    const auto asciiBsaExt = btu::common::as_ascii(u8BsaExt);
    return QString::fromUtf8(asciiBsaExt.data(), static_cast<int>(asciiBsaExt.size()));
}

ProfilePlanningSnapshot currentProfilePlanningSnapshot()
{
    return ProfilePlanningSnapshot{Profiles::bsaEnabled(),
                                   Profiles::meshesEnabled(),
                                   Profiles::animationsEnabled(),
                                   Profiles::texturesEnabled(),
                                   Profiles::texturesConvertTga(),
                                   currentBsaExtension()};
}
}

Manager::Manager(const OptionsCAO& opt)
  : _options(opt)

{
    init();
}

void Manager::init()
{
    //Preparing logging
    initCustomLogger(Profiles::logPath(), _options.bDebugLog);

    PLOG_VERBOSE << "Checking settings...";
    const QString error = _options.isValid();
    if (!error.isEmpty())
    {
        PLOG_FATAL << error;
        throw std::runtime_error("Options are not valid." + error.toStdString());
    }

    readIgnoredMods();
}

void Manager::printProgress(const int &total, const QString &text = "Processing files")
{
#ifndef GUI
    QTextStream(stdout) << "PROGRESS:|" << text << " - %v/%m - %p%|" << _numberCompletedFiles << '|' << total << endl;
#endif
#ifdef GUI
    emit progressBarTextChanged(text + "- %v/%m - %p%", total, _numberCompletedFiles);
#endif
}

void Manager::cancelProcess()
{
    _isCancelled = true;
}

void Manager::readIgnoredMods()
{
    QFile &&ignoredModsFile = Profiles::getFile("ignoredMods.txt");
    _ignoredMods = FilesystemOperations::readFile(ignoredModsFile);

    if (_ignoredMods.isEmpty())
    {
        PLOG_WARNING << "ignoredMods.txt not found. All mods will be processed, including tools such as Nemesis or "
                        "Bodyslide studio.";
    }
}

AssetWorkPlanRequest Manager::createAssetWorkPlanRequest() const
{
    return ManagerPlanning::createAssetWorkPlanRequest(_options, _ignoredMods, currentProfilePlanningSnapshot());
}

void Manager::runOptimization()
{
    PLOG_DEBUG << "Game: " << Profiles::currentProfile();
    PLOG_INFO << "Processing: " + _options.userPath;
    PLOG_INFO << "Beginning...";

    MainOptimizer optimizer(_options);
    PLOG_INFO << "Listing files and directories...";
    const AssetWorkPlanner planner(createAssetWorkPlanRequest());
    const auto archivePlan = planner.planArchives();

    //Extracting BSAs
    _numberCompletedFiles = 0;
    for (const auto &archive : archivePlan.archivesToExtract) {
        if (_isCancelled)
            return;

        optimizer.extractArchive(archive);
        ++_numberCompletedFiles;
        printProgress(static_cast<int>(archivePlan.archivesToExtract.size()), "Extracting BSAs");
    }

    //Listing newly extracted files
    const auto loosePlan = planner.planLooseAssets(archivePlan.modsToProcess);

    _numberCompletedFiles = 0;
    printProgress(static_cast<int>(loosePlan.looseAssetsToOptimize.size()));

    //Using time in order to prevent printing progress too often
    QDateTime time1 = QDateTime::currentDateTime();
    QDateTime time2;
    for (const auto &asset : loosePlan.looseAssetsToOptimize)
    {
        if (_isCancelled)
            return;

        optimizer.processLooseAsset(asset);
        ++_numberCompletedFiles;
        if (_isCancelled)
            return;

        time2 = QDateTime::currentDateTime();
        if (time2 > time1.addMSecs(2000)) {
            printProgress(static_cast<int>(loosePlan.looseAssetsToOptimize.size()));
            time1 = time2;
        }
    }

    _numberCompletedFiles = 0;

    //Packing BSAs
    if (!archivePlan.archivesToPack.isEmpty())
        printProgress(static_cast<int>(archivePlan.archivesToPack.size()), "Packing BSAs");

    for (const auto &archive : archivePlan.archivesToPack)
    {
        if (_isCancelled)
            return;

        optimizer.packArchive(archive);
        ++_numberCompletedFiles;
        printProgress(static_cast<int>(archivePlan.archivesToPack.size()),
                      "Packing BSAs - Folder:  " + QFileInfo(archive.folder).fileName());
    }

    FilesystemOperations::deleteEmptyDirectories(_options.userPath);
    PLOG_INFO << "Process completed<br><br><br>";
    emit end();
}
