/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#include "Manager.h"

#include "BsaOptimizer.h"
#include "MainOptimizer.h"
#include "Run/AssetRun.h"

#include <filesystem>
#include <vector>

namespace {
/// Returns the stable domain label used in one aggregate skip log message.
QString skipReasonName(const cao::routing::SkipReason reason) {
    switch (reason) {
        case cao::routing::SkipReason::DisabledPhase:
            return QStringLiteral("DisabledPhase");
        case cao::routing::SkipReason::DisabledAssetKind:
            return QStringLiteral("DisabledAssetKind");
        case cao::routing::SkipReason::ExcludedAssetVariant:
            return QStringLiteral("ExcludedAssetVariant");
    }
    return QStringLiteral("UnknownSkipReason");
}
}  // namespace

Manager::Manager(const OptionsCAO& opt, cao::routing::RoutingPolicy routingPolicy)
    : _options(opt),
      _routingPolicy(std::move(routingPolicy))

{
    init();
}

void Manager::init() {
    PLOG_VERBOSE << "Checking settings...";
    const QString error = _options.isValid();
    if (!error.isEmpty()) {
        PLOG_FATAL << error;
        throw std::runtime_error("Options are not valid." + error.toStdString());
    }

    readIgnoredMods();

    PLOG_INFO << "Listing files and directories...";
    listDirectories();
}

void Manager::listDirectories() {
    _modsToProcess.clear();

    if (_options.mode == OptionsCAO::SingleMod)
        _modsToProcess << _options.userPath;

    else if (_options.mode == OptionsCAO::SeveralMods) {
        const QDir dir(_options.userPath);
        for (auto subDir : dir.entryList(QDir::Dirs | QDir::NoDotAndDotDot))
            if (!subDir.contains("separator") &&
                !_ignoredMods.contains(
                    subDir, Qt::CaseInsensitive))  // Separators are empty directories used by MO2
                _modsToProcess << dir.filePath(subDir);
    }
}

void Manager::printProgress(const int& total, const QString& text = "Processing files") {
#ifndef GUI
    QTextStream(stdout) << "PROGRESS:|" << text << " - %v/%m - %p%|" << _numberCompletedFiles << '|'
                        << total << endl;
#endif
#ifdef GUI
    emit progressBarTextChanged(text + "- %v/%m - %p%", total, _numberCompletedFiles);
#endif
}

void Manager::cancelProcess() { _isCancelled = true; }

void Manager::readIgnoredMods() {
    QFile&& ignoredModsFile = Profiles::getFile("ignoredMods.txt");
    _ignoredMods = FilesystemOperations::readFile(ignoredModsFile);

    if (_ignoredMods.isEmpty()) {
        PLOG_WARNING << "ignoredMods.txt not found. All mods will be processed, including tools "
                        "such as Nemesis or "
                        "Bodyslide studio.";
    }
}

void Manager::runOptimization() {
    PLOG_DEBUG << "Game: " << Profiles::currentProfile();
    PLOG_INFO << "Processing: " + _options.userPath;
    PLOG_INFO << "Beginning...";

    MainOptimizer optimizer(_options);
    BSAOptimizer bsaOptimizer;
    std::vector<std::filesystem::path> roots;
    roots.reserve(static_cast<std::size_t>(_modsToProcess.size()));
    for (const auto& mod : _modsToProcess) roots.emplace_back(mod.toStdWString());

    const cao::run::AssetRun assetRun(_routingPolicy);
    auto lastLooseProgress = QDateTime::currentDateTime();
    const auto result = assetRun.execute(
        roots,
        cao::run::AssetRunAdapters{
            [&](const cao::routing::RoutedAsset& archive) {
                bsaOptimizer.extract(QString::fromStdWString(archive.executionPath().wstring()),
                                     _options.bBsaDeleteBackup);
            },
            [&](const cao::routing::RoutedAsset& asset) {
                // MainOptimizer owns the outcome: it quarantines unreadable inputs and records
                // failed Texture conversions so the later Mesh phase withholds the dependent
                // reference rewrite. The run itself continues past a single failed Asset.
                static_cast<void>(optimizer.process(asset));
            },
            [&](const cao::run::AssetRunProgress& progress) {
                _numberCompletedFiles = static_cast<int>(progress.completed);
                const auto text =
                    progress.phase == cao::routing::RoutedAssetPhase::ArchiveExtraction
                        ? QStringLiteral("Extracting BSAs")
                        : QStringLiteral("Processing files");
                const auto now = QDateTime::currentDateTime();
                const bool shouldReport =
                    progress.phase == cao::routing::RoutedAssetPhase::ArchiveExtraction ||
                    progress.completed == progress.total || now > lastLooseProgress.addMSecs(2000);
                if (shouldReport) {
                    // Loose Asset progress is throttled because GUI signal delivery and CLI
                    // output are comparatively expensive.
                    printProgress(static_cast<int>(progress.total), text);
                    lastLooseProgress = now;
                }
            },
            [&] { return _isCancelled; },
            [&] {
                _numberCompletedFiles = 0;
                printProgress(_modsToProcess.size(), "Packing BSAs");

                // Packing BSAs. The compiled policy, not the raw option, is the run authority:
                // it already rejected Archive creation the selected profile does not support.
                if (_routingPolicy.requests(cao::routing::RequestedWork::ArchiveCreation))
                    for (const auto& folder : _modsToProcess) {
                        if (_isCancelled) return false;

                        if (QDir(folder).exists()) {
                            PLOG_INFO << "Creating BSA...";
                            bsaOptimizer.packAll(folder, _options);
                        }
                        ++_numberCompletedFiles;
                        printProgress(_modsToProcess.size(),
                                      "Packing BSAs - Folder:  " + QFileInfo(folder).fileName());
                    }

                FilesystemOperations::deleteEmptyDirectories(_options.userPath);
                return true;
            },
            [&](const cao::run::AssetRunDiagnostics& diagnostics) {
                for (const auto reason : {cao::routing::SkipReason::DisabledPhase,
                                          cao::routing::SkipReason::DisabledAssetKind,
                                          cao::routing::SkipReason::ExcludedAssetVariant}) {
                    const auto count = diagnostics.skippedAssetCount(reason);
                    if (count != 0) {
                        PLOG_INFO << QStringLiteral("Skipped %1 recognized Assets: %2")
                                         .arg(count)
                                         .arg(skipReasonName(reason));
                    }
                }
                for (const auto& path : diagnostics.unsupportedExplicitPaths()) {
                    PLOG_ERROR << "Cannot process: " + QString::fromStdWString(path.wstring());
                }
            }});

    if (result.cancelled()) return;

    PLOG_INFO << "Process completed<br><br><br>";
    emit end();
}
