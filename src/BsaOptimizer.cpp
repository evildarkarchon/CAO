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
#include <cstring>
#include <fstream>
#include <limits>
#include <map>
#include <optional>
#include <set>
#include <vector>

#ifdef _WIN32
#include <Windows.h>
#endif

namespace {
/// Estimates source content and format overhead; planning can stop between source stats, while
/// finalization intentionally finishes each atomic output attempt once it starts.
std::uintmax_t estimatePackedCapacity(const cao::run::ArchiveFinalizationOutput& output,
                                      const std::stop_token stop = {}) {
    using namespace cao::run;
    auto estimate = std::uintmax_t{65536};
    for (const auto& source : output.sources) {
        if (stop.stop_requested()) throw cao::run::ArchiveFinalizationPlanningCancelled{};
        // Zlib/LZ4 framing, up to four BA2 texture chunks, and BSA directory/name tables
        // need space beyond source bytes. Filesystem allocation and metadata remain estimates.
        const auto payload = saturatedCapacityMultiply(std::filesystem::file_size(source), 2);
        const auto names = saturatedCapacityMultiply(
            source.lexically_relative(output.modRoot).generic_u8string().size(), 3);
        estimate = saturatedCapacityAdd(
            estimate, saturatedCapacityAdd(payload, saturatedCapacityAdd(65536, names)));
    }
    return estimate;
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

#ifdef _WIN32
using SourceFileHandle = std::unique_ptr<void, decltype(&CloseHandle)>;

struct SourceFileFacts final {
    BY_HANDLE_FILE_INFORMATION file{};
    FILE_BASIC_INFO basic{};
};

/// Captures a native file ID and change metadata, rejecting aliases to links or directories.
SourceFileFacts inspectSourceFile(HANDLE handle, const std::filesystem::path& source) {
    SourceFileFacts facts;
    if (!GetFileInformationByHandle(handle, &facts.file) ||
        !GetFileInformationByHandleEx(handle, FileBasicInfo, &facts.basic,
                                      sizeof(facts.basic)))
        throw std::filesystem::filesystem_error(
            "Could not inspect source file", source,
            std::error_code(static_cast<int>(GetLastError()), std::system_category()));
    if (facts.file.dwFileAttributes & (FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_DIRECTORY))
        throw std::runtime_error("A source file is no longer an ordinary file.");
    return facts;
}

/// Requires the same file object and unchanged content-relevant metadata after Archive I/O.
bool sameSourceFile(const SourceFileFacts& earlier, const SourceFileFacts& current) {
    return earlier.file.dwVolumeSerialNumber == current.file.dwVolumeSerialNumber &&
           earlier.file.nFileIndexHigh == current.file.nFileIndexHigh &&
           earlier.file.nFileIndexLow == current.file.nFileIndexLow &&
           earlier.file.ftCreationTime.dwHighDateTime ==
               current.file.ftCreationTime.dwHighDateTime &&
           earlier.file.ftCreationTime.dwLowDateTime ==
               current.file.ftCreationTime.dwLowDateTime &&
           earlier.file.ftLastWriteTime.dwHighDateTime ==
               current.file.ftLastWriteTime.dwHighDateTime &&
           earlier.file.ftLastWriteTime.dwLowDateTime ==
               current.file.ftLastWriteTime.dwLowDateTime &&
           earlier.file.nFileSizeHigh == current.file.nFileSizeHigh &&
           earlier.file.nFileSizeLow == current.file.nFileSizeLow &&
           earlier.basic.ChangeTime.QuadPart == current.basic.ChangeTime.QuadPart;
}
#endif

/// Publishes a source backup without replacing any directory entry, including dangling links.
/// Retries occupied names; other filesystem failures leave the source or published backup intact.
#ifdef _WIN32
void backupExtractedArchive(const std::filesystem::path& source, HANDLE handle) {
#else
void backupExtractedArchive(const std::filesystem::path& source) {
#endif
    auto destination = source;
    for (;;) {
        destination += ".bak";
        std::error_code error;
#ifdef _WIN32
        // Rename the verified file object, not a pathname that could be replaced after checking.
        const auto& name = destination.native();
        const auto nameBytes = name.size() * sizeof(wchar_t);
        if (nameBytes > (std::numeric_limits<DWORD>::max)() - sizeof(FILE_RENAME_INFO))
            throw std::invalid_argument("Archive backup name is too long.");
        std::vector<std::byte> buffer(sizeof(FILE_RENAME_INFO) + nameBytes);
        auto* rename = reinterpret_cast<FILE_RENAME_INFO*>(buffer.data());
        rename->ReplaceIfExists = FALSE;
        rename->RootDirectory = nullptr;
        rename->FileNameLength = static_cast<DWORD>(nameBytes);
        std::memcpy(rename->FileName, name.data(), nameBytes);
        if (SetFileInformationByHandle(handle, FileRenameInfo, rename,
                                       static_cast<DWORD>(buffer.size())))
            return;
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

#ifdef _WIN32
struct cao::run::SourceFilePin::State final {
    std::filesystem::path source;
    SourceFileHandle pinned{nullptr, &CloseHandle};
    SourceFileFacts facts;

    /// Reopens the recorded path for native cleanup and rejects a substituted file object.
    SourceFileHandle openForCleanup() {
        SourceFileHandle guardian{nullptr, &CloseHandle};
        if (pinned) {
            // Keep the original file ID live while transitioning from a read-only pin to a
            // DELETE-capable handle. The first pin still denies replacement until this opens.
            const auto handle = CreateFileW(source.c_str(), FILE_READ_ATTRIBUTES,
                                            FILE_SHARE_READ | FILE_SHARE_DELETE, nullptr,
                                            OPEN_EXISTING, FILE_FLAG_OPEN_REPARSE_POINT, nullptr);
            if (handle == INVALID_HANDLE_VALUE)
                throw std::filesystem::filesystem_error(
                    "Could not retain source identity for cleanup", source,
                    std::error_code(static_cast<int>(GetLastError()), std::system_category()));
            guardian.reset(handle);
            pinned.reset();
        }
        const auto handle = CreateFileW(source.c_str(), DELETE | FILE_READ_ATTRIBUTES,
                                        FILE_SHARE_READ, nullptr, OPEN_EXISTING,
                                        FILE_FLAG_OPEN_REPARSE_POINT, nullptr);
        if (handle == INVALID_HANDLE_VALUE)
            throw std::filesystem::filesystem_error(
                "Could not reopen source for cleanup", source,
                std::error_code(static_cast<int>(GetLastError()), std::system_category()));
        SourceFileHandle candidate(handle, &CloseHandle);
        if (!sameSourceFile(facts, inspectSourceFile(handle, source)))
            throw std::runtime_error("A source file changed before cleanup.");
        return candidate;
    }
};

cao::run::SourceFilePin::SourceFilePin(std::filesystem::path source)
    : _state(std::make_unique<State>()) {
    _state->source = std::filesystem::absolute(std::move(source)).lexically_normal();
    const auto handle = CreateFileW(_state->source.c_str(), GENERIC_READ, FILE_SHARE_READ,
                                    nullptr, OPEN_EXISTING, FILE_FLAG_OPEN_REPARSE_POINT, nullptr);
    if (handle == INVALID_HANDLE_VALUE)
        throw std::filesystem::filesystem_error(
            "Could not pin source file", _state->source,
            std::error_code(static_cast<int>(GetLastError()), std::system_category()));
    _state->pinned.reset(handle);
    _state->facts = inspectSourceFile(handle, _state->source);
}

cao::run::SourceFilePin::~SourceFilePin() = default;
cao::run::SourceFilePin::SourceFilePin(SourceFilePin&&) noexcept = default;
cao::run::SourceFilePin& cao::run::SourceFilePin::operator=(SourceFilePin&&) noexcept =
    default;

void cao::run::SourceFilePin::releaseForCleanup() noexcept {
    if (_state) _state->pinned.reset();
}

void cao::run::SourceFilePin::removeIfUnchanged() {
    if (!_state) throw std::logic_error("A moved source pin cannot remove a source.");
    const auto candidate = _state->openForCleanup();
    FILE_DISPOSITION_INFO disposition{TRUE};
    if (!SetFileInformationByHandle(candidate.get(), FileDispositionInfo, &disposition,
                                    sizeof(disposition)))
        throw std::system_error(static_cast<int>(GetLastError()), std::system_category(),
                                "A source file could not be removed");
}

void cao::run::SourceFilePin::backupIfUnchanged() {
    if (!_state) throw std::logic_error("A moved source pin cannot back up a source.");
    const auto candidate = _state->openForCleanup();
    backupExtractedArchive(_state->source, candidate.get());
}

void cao::run::SourceFilePin::pinUnchangedForRecovery() {
    if (!_state) throw std::logic_error("A moved source pin cannot verify a source.");
    const auto handle = CreateFileW(_state->source.c_str(), GENERIC_READ, FILE_SHARE_READ,
                                    nullptr, OPEN_EXISTING, FILE_FLAG_OPEN_REPARSE_POINT, nullptr);
    if (handle == INVALID_HANDLE_VALUE)
        throw std::filesystem::filesystem_error(
            "Could not pin retained source file", _state->source,
            std::error_code(static_cast<int>(GetLastError()), std::system_category()));
    SourceFileHandle candidate(handle, &CloseHandle);
    if (!sameSourceFile(_state->facts, inspectSourceFile(handle, _state->source)))
        throw std::runtime_error("The retained source file changed after extraction.");
    _state->pinned = std::move(candidate);
}
#endif

BSAOptimizer::BSAOptimizer() : BSAOptimizer(OptimizerProfileSnapshot::capture()) {}

BSAOptimizer::BSAOptimizer(OptimizerProfileSnapshot profile) : _profile(std::move(profile)) {
    // Reading filesToNotPack to add them to the list.
    // Done in the constructor since the file won't change at runtime.

    auto lines = _profile.filesToNotPack;
    for (auto& line : lines) line = QDir::toNativeSeparators(line);

    for (auto&& line : lines)
        filesToNotPack.emplace_back(btu::common::as_utf8_string(std::move(line).toStdString()));

    if (filesToNotPack.empty()) {
        PLOG_ERROR << "FilesToNotPack.txt not found. This can cause a number of issues. For "
                      "example, for Skyrim, "
                      "animations will be packed to BSA, preventing them from being detected "
                      "by FNIS and Nemesis.";
    }
}

/// Builds archive settings from the run-owned profile snapshot.
btu::bsa::Settings getSettings(const OptimizerProfileSnapshot& profile) {
    auto sets = btu::bsa::Settings::get(profile.bsaGame);
    if (profile.maxBsaUncompressedSize > sets.max_size)
        sets.max_size = profile.maxBsaUncompressedSize;
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
#ifdef _WIN32
    std::optional<cao::run::SourceFilePin> sourcePin;
    try {
        // ArchiveExtractor reopens this path for inventory and payload reads. Deny replacement
        // across all of those reads and the merge that follows them.
        sourcePin.emplace(plan.archivePath);
    } catch (const std::exception& error) {
        cao::run::ArchiveExtractionResult result{plan.archivePath};
        result.modRoot = plan.modRoot;
        result.failure = cao::run::ArchiveExtractionFailure::ExtractionFailed;
        result.detail = error.what();
        return result;
    }
#endif
    auto result = cao::run::ArchiveExtractor(artifacts).extract(plan);
    if (!result.succeeded()) return result;

    try {
        // Keep the original name and bytes throughout extraction and merge so a failed attempt
        // remains recoverable. Never replace a pre-existing backup based only on matching size.
#ifdef _WIN32
        if (deleteBackup)
            sourcePin->removeIfUnchanged();
        else
            sourcePin->backupIfUnchanged();
#else
        if (deleteBackup) {
            if (!std::filesystem::remove(plan.archivePath))
                throw std::runtime_error("The extracted source Archive could not be removed.");
        } else {
            backupExtractedArchive(plan.archivePath);
        }
#endif
        result.mutation = cao::execution::MutationState::Committed;
    } catch (const std::exception& error) {
        result.failure = cao::run::ArchiveExtractionFailure::SourceCleanupFailed;
        result.safeToContinue = false;
        result.detail = error.what();
        try {
            // Existence alone cannot prove that the retained source is still usable. Reopen
            // its manifest after the failed mutation before allowing later phases to proceed.
            if (result.mutation == cao::execution::MutationState::Committed) {
#ifdef _WIN32
                // A valid replacement Archive is not recovery material for the extracted bytes.
                // Keep this read pin alive through the library's pathname-based reopen.
                sourcePin->pinUnchangedForRecovery();
#endif
                result.safeToContinue = btu::bsa::read_archive(plan.archivePath).has_value();
            }
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
    const std::span<const std::filesystem::path> roots, const OptionsCAO& options,
    const std::stop_token stop) const {
    namespace fs = std::filesystem;
    using namespace btu::bsa;
    cao::run::ArchiveFinalizationPlan plan;
    plan._settings = getSettings(_profile);
    plan._compress = options.bBsaCompress;
    plan._deleteSources = options.bBsaDeleteSource;
    plan._createDummies = options.bBsaCreateDummies;
    const auto& settings = plan._settings;
    std::set<fs::path> reserved;
    // Planning must not publish a partial work total after cancellation; no output has mutated.
    const auto checkCancelled = [&] {
        if (stop.stop_requested()) throw cao::run::ArchiveFinalizationPlanningCancelled{};
    };
    for (const auto& inputRoot : roots) {
        checkCancelled();
        const auto root = fs::canonical(inputRoot);
        plan._roots.push_back(root);
        auto& dummyCapacity = plan._dummyCapacityByRoot[root];
        if (plan._createDummies && settings.s_dummy_plugin) {
            // Existing Archives can also need plugins in the final cleanup pass, even with no
            // new outputs. Count every Archive conservatively without assuming plugin reuse.
            checkCancelled();
            const auto existing = list_archive(fs::directory_iterator(root), {}, settings);
            dummyCapacity = cao::run::saturatedCapacityAdd(
                dummyCapacity, cao::run::saturatedCapacityMultiply(
                                   existing.size(), settings.s_dummy_plugin->size()));
        }
        auto plugins = list_plugins(fs::directory_iterator(root), {}, settings);
        // Dummy cleanup is a mutation. Ignore their names for planning, but retain them until
        // all output attempts finish so cancellation cannot strand an existing Archive.
        if (settings.s_dummy_plugin) {
            std::erase_if(plugins, [&](const auto& plugin) {
                checkCancelled();
                return fs::file_size(plugin.full_path()) == settings.s_dummy_plugin->size();
            });
        }
        std::sort(plugins.begin(), plugins.end());
        if (plugins.empty())
            plugins.emplace_back(root, root.filename().u8string(), u8"", u8".esp",
                                 FileTypes::Plugin);

        std::vector<fs::path> sources;
        for (auto it = fs::recursive_directory_iterator(root);
             it != fs::recursive_directory_iterator(); ++it) {
            checkCancelled();
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
            checkCancelled();
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
            checkCancelled();
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
                checkCancelled();
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
                    checkCancelled();
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
                    checkCancelled();
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
            auto& output = plan._outputs.back();
            output.estimatedCapacityBytes = estimatePackedCapacity(output, stop);
            if (pluginPath)
                output.estimatedCapacityBytes = cao::run::saturatedCapacityAdd(
                    output.estimatedCapacityBytes, settings.s_dummy_plugin->size());
            plan._archives.push_back(std::move(archive));
        }
    }
    checkCancelled();
    return plan;
}

cao::run::ArchiveFinalizationResult BSAOptimizer::finalize(
    const cao::run::ArchiveFinalizationPlan& plan, cao::run::TemporaryArtifactRegistry& artifacts,
    const std::stop_token stop,
    std::function<void(const cao::run::ArchiveFinalizationProgress&)> progress,
    cao::run::CapacityProbe capacity,
    std::function<void(const cao::run::ArchiveFinalizationAttempt&)> onAttempt,
    cao::run::VolumeIdentityProbe volumeIdentity) const {
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
    const auto hasCapacity = [&](const fs::path& root, const fs::path& archive,
                                 std::uintmax_t required) {
        std::optional<std::uintmax_t> available;
        try {
            if (capacity) available = capacity(root);
        } catch (...) {
            // A failed capacity query is unknown; the atomic writer still handles real I/O errors.
        }
        if (!available || required <= *available) return true;
        if (archive.empty()) {
            result.failure = ArchiveFinalizationFailure::InsufficientCapacity;
            result.detail = archiveCapacityDetail(required, *available);
            return false;
        }
        ArchiveFinalizationAttempt attempt{archive};
        attempt.modRoot = root;
        attempt.failure = ArchiveFinalizationFailure::InsufficientCapacity;
        attempt.detail = archiveCapacityDetail(required, *available);
        result.attempts.push_back(std::move(attempt));
        if (onAttempt) onAttempt(result.attempts.back());
        ++counts.completed;
        ++counts.failed;
        report();
        return false;
    };
    if (stop.stop_requested()) {
        result.cancelled = true;
        return result;
    }
    // Freeze each canonical root's volume once for all preflight and later rechecks. Rebuilding
    // the remaining dummy reserve must not turn Several Mods into repeated native volume queries.
    std::map<fs::path, std::optional<std::string>> volumeByRoot;
    for (const auto& root : plan._roots)
        volumeByRoot.emplace(root, volumeIdentity ? volumeIdentity(root) : std::nullopt);
    const auto cachedVolumeIdentity = [&](const fs::path& root) { return volumeByRoot.at(root); };
    ArchiveVolumeCapacityRequirements phaseCapacity(cachedVolumeIdentity);
    ArchiveVolumeCapacityRequirements dummyCapacity(cachedVolumeIdentity);
    for (const auto& root : plan._roots) {
        const auto required = plan._dummyCapacityByRoot.at(root);
        phaseCapacity.add(root, required);
        dummyCapacity.add(root, required);
    }
    for (const auto& output : plan.outputs())
        phaseCapacity.add(output.modRoot, output.estimatedCapacityBytes);
    // A volume must fit its complete phase before any root mutates; planned source deletion
    // cannot be treated as available space before it actually happens.
    for (const auto& root : plan._roots) {
        const auto output = std::find_if(plan.outputs().begin(), plan.outputs().end(),
                                         [&](const auto& value) { return value.modRoot == root; });
        // A Mod Root with no planned writes cannot run out of staging space during this phase.
        if (output == plan.outputs().end() && plan._dummyCapacityByRoot.at(root) == 0) continue;
        if (!hasCapacity(root, output == plan.outputs().end() ? fs::path{} : output->archivePath,
                         phaseCapacity.requiredAt(root)))
            return result;
    }
    for (std::size_t index = 0; index < plan.outputs().size(); ++index) {
        if (stop.stop_requested()) {
            result.cancelled = true;
            break;
        }
        const auto& output = plan.outputs()[index];
        ArchiveFinalizationAttempt attempt{output.archivePath};
        attempt.modRoot = output.modRoot;
        auto boundary = ArchiveFinalizationFailure::WriteFailed;
        std::size_t removedSources = 0;
        try {
            // Sources can grow after planning. Re-stat before mutation, retaining the frozen
            // allowance if files shrink; capacity itself is still only a momentary sample.
            auto currentCapacity = estimatePackedCapacity(output);
            if (output.pluginPath)
                currentCapacity =
                    saturatedCapacityAdd(currentCapacity, plan._settings.s_dummy_plugin->size());
            if (!hasCapacity(
                    output.modRoot, output.archivePath,
                    saturatedCapacityAdd(std::max(output.estimatedCapacityBytes, currentCapacity),
                                         dummyCapacity.requiredAt(output.modRoot))))
                return result;
#ifdef _WIN32
            std::vector<SourceFilePin> sourcePins;
            if (plan._deleteSources) {
                sourcePins.reserve(output.sources.size());
                for (const auto& source : output.sources) sourcePins.emplace_back(source);
            }
#endif
            auto staged = artifacts.stageArchiveFileForPublication(output.modRoot);
            auto archive = plan._archives[index];
            archive.set_out_path(staged.path());
            const auto errors = btu::bsa::write(plan._compress, std::move(archive), output.modRoot);
            if (!errors.empty()) throw std::runtime_error(errors.front().second);
            boundary = ArchiveFinalizationFailure::CommitFailed;
            const auto archivePublication =
                staged.publish(output.archivePath, PublicationPolicy::NoReplace);
            // Native publication commits the Archive before durable ownership release.
            if (archivePublication.state != PublicationState::NotPublished)
                attempt.mutation = MutationState::Committed;
            if (archivePublication.state != PublicationState::PublishedAndReleased)
                throw std::runtime_error(archivePublication.errorDetail.empty()
                                             ? "Archive publication did not complete."
                                             : archivePublication.errorDetail);
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
                    auto stagedPlugin = artifacts.stageArchiveFileForPublication(output.modRoot);
                    std::ofstream file(stagedPlugin.path(), std::ios::binary | std::ios::trunc);
                    file.write(reinterpret_cast<const char*>(bytes.data()),
                               static_cast<std::streamsize>(bytes.size()));
                    file.close();
                    if (!file) throw std::runtime_error("Could not stage the loading plugin.");
                    // Recheck the leaf natively after the exact-dummy probe: a newcomer wins.
                    const auto pluginPublication =
                        stagedPlugin.publish(plugin, PublicationPolicy::NoReplace);
                    if (pluginPublication.state != PublicationState::PublishedAndReleased)
                        throw std::runtime_error(pluginPublication.errorDetail.empty()
                                                     ? "Loading plugin publication did not complete."
                                                     : pluginPublication.errorDetail);
                }
            }
            boundary = ArchiveFinalizationFailure::SourceCleanupFailed;
            if (plan._deleteSources) {
#ifdef _WIN32
                for (auto& pin : sourcePins) {
                    pin.releaseForCleanup();
                    pin.removeIfUnchanged();
                    ++removedSources;
                }
#else
                for (const auto& source : output.sources) {
                    if (!fs::remove(source))
                        throw std::runtime_error("A packed source could not be removed.");
                    ++removedSources;
                }
#endif
            }
        } catch (const std::exception& error) {
            attempt.failure = boundary;
            attempt.detail = error.what();
            // Publication or plugin errors leave sources intact. Once committed, only a usable
            // Archive and surviving sources can justify continuing after later failures. The
            // publication result remains authoritative even when continuation is unsafe.
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
        if (onAttempt) onAttempt(result.attempts.back());
        report();
        if (!result.safeToContinue) break;
    }
    result.cancelled = result.cancelled || stop.stop_requested();
    if (!result.cancelled && result.safeToContinue) {
        const auto pluginPaths = [](const std::vector<btu::bsa::FilePath>& plugins) {
            std::set<fs::path> paths;
            for (const auto& plugin : plugins) paths.insert(plugin.full_path());
            return paths;
        };
        try {
            for (auto rootIt = plan._roots.begin(); rootIt != plan._roots.end(); ++rootIt) {
                if (stop.stop_requested()) {
                    result.cancelled = true;
                    break;
                }
                const auto& root = *rootIt;
                if (plan._dummyCapacityByRoot.at(root) != 0) {
                    ArchiveVolumeCapacityRequirements remainingDummy(cachedVolumeIdentity);
                    for (auto remaining = rootIt; remaining != plan._roots.end(); ++remaining)
                        remainingDummy.add(*remaining, plan._dummyCapacityByRoot.at(*remaining));
                    // Earlier roots have already consumed their plugin allowance, so rechecking
                    // it would reject later roots despite a sufficient initial phase budget.
                    if (!hasCapacity(root, {}, remainingDummy.requiredAt(root))) return result;
                }
                // Capacity and directory probes may block while a cancellation arrives; do not
                // start the next root's plugin mutation after either probe completes.
                if (stop.stop_requested()) {
                    result.cancelled = true;
                    break;
                }
                auto plugins =
                    btu::bsa::list_plugins(fs::directory_iterator(root), {}, plan._settings);
                if (stop.stop_requested()) {
                    result.cancelled = true;
                    break;
                }
                // Output-owned plugins are already represented by their Archive attempts.
                // Snapshot only this cleanup pass so existing-Archive plugin changes remain
                // authoritative evidence even when a bethutil helper stops partway through.
                const auto before = pluginPaths(plugins);
                const auto recordPluginChanges = [&] {
                    std::set<fs::path> after;
                    try {
                        after = pluginPaths(btu::bsa::list_plugins(
                            fs::directory_iterator(root), {}, plan._settings));
                    } catch (...) {
                        // If the post-mutation inventory is unreadable, the effect is unknown.
                        result.mutations.push_back(
                            {.modRoot = root,
                             .path = root,
                             .kind = plan._createDummies
                                         ? ArchiveFinalizationMutationKind::PluginCreation
                                         : ArchiveFinalizationMutationKind::PluginRemoval,
                             .mutation = MutationState::PartialOrUnknown});
                        throw;
                    }
                    for (const auto& plugin : before) {
                        if (!after.contains(plugin))
                            result.mutations.push_back(
                                {.modRoot = root,
                                 .path = plugin,
                                 .kind = ArchiveFinalizationMutationKind::PluginRemoval,
                                 .mutation = MutationState::Committed});
                    }
                    for (const auto& plugin : after) {
                        if (!before.contains(plugin))
                            result.mutations.push_back(
                                {.modRoot = root,
                                 .path = plugin,
                                 .kind = ArchiveFinalizationMutationKind::PluginCreation,
                                 .mutation = MutationState::Committed});
                    }
                };
                try {
                    // Do not remove loading plugins already committed as part of output attempts.
                    // When dummy creation is disabled, retain the existing explicit cleanup choice.
                    if (!plan._createDummies)
                        btu::bsa::clean_dummy_plugins(plugins, plan._settings);
                    if (plan._createDummies) {
                        const auto archives = btu::bsa::list_archive(fs::directory_iterator(root),
                                                                     {}, plan._settings);
                        if (stop.stop_requested()) {
                            result.cancelled = true;
                            break;
                        }
                        btu::bsa::make_dummy_plugins(archives, plan._settings);
                    }
                } catch (...) {
                    recordPluginChanges();
                    throw;
                }
                recordPluginChanges();
            }
            // All planned outputs and plugin work must finish before pruning any Mod Root.
            // Recoverable attempts retain their source evidence; cancellation retains all paths.
            for (const auto& root : plan._roots) {
                if (stop.stop_requested()) {
                    result.cancelled = true;
                    break;
                }
                const auto pruned = FilesystemOperations::deleteEmptyDirectories(
                    QString::fromStdWString(root.wstring()));
                if (pruned != 0)
                    result.mutations.push_back(
                        {.modRoot = root,
                         .path = root,
                         .kind = ArchiveFinalizationMutationKind::EmptyDirectoryPruning,
                         .mutation = MutationState::Committed,
                         .count = pruned});
            }
        } catch (const std::exception& error) {
            result.safeToContinue = false;
            result.failure = ArchiveFinalizationFailure::UnexpectedException;
            result.detail = error.what();
        } catch (...) {
            result.safeToContinue = false;
            result.failure = ArchiveFinalizationFailure::UnexpectedException;
            result.detail = "Unexpected Archive finalization cleanup exception.";
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
