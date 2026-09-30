#include "Run/ArchiveFinalization.h"

#include "FilesystemOperations.h"
#include "Run/ArchiveFinalizationResult.h"
#include "Run/NativeFilePins.h"
#include "Run/RunEvidence.h"
#include "Run/RunPreparation.h"
#include "Run/StagingPaths.h"
#include "Run/TemporaryArtifactRegistry.h"

#include <btu/bsa/archive_data.hpp>

#include <algorithm>
#include <array>
#include <cstring>
#include <exception>
#include <fstream>
#include <limits>
#include <map>
#include <optional>
#include <set>
#include <span>
#include <stdexcept>
#include <vector>

#ifdef _WIN32
#include <Windows.h>
#endif

namespace cao::run {
namespace {
/// Aborts an incomplete mutation-free plan when cancellation arrives during source traversal.
class ArchiveFinalizationPlanningCancelled final : public std::runtime_error {
   public:
    ArchiveFinalizationPlanningCancelled()
        : std::runtime_error("Archive finalization planning was cancelled.") {}
};

/// One frozen output and the complete source set consumed by its atomic attempt.
struct ArchiveFinalizationOutput final {
    std::filesystem::path modRoot;
    std::filesystem::path archivePath;
    std::vector<std::filesystem::path> sources;
    /// The fallback Dummy Plugin destination when planning finds no Loading Plugin. The attempt
    /// rechecks recognized names before creation and source deletion.
    std::optional<std::filesystem::path> pluginPath;
    /// Conservative content and framing allowance, not a reservation or filesystem quota guarantee.
    std::uintmax_t estimatedCapacityBytes{};
    /// Profile-recognized Loading Plugin names; the last is the suffix-free dummy destination.
    /// Finalization rechecks them because a plugin may appear or disappear after planning.
    std::vector<std::filesystem::path> loadingPluginPaths;
};

/// Owns all output names, partitions, and settings before any finalization mutation.
/// The output count is the phase's complete progress total.
struct ArchiveFinalizationPlan final {
    std::vector<ArchiveFinalizationOutput> outputs;
    std::vector<btu::bsa::ArchiveData> archives;
    std::vector<std::filesystem::path> roots;
    btu::bsa::Settings settings;
    bool compress{};
    bool deleteSources{};
    bool createDummies{};
    std::map<std::filesystem::path, std::uintmax_t> dummyCapacityByRoot;
};

/// Carries an exception raised by Run Evidence itself past the phase's work-failure handlers.
/// Deliberately not a std::exception, so no work handler can mistake it for a work failure.
struct EvidenceRecordingFailed final {
    std::exception_ptr error;
};

/// Submits the phase's evidence protocol (output total, attempts, one result) and keeps every
/// exception from Run Evidence distinct from the phase's own work failures.
class EvidenceRecorder final {
   public:
    /// Borrows the work evidence view for one synchronous phase run.
    explicit EvidenceRecorder(RunWorkEvidence& evidence) noexcept : _evidence(evidence) {}

    /// Starts determinate progress with the frozen output total.
    void plan(const std::size_t total) {
        guarded([&] { _evidence.recordArchiveFinalizationPlan(total); });
        _total = total;
    }

    /// Retains one completed output attempt before the next output starts.
    void attempt(const ArchiveFinalizationAttempt& completed) {
        guarded([&] { _evidence.recordArchiveFinalizationAttempt(completed, _total); });
    }

    /// Retains the phase's single final result, whose attempts match those already recorded.
    void result(ArchiveFinalizationResult finalization) {
        guarded([&] { _evidence.recordArchiveFinalization(std::move(finalization)); });
    }

   private:
    /// Runs one evidence call, wrapping anything it throws for unchanged rethrow by run().
    template <typename Record>
    static void guarded(Record&& record) {
        try {
            record();
        } catch (...) {
            throw EvidenceRecordingFailed{std::current_exception()};
        }
    }

