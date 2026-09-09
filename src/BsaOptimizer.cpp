/* Copyright (C) 2019 G'k
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#include "BsaOptimizer.h"
#include "FilesystemOperations.h"
#include "OptionsCAO.h"
#include "PluginsOperations.h"
#include "Run/ArchiveFirstAssetDiscovery.h"
#include "Run/StagingPaths.h"

#include <array>
#include <fstream>
#include <set>

#ifdef _WIN32
#include <Windows.h>
#endif

namespace {
/// Publishes flushed same-volume bytes without replacing a competing entry. Records durable
/// mutation immediately, even if releasing the old staging name subsequently fails.
void publishArchiveFile(const std::filesystem::path& staged,
                        const std::filesystem::path& destination,
                        cao::execution::MutationState& mutation) {
#ifdef _WIN32
    const auto file = CreateFileW(staged.c_str(), GENERIC_WRITE, FILE_SHARE_READ, nullptr,
                                  OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (file == INVALID_HANDLE_VALUE)
        throw std::system_error(static_cast<int>(GetLastError()), std::system_category());
    const bool flushed = FlushFileBuffers(file) != 0;
    const auto error = GetLastError();
    CloseHandle(file);
    if (!flushed) throw std::system_error(static_cast<int>(error), std::system_category());
    // Names were reserved only in memory. A competing creator must not be overwritten,
    // and omitting COPY_ALLOWED ensures this commit never becomes a cross-volume copy.
    if (!MoveFileExW(staged.c_str(), destination.c_str(), MOVEFILE_WRITE_THROUGH))
        throw std::system_error(static_cast<int>(GetLastError()), std::system_category());
    mutation = cao::execution::MutationState::Committed;
#else
    std::filesystem::create_hard_link(staged, destination);
    mutation = cao::execution::MutationState::Committed;
    std::filesystem::remove(staged);
#endif
}

/// Proves retained source bytes can still be read after a failed deletion, without loading them
/// all into memory. A directory or substituted link is not usable retained source material.
bool readablePackedSource(const std::filesystem::path& source) {
    if (!std::filesystem::is_regular_file(std::filesystem::symlink_status(source))) return false;
    const auto expectedSize = std::filesystem::file_size(source);
    std::ifstream input(source, std::ios::binary);
    if (!input) return false;
    std::array<char, 8192> buffer{};
    std::uintmax_t bytesRead = 0;
    do {
        input.read(buffer.data(), buffer.size());
        bytesRead += static_cast<std::uintmax_t>(input.gcount());
    } while (input);
    // Some file buffers report OS read failures as EOF (for example Windows byte locks).
    // Require every expected byte before treating the retained file as usable recovery data.
    return input.eof() && !input.bad() && bytesRead == expectedSize;
}

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

cao::run::ArchiveFinalizationPlan BSAOptimizer::planFinalization(
    const std::span<const std::filesystem::path> roots, const OptionsCAO& options) const {
    namespace fs = std::filesystem;
    using namespace btu::bsa;
    cao::run::ArchiveFinalizationPlan plan;
    plan._settings = getSettings();
    plan._compress = options.bBsaCompress;
    plan._deleteSources = options.bBsaDeleteSource;
    plan._createDummies = options.bBsaCreateDummies;
    const auto& settings = plan._settings;
    std::set<fs::path> reserved;
    for (const auto& inputRoot : roots) {
        const auto root = fs::canonical(inputRoot);
        plan._roots.push_back(root);
        auto plugins = list_plugins(fs::directory_iterator(root), {}, settings);
        // Dummy cleanup is a mutation. Ignore their names for planning, but retain them until
        // all output attempts finish so cancellation cannot strand an existing Archive.
        if (settings.s_dummy_plugin) {
            std::erase_if(plugins, [&](const auto& plugin) {
                return fs::file_size(plugin.full_path()) == settings.s_dummy_plugin->size();
            });
        }
        std::sort(plugins.begin(), plugins.end());
        if (plugins.empty())
            plugins.emplace_back(root, root.filename().u8string(), u8"", u8".esp", FileTypes::Plugin);

        std::vector<fs::path> sources;
        for (auto it = fs::recursive_directory_iterator(root); it != fs::recursive_directory_iterator(); ++it) {
            const auto path = it->path();
            bool excluded = cao::run::hasStagingComponent(path.lexically_relative(root)) ||
                            fs::is_symlink(it->symlink_status());
#ifdef _WIN32
            const auto attributes = GetFileAttributesW(path.c_str());
            if (attributes == INVALID_FILE_ATTRIBUTES)
                throw std::runtime_error("Cannot inspect finalization input.");
            excluded = excluded || (attributes & FILE_ATTRIBUTE_REPARSE_POINT) != 0;
#endif
            // Do not descend into reserved staging or directory links just to filter their files.
            if (excluded) {
                it.disable_recursion_pending();
                continue;
            }
            if (fs::is_regular_file(it->symlink_status()) && default_is_allowed_path(root, *it) &&
                isAllowedFile(root, *it))
                sources.push_back(path);
        }
        std::sort(sources.begin(), sources.end());
        auto standard = ArchiveData(settings, ArchiveType::Standard);
        auto incompressible = ArchiveData(settings, ArchiveType::Incompressible);
        auto textures = ArchiveData(settings, ArchiveType::Textures);
        std::vector<ArchiveData> archives;
        for (const auto& source : sources) {
            const auto type = get_filetype(source, root, settings);
            if (type != FileTypes::Standard && type != FileTypes::Texture &&
                type != FileTypes::Incompressible)
                continue;
            auto& archive = type == FileTypes::Texture          ? textures
                            : type == FileTypes::Incompressible ? incompressible
                                                                : standard;
            if (!archive.add_file(source)) {
                const auto archiveType = archive.get_type();
                archives.push_back(std::move(archive));
                archive = ArchiveData(settings, archiveType);
                if (!archive.add_file(source))
                    throw std::runtime_error("An Asset exceeds the output Archive size limit.");
            }
        }
        // The library merge contract requires the three unfinished partitions in this order.
        archives.insert(archives.end(),
                        {std::move(standard), std::move(incompressible), std::move(textures)});
        auto mergeSettings = static_cast<MergeSettings>(0);
        if (options.bBsaMergeIncomp) mergeSettings |= MergeSettings::MergeIncompressible;
        if (options.bBsaMergeTexture) mergeSettings |= MergeSettings::MergeTextures;
        merge(archives, mergeSettings);
        for (auto& archive : archives) {
            const auto suffix = archive.get_type() == ArchiveType::Textures
                                    ? settings.texture_suffix.value_or(u8"")
                                    : settings.suffix.value_or(u8"");
            const auto available = [&](const FilePath& candidate) {
                const auto path = candidate.full_path();
                std::error_code error;
                const auto status = fs::symlink_status(path, error);
                if (error && error != std::errc::no_such_file_or_directory)
                    throw fs::filesystem_error("Cannot plan output Archive", path, error);
                return !fs::exists(status) && !reserved.contains(path);
            };
            std::optional<FilePath> selected;
            for (auto plugin : plugins) {
                plugin.ext = settings.extension;
                plugin.suffix = suffix;
                if (available(plugin)) {
                    selected = plugin;
                    break;
                }
            }
            if (!selected) {
                auto candidate = plugins.front();
                candidate.ext = settings.extension;
                candidate.suffix = suffix;
                for (std::uint32_t counter = 0; counter < 255; ++counter) {
                    candidate.counter = counter;
                    if (available(candidate)) {
                        selected = candidate;
                        break;
                    }
                }
            }
            if (!selected) throw std::runtime_error("No available output Archive name.");
            const auto destination = selected->full_path();
            reserved.insert(destination);
            std::optional<fs::path> pluginPath;
            if (plan._createDummies && settings.s_dummy_plugin) {
                bool loaded = false;
                for (const auto& extension : settings.plugin_extensions) {
                    auto plugin = *selected;
                    plugin.ext = extension;
                    loaded = loaded || fs::exists(plugin.full_path());
                    plugin.suffix.clear();
                    loaded = loaded || fs::exists(plugin.full_path());
                }
                if (!loaded) {
                    auto plugin = *selected;
                    plugin.ext = settings.plugin_extensions.back();
                    plugin.suffix.clear();
                    pluginPath = plugin.full_path();
                }
            }
            plan._outputs.push_back(
                {root, destination, {archive.begin(), archive.end()}, pluginPath});
            plan._archives.push_back(std::move(archive));
        }
    }
    return plan;
}

cao::run::ArchiveFinalizationResult BSAOptimizer::finalize(
    const cao::run::ArchiveFinalizationPlan& plan, cao::run::TemporaryArtifactRegistry& artifacts,
    const std::stop_token stop,
    std::function<void(const cao::run::ArchiveFinalizationProgress&)> progress) const {
    namespace fs = std::filesystem;
    using namespace cao::run;
    using cao::execution::MutationState;
    ArchiveFinalizationResult result;
    ArchiveFinalizationProgress counts{0, plan.outputs().size(), 0, 0};
    const auto report = [&] {
        try {
            if (progress) progress(counts);
        } catch (...) {
            // Presentation failures cannot abandon a committed output or suppress source cleanup.
        }
    };
    report();
    for (std::size_t index = 0; index < plan.outputs().size(); ++index) {
        if (stop.stop_requested()) {
            result.cancelled = true;
            break;
        }
        const auto& output = plan.outputs()[index];
        ArchiveFinalizationAttempt attempt{output.archivePath};
        auto boundary = ArchiveFinalizationFailure::WriteFailed;
        std::size_t removedSources = 0;
        try {
            const auto staged = artifacts.stageArchiveFile(output.modRoot);
            auto archive = plan._archives[index];
            archive.set_out_path(staged.path);
            const auto errors = btu::bsa::write(plan._compress, std::move(archive), output.modRoot);
            if (!errors.empty()) throw std::runtime_error(errors.front().second);
            boundary = ArchiveFinalizationFailure::CommitFailed;
            publishArchiveFile(staged.path, output.archivePath, attempt.mutation);
            artifacts.commit(staged.registration);
            boundary = ArchiveFinalizationFailure::PluginCreationFailed;
            if (output.pluginPath) {
                const auto& bytes = *plan._settings.s_dummy_plugin;
                const auto& plugin = *output.pluginPath;
                // Standard and Texture Archives can share a planned plugin. Reuse only the
                // exact dummy already committed by a previous output; never truncate a newcomer.
                if (fs::exists(fs::symlink_status(plugin))) {
                    std::ifstream existing(plugin, std::ios::binary);
                    const std::vector<char> contents{std::istreambuf_iterator<char>(existing), {}};
                    if (!existing || contents.size() != bytes.size() ||
                        !std::equal(contents.begin(), contents.end(), bytes.begin(),
                                    [](char left, std::uint8_t right) {
                                        return static_cast<unsigned char>(left) == right;
                                    }))
                        throw std::runtime_error("The planned loading plugin is occupied.");
                } else {
                    const auto stagedPlugin = artifacts.stageArchiveFile(output.modRoot);
                    std::ofstream file(stagedPlugin.path, std::ios::binary | std::ios::trunc);
                    file.write(reinterpret_cast<const char*>(bytes.data()),
                               static_cast<std::streamsize>(bytes.size()));
                    file.close();
                    if (!file) throw std::runtime_error("Could not stage the loading plugin.");
                    publishArchiveFile(stagedPlugin.path, plugin, attempt.mutation);
                    artifacts.commit(stagedPlugin.registration);
                }
            }
            boundary = ArchiveFinalizationFailure::SourceCleanupFailed;
            if (plan._deleteSources) {
                for (const auto& source : output.sources) {
                    if (!fs::remove(source))
                        throw std::runtime_error("A packed source could not be removed.");
                    ++removedSources;
                }
            }
        } catch (const std::exception& error) {
            attempt.failure = boundary;
            attempt.detail = error.what();
            // Write/commit errors leave sources intact. Once committed, only a usable Archive
            // can justify continuing after source cleanup or ownership release fails.
            if (attempt.mutation == MutationState::Committed) {
                try {
                    attempt.safeToContinue =
                        boundary != ArchiveFinalizationFailure::PluginCreationFailed &&
                        btu::bsa::read_archive(output.archivePath).has_value();
                    for (auto remaining = removedSources;
                         attempt.safeToContinue && remaining < output.sources.size(); ++remaining)
                        attempt.safeToContinue = readablePackedSource(output.sources[remaining]);
                } catch (...) {
                    // Verification failure preserves the original error and stops later attempts.
                    attempt.safeToContinue = false;
                }
                if (!attempt.safeToContinue) attempt.mutation = MutationState::PartialOrUnknown;
            }
        } catch (...) {
            attempt.failure = ArchiveFinalizationFailure::UnexpectedException;
            attempt.detail = "Unexpected Archive finalization exception.";
            attempt.safeToContinue = false;
        }
        ++counts.completed;
        if (attempt.succeeded())
            ++counts.succeeded;
        else
            ++counts.failed;
        result.safeToContinue = attempt.safeToContinue;
        result.attempts.push_back(std::move(attempt));
        report();
        if (!result.safeToContinue) break;
    }
    result.cancelled = result.cancelled || stop.stop_requested();
    if (!result.cancelled && result.safeToContinue) {
        try {
            for (const auto& root : plan._roots) {
                auto plugins =
                    btu::bsa::list_plugins(fs::directory_iterator(root), {}, plan._settings);
                // Do not remove loading plugins already committed as part of output attempts.
                // When dummy creation is disabled, retain the existing explicit cleanup choice.
                if (!plan._createDummies) btu::bsa::clean_dummy_plugins(plugins, plan._settings);
                if (plan._createDummies) {
                    const auto archives =
                        btu::bsa::list_archive(fs::directory_iterator(root), {}, plan._settings);
                    btu::bsa::make_dummy_plugins(archives, plan._settings);
                }
            }
            // All planned outputs and plugin work must finish before pruning any Mod Root.
            // Recoverable attempts retain their source evidence; cancellation retains all paths.
            for (const auto& root : plan._roots) {
                if (stop.stop_requested()) {
                    result.cancelled = true;
                    break;
                }
                FilesystemOperations::deleteEmptyDirectories(QString::fromStdWString(root.wstring()));
            }
        } catch (const std::exception& error) {
            result.safeToContinue = false;
            result.detail = error.what();
        }
    }
    return result;
}

void BSAOptimizer::packAll(const QString& folderPath, const OptionsCAO& options) const {
    const std::array roots{std::filesystem::path(folderPath.toStdU16String())};
    cao::run::TemporaryArtifactRegistry artifacts;
    try {
        const auto plan = planFinalization(roots, options);
        const auto result = finalize(plan, artifacts);
        for (const auto& attempt : result.attempts)
            if (!attempt.succeeded()) PLOG_ERROR << attempt.detail;
        if (!result.detail.empty()) PLOG_ERROR << result.detail;
    } catch (const std::exception& error) {
        PLOG_ERROR << error.what();
    }
    for (const auto& failure : artifacts.performSafetyCleanup()) PLOG_ERROR << failure.detail();
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
