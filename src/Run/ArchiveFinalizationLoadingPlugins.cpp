#include "Run/ArchiveFinalizationInternals.h"

#include "Run/NativeFilePins.h"
#include "Run/TemporaryArtifactRegistry.h"

#include <algorithm>
#include <array>
#include <fstream>

#ifdef _WIN32
#include <Windows.h>
#endif

namespace cao::run::archive_finalization {
namespace {
namespace fs = std::filesystem;
using execution::MutationState;

/// Reports whether one recognized name loads its Archive at the given strength.
/// Throws filesystem_error when the entry cannot be inspected.
bool isLoadingPlugin(const fs::path& path, const LoadingPluginStrength strength) {
    if (strength == LoadingPluginStrength::DurableForSourceDeletion)
        return fs::is_regular_file(fs::symlink_status(path)) && fs::file_size(path) != 0;
    // Loading follows links to regular plugins; exact Dummy Plugin recognition does not.
    return fs::is_regular_file(path);
}

/// The outcome of one Dummy Plugin publication attempt after staging succeeded.
struct DummyPluginPublication final {
    /// The publication result's mutation fact (see PublicationResult::mutation).
    MutationState mutation{MutationState::None};
    /// The publication result's continuation verdict (see PublicationResult::safeToContinue).
    bool safeToContinue{true};
    /// True only once the plugin is published and its Temporary Ownership released.
    bool completed{};
    /// Why publication did not complete; empty when it did.
    std::string detail;
};

/// Stages the profile's canonical Dummy Plugin bytes under Temporary Ownership and publishes them
/// at destination with NoReplace, so a late occupant always wins. The mutation and continuation
/// verdict come from the publication result, and a Committed destination is appended to
/// mutations as a PluginCreation fact before returning. Throws only before publication starts,
/// when no destination mutation can have happened; nothing after publication can throw.
DummyPluginPublication publishDummyPlugin(TemporaryArtifactRegistry& artifacts,
                                          const fs::path& modRoot, const fs::path& destination,
                                          const std::vector<std::uint8_t>& bytes,
                                          std::vector<ArchiveFinalizationMutation>& mutations) {
    auto staged = artifacts.stageArchiveFileForPublication(modRoot);
    std::ofstream file(staged.path(), std::ios::binary | std::ios::trunc);
    file.write(reinterpret_cast<const char*>(bytes.data()),
               static_cast<std::streamsize>(bytes.size()));
    file.close();
    if (!file) throw std::runtime_error("Could not stage the loading plugin.");

    // Build and reserve evidence before publishing, so recording a Committed Mutation cannot
    // allocate after the native rename succeeds and an allocation failure cannot hide it.
    // Grow geometrically and only when full: reserve(size() + 1) reallocates exactly on MSVC,
    // which would copy the whole fact list once per plugin across a Several Mods run.
    ArchiveFinalizationMutation created{.modRoot = modRoot,
                                        .path = destination,
                                        .kind = ArchiveFinalizationMutationKind::PluginCreation,
                                        .mutation = MutationState::Committed};
    if (mutations.size() == mutations.capacity())
        mutations.reserve(std::max<std::size_t>(4, mutations.capacity() * 2));
    std::string incomplete = "Loading plugin publication did not complete.";
    auto published = staged.publish(destination, PublicationPolicy::NoReplace);

    DummyPluginPublication outcome;
    outcome.mutation = published.mutation();
    outcome.safeToContinue = published.safeToContinue();
    // The receipt, rather than a later directory listing, identifies the exact committed path.
    // A successful native rename is a separate durable file effect even if releasing its
    // temporary ownership subsequently fails.
    if (outcome.mutation == MutationState::Committed) mutations.push_back(std::move(created));
    if (published.state != PublicationState::PublishedAndReleased) {
        outcome.detail = published.errorDetail.empty() ? std::move(incomplete)
                                                       : std::move(published.errorDetail);
        return outcome;
    }
    outcome.completed = true;
    return outcome;
}

enum class DummyPluginRemoval { NotDummy, Removed, Uncertain };

#ifdef _WIN32
/// Pins ordinary parents and a single-link file while checking canonical bytes and deleting by
/// that handle. Returns NotDummy for other content and Uncertain only after disposition succeeds;
/// all exceptions occur before deletion and therefore describe known no-mutation failures.
DummyPluginRemoval removeExactDummyPlugin(const fs::path& plugin, const fs::path& modRoot,
                                          const std::vector<std::uint8_t>& bytes) {
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
DummyPluginRemoval removeExactDummyPlugin(const fs::path& plugin, const fs::path&,
                                          const std::vector<std::uint8_t>& bytes) {
    if (!hasExactDummyBytes(plugin, bytes)) return DummyPluginRemoval::NotDummy;
    if (!fs::remove(plugin)) throw std::runtime_error("The Dummy Plugin could not be removed.");
    return DummyPluginRemoval::Removed;
}
#endif

/// Publishes a Dummy Plugin for each existing Archive in root that no recognized name loads.
/// Returns false after recording a plugin failure or cancellation in result.
bool createMissingDummyPlugins(const ArchiveFinalizationPlan& plan, const fs::path& root,
                               TemporaryArtifactRegistry& artifacts, const std::stop_token stop,
                               ArchiveFinalizationResult& result) {
    const auto archives = btu::bsa::list_archive(fs::directory_iterator(root), {}, plan.settings);
    for (const auto& archive : archives) {
        if (stop.stop_requested()) {
            result.cancelled = true;
            return false;
        }
        const auto names = loadingPluginNames(archive, plan.settings);
        const auto& destination = names.back();
        try {
            if (findLoadingPlugin(names, LoadingPluginStrength::Present)) continue;
            // A non-plugin entry at the chosen name must survive even if the
            // earlier read-only probe did not recognize it as a Loading Plugin.
            if (fs::exists(fs::symlink_status(destination)))
                throw std::runtime_error("The loading plugin name is occupied.");

            auto published = publishDummyPlugin(artifacts, root, destination,
                                                *plan.settings.s_dummy_plugin, result.mutations);
            if (!published.completed) {
                result.failure = ArchiveFinalizationFailure::PluginCreationFailed;
                // Publication's verdict stands (see PublicationResult::safeToContinue).
                result.safeToContinue = published.safeToContinue;
                // Moving keeps the post-publication path allocation-free, so the catch below
                // still only ever sees exceptions from before publication.
                result.detail = std::move(published.detail);
                result.cancelled = stop.stop_requested();
                return false;
            }
        } catch (const std::exception& error) {
            // Staging and the checks above throw before publication, so nothing was mutated.
            result.failure = ArchiveFinalizationFailure::PluginCreationFailed;
            result.detail = error.what();
            result.cancelled = stop.stop_requested();
            return false;
        } catch (...) {
            // Like the branch above, this predates publication, so no mutation fact is recorded.
            // A non-standard exception's origin cannot be named, though, so this phase stays
            // stricter than the receipt and conservatively marks the run unsafe.
            result.failure = ArchiveFinalizationFailure::PluginCreationFailed;
            result.detail = "Unexpected loading plugin creation exception.";
            result.safeToContinue = false;
            result.cancelled = stop.stop_requested();
            return false;
        }
    }
    return true;
}

/// Removes every exact Dummy Plugin in root through the guarded native removal. Returns false
/// after recording a removal failure or cancellation in result.
bool removeDummyPlugins(const ArchiveFinalizationPlan& plan, const fs::path& root,
                        const std::stop_token stop, ArchiveFinalizationResult& result) {
    try {
        const auto plugins =
            btu::bsa::list_plugins(fs::directory_iterator(root), {}, plan.settings);
        // Reserve evidence before any deletion, so recording a completed action
        // cannot allocate after its native disposition succeeds.
        result.mutations.reserve(result.mutations.size() + plugins.size());
        for (const auto& plugin : plugins) {
            if (stop.stop_requested()) {
                result.cancelled = true;
                return false;
            }
            const auto path = plugin.full_path();
            ArchiveFinalizationMutation mutation{
                .modRoot = root,
                .path = path,
                .kind = ArchiveFinalizationMutationKind::PluginRemoval,
                .mutation = MutationState::Committed};
            const auto removal = removeExactDummyPlugin(path, root, *plan.settings.s_dummy_plugin);
            if (removal == DummyPluginRemoval::NotDummy) continue;
            if (removal == DummyPluginRemoval::Uncertain) {
                mutation.mutation = MutationState::PartialOrUnknown;
                result.mutations.push_back(std::move(mutation));
                result.failure = ArchiveFinalizationFailure::PluginRemovalFailed;
                result.safeToContinue = false;
                result.detail = "Dummy Plugin removal may be incomplete.";
                result.cancelled = stop.stop_requested();
                return false;
            }
            result.mutations.push_back(std::move(mutation));
        }
    } catch (const std::exception& error) {
        result.failure = ArchiveFinalizationFailure::PluginRemovalFailed;
        result.detail = error.what();
        result.cancelled = stop.stop_requested();
        return false;
    } catch (...) {
        result.failure = ArchiveFinalizationFailure::PluginRemovalFailed;
        result.detail = "Unexpected Dummy Plugin removal exception.";
        result.cancelled = stop.stop_requested();
        return false;
    }
    return true;
}
}  // namespace

std::vector<fs::path> loadingPluginNames(const btu::bsa::FilePath& archive,
                                         const btu::bsa::Settings& settings) {
    std::vector<fs::path> names;
    names.reserve(settings.plugin_extensions.size() * 2);
    for (const auto& extension : settings.plugin_extensions) {
        auto plugin = archive;
        plugin.ext = extension;
        names.push_back(plugin.full_path());
        plugin.suffix.clear();
        names.push_back(plugin.full_path());
    }
    return names;
}

std::optional<fs::path> findLoadingPlugin(const std::span<const fs::path> names,
                                          const LoadingPluginStrength strength,
                                          const fs::path& excluded) {
    // Exclusion is by value: without an Archive suffix the destination appears twice.
    const auto found = std::find_if(names.begin(), names.end(), [&](const fs::path& path) {
        return path != excluded && isLoadingPlugin(path, strength);
    });
    if (found == names.end()) return std::nullopt;
    return *found;
}

bool hasExactDummyBytes(const fs::path& path, const std::vector<std::uint8_t>& bytes) {
    std::error_code error;
    if (!fs::is_regular_file(fs::symlink_status(path, error)) || error ||
        fs::file_size(path, error) != bytes.size() || error)
        return false;
    std::ifstream input(path, std::ios::binary);
    std::vector<char> contents(bytes.size());
    input.read(contents.data(), static_cast<std::streamsize>(contents.size()));
    if (!input) return false;
    const auto complete =
        input.peek() == std::char_traits<char>::eof() && input.eof() && !input.bad();
    return complete && std::equal(contents.begin(), contents.end(), bytes.begin(),
                                  [](char left, std::uint8_t right) {
                                      return static_cast<unsigned char>(left) == right;
                                  });
}

fs::path ensureOutputLoadingPlugin(const ArchiveFinalizationPlan& plan,
                                   const ArchiveFinalizationOutput& output,
                                   TemporaryArtifactRegistry& artifacts,
                                   std::vector<ArchiveFinalizationMutation>& mutations) {
    const auto& bytes = *plan.settings.s_dummy_plugin;
    const auto& plugin = output.loadingPluginPaths.back();
    auto strength = LoadingPluginStrength::Present;
#ifdef _WIN32
    // Attribute-only reparse writes can retarget a symlink through a read pin.
    // Source deletion therefore needs an ordinary, nonempty plugin entry.
    if (plan.deleteSources) strength = LoadingPluginStrength::DurableForSourceDeletion;
#endif
    // A plugin at another profile-recognized name can arrive after planning, or a
    // planned one can disappear. The chosen dummy destination is checked separately
    // so a late non-dummy occupant never becomes our successful publication.
    if (const auto loadedElsewhere = findLoadingPlugin(output.loadingPluginPaths, strength, plugin))
        return *loadedElsewhere;

    auto destination = plugin;
    // An occupant at the dummy destination that cannot justify source deletion is left alone;
    // the fallback moves to the first recognized name with no entry at all.
    if (strength == LoadingPluginStrength::DurableForSourceDeletion &&
        fs::exists(fs::symlink_status(plugin)) && !isLoadingPlugin(plugin, strength)) {
        const auto free = std::find_if(
            output.loadingPluginPaths.begin(), output.loadingPluginPaths.end(),
            [&](const fs::path& path) { return !fs::exists(fs::symlink_status(path)); });
        if (free == output.loadingPluginPaths.end())
            throw std::runtime_error("No ordinary loading plugin name is available.");
        destination = *free;
    }
    if (destination == plugin && fs::exists(fs::symlink_status(plugin))) {
        if (output.pluginPath && !hasExactDummyBytes(plugin, bytes))
            throw std::runtime_error("The planned loading plugin is occupied.");
        if (!fs::is_regular_file(plugin))
            throw std::runtime_error("The planned loading plugin is not a file.");
        return destination;
    }
    // Recheck the leaf natively after the exact-dummy probe: a newcomer wins.
    const auto published =
        publishDummyPlugin(artifacts, output.modRoot, destination, bytes, mutations);
    if (!published.completed) throw std::runtime_error(published.detail);
    return destination;
}

bool maintainExistingLoadingPlugins(const ArchiveFinalizationPlan& plan, const fs::path& root,
                                    TemporaryArtifactRegistry& artifacts,
                                    const std::stop_token stop, ArchiveFinalizationResult& result) {
    if (!plan.settings.s_dummy_plugin) return true;
    if (plan.createDummies) return createMissingDummyPlugins(plan, root, artifacts, stop, result);
    return removeDummyPlugins(plan, root, stop, result);
}
}  // namespace cao::run::archive_finalization
