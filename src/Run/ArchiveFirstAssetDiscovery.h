#pragma once

#include "AssetRouting/AssetRouter.h"

#include <filesystem>
#include <functional>
#include <map>
#include <span>
#include <vector>

namespace cao::run {
using ArchiveExtractionOperation = std::function<bool(std::span<const routing::RoutedAsset>)>;

/// Extracts one Archive through bethutil while preserving every existing Loose Asset.
/// When removeArchive is true, the Archive is removed only according to bethutil's extraction
/// contract.
void extractArchiveNoOverwrite(const std::filesystem::path& archivePath, bool removeArchive);

/// Owns the definitive non-Archive filesystem paths discovered after Archive extraction.
class EffectiveAssetTree final {
   public:
    /// Returns paths in traversal order; the view remains valid for this tree's lifetime.
    [[nodiscard]] std::span<const std::filesystem::path> paths() const noexcept;

   private:
    friend class ArchiveFirstAssetDiscovery;

    /// Takes ownership of paths collected by one definitive post-extraction traversal.
    explicit EffectiveAssetTree(std::vector<std::filesystem::path> paths) noexcept;

    std::vector<std::filesystem::path> _paths;
};

/// Owns the Effective Asset Tree plus Archive-pass exclusions and explicit unsupported roots.
class ArchiveFirstAssetDiscoveryResult final {
   public:
    /// Returns the definitive post-extraction non-Archive tree.
    [[nodiscard]] const EffectiveAssetTree& effectiveAssetTree() const noexcept;

    /// Returns the number of recognized Archives excluded for one stable Skip Reason.
    [[nodiscard]] std::size_t skippedArchiveCount(routing::SkipReason reason) const noexcept;

    /// Returns unsupported roots supplied explicitly as files; directory entries remain absent.
    [[nodiscard]] std::span<const std::filesystem::path> unsupportedExplicitPaths() const noexcept;

    /// Returns how many distinct Archives appeared only after extraction.
    ///
    /// The game reads no Archive nested inside another, so each one counted here is malformed mod
    /// content the run deliberately left alone. Only the count is kept: a malformed Archive can
    /// hold arbitrarily many, and the actionable fact is that the run found any at all.
    [[nodiscard]] std::size_t nestedArchiveCount() const noexcept;

   private:
    friend class ArchiveFirstAssetDiscovery;

    /// Takes ownership of all Archive-first discovery outcomes.
    ArchiveFirstAssetDiscoveryResult(
        EffectiveAssetTree effectiveAssetTree,
        std::map<routing::SkipReason, std::size_t> skippedArchiveCounts,
        std::vector<std::filesystem::path> unsupportedExplicitPaths,
        std::size_t nestedArchiveCount) noexcept;

    EffectiveAssetTree _effectiveAssetTree;
    std::map<routing::SkipReason, std::size_t> _skippedArchiveCounts;
    std::vector<std::filesystem::path> _unsupportedExplicitPaths;
    std::size_t _nestedArchiveCount;
};

/// Orchestrates Archive selection and extraction before one definitive Effective Asset Tree
/// traversal.
class ArchiveFirstAssetDiscovery final {
   public:
    /// Owns an immutable policy copy used to recognize and enable Archive extraction.
    explicit ArchiveFirstAssetDiscovery(routing::RoutingPolicy policy) noexcept;

    /// Selects enabled Archives, passes the complete batch for synchronous extraction, then
    /// traverses roots once for definitive paths. An Archive supplied directly as a root also
    /// contributes the Assets extraction newly produced in its containing directory; Assets that
    /// already existed there were never named by the caller and stay out of the tree. No Archive
    /// ever enters the tree, including one that appears only after extraction: the game does not
    /// read an Archive nested inside another, so such a file is malformed mod content rather than
    /// deferred work. Extraction is deliberately never repeated to reach it; it is counted by
    /// `nestedArchiveCount` instead. The
    /// operation returns false when extraction was cancelled, which skips definitive traversal.
    /// Filesystem races and permission failures are skipped during traversal; extraction
    /// exceptions propagate to the caller.
    /// Roots must not overlap; each filesystem occurrence is preserved in traversal order.
    [[nodiscard]] ArchiveFirstAssetDiscoveryResult discover(
        std::span<const std::filesystem::path> roots,
        const ArchiveExtractionOperation& extractArchive) const;

   private:
    routing::RoutingPolicy _policy;
};
}  // namespace cao::run
