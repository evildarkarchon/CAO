#pragma once

#include "AssetRouting/AssetRouter.h"

#include <filesystem>
#include <functional>
#include <span>
#include <vector>

namespace cao::run
{
using ArchiveExtractionOperation = std::function<void(const routing::RoutedAsset &)>;

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

/// Orchestrates Archive selection and extraction before one definitive Effective Asset Tree traversal.
class ArchiveFirstAssetDiscovery final
{
public:
    /// Owns an immutable policy copy used to recognize and enable Archive extraction.
    explicit ArchiveFirstAssetDiscovery(routing::RoutingPolicy policy) noexcept;

    /// Extracts enabled Archives synchronously, then traverses roots once for definitive paths.
    /// The extraction operation must return only after each Archive is available beneath its root.
    /// Roots must not overlap; each filesystem occurrence is preserved in traversal order.
    [[nodiscard]] EffectiveAssetTree discover(
        std::span<const std::filesystem::path> roots,
        const ArchiveExtractionOperation &extractArchive) const;

private:
    routing::RoutingPolicy _policy;
};
}
