/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#include "BsaOptimizer.h"
#include "OptionsCAO.h"
#include "PluginsOperations.h"
#include "Run/ArchiveFirstAssetDiscovery.h"
#include "Run/StagingPaths.h"

#ifdef _WIN32
#include <Windows.h>
#endif

namespace {
/// Publishes a source backup without replacing any directory entry, including dangling links.
/// Retries occupied names; other filesystem failures leave the source or published backup intact.
void backupExtractedArchive(const std::filesystem::path& source) {
    auto destination = source;
    for (;;) {
        destination += ".bak";
        std::error_code error;
#ifdef _WIN32
        // No REPLACE_EXISTING flag: a competing creator must never lose its backup.
        if (MoveFileExW(source.c_str(), destination.c_str(), MOVEFILE_WRITE_THROUGH)) return;
        error = std::error_code(static_cast<int>(GetLastError()), std::system_category());
#else
        // Same-directory hard-link publication is atomic and rejects occupied names.
        std::filesystem::create_hard_link(source, destination, error);
        if (!error) {
            if (!std::filesystem::remove(source))
                throw std::runtime_error("The backed-up source Archive could not be removed.");
            return;
        }
#endif
        std::error_code statusError;
        const auto status = std::filesystem::symlink_status(destination, statusError);
        if (!statusError && std::filesystem::exists(status)) continue;
        throw std::filesystem::filesystem_error("Could not back up extracted Archive", source,
                                                destination, error);
    }
}
}  // namespace

BSAOptimizer::BSAOptimizer() {
    // Reading filesToNotPack to add them to the list.
    // Done in the constructor since the file won't change at runtime.

    QFile&& filesToNotPackFile = Profiles::getFile("FilesToNotPack.txt");

    auto lines = FilesystemOperations::readFile(
        filesToNotPackFile, [](QString& line) { line = QDir::toNativeSeparators(line); });

    for (auto&& line : lines)
        filesToNotPack.emplace_back(btu::common::as_utf8_string(std::move(line).toStdString()));

    if (filesToNotPack.empty()) {
        PLOG_ERROR << "FilesToNotPack.txt not found. This can cause a number of issues. For "
                      "example, for Skyrim, "
                      "animations will be packed to BSA, preventing them from being detected "
                      "by FNIS and Nemesis.";
    }
}

btu::bsa::Settings getSettings() {
    auto sets = btu::bsa::Settings::get(Profiles::bsaGame());
    if (Profiles::maxBsaUncompressedSize() > sets.max_size)
        sets.max_size = Profiles::maxBsaUncompressedSize();
    return sets;
}

void BSAOptimizer::extract(QString bsaPath, const bool deleteBackup) const {
    if (!deleteBackup) bsaPath = backup(bsaPath);

    PLOG_VERBOSE << bsaPath;

    try {
        cao::run::extractArchiveNoOverwrite(bsaPath.toStdU16String(), deleteBackup);
    } catch (const std::exception& e) {
        PLOG_ERROR << e.what();
        PLOG_ERROR << "An error occured during the extraction of: " + bsaPath + '\n' +
                          "Please extract it manually. The BSA was not deleted.";
        return;
    }

    PLOG_INFO << "BSA successfully extracted: " + bsaPath;
}

cao::run::ArchiveExtractionResult BSAOptimizer::extract(
    const cao::run::ArchiveExtractionPlan& plan, const bool deleteBackup,
    cao::run::TemporaryArtifactRegistry& artifacts) const {
    auto result = cao::run::ArchiveExtractor(artifacts).extract(plan);
    if (!result.succeeded()) return result;

    try {
        // Keep the original name and bytes throughout extraction and merge so a failed attempt
        // remains recoverable. Never replace a pre-existing backup based only on matching size.
        if (deleteBackup) {
            if (!std::filesystem::remove(plan.archivePath))
                throw std::runtime_error("The extracted source Archive could not be removed.");
        } else {
            backupExtractedArchive(plan.archivePath);
        }
        result.mutation = cao::execution::MutationState::Committed;
    } catch (const std::exception& error) {
        result.failure = cao::run::ArchiveExtractionFailure::SourceCleanupFailed;
        result.safeToContinue = false;
        result.detail = error.what();
        try {
            // Existence alone cannot prove that the retained source is still usable. Reopen
            // its manifest after the failed mutation before allowing later phases to proceed.
            result.safeToContinue = result.mutation == cao::execution::MutationState::Committed &&
                                    btu::bsa::read_archive(plan.archivePath).has_value();
        } catch (...) {
            // A failed verification leaves continuation unsafe and preserves the cleanup error.
        }
        if (!result.safeToContinue)
            result.mutation = cao::execution::MutationState::PartialOrUnknown;
        return result;
    }
    PLOG_INFO << "BSA successfully extracted: "
              << QString::fromStdWString(plan.archivePath.wstring());
    return result;
}

