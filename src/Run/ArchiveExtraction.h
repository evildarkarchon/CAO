#pragma once

#include "AssetExecution/AssetExecutor.h"

namespace cao::run {
/// Normalizes a manifest name to a case-folded game path, rejecting unsafe or reserved names.
/// Throws invalid_argument for invalid paths and runtime_error for invalid UTF-8.
[[nodiscard]] std::string canonicalArchiveEntryPath(std::string name);

/// Owns the manifest paths and precedence winners frozen before any Archive is extracted.
/// Entry names are canonical paths relative to the Archive's containing directory.
struct ArchiveExtractionPlan final {
    std::filesystem::path archivePath;
    std::filesystem::path modRoot;
    std::vector<std::string> entries{};
    std::vector<std::string> mergeEntries{};
};

enum class ArchiveExtractionFailure { ExtractionFailed, MergeFailed, SourceCleanupFailed };

/// Owns one Archive attempt's durable mutation evidence; staging bytes do not count as mutation.
struct ArchiveExtractionResult final {
    std::filesystem::path archivePath;
    execution::MutationState mutation{execution::MutationState::None};
    std::optional<ArchiveExtractionFailure> failure{};
    bool safeToContinue{true};
    std::string detail{};

    /// Reports whether extraction, merge, and any requested source cleanup completed.
    [[nodiscard]] bool succeeded() const noexcept { return !failure; }
};

/// Stages complete Archives before merging only preflight winners, retaining the source Archive.
/// The caller owns the registry through Safety Cleanup and serializes attempts on this service.
class ArchiveExtractor final {
   public:
    /// Borrows the run's durable temporary ownership until this extractor is destroyed.
    explicit ArchiveExtractor(TemporaryArtifactRegistry& artifacts) : _artifacts(artifacts) {}

    /// Returns contained extraction failures or unsafe merge failures without throwing library
    /// exceptions. Merge never replaces existing entries; source backup/deletion belongs to callers.
    [[nodiscard]] ArchiveExtractionResult extract(const ArchiveExtractionPlan& plan) const;

   private:
    TemporaryArtifactRegistry& _artifacts;
};
}  // namespace cao::run
