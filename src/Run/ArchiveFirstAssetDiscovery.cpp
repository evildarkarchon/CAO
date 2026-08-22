#include "ArchiveFirstAssetDiscovery.h"

#include <btu/bsa/unpack.hpp>

#include <map>
#include <unordered_set>
#include <utility>
#include <variant>

namespace cao::run
{
void extractArchiveNoOverwrite(const std::filesystem::path &archivePath,
                               const bool removeArchive)
{
    // Bethesda gives existing Loose Assets precedence, so Archive extraction must never replace them.
    btu::bsa::unpack(btu::bsa::UnpackSettings{archivePath, removeArchive, false});
}

namespace
{
/// Visits each regular file and tells the visitor whether the path was an explicitly supplied root.
template<typename Visitor>
void visitRegularFiles(const std::span<const std::filesystem::path> roots,
                       Visitor &&visitor)
{
    for (const auto &root : roots) {
        if (std::filesystem::is_regular_file(root)) {
            visitor(root, true);
            continue;
        }

        for (const auto &entry : std::filesystem::recursive_directory_iterator(root)) {
            if (entry.is_regular_file())
                visitor(entry.path(), false);
        }
    }
}
}

EffectiveAssetTree::EffectiveAssetTree(std::vector<std::filesystem::path> paths) noexcept
    : _paths(std::move(paths))
{}

std::span<const std::filesystem::path> EffectiveAssetTree::paths() const noexcept
{
    return _paths;
}

ArchiveFirstAssetDiscoveryResult::ArchiveFirstAssetDiscoveryResult(
    EffectiveAssetTree effectiveAssetTree,
    std::map<routing::SkipReason, std::size_t> skippedArchiveCounts,
    std::vector<std::filesystem::path> unsupportedExplicitPaths) noexcept
    : _effectiveAssetTree(std::move(effectiveAssetTree))
    , _skippedArchiveCounts(std::move(skippedArchiveCounts))
    , _unsupportedExplicitPaths(std::move(unsupportedExplicitPaths))
{}

const EffectiveAssetTree &ArchiveFirstAssetDiscoveryResult::effectiveAssetTree() const noexcept
{
    return _effectiveAssetTree;
}

std::size_t ArchiveFirstAssetDiscoveryResult::skippedArchiveCount(
    const routing::SkipReason reason) const noexcept
{
    const auto count = _skippedArchiveCounts.find(reason);
    return count == _skippedArchiveCounts.end() ? 0 : count->second;
}

std::span<const std::filesystem::path>
ArchiveFirstAssetDiscoveryResult::unsupportedExplicitPaths() const noexcept
{
    return _unsupportedExplicitPaths;
}

ArchiveFirstAssetDiscovery::ArchiveFirstAssetDiscovery(
    routing::RoutingPolicy policy) noexcept
    : _policy(std::move(policy))
{}

ArchiveFirstAssetDiscoveryResult ArchiveFirstAssetDiscovery::discover(
    const std::span<const std::filesystem::path> roots,
    const ArchiveExtractionOperation &extractArchive) const
{
    routing::AssetRouter router(_policy);
    std::unordered_set<std::filesystem::path> recognizedArchivePaths;
    std::vector<routing::RoutedAsset> selectedArchives;
    std::map<routing::SkipReason, std::size_t> skippedArchiveCounts;
    std::vector<std::filesystem::path> unsupportedExplicitPaths;
    visitRegularFiles(roots, [&](const std::filesystem::path &path, const bool explicitRoot) {
        auto decision = router.route(path);
        if (auto *routedAsset = std::get_if<routing::RoutedAsset>(&decision)) {
            if (routedAsset->kind() == routing::AssetKind::Archive
                && recognizedArchivePaths.insert(path.lexically_normal()).second) {
                selectedArchives.push_back(std::move(*routedAsset));
            }
            return;
        }
        if (const auto *skippedAsset = std::get_if<routing::SkippedAsset>(&decision)) {
            if (skippedAsset->kind() == routing::AssetKind::Archive
                && recognizedArchivePaths.insert(path.lexically_normal()).second) {
                ++skippedArchiveCounts[skippedAsset->reason()];
            }
            return;
        }
        if (explicitRoot)
            unsupportedExplicitPaths.push_back(path);
    });

    if (!selectedArchives.empty())
        extractArchive(selectedArchives);

    // Extraction is synchronous so this is the single definitive view of all non-Archive paths.
    std::vector<std::filesystem::path> effectivePaths;
    visitRegularFiles(roots, [&](const std::filesystem::path &path, const bool) {
        if (!recognizedArchivePaths.contains(path.lexically_normal()))
            effectivePaths.push_back(path);
    });
    return ArchiveFirstAssetDiscoveryResult(
        EffectiveAssetTree(std::move(effectivePaths)),
        std::move(skippedArchiveCounts),
        std::move(unsupportedExplicitPaths));
}
}
