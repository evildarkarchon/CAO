#pragma once

#include "AssetExecution/AssetExecutor.h"
#include "ArchiveCapacity.h"

namespace cao::run {
/// Normalizes a manifest name to a case-folded game path, rejecting unsafe or reserved names.
/// Throws invalid_argument for invalid paths and runtime_error for invalid UTF-8.
[[nodiscard]] std::string canonicalArchiveEntryPath(std::string name);

/// Owns raw manifest names and estimated staging bytes, including shadowed payloads and overhead.
struct ArchiveInventory final {
    std::vector<std::string> names;
    std::uintmax_t estimatedCapacityBytes{65536};
};

/// Reads raw names and decompressed sizes without extracting. Throws on unreadable containers.
/// The overhead allowance is conservative, not a guarantee of filesystem allocation or free space.
[[nodiscard]] ArchiveInventory inspectArchiveInventory(const std::filesystem::path& path);

/// Owns the manifest paths and precedence winners frozen before any Archive is extracted.
/// Entry names are canonical paths relative to the Archive's containing directory.
struct ArchiveExtractionPlan final {
    std::filesystem::path archivePath;
    std::filesystem::path modRoot;
    std::vector<std::string> entries{};
    std::vector<std::string> mergeEntries{};
    std::uintmax_t estimatedCapacityBytes{};
};

enum class ArchiveExtractionFailure {
    ExtractionFailed,
    MergeFailed,
    SourceCleanupFailed,
    InsufficientCapacity
};

/// Owns one Archive attempt's durable mutation evidence; staging bytes do not count as mutation.
struct ArchiveExtractionResult final {
    std::filesystem::path archivePath;
    execution::MutationState mutation{execution::MutationState::None};
    std::optional<ArchiveExtractionFailure> failure{};
    bool safeToContinue{true};
    std::string detail{};
    /// The frozen preflight scope, independent of the Archive's containing directory.
    std::filesystem::path modRoot{};

    /// Reports whether extraction, merge, and any requested source cleanup completed.
    [[nodiscard]] bool succeeded() const noexcept { return !failure; }
};

/// Stages complete Archives before merging only preflight winners, retaining the source Archive.
/// The caller owns the registry through Safety Cleanup and serializes attempts on this service.
class ArchiveExtractor final {
   public:
    /// Borrows the run's durable temporary ownership until this extractor is destroyed.
    explicit ArchiveExtractor(TemporaryArtifactRegistry& artifacts,
                              CapacityProbe capacity = availableArchiveCapacity)
        : _artifacts(artifacts), _capacity(std::move(capacity)) {}

    /// Returns contained extraction failures or unsafe merge failures without throwing library
    /// exceptions. Merge never replaces existing entries; source backup/deletion belongs to
    /// callers.
    [[nodiscard]] ArchiveExtractionResult extract(const ArchiveExtractionPlan& plan) const;

   private:
    TemporaryArtifactRegistry& _artifacts;
    CapacityProbe _capacity;
};
}  // namespace cao::run
