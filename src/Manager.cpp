/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
#include "Manager.h"

#include "BsaOptimizer.h"
#include "MainOptimizer.h"
#include "Run/AssetRun.h"
#include "Run/TemporaryArtifactRegistry.h"

#include <algorithm>
#include <filesystem>
#include <vector>

namespace {
/// Keeps ownership locks alive through terminal cleanup, including exception unwinding.
class RunArtifacts final {
   public:
    cao::run::TemporaryArtifactRegistry registry;

    /// Performs and reports cleanup on exceptional exits without masking the original exception.
    ~RunArtifacts() noexcept {
        if (_finished) return;
        try {
            static_cast<void>(finish());
        } catch (...) {
            // Cleanup must not replace the exception that already stopped optimization.
            PLOG_ERROR
                << "Temporary staging cleanup could not finish; recovery will retry next run.";
        }
    }

    /// Collects cleanup failures before publishing the terminal result; returns true on success.
    bool finish() {
        const auto failures = registry.performSafetyCleanup();
        _finished = true;
        for (const auto& failure : failures)
            PLOG_ERROR << QString::fromStdString(failure.detail()) << ": "
                       << QString::fromStdWString(failure.path().wstring());
        return failures.empty();
    }

   private:
    bool _finished{};
};

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

void Manager::cancelProcess() { _stop.request_stop(); }

void Manager::readIgnoredMods() {
    QFile&& ignoredModsFile = Profiles::getFile("ignoredMods.txt");
    _ignoredMods = FilesystemOperations::readFile(ignoredModsFile);

    if (_ignoredMods.isEmpty()) {
        PLOG_WARNING << "ignoredMods.txt not found. All mods will be processed, including tools "
                        "such as Nemesis or "
                        "Bodyslide studio.";
    }
}

bool Manager::runOptimization() {
    PLOG_DEBUG << "Game: " << Profiles::currentProfile();
    PLOG_INFO << "Processing: " + _options.userPath;
    PLOG_INFO << "Beginning...";

    MainOptimizer optimizer(_options);
    BSAOptimizer bsaOptimizer;
    std::vector<std::filesystem::path> roots;
    roots.reserve(static_cast<std::size_t>(_modsToProcess.size()));
    for (const auto& mod : _modsToProcess)
        roots.push_back(std::filesystem::canonical(std::filesystem::path(mod.toStdWString())));

    RunArtifacts artifacts;
    if (_routingPolicy.executionMode() == cao::routing::ExecutionMode::Apply) {
        for (const auto& root : roots) {
            if (const auto failure = artifacts.registry.prepareRoot(root, _stop.get_token())) {
                PLOG_ERROR << QString::fromStdString(failure->detail()) << ": "
                           << QString::fromStdWString(failure->path().wstring());
                static_cast<void>(artifacts.finish());
                emit end();
                return false;
            }
        }
    }

    const cao::run::AssetRun assetRun(_routingPolicy);
    std::size_t failedAssets = 0;
    auto lastLooseProgress = QDateTime::currentDateTime();
    const auto result = assetRun.execute(
        roots,
        cao::run::AssetRunAdapters{
            [&](const cao::routing::RoutedAsset& archive) {
                bsaOptimizer.extract(QString::fromStdWString(archive.executionPath().wstring()),
                                     _options.bBsaDeleteBackup);
            },
            {},
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
            [&] { return _stop.stop_requested(); },
            [&] {
                _numberCompletedFiles = 0;
                printProgress(_modsToProcess.size(), "Packing BSAs");

                // Packing BSAs. The compiled policy, not the raw option, is the run authority:
                // it already rejected Archive creation the selected profile does not support.
                if (_routingPolicy.requests(cao::routing::RequestedWork::ArchiveCreation))
                    for (const auto& folder : _modsToProcess) {
                        if (_stop.stop_requested()) return false;

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
                for (const auto& diagnostic : diagnostics.diagnostics()) {
                    PLOG_WARNING << QStringLiteral("%1: %2")
                                        .arg(QString::fromStdString(diagnostic.detail()))
                                        .arg(QString::fromStdWString(diagnostic.path().wstring()));
                }
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
                // A warning rather than an error: the run is intact and every other Asset was
                // processed. The author still needs telling, because the game will not read these
                // Archives either, so their contents are silently absent in-game.
                if (const auto nested = diagnostics.nestedArchiveCount(); nested != 0) {
                    PLOG_WARNING << QStringLiteral(
                                        "Ignored %1 Archives found inside another Archive, which "
                                        "the game does not read")
                                        .arg(nested);
                }
            },
            [&](const std::span<const cao::run::ArchiveCollision> collisions) {
                for (const auto& collision : collisions) {
                    PLOG_WARNING
                        << QStringLiteral(
                               "Archive collision: %1; winning Archive: %2; Loose Asset wins: %3")
                               .arg(QString::fromStdWString(collision.gamePath().wstring()))
                               .arg(QString::fromStdWString(collision.winningArchive().wstring()))
                               .arg(collision.looseAssetWins() ? QStringLiteral("yes")
                                                               : QStringLiteral("no"));
                    for (const auto& archive : collision.shadowedArchives())
                        PLOG_WARNING << QStringLiteral("Shadowed Archive: %1")
                                            .arg(QString::fromStdWString(archive.wstring()));
                }
            },
            [&](const cao::run::RunFailure& failure) {
                PLOG_ERROR << QStringLiteral("Archive discovery failed: %1: %2")
                                  .arg(QString::fromStdString(failure.detail()))
                                  .arg(QString::fromStdWString(failure.path().wstring()));
            },
            [&](const cao::routing::RoutedAsset& asset) {
                // Preserve failures for the Run Outcome while routed execution uses mutation
                // evidence to stop before another attempt or packing when continuation is unsafe.
                const auto root =
                    std::find_if(roots.begin(), roots.end(), [&](const auto& candidate) {
                        const auto relative = asset.executionPath().lexically_relative(candidate);
                        return !relative.empty() && *relative.begin() != ".." &&
                               !relative.is_absolute();
                    });
                if (root == roots.end())
                    throw std::runtime_error("Routed Asset has no selected Mod Root.");
                auto attempt = optimizer.process(asset, artifacts.registry, *root);
                if (!attempt.succeeded()) ++failedAssets;
                return attempt;
            }});

    const bool cleaned = artifacts.finish();
    if (result.cancelled() || !result.failures().empty()) return false;

    for (const auto& failure : result.executionFailures()) {
        if (!failure.safeToContinue()) {
            PLOG_ERROR << "Optimization Run stopped because an Asset mutation could not be "
                          "completed safely.";
            emit end();
            return false;
        }
    }

    if (failedAssets != 0) {
        PLOG_ERROR << QStringLiteral("Process completed with %1 failed Assets<br><br><br>")
                          .arg(failedAssets);
    } else if (!cleaned) {
        PLOG_ERROR << "Process completed with temporary staging cleanup failures<br><br><br>";
    } else {
        PLOG_INFO << "Process completed<br><br><br>";
    }
    emit end();
    return failedAssets == 0 && cleaned;
}
