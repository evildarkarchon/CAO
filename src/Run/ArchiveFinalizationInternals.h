#pragma once

// Private parts of the Archive Finalization Run Phase. Only the phase's own translation units
// include this header; nothing declared here is part of ArchiveFinalization.h's interface, and
// tests observe it only through ArchiveFinalization::run.

#include "Run/ArchiveCapacity.h"
#include "Run/ArchiveFinalization.h"
#include "Run/ArchiveFinalizationResult.h"

#include <btu/bsa/archive_data.hpp>
#include <btu/bsa/plugin.hpp>
#include <btu/bsa/settings.hpp>

#include <cstddef>
#include <cstdint>
#include <filesystem>
#include <map>
#include <optional>
#include <span>
#include <stdexcept>
#include <stop_token>
#include <string>
#include <vector>

namespace cao::run {
class TemporaryArtifactRegistry;
}

namespace cao::run::archive_finalization {
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

// --- Planning (ArchiveFinalizationPlanning.cpp) ---

/// Freezes output names and source partitions for all ordered Mod Roots without mutation.
/// Polls stop between inputs and throws ArchiveFinalizationPlanningCancelled instead of
/// publishing an incomplete plan. Other unreadable or unplannable inputs also throw.
[[nodiscard]] ArchiveFinalizationPlan planFinalization(
    std::span<const std::filesystem::path> roots, const ArchiveFinalizationSettings& choices,
    btu::bsa::Settings bsaSettings, std::span<const std::u8string> filesToNotPack,
    std::stop_token stop);

/// Estimates source content and format overhead, plus the fallback Dummy Plugin allowance when
/// the output maintains Loading Plugins; planning can stop between source stats, while
/// finalization intentionally finishes each atomic output attempt once it starts. Throws
/// ArchiveFinalizationPlanningCancelled on stop, or filesystem_error when a source cannot be sized.
[[nodiscard]] std::uintmax_t estimatePackedCapacity(const ArchiveFinalizationOutput& output,
                                                    const btu::bsa::Settings& settings,
                                                    std::stop_token stop = {});

// --- Capacity (ArchiveFinalization.cpp) ---

/// Samples a Mod Root's capacity against an estimate. Returns the rejection detail when the
/// estimate is known not to fit; nullopt when it fits or capacity is unknown, including when
/// the probe itself fails.
[[nodiscard]] std::optional<std::string> capacityShortfall(const CapacityProbe& capacity,
                                                           const std::filesystem::path& root,
                                                           std::uintmax_t required);

// --- Per-output attempt (ArchiveFinalizationAttempt.cpp) ---

/// Runs one planned output's atomic attempt: capacity recheck, Archive write and no-replace
/// publication, its Loading Plugin, then packed-source cleanup, without mid-attempt
/// cancellation. Never throws for the attempt's own failures; they are reported in the returned
/// attempt. A capacity rejection returns an InsufficientCapacity attempt before any mutation.
/// The Archive's mutation and continuation verdict come from its publication result; a release
/// failure keeps the packed sources.
/// dummyReserve is the Loading Plugin allowance still needed on this output's volume. Plugin
/// publication facts are appended to mutations as they happen, so the caller keeps them even if
/// the attempt later fails.
[[nodiscard]] ArchiveFinalizationAttempt attemptOutput(
    const ArchiveFinalizationPlan& plan, std::size_t index, TemporaryArtifactRegistry& artifacts,
    const CapacityProbe& capacity, std::uintmax_t dummyReserve,
    std::vector<ArchiveFinalizationMutation>& mutations);

// --- Loading Plugin maintenance (ArchiveFinalizationLoadingPlugins.cpp) ---

/// How strong the evidence must be before a recognized name counts as loading an Archive.
enum class LoadingPluginStrength {
    /// Any regular file at a recognized name, following links. Used by planning and by
    /// existing-Archive maintenance.
    Present,
    /// An ordinary, non-empty file that is not a link. Attribute-only reparse writes can
    /// retarget a symlink through a read pin, so deleting Loose Assets on Windows needs this.
    DurableForSourceDeletion,
};

/// Lists the profile-recognized Loading Plugin names for an Archive: each plugin extension with
/// and then without the Archive suffix. The last entry is the suffix-free Dummy Plugin
/// destination. The list may repeat a name when the Archive has no suffix.
[[nodiscard]] std::vector<std::filesystem::path> loadingPluginNames(
    const btu::bsa::FilePath& archive, const btu::bsa::Settings& settings);

/// Returns the first recognized name that loads the Archive at the given strength, skipping every
/// name equal to excluded. Probes stop at the first match. Throws filesystem_error on probe errors.
[[nodiscard]] std::optional<std::filesystem::path> findLoadingPlugin(
    std::span<const std::filesystem::path> names, LoadingPluginStrength strength,
    const std::filesystem::path& excluded = {});

/// Recognizes only an ordinary plugin whose complete bytes match this profile's canonical dummy.
/// An unverified file remains a Loading Plugin name during planning, not a proven dummy.
[[nodiscard]] bool hasExactDummyBytes(const std::filesystem::path& path,
                                      const std::vector<std::uint8_t>& bytes);

/// Rechecks a published output's recognized names and publishes the fallback Dummy Plugin when
/// no other Loading Plugin loads it. Returns the entry that loads the Archive, which the caller
/// pins before source deletion. Requires a non-empty output.loadingPluginPaths. Throws on any
/// failure; a plugin publication with durable effects is appended to mutations first.
[[nodiscard]] std::filesystem::path ensureOutputLoadingPlugin(
    const ArchiveFinalizationPlan& plan, const ArchiveFinalizationOutput& output,
    TemporaryArtifactRegistry& artifacts, std::vector<ArchiveFinalizationMutation>& mutations);

/// Maintains Loading Plugins for every existing Archive in one Mod Root: creates missing Dummy
/// Plugins when requested, otherwise removes only exact Dummy Plugins. Each plugin action is a
/// separate mutation fact, never an output attempt. Returns false when finalization must stop
/// without pruning, after recording a plugin failure or cancellation in result. When creating,
/// an exception from listing the Mod Root's Archives escapes to the caller; removal records any
/// listing failure as PluginRemovalFailed.
[[nodiscard]] bool maintainExistingLoadingPlugins(const ArchiveFinalizationPlan& plan,
                                                  const std::filesystem::path& root,
                                                  TemporaryArtifactRegistry& artifacts,
                                                  std::stop_token stop,
                                                  ArchiveFinalizationResult& result);
}  // namespace cao::run::archive_finalization
