#pragma once

#include "AssetRouting/AssetRouter.h"

#include <filesystem>
#include <functional>
#include <map>
#include <span>
#include <vector>

namespace cao::run
{
using ArchiveExtractionOperation =
    std::function<bool(std::span<const routing::RoutedAsset>)>;

/// Extracts one Archive through bethutil while preserving every existing Loose Asset.
/// When removeArchive is true, the Archive is removed only according to bethutil's extraction contract.
void extractArchiveNoOverwrite(const std::filesystem::path &archivePath,
                               bool removeArchive);

/// Owns the definitive non-Archive filesystem paths discovered after Archive extraction.
class EffectiveAssetTree final
{
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
class ArchiveFirstAssetDiscoveryResult final
{
public:
    /// Returns the definitive post-extraction non-Archive tree.
    [[nodiscard]] const EffectiveAssetTree &effectiveAssetTree() const noexcept;

    /// Returns the number of recognized Archives excluded for one stable Skip Reason.
    [[nodiscard]] std::size_t skippedArchiveCount(routing::SkipReason reason) const noexcept;

    /// Returns unsupported roots supplied explicitly as files; directory entries remain absent.
    [[nodiscard]] std::span<const std::filesystem::path> unsupportedExplicitPaths() const noexcept;

private:
    friend class ArchiveFirstAssetDiscovery;

    /// Takes ownership of all Archive-first discovery outcomes.
    ArchiveFirstAssetDiscoveryResult(
        EffectiveAssetTree effectiveAssetTree,
        std::map<routing::SkipReason, std::size_t> skippedArchiveCounts,
        std::vector<std::filesystem::path> unsupportedExplicitPaths) noexcept;

    EffectiveAssetTree _effectiveAssetTree;
    std::map<routing::SkipReason, std::size_t> _skippedArchiveCounts;
    std::vector<std::filesystem::path> _unsupportedExplicitPaths;
};

/// Orchestrates Archive selection and extraction before one definitive Effective Asset Tree traversal.
class ArchiveFirstAssetDiscovery final
{
public:
    /// Owns an immutable policy copy used to recognize and enable Archive extraction.
    explicit ArchiveFirstAssetDiscovery(routing::RoutingPolicy policy) noexcept;

    /// Selects enabled Archives, passes the complete batch for synchronous extraction, then
    /// traverses roots once for definitive paths. The operation returns false when extraction was
    /// cancelled, which skips definitive traversal. Filesystem races and permission failures are
    /// skipped during traversal; extraction exceptions propagate to the caller.
    /// Roots must not overlap; each filesystem occurrence is preserved in traversal order.
    [[nodiscard]] ArchiveFirstAssetDiscoveryResult discover(
        std::span<const std::filesystem::path> roots,
        const ArchiveExtractionOperation &extractArchive) const;

private:
    routing::RoutingPolicy _policy;
};
}
