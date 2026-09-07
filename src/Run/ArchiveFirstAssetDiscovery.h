#pragma once

#include "AssetRouting/AssetRouter.h"
#include "RunLifecycle.h"

#include <filesystem>
#include <functional>
#include <map>
#include <span>
#include <vector>

namespace cao::run {
using ArchiveExtractionOperation = std::function<bool(std::span<const routing::RoutedAsset>)>;
using AssetDiscoveryCancellationPredicate = std::function<bool()>;

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
    /// Returns the definitive post-extraction non-Archive tree, or an empty tree on cancellation.
    [[nodiscard]] const EffectiveAssetTree& effectiveAssetTree() const noexcept;

    /// Reports interruption during traversal or extraction; partial trees are never returned.
    [[nodiscard]] bool cancelled() const noexcept;

    /// Borrows structured exclusions in first-observation order; each linked entry appears once.
    [[nodiscard]] std::span<const RunDiagnostic> diagnostics() const noexcept { return _diagnostics; }

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
        std::size_t nestedArchiveCount, std::vector<RunDiagnostic> diagnostics,
        bool cancelled = false) noexcept;

    EffectiveAssetTree _effectiveAssetTree;
    std::map<routing::SkipReason, std::size_t> _skippedArchiveCounts;
    std::vector<std::filesystem::path> _unsupportedExplicitPaths;
    std::size_t _nestedArchiveCount;
    bool _cancelled;
    std::vector<RunDiagnostic> _diagnostics;
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
    /// `nestedArchiveCount` instead. An extraction operation returning false, or the optional
    /// cancellation predicate returning true between filesystem entries, stops discovery and
    /// returns a cancelled result with an empty tree. In-flight extraction is never interrupted.
    /// Filesystem races and permission failures are skipped during traversal; extraction
    /// exceptions propagate to the caller.
    /// Selected directory aliases resolve once before discovery. Directory links within a tree
    /// are never followed; unresolved and escaping file links are skipped with Run Diagnostics.
    /// Roots must not overlap. Archives retain root order and sort within each root by normalized
    /// relative UTF-8 path, case-folded first with an ordinal spelling tie-breaker. Dry Run only
    /// counts disabled Archives: no manifest inspection or extraction is performed.
    [[nodiscard]] ArchiveFirstAssetDiscoveryResult discover(
        std::span<const std::filesystem::path> roots,
        const ArchiveExtractionOperation& extractArchive,
        const AssetDiscoveryCancellationPredicate& isCancelled = {}) const;

   private:
    routing::RoutingPolicy _policy;
};
}  // namespace cao::run
