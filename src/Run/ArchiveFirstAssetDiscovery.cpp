#include "ArchiveFirstAssetDiscovery.h"

#include <btu/bsa/unpack.hpp>

#include <algorithm>
#include <map>
#include <system_error>
#include <unordered_set>
#include <utility>
#include <variant>

namespace cao::run {
void extractArchiveNoOverwrite(const std::filesystem::path& archivePath, const bool removeArchive) {
    // Bethesda gives existing Loose Assets precedence, so Archive extraction must never replace
    // them.
    btu::bsa::unpack(btu::bsa::UnpackSettings{archivePath, removeArchive, false});
}

namespace {
/// Visits each regular file and tells the visitor whether the path was an explicitly supplied root.
/// Returns false as soon as cancellation is observed before a root or directory entry.
template <typename Visitor>
bool visitRegularFiles(const std::span<const std::filesystem::path> roots,
                       const AssetDiscoveryCancellationPredicate& isCancelled, Visitor&& visitor) {
    for (const auto& root : roots) {
        if (isCancelled && isCancelled()) return false;
        std::error_code error;
        if (std::filesystem::is_regular_file(root, error)) {
            visitor(root, true);
            continue;
        }

        // A mod tree can change while an Archive adapter runs, so traversal treats disappeared or
        // unreadable entries as absent from this pass instead of escaping the GUI worker.
        error.clear();
        auto entry = std::filesystem::recursive_directory_iterator(
            root, std::filesystem::directory_options::skip_permission_denied, error);
        const auto end = std::filesystem::recursive_directory_iterator();
        while (entry != end) {
            // Poll even for directories and unsupported files: they may comprise the entire tree,
            // so neither extraction nor execution is guaranteed to offer a cancellation seam.
            if (isCancelled && isCancelled()) return false;
            error.clear();
            if (entry->is_regular_file(error)) visitor(entry->path(), false);

            error.clear();
            entry.increment(error);
        }
    }
    return true;
}
}  // namespace

EffectiveAssetTree::EffectiveAssetTree(std::vector<std::filesystem::path> paths) noexcept
    : _paths(std::move(paths)) {}

std::span<const std::filesystem::path> EffectiveAssetTree::paths() const noexcept { return _paths; }

ArchiveFirstAssetDiscoveryResult::ArchiveFirstAssetDiscoveryResult(
    EffectiveAssetTree effectiveAssetTree,
    std::map<routing::SkipReason, std::size_t> skippedArchiveCounts,
    std::vector<std::filesystem::path> unsupportedExplicitPaths,
    const std::size_t nestedArchiveCount, const bool cancelled) noexcept
    : _effectiveAssetTree(std::move(effectiveAssetTree)),
      _skippedArchiveCounts(std::move(skippedArchiveCounts)),
      _unsupportedExplicitPaths(std::move(unsupportedExplicitPaths)),
      _nestedArchiveCount(nestedArchiveCount),
      _cancelled(cancelled) {}

const EffectiveAssetTree& ArchiveFirstAssetDiscoveryResult::effectiveAssetTree() const noexcept {
    return _effectiveAssetTree;
}

bool ArchiveFirstAssetDiscoveryResult::cancelled() const noexcept { return _cancelled; }

std::size_t ArchiveFirstAssetDiscoveryResult::skippedArchiveCount(
    const routing::SkipReason reason) const noexcept {
    const auto count = _skippedArchiveCounts.find(reason);
    return count == _skippedArchiveCounts.end() ? 0 : count->second;
}

std::span<const std::filesystem::path> ArchiveFirstAssetDiscoveryResult::unsupportedExplicitPaths()
    const noexcept {
    return _unsupportedExplicitPaths;
}

std::size_t ArchiveFirstAssetDiscoveryResult::nestedArchiveCount() const noexcept {
    return _nestedArchiveCount;
}

ArchiveFirstAssetDiscovery::ArchiveFirstAssetDiscovery(routing::RoutingPolicy policy) noexcept
    : _policy(std::move(policy)) {}

ArchiveFirstAssetDiscoveryResult ArchiveFirstAssetDiscovery::discover(
    const std::span<const std::filesystem::path> roots,
    const ArchiveExtractionOperation& extractArchive,
    const AssetDiscoveryCancellationPredicate& isCancelled) const {
    routing::AssetRouter router(_policy);
    // Routing is filename-only, so recognizing an Archive is cheap enough to repeat during the
    // definitive traversal. That traversal cannot ask the Archive pass instead: extraction can
    // produce an Archive of its own, which by definition was never seen while Archives were being
    // selected. Extracting that one in a second round would be wrong rather than merely expensive,
    // because the game never reads an Archive nested inside another and so no such file is a real
    // Archive Precedence participant.
    const auto namesAnArchive = [&router](const std::filesystem::path& path) {
        const auto decision = router.route(path);
        if (const auto* routedAsset = std::get_if<routing::RoutedAsset>(&decision))
            return routedAsset->kind() == routing::AssetKind::Archive;
        if (const auto* skippedAsset = std::get_if<routing::SkippedAsset>(&decision))
            return skippedAsset->kind() == routing::AssetKind::Archive;

        return false;
    };
    std::unordered_set<std::filesystem::path> recognizedArchivePaths;
    std::vector<routing::RoutedAsset> selectedArchives;
    std::vector<std::filesystem::path> extractionDestinations;
    std::map<routing::SkipReason, std::size_t> skippedArchiveCounts;
    std::vector<std::filesystem::path> unsupportedExplicitPaths;
    // A partial scan is not a definitive tree and must never become executable work.
    const auto cancelledResult = [&] {
        return ArchiveFirstAssetDiscoveryResult(EffectiveAssetTree({}),
                                                std::move(skippedArchiveCounts),
                                                std::move(unsupportedExplicitPaths), 0, true);
    };
    const auto archivePassComplete = visitRegularFiles(
        roots, isCancelled, [&](const std::filesystem::path& path, const bool explicitRoot) {
            auto decision = router.route(path);
            if (auto* routedAsset = std::get_if<routing::RoutedAsset>(&decision)) {
                if (routedAsset->kind() == routing::AssetKind::Archive &&
                    recognizedArchivePaths.insert(path.lexically_normal()).second) {
                    // Extraction writes beside the Archive, so an Archive named directly as a root
                    // needs its containing directory traversed later; re-traversing the Archive file
                    // itself would only rediscover the excluded Archive, or nothing at all once
                    // extraction removed it.
                    if (explicitRoot) {
                        auto destination = path.parent_path();
                        if (destination.empty()) destination = ".";
                        if (std::find(extractionDestinations.begin(), extractionDestinations.end(),
                                      destination) == extractionDestinations.end()) {
                            extractionDestinations.push_back(std::move(destination));
                        }
                    }
                    selectedArchives.push_back(std::move(*routedAsset));
                }
                return;
            }
            if (const auto* skippedAsset = std::get_if<routing::SkippedAsset>(&decision)) {
                if (skippedAsset->kind() == routing::AssetKind::Archive &&
                    recognizedArchivePaths.insert(path.lexically_normal()).second) {
                    ++skippedArchiveCounts[skippedAsset->reason()];
                }
                return;
            }
            if (explicitRoot) unsupportedExplicitPaths.push_back(path);
        });
    if (!archivePassComplete) return cancelledResult();

    // A destination directory is only ever reached through the Archive a caller named explicitly,
    // so the Assets it already holds were never requested. Censusing them before extraction is
    // what keeps the definitive pass below limited to what extraction actually produced.
    std::unordered_set<std::filesystem::path> preExistingDestinationPaths;
    const auto censusComplete = visitRegularFiles(
        extractionDestinations, isCancelled, [&](const std::filesystem::path& path, const bool) {
            preExistingDestinationPaths.insert(path.lexically_normal());
        });
    if (!censusComplete) return cancelledResult();

    if (!selectedArchives.empty() && !extractArchive(selectedArchives)) {
        return cancelledResult();
    }

    // Extraction is synchronous so this is the single definitive view of all non-Archive paths.
    // Every Archive is excluded by recognition rather than by the paths the Archive pass recorded,
    // because an Archive that extraction itself produced was never offered for extraction: the
    // post-extraction targets perform no Archive work, so admitting one would inflate the run's
    // work total with an Asset nothing can execute and that the game would not have read anyway.
    std::vector<std::filesystem::path> effectivePaths;
    // Distinct paths rather than visits: roots and an extraction destination can reach the same
    // file, and a count that said two when one Archive was nested would misreport how malformed
    // the mod actually is.
    std::unordered_set<std::filesystem::path> nestedArchivePaths;
    // Separates the two kinds of Archive this pass can see. One the Archive pass already recorded
    // was either extracted or excluded by policy, and is accounted for either way; one it did not
    // exists only because extraction wrote it, which is the malformed nesting worth counting.
    const auto excludeArchive = [&](const std::filesystem::path& normalizedPath) {
        if (recognizedArchivePaths.contains(normalizedPath)) return;

        nestedArchivePaths.insert(normalizedPath);
    };
    const auto rootPassComplete = visitRegularFiles(
        roots, isCancelled, [&](const std::filesystem::path& path, const bool) {
            if (namesAnArchive(path)) {
                excludeArchive(path.lexically_normal());
                return;
            }
            effectivePaths.push_back(path);
        });
    if (!rootPassComplete) return cancelledResult();
    const auto destinationPassComplete = visitRegularFiles(
        extractionDestinations, isCancelled, [&](const std::filesystem::path& path, const bool) {
            const auto normalizedPath = path.lexically_normal();
            // An Asset the destination already held was never named by the caller, so it is neither
            // the run's work nor the run's business to report, whatever kind it is.
            if (preExistingDestinationPaths.contains(normalizedPath)) return;
            if (namesAnArchive(path)) {
                excludeArchive(normalizedPath);
                return;
            }
            effectivePaths.push_back(path);
        });
    if (!destinationPassComplete) return cancelledResult();
    return ArchiveFirstAssetDiscoveryResult(
        EffectiveAssetTree(std::move(effectivePaths)), std::move(skippedArchiveCounts),
        std::move(unsupportedExplicitPaths), nestedArchivePaths.size());
}
}  // namespace cao::run