    RunWorkEvidence& _evidence;
    std::size_t _total{};
};

/// Builds archive settings from the run-owned profile snapshot.
btu::bsa::Settings archiveSettings(const OptimizerProfileSnapshot& profile) {
    auto sets = btu::bsa::Settings::get(profile.bsaGame);
    if (profile.maxBsaUncompressedSize > sets.max_size)
        sets.max_size = profile.maxBsaUncompressedSize;
    return sets;
}

/// Recognizes only an ordinary plugin whose complete bytes match this profile's canonical dummy.
/// An unverified file remains a Loading Plugin name during planning, not a proven dummy.
bool hasExactDummyBytes(const std::filesystem::path& path,
                        const std::vector<std::uint8_t>& bytes) {
    namespace fs = std::filesystem;
    std::error_code error;
    if (!fs::is_regular_file(fs::symlink_status(path, error)) || error ||
        fs::file_size(path, error) != bytes.size() || error)
        return false;
    std::ifstream input(path, std::ios::binary);
    std::vector<char> contents(bytes.size());
    input.read(contents.data(), static_cast<std::streamsize>(contents.size()));
    if (!input) return false;
    const auto complete = input.peek() == std::char_traits<char>::eof() && input.eof() &&
                          !input.bad();
    return complete && std::equal(contents.begin(), contents.end(), bytes.begin(),
                                  [](char left, std::uint8_t right) {
                                      return static_cast<unsigned char>(left) == right;
                                  });
}

/// Estimates source content and format overhead; planning can stop between source stats, while
/// finalization intentionally finishes each atomic output attempt once it starts.
std::uintmax_t estimatePackedCapacity(const ArchiveFinalizationOutput& output,
                                      const std::stop_token stop = {}) {
    auto estimate = std::uintmax_t{65536};
    for (const auto& source : output.sources) {
        if (stop.stop_requested()) throw ArchiveFinalizationPlanningCancelled{};
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

enum class DummyPluginRemoval { NotDummy, Removed, Uncertain };

#ifdef _WIN32
/// Pins ordinary parents and a single-link file while checking canonical bytes and deleting by
/// that handle. Returns NotDummy for other content and Uncertain only after disposition succeeds;
/// all exceptions occur before deletion and therefore describe known no-mutation failures.
DummyPluginRemoval removeExactDummyPlugin(const std::filesystem::path& plugin,
                                          const std::filesystem::path& modRoot,
                                          const std::vector<std::uint8_t>& bytes) {
    namespace fs = std::filesystem;
    const auto path = fs::absolute(plugin).lexically_normal();
    const auto root = fs::absolute(modRoot).lexically_normal();
    std::map<fs::path, SourceFileHandle> directories;
    // The pathname cannot escape through a junction or switch parents during the native delete.
    pinSourceDirectories(path, root, directories, FILE_SHARE_READ);
    const auto entry = CreateFileW(path.c_str(), GENERIC_READ | DELETE, FILE_SHARE_READ, nullptr,
                                   OPEN_EXISTING, FILE_FLAG_OPEN_REPARSE_POINT, nullptr);
    if (entry == INVALID_HANDLE_VALUE)
        throw fs::filesystem_error(
            "Could not pin Dummy Plugin for removal", path,
            std::error_code(static_cast<int>(GetLastError()), std::system_category()));
    SourceFileHandle pinned(entry, &CloseHandle);
    const auto original = inspectSourceFile(entry, path);
    // A hard link could make the same file accessible under an unrelated user's pathname.
    if (original.file.nNumberOfLinks != 1)
        throw std::runtime_error("A linked Dummy Plugin cannot be removed.");
    const auto size = (static_cast<std::uint64_t>(original.file.nFileSizeHigh) << 32) |
                      original.file.nFileSizeLow;
    if (size != bytes.size()) return DummyPluginRemoval::NotDummy;

    std::array<std::uint8_t, 8192> buffer{};
    std::size_t offset = 0;
    while (offset < bytes.size()) {
        DWORD count = 0;
        const auto chunk = static_cast<DWORD>(std::min(buffer.size(), bytes.size() - offset));
        if (!ReadFile(entry, buffer.data(), chunk, &count, nullptr))
            throw fs::filesystem_error(
                "Could not read pinned Dummy Plugin", path,
                std::error_code(static_cast<int>(GetLastError()), std::system_category()));
        if (count == 0) throw std::runtime_error("A Dummy Plugin changed during verification.");
        if (!std::equal(buffer.begin(), buffer.begin() + count, bytes.begin() + offset))
            return DummyPluginRemoval::NotDummy;
        offset += count;
    }
    const auto verified = inspectSourceFile(entry, path);
    if (verified.file.nNumberOfLinks != 1 || !sameSourceFile(original, verified))
        throw std::runtime_error("A Dummy Plugin changed during verification.");
    FILE_DISPOSITION_INFO disposition{TRUE};
    if (!SetFileInformationByHandle(entry, FileDispositionInfo, &disposition, sizeof(disposition)))
        throw fs::filesystem_error(
            "Could not remove pinned Dummy Plugin", path,
            std::error_code(static_cast<int>(GetLastError()), std::system_category()));
    // Once disposition succeeds, a failed close leaves the final directory state uncertain.
    if (!CloseHandle(pinned.release())) return DummyPluginRemoval::Uncertain;
    return DummyPluginRemoval::Removed;
}
#else
/// Keeps the non-Windows compatibility path on exact bytes while Windows owns the native guard.
/// Returns NotDummy for other content or Removed after deletion; throws if deletion fails.
DummyPluginRemoval removeExactDummyPlugin(const std::filesystem::path& plugin,
                                          const std::filesystem::path&,
                                          const std::vector<std::uint8_t>& bytes) {
    if (!hasExactDummyBytes(plugin, bytes)) return DummyPluginRemoval::NotDummy;
    if (!std::filesystem::remove(plugin))
        throw std::runtime_error("The Dummy Plugin could not be removed.");
    return DummyPluginRemoval::Removed;
}
#endif

/// Rejects reserved staging paths and entries matched by the profile's FilesToNotPack list.
/// Returns true when the entry may be packed and later deleted as a packed source.
bool isAllowedFile(const btu::Path& dir, const std::filesystem::directory_entry& fileinfo,
                   std::span<const std::u8string> filesToNotPack) {
    // Packing and its source-deletion pass must never consume another lifecycle's temporary data.
    if (hasStagingComponent(fileinfo.path().lexically_relative(dir))) return false;
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

/// Freezes output names and source partitions for all ordered Mod Roots without mutation.
/// Polls stop between inputs and throws ArchiveFinalizationPlanningCancelled instead of
/// publishing an incomplete plan. Other unreadable or unplannable inputs also throw.
ArchiveFinalizationPlan planFinalization(const std::span<const std::filesystem::path> roots,
                                         const ArchiveFinalizationSettings& choices,
                                         btu::bsa::Settings bsaSettings,
                                         const std::span<const std::u8string> filesToNotPack,
                                         const std::stop_token stop) {
    namespace fs = std::filesystem;
    using namespace btu::bsa;
    ArchiveFinalizationPlan plan;
    plan.settings = std::move(bsaSettings);
    plan.compress = choices.compress;
    plan.deleteSources = choices.deleteSources;
    plan.createDummies = choices.createDummyPlugins;
    const auto& settings = plan.settings;
    std::set<fs::path> reserved;
    // Planning must not publish a partial work total after cancellation; no output has mutated.
    const auto checkCancelled = [&] {
        if (stop.stop_requested()) throw ArchiveFinalizationPlanningCancelled{};
    };
    for (const auto& inputRoot : roots) {
        checkCancelled();
        const auto root = fs::canonical(inputRoot);
        plan.roots.push_back(root);
        auto& dummyCapacity = plan.dummyCapacityByRoot[root];
        if (plan.createDummies && settings.s_dummy_plugin) {
            // Existing Archives can also need plugins in the final cleanup pass, even with no
            // new outputs. Count every Archive conservatively without assuming plugin reuse.
            checkCancelled();
            const auto existing = list_archive(fs::directory_iterator(root), {}, settings);
            dummyCapacity = saturatedCapacityAdd(
                dummyCapacity,
                saturatedCapacityMultiply(existing.size(), settings.s_dummy_plugin->size()));
        }
        auto plugins = list_plugins(fs::directory_iterator(root), {}, settings);
        // Ignore only proven Dummy Plugin names while planning; retain their files until all
        // output attempts finish so cancellation cannot strand an existing Archive.
        if (settings.s_dummy_plugin) {
            std::erase_if(plugins, [&](const auto& plugin) {
                checkCancelled();
                return hasExactDummyBytes(plugin.full_path(), *settings.s_dummy_plugin);
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
            bool excluded = hasStagingComponent(path.lexically_relative(root)) ||
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
                isAllowedFile(root, *it, filesToNotPack))
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
        if (choices.mergeIncompressible) mergeSettings |= MergeSettings::MergeIncompressible;
        if (choices.mergeTextures) mergeSettings |= MergeSettings::MergeTextures;
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
            std::vector<fs::path> loadingPluginPaths;
            if (plan.createDummies && settings.s_dummy_plugin) {
                bool loaded = false;
                // Loading follows links to regular plugins; exact Dummy Plugin recognition does not.
                for (const auto& extension : settings.plugin_extensions) {
                    checkCancelled();
                    auto plugin = *selected;
                    plugin.ext = extension;
                    loadingPluginPaths.push_back(plugin.full_path());
                    loaded = loaded || fs::is_regular_file(loadingPluginPaths.back());
                    plugin.suffix.clear();
                    loadingPluginPaths.push_back(plugin.full_path());
                    loaded = loaded || fs::is_regular_file(loadingPluginPaths.back());
                }
                if (!loaded) pluginPath = loadingPluginPaths.back();
            }
            plan.outputs.push_back({root, destination, {archive.begin(), archive.end()},
                                    pluginPath, 0, std::move(loadingPluginPaths)});
            auto& output = plan.outputs.back();
            output.estimatedCapacityBytes = estimatePackedCapacity(output, stop);
            // A Loading Plugin can disappear before publication, requiring the fallback dummy.
            if (!output.loadingPluginPaths.empty())
                output.estimatedCapacityBytes = saturatedCapacityAdd(
                    output.estimatedCapacityBytes, settings.s_dummy_plugin->size());
            plan.archives.push_back(std::move(archive));
        }
    }
    checkCancelled();
    return plan;
}

/// Prunes empty children of each Mod Root, recording one mutation per root that lost any.
/// Preserves the roots themselves and reserved staging, which belongs to Safety Cleanup.
void pruneEmptyDirectories(const std::span<const std::filesystem::path> roots,
                           const std::stop_token stop, ArchiveFinalizationResult& result) {
    for (const auto& root : roots) {
        if (stop.stop_requested()) {
            result.cancelled = true;
            break;
        }
        const auto pruned =
            FilesystemOperations::deleteEmptyDirectories(QString::fromStdWString(root.wstring()));
        if (pruned != 0)
            result.mutations.push_back(
                {.modRoot = root,
                 .path = root,
                 .kind = ArchiveFinalizationMutationKind::EmptyDirectoryPruning,
                 .mutation = execution::MutationState::Committed,
                 .count = pruned});
    }
}

/// Publishes each planned Archive and missing loading plugin through one-use no-replace
/// staging, then cleans its sources without mid-attempt cancellation. A release or later
/// cleanup failure retains the committed Archive mutation in the completed attempt.
/// After all output attempts, maintains Loading Plugins for existing Archives even when the
/// output total is zero. Each new plugin is a separate mutation fact, never an output attempt.
/// Prunes empty children per Mod Root only after all outputs finish without cancellation or
/// unsafe failure. Recoverable source-cleanup failures retain readable evidence and continue.
/// Known capacity shortages stop before mutation; unknown capacity proceeds with atomic
/// attempts. Each completed attempt, including a capacity rejection, is recorded before the
/// next output starts. volumeIdentity groups roots for batch capacity checks; unknown identity
/// retains a conservative whole-batch estimate. A known no-mutation plugin creation or removal
/// failure is a safe phase failure; a plugin action with uncertain effects stops finalization
/// as unsafe. Accumulates into the caller's result so an escaping exception keeps every
/// attempt and mutation fact already established.
void finalizePlan(const ArchiveFinalizationPlan& plan, TemporaryArtifactRegistry& artifacts,
                  const std::stop_token stop, const CapacityProbe& capacity,
                  const VolumeIdentityProbe& volumeIdentity, EvidenceRecorder& recorder,
                  ArchiveFinalizationResult& result) {
    namespace fs = std::filesystem;
    using execution::MutationState;
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
        recorder.attempt(result.attempts.back());
        return false;
    };
    if (stop.stop_requested()) {
        result.cancelled = true;
        return;
    }
    // Freeze each canonical root's volume once for all preflight and later rechecks. Rebuilding
    // the remaining dummy reserve must not turn Several Mods into repeated native volume queries.
    std::map<fs::path, std::optional<std::string>> volumeByRoot;
    for (const auto& root : plan.roots)
        volumeByRoot.emplace(root, volumeIdentity ? volumeIdentity(root) : std::nullopt);
    const auto cachedVolumeIdentity = [&](const fs::path& root) { return volumeByRoot.at(root); };
    ArchiveVolumeCapacityRequirements phaseCapacity(cachedVolumeIdentity);
    ArchiveVolumeCapacityRequirements dummyCapacity(cachedVolumeIdentity);
    for (const auto& root : plan.roots) {
        const auto required = plan.dummyCapacityByRoot.at(root);
        phaseCapacity.add(root, required);
        dummyCapacity.add(root, required);
    }
    for (const auto& output : plan.outputs)
        phaseCapacity.add(output.modRoot, output.estimatedCapacityBytes);
    // A volume must fit its complete phase before any root mutates; planned source deletion
    // cannot be treated as available space before it actually happens.
    for (const auto& root : plan.roots) {
        const auto output = std::find_if(plan.outputs.begin(), plan.outputs.end(),
                                         [&](const auto& value) { return value.modRoot == root; });
        // A Mod Root with no planned writes cannot run out of staging space during this phase.
        if (output == plan.outputs.end() && plan.dummyCapacityByRoot.at(root) == 0) continue;
        if (!hasCapacity(root, output == plan.outputs.end() ? fs::path{} : output->archivePath,
                         phaseCapacity.requiredAt(root)))
            return;
    }
    for (std::size_t index = 0; index < plan.outputs.size(); ++index) {
        if (stop.stop_requested()) {
            result.cancelled = true;
            break;
        }
        const auto& output = plan.outputs[index];
        ArchiveFinalizationAttempt attempt{output.archivePath};
        attempt.modRoot = output.modRoot;
        auto boundary = ArchiveFinalizationFailure::WriteFailed;
        std::size_t removedSources = 0;
        try {
            // Sources can grow after planning. Re-stat before mutation, retaining the frozen
            // allowance if files shrink; capacity itself is still only a momentary sample.
            auto currentCapacity = estimatePackedCapacity(output);
            if (!output.loadingPluginPaths.empty())
                currentCapacity =
                    saturatedCapacityAdd(currentCapacity, plan.settings.s_dummy_plugin->size());
            if (!hasCapacity(
                    output.modRoot, output.archivePath,
                    saturatedCapacityAdd(std::max(output.estimatedCapacityBytes, currentCapacity),
                                         dummyCapacity.requiredAt(output.modRoot))))
                return;
#ifdef _WIN32
            std::vector<SourceFilePin> sourcePins;
            if (plan.deleteSources) {
                sourcePins.reserve(output.sources.size());
                const auto directoryPins = SourceFilePin::sharedDirectoryPins(output.modRoot);
                for (const auto& source : output.sources)
                    sourcePins.emplace_back(source, output.modRoot, directoryPins);
            }
#endif
            auto staged = artifacts.stageArchiveFileForPublication(output.modRoot);
            auto archive = plan.archives[index];
            archive.set_out_path(staged.path());
            const auto errors = btu::bsa::write(plan.compress, std::move(archive), output.modRoot);
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
#ifdef _WIN32
            std::optional<LoadingPluginPin> loadingPluginPin;
#endif
            if (!output.loadingPluginPaths.empty()) {
                const auto& bytes = *plan.settings.s_dummy_plugin;
                const auto& plugin = output.loadingPluginPaths.back();
                // A plugin at another profile-recognized name can arrive after planning, or a
                // planned one can disappear. The chosen dummy destination is checked separately
                // so a late non-dummy occupant never becomes our successful publication.
#ifdef _WIN32
                // Attribute-only reparse writes can retarget a symlink through a read pin.
                // Source deletion therefore needs an ordinary, nonempty plugin entry.
#endif
                const auto loadedElsewhere = std::find_if(
                    output.loadingPluginPaths.begin(), output.loadingPluginPaths.end(),
                    [&](const fs::path& path) {
                        if (path == plugin) return false;
#ifdef _WIN32
                        if (plan.deleteSources) {
                            const auto status = fs::symlink_status(path);
                            return fs::is_regular_file(status) && fs::file_size(path) != 0;
                        }
#endif
                        return fs::is_regular_file(path);
                    });
                auto destination = plugin;
                if (loadedElsewhere == output.loadingPluginPaths.end()) {
#ifdef _WIN32
                    if (plan.deleteSources) {
                        const auto status = fs::symlink_status(plugin);
                        if (fs::exists(status) &&
                            (!fs::is_regular_file(status) || fs::file_size(plugin) == 0)) {
                            const auto free = std::find_if(
                                output.loadingPluginPaths.begin(),
                                output.loadingPluginPaths.end(),
                                [&](const fs::path& path) {
                                    return !fs::exists(fs::symlink_status(path));
                                });
                            if (free == output.loadingPluginPaths.end())
                                throw std::runtime_error(
                                    "No ordinary loading plugin name is available.");
                            destination = *free;
                        }
                    }
#endif
                    if (destination == plugin && fs::exists(fs::symlink_status(plugin))) {
                        if (output.pluginPath && !hasExactDummyBytes(plugin, bytes))
                            throw std::runtime_error("The planned loading plugin is occupied.");
                        if (!fs::is_regular_file(plugin))
                            throw std::runtime_error("The planned loading plugin is not a file.");
                    } else {
                        auto stagedPlugin = artifacts.stageArchiveFileForPublication(output.modRoot);
                        std::ofstream file(stagedPlugin.path(), std::ios::binary | std::ios::trunc);
                        file.write(reinterpret_cast<const char*>(bytes.data()),
                                   static_cast<std::streamsize>(bytes.size()));
                        file.close();
                        if (!file) throw std::runtime_error("Could not stage the loading plugin.");
                        // Recheck the leaf natively after the exact-dummy probe: a newcomer wins.
                        const auto pluginPublication =
                            stagedPlugin.publish(destination, PublicationPolicy::NoReplace);
                        // A successful native rename is a separate durable file effect even if
                        // releasing its temporary ownership subsequently fails.
                        if (pluginPublication.state != PublicationState::NotPublished)
                            result.mutations.push_back(
                                {.modRoot = output.modRoot,
                                 .path = destination,
                                 .kind = ArchiveFinalizationMutationKind::PluginCreation,
                                 .mutation = MutationState::Committed});
                        if (pluginPublication.state != PublicationState::PublishedAndReleased)
                            throw std::runtime_error(pluginPublication.errorDetail.empty()
                                                     ? "Loading plugin publication did not complete."
                                                     : pluginPublication.errorDetail);
                    }
                }
#ifdef _WIN32
                // Retain the selected entry after discovery or publication; acquiring the pin
                // must finish before source deletion, and any race during acquisition fails safe.
                if (plan.deleteSources)
                    loadingPluginPin.emplace(loadedElsewhere == output.loadingPluginPaths.end()
                                                 ? destination
                                                 : *loadedElsewhere,
                                             output.modRoot);
#endif
            }
            boundary = ArchiveFinalizationFailure::SourceCleanupFailed;
            if (plan.deleteSources) {
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
        } catch (const EvidenceRecordingFailed&) {
            // A capacity rejection records its attempt inside this block; Run Evidence failures
            // must reach run() unchanged rather than becoming this output's Operation Failure.
            throw;
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
        result.safeToContinue = attempt.safeToContinue;
        result.attempts.push_back(std::move(attempt));
        // Evidence owns each atomic result before the next output can start or throw.
        recorder.attempt(result.attempts.back());
        if (!result.safeToContinue) break;
    }
    result.cancelled = result.cancelled || stop.stop_requested();
    if (!result.cancelled && result.safeToContinue) {
        try {
            for (auto rootIt = plan.roots.begin(); rootIt != plan.roots.end(); ++rootIt) {
                if (stop.stop_requested()) {
                    result.cancelled = true;
                    break;
                }
                const auto& root = *rootIt;
                if (plan.dummyCapacityByRoot.at(root) != 0) {
                    ArchiveVolumeCapacityRequirements remainingDummy(cachedVolumeIdentity);
                    for (auto remaining = rootIt; remaining != plan.roots.end(); ++remaining)
                        remainingDummy.add(*remaining, plan.dummyCapacityByRoot.at(*remaining));
                    // Earlier roots have already consumed their plugin allowance, so rechecking
                    // it would reject later roots despite a sufficient initial phase budget.
                    if (!hasCapacity(root, {}, remainingDummy.requiredAt(root))) return;
                }
                // Capacity and directory probes may block while a cancellation arrives; do not
                // start the next root's plugin mutation after either probe completes.
                if (stop.stop_requested()) {
                    result.cancelled = true;
                    break;
                }
                if (plan.createDummies && plan.settings.s_dummy_plugin) {
                    const auto archives =
                        btu::bsa::list_archive(fs::directory_iterator(root), {}, plan.settings);
                    for (const auto& archive : archives) {
                        if (stop.stop_requested()) {
                            result.cancelled = true;
                            break;
                        }
                        auto chosen = archive;
                        chosen.ext = plan.settings.plugin_extensions.back();
                        chosen.suffix.clear();
                        const auto destination = chosen.full_path();
                        bool publicationStarted = false;
                        try {
                            bool loaded = false;
                            for (const auto& extension : plan.settings.plugin_extensions) {
                                auto candidate = archive;
                                candidate.ext = extension;
                                loaded = loaded || fs::is_regular_file(candidate.full_path());
                                candidate.suffix.clear();
                                loaded = loaded || fs::is_regular_file(candidate.full_path());
                            }
                            if (loaded) continue;
                            // A non-plugin entry at the chosen name must survive even if the
                            // earlier read-only probe did not recognize it as a Loading Plugin.
                            if (fs::exists(fs::symlink_status(destination)))
                                throw std::runtime_error("The loading plugin name is occupied.");

                            auto staged = artifacts.stageArchiveFileForPublication(root);
                            const auto& bytes = *plan.settings.s_dummy_plugin;
                            std::ofstream file(staged.path(), std::ios::binary | std::ios::trunc);
                            file.write(reinterpret_cast<const char*>(bytes.data()),
                                       static_cast<std::streamsize>(bytes.size()));
                            file.close();
                            if (!file)
                                throw std::runtime_error("Could not stage the loading plugin.");
                            publicationStarted = true;
                            const auto published =
                                staged.publish(destination, PublicationPolicy::NoReplace);
                            publicationStarted = false;
                            // The receipt, rather than a later directory listing, identifies
                            // the exact committed path even if ownership release then fails.
                            if (published.state != PublicationState::NotPublished)
                                result.mutations.push_back(
                                    {.modRoot = root,
                                     .path = destination,
                                     .kind = ArchiveFinalizationMutationKind::PluginCreation,
                                     .mutation = MutationState::Committed});
                            if (published.state != PublicationState::PublishedAndReleased) {
                                result.failure = ArchiveFinalizationFailure::PluginCreationFailed;
                                result.safeToContinue =
                                    published.state == PublicationState::NotPublished;
                                result.detail = published.errorDetail.empty()
                                                    ? "Loading plugin publication did not complete."
                                                    : published.errorDetail;
                                result.cancelled = stop.stop_requested();
                                return;
                            }
                        } catch (const std::exception& error) {
                            result.failure = ArchiveFinalizationFailure::PluginCreationFailed;
                            result.detail = error.what();
                            if (publicationStarted) {
                                // An exception escaping publication gives no receipt, so the
                                // destination effect cannot be classified by a later listing.
                                result.mutations.push_back(
                                    {.modRoot = root,
                                     .path = destination,
                                     .kind = ArchiveFinalizationMutationKind::PluginCreation,
                                     .mutation = MutationState::PartialOrUnknown});
                                result.safeToContinue = false;
                            }
                            result.cancelled = stop.stop_requested();
                            return;
                        } catch (...) {
                            result.failure = ArchiveFinalizationFailure::PluginCreationFailed;
                            result.detail = "Unexpected loading plugin creation exception.";
                            result.mutations.push_back(
                                {.modRoot = root,
                                 .path = destination,
                                 .kind = ArchiveFinalizationMutationKind::PluginCreation,
                                 .mutation = MutationState::PartialOrUnknown});
                            result.safeToContinue = false;
                            result.cancelled = stop.stop_requested();
                            return;
                        }
                    }
                    if (result.cancelled) break;
                } else if (!plan.createDummies && plan.settings.s_dummy_plugin) {
                    try {
                        const auto plugins = btu::bsa::list_plugins(fs::directory_iterator(root),
                                                                    {}, plan.settings);
                        // Reserve evidence before any deletion, so recording a completed action
                        // cannot allocate after its native disposition succeeds.
                        result.mutations.reserve(result.mutations.size() + plugins.size());
                        for (const auto& plugin : plugins) {
                            if (stop.stop_requested()) {
                                result.cancelled = true;
                                break;
                            }
                            const auto path = plugin.full_path();
                            ArchiveFinalizationMutation mutation{
                                .modRoot = root,
                                .path = path,
                                .kind = ArchiveFinalizationMutationKind::PluginRemoval,
                                .mutation = MutationState::Committed};
                            const auto removal =
                                removeExactDummyPlugin(path, root, *plan.settings.s_dummy_plugin);
                            if (removal == DummyPluginRemoval::NotDummy) continue;
                            if (removal == DummyPluginRemoval::Uncertain) {
                                mutation.mutation = MutationState::PartialOrUnknown;
                                result.mutations.push_back(std::move(mutation));
                                result.failure = ArchiveFinalizationFailure::PluginRemovalFailed;
                                result.safeToContinue = false;
                                result.detail = "Dummy Plugin removal may be incomplete.";
                                result.cancelled = stop.stop_requested();
                                return;
                            }
                            result.mutations.push_back(std::move(mutation));
                        }
                    } catch (const std::exception& error) {
                        result.failure = ArchiveFinalizationFailure::PluginRemovalFailed;
                        result.detail = error.what();
                        result.cancelled = stop.stop_requested();
                        return;
                    } catch (...) {
                        result.failure = ArchiveFinalizationFailure::PluginRemovalFailed;
                        result.detail = "Unexpected Dummy Plugin removal exception.";
                        result.cancelled = stop.stop_requested();
                        return;
                    }
                    if (result.cancelled) break;
                }
            }
            // All planned outputs and plugin work must finish before pruning any Mod Root.
            // Recoverable attempts retain their source evidence; cancellation retains all paths.
            if (!result.cancelled) pruneEmptyDirectories(plan.roots, stop, result);
        } catch (const EvidenceRecordingFailed&) {
            // Keep Run Evidence failures distinct from this cleanup pass's own exceptions.
            throw;
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
}
}  // namespace

ArchiveFinalization::ArchiveFinalization(OptimizerProfileSnapshot profile,
                                         const ArchiveFinalizationSettings settings,
                                         CapacityProbe capacity,
                                         VolumeIdentityProbe volumeIdentity)
    : _profile(std::move(profile)),
      _settings(settings),
      _capacity(std::move(capacity)),
      _volumeIdentity(std::move(volumeIdentity)) {
    // The profile snapshot already loaded FilesToNotPack.txt during Preparing, so this only
    // converts its lines to the separator form that packing compares against.
    auto lines = _profile.filesToNotPack;
    for (auto& line : lines) line = QDir::toNativeSeparators(line);

    for (auto&& line : lines)
        _filesToNotPack.emplace_back(btu::common::as_utf8_string(std::move(line).toStdString()));

    if (_filesToNotPack.empty()) {
        PLOG_ERROR << "FilesToNotPack.txt not found. This can cause a number of issues. For "
                      "example, for Skyrim, "
                      "animations will be packed to BSA, preventing them from being detected "
                      "by FNIS and Nemesis.";
    }
}

void ArchiveFinalization::run(const RunPreparation& preparation, RunWorkEvidence& evidence,
                              TemporaryArtifactRegistry& artifacts,
                              const std::stop_token stop) const {
    EvidenceRecorder recorder(evidence);
    ArchiveFinalizationResult result;
    try {
        if (!preparation.policy().requests(routing::RequestedWork::ArchiveCreation)) {
            // Empty-directory pruning is the legacy Apply finalization even without packing.
            recorder.plan(0);
            pruneEmptyDirectories(preparation.modRoots(), stop, result);
        } else {
            std::optional<ArchiveFinalizationPlan> plan;
            try {
                plan.emplace(planFinalization(preparation.modRoots(), _settings,
                                              archiveSettings(_profile), _filesToNotPack, stop));
            } catch (const ArchiveFinalizationPlanningCancelled&) {
                // Planning made no mutations or trustworthy output total before cancellation.
                result.cancelled = true;
                recorder.result(std::move(result));
                return;
            }
            recorder.plan(plan->outputs.size());
            finalizePlan(*plan, artifacts, stop, _capacity, _volumeIdentity, recorder, result);
        }
        recorder.result(std::move(result));
        return;
    } catch (const EvidenceRecordingFailed& failure) {
        std::rethrow_exception(failure.error);
    } catch (const std::exception& error) {
        result.detail = error.what();
    } catch (...) {
        result.detail = "Unexpected Archive finalization exception.";
    }
    // The escaped exception gives no reliable boundary for further durable effects. Every
    // attempt in result was already recorded, so the phase-level failure keeps that prefix.
    result.failure = ArchiveFinalizationFailure::UnexpectedException;
    result.safeToContinue = false;
    // Recorded outside the guarded region so a Run Evidence failure propagates unchanged.
    evidence.recordArchiveFinalization(std::move(result));
}
}  // namespace cao::run