void handle_errors(std::vector<std::pair<btu::Path, std::string>> errs) {
    if (errs.empty()) return;

    PLOG_WARNING << "The following files failed to be packed. They will be "
                    "renamed to *.caobad:\n";
    for (auto&& [file, err] : errs) {
        PLOG_WARNING << file.native() << " : " << err;
        std::filesystem::rename(file, file.u8string() + u8".caobad");
    }
}

void BSAOptimizer::packAll(const QString& folderPath, const OptionsCAO& options) const {
    using dir_it = std::filesystem::directory_iterator;

    PLOG_VERBOSE << "Packing all loose files into BSAs";

    const auto game = getSettings();
    const std::filesystem::path dir = folderPath.toStdU16String();

    auto plugins = btu::bsa::list_plugins(dir_it(dir), {}, game);
    btu::bsa::clean_dummy_plugins(plugins, game);

    auto bsas = btu::bsa::split(
        dir, game, [this](const btu::Path& dir, btu::fs::directory_entry const& fileinfo) {
            return btu::bsa::default_is_allowed_path(dir, fileinfo) && isAllowedFile(dir, fileinfo);
        });

    if (options.bBsaMergeIncomp || options.bBsaMergeTexture) {
        const auto msets = [&] {
            btu::bsa::MergeSettings sets = static_cast<btu::bsa::MergeSettings>(0);
            if (options.bBsaMergeIncomp) sets |= btu::bsa::MergeSettings::MergeIncompressible;
            if (options.bBsaMergeTexture) sets |= btu::bsa::MergeSettings::MergeTextures;
            return sets;
        }();
        btu::bsa::merge(bsas, msets);
    }

    const auto default_plug = btu::bsa::FilePath(dir, dir.filename().u8string(), u8"", u8".esp",
                                                 btu::bsa::FileTypes::Plugin);
    if (plugins.empty())  // Used to find BSA name
        plugins.emplace_back(default_plug);

    for (auto&& bsa : bsas) {
        try {
            const auto files = std::vector(bsa.begin(), bsa.end());
            auto name = btu::bsa::find_archive_name(plugins, game, bsa.get_type());
            bsa.set_out_path(std::move(name).full_path());

            const auto errs = btu::bsa::write(options.bBsaCompress, std::move(bsa), dir);
            handle_errors(std::move(errs));
            if (options.bBsaDeleteSource) {
                std::for_each(files.begin(), files.end(), [](auto&& p) {
                    try {
                        std::filesystem::remove(p);
                    } catch (const std::exception&) {
                        PLOG_ERROR << "Failed to remove packed file: " << p.native();
                    }
                });
            }

        } catch (const std::exception& e) {
            PLOG_ERROR << QString("An error occurred while packing BSAs: \n%2").arg(e.what());
        }
    }

    if (options.bBsaCreateDummies) {
        const auto archives = btu::bsa::list_archive(dir_it(dir), {}, game);
        btu::bsa::make_dummy_plugins(archives, game);
    }
}

QString BSAOptimizer::backup(const QString& bsaPath) const {
    QFile bsaBackupFile(bsaPath + ".bak");
    const QFile bsaFile(bsaPath);

    if (!bsaBackupFile.exists())

        while (bsaBackupFile.exists()) {
            if (bsaFile.size() == bsaBackupFile.size())
                QFile::remove(bsaBackupFile.fileName());
            else
                bsaBackupFile.setFileName(bsaBackupFile.fileName() + ".bak");
        }

    QFile::rename(bsaPath, bsaBackupFile.fileName());

    PLOG_VERBOSE << "Backuping BSA : " << bsaPath << " to " << bsaBackupFile.fileName();

    return bsaBackupFile.fileName();
}

bool BSAOptimizer::isAllowedFile(btu::Path const& dir,
                                 btu::fs::directory_entry const& fileinfo) const {
    // Packing and its source-deletion pass must never consume another lifecycle's temporary data.
    if (cao::run::hasStagingComponent(fileinfo.path().lexically_relative(dir))) return false;
    const auto& path = fileinfo.path().u8string();
    for (const auto& fileToNotPack : filesToNotPack) {
        if (btu::common::str_contain(path, fileToNotPack, false)) {
            PLOG_VERBOSE << btu::common::as_ascii(path)
                         << " ignored because of filesToNotPack. Rule: "
                         << btu::common::as_ascii(fileToNotPack);
            return false;
        }
    }

    return true;
}
