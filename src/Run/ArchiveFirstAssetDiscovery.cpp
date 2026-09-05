#include "ArchiveFirstAssetDiscovery.h"

#include <btu/bsa/unpack.hpp>

#include <algorithm>
#include <map>
#include <system_error>
#include <unordered_set>
#include <utility>
#include <variant>

#ifdef _WIN32
#include <Windows.h>
#endif

namespace cao::run {
void extractArchiveNoOverwrite(const std::filesystem::path& archivePath, const bool removeArchive) {
    // Bethesda gives existing Loose Assets precedence, so Archive extraction must never replace
    // them.
    btu::bsa::unpack(btu::bsa::UnpackSettings{archivePath, removeArchive, false});
}

namespace {
/// Checks the resolved path's ancestry using native filesystem identity, including Windows casing.
bool isWithinRoot(const std::filesystem::path& resolvedPath,
                  const std::filesystem::path& canonicalRoot) {
    for (auto ancestor = resolvedPath; !ancestor.empty();) {
        std::error_code error;
        if (std::filesystem::equivalent(ancestor, canonicalRoot, error)) return true;
        const auto parent = ancestor.parent_path();
        if (parent == ancestor) break;
        ancestor = parent;
    }
    return false;
}

/// Rejects directory links and linked files whose target cannot be proven inside this Mod Root.
/// Filesystem races remain absent entries; identified but unresolved links receive an exclusion.
template <typename ExcludedVisitor>
bool entryIsWithinScope(const std::filesystem::path& path,
                        const std::filesystem::path& canonicalRoot,
                        ExcludedVisitor&& excluded) {
    std::error_code error;
    const auto status = std::filesystem::symlink_status(path, error);
    if (error) return false;
    auto linked = std::filesystem::is_symlink(status);
#ifdef _WIN32
    // Junctions and other reparse directories are not consistently classified as symlinks by
    // filesystem implementations. The native attribute prevents traversal for every reparse tag.
    const auto attributes = GetFileAttributesW(path.c_str());
    if (attributes == INVALID_FILE_ATTRIBUTES && !linked) return false;
    if (attributes != INVALID_FILE_ATTRIBUTES)
        linked = linked || (attributes & FILE_ATTRIBUTE_REPARSE_POINT) != 0;
#endif
    if (!linked) return true;

    if (std::filesystem::is_directory(path, error)) {
        excluded(path, "Directory links and reparse points are not followed during discovery.");
        return false;
    }

    error.clear();
    const auto resolved = std::filesystem::canonical(path, error);
    if (error) {
        excluded(path, "Linked entry could not be resolved within the Mod Root.");
        return false;
    }
    if (!isWithinRoot(resolved, canonicalRoot)) {
        excluded(path, "Linked entry resolves outside the Mod Root.");
        return false;
    }
    return true;
}

/// Visits each regular file and tells the visitor whether the path was an explicitly supplied root.
/// Returns false as soon as cancellation is observed before a root or directory entry.
template <typename ExcludedVisitor, typename Visitor>
bool visitRegularFiles(const std::span<const std::filesystem::path> roots,
                       const AssetDiscoveryCancellationPredicate& isCancelled,
                       ExcludedVisitor&& excluded, Visitor&& visitor) {
    for (const auto& root : roots) {
        if (isCancelled && isCancelled()) return false;
        std::error_code error;
        const auto directoryRoot = std::filesystem::is_directory(root, error);
        auto boundary = directoryRoot ? root : root.parent_path();
        if (boundary.empty()) boundary = ".";
        error.clear();
        const auto canonicalRoot = std::filesystem::canonical(boundary, error);
        if (error || !entryIsWithinScope(root, canonicalRoot, excluded)) continue;

        error.clear();
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
            if (!entryIsWithinScope(entry->path(), canonicalRoot, excluded)) {
                // Disable recursion explicitly even when the standard iterator currently declines
                // symlinks: Windows reparse tags must never become another Mod Root's work.
                entry.disable_recursion_pending();
            } else {
                error.clear();
                if (entry->is_regular_file(error)) visitor(entry->path(), false);
            }

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
    const std::size_t nestedArchiveCount, std::vector<RunDiagnostic> diagnostics,
    const bool cancelled) noexcept
    : _effectiveAssetTree(std::move(effectiveAssetTree)),
      _skippedArchiveCounts(std::move(skippedArchiveCounts)),
      _unsupportedExplicitPaths(std::move(unsupportedExplicitPaths)),
      _nestedArchiveCount(nestedArchiveCount),
      _cancelled(cancelled),
      _diagnostics(std::move(diagnostics)) {}

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
    std::vector<RunDiagnostic> diagnostics;
    std::unordered_set<std::filesystem::path> diagnosedPaths;
    auto discoveryPhase = RunPhase::DiscoveringArchives;
    const auto excludeLinkedEntry = [&](const std::filesystem::path& path, const char* detail) {
        // Both discovery passes see unchanged links. Retain their first observation so one skipped
        // entry yields one actionable diagnostic rather than reporting the same exclusion twice.
        if (diagnosedPaths.insert(path.lexically_normal()).second)
            diagnostics.emplace_back(RunDiagnosticCode::LinkedEntryExcluded, discoveryPhase,
                                     detail, path);
    };
    // A partial scan is not a definitive tree and must never become executable work.
    const auto cancelledResult = [&] {
        return ArchiveFirstAssetDiscoveryResult(EffectiveAssetTree({}),
                                                std::move(skippedArchiveCounts),
                                                std::move(unsupportedExplicitPaths), 0,
                                                std::move(diagnostics), true);
    };
    std::vector<std::filesystem::path> resolvedRoots;
    resolvedRoots.reserve(roots.size());
    for (const auto& root : roots) {
        if (isCancelled && isCancelled()) return cancelledResult();
        std::error_code error;
        if (std::filesystem::is_directory(root, error)) {
            // An explicitly selected directory alias identifies one Mod Root for the entire run.
            // Freeze it before extraction so retargeting the alias cannot change pass two's scope.
            auto resolved = std::filesystem::canonical(root, error);
            if (!error) resolvedRoots.push_back(std::move(resolved));
        } else {
            resolvedRoots.push_back(root);
        }
    }
    const auto archivePassComplete = visitRegularFiles(
        resolvedRoots, isCancelled, excludeLinkedEntry,
        [&](const std::filesystem::path& path, const bool explicitRoot) {
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
        extractionDestinations, isCancelled, excludeLinkedEntry,
        [&](const std::filesystem::path& path, const bool) {
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
    discoveryPhase = RunPhase::BuildingEffectiveAssetTree;
    const auto rootPassComplete = visitRegularFiles(
        resolvedRoots, isCancelled, excludeLinkedEntry,
        [&](const std::filesystem::path& path, const bool) {
            if (namesAnArchive(path)) {
                excludeArchive(path.lexically_normal());
                return;
            }
            effectivePaths.push_back(path);
        });
    if (!rootPassComplete) return cancelledResult();
    const auto destinationPassComplete = visitRegularFiles(
        extractionDestinations, isCancelled, excludeLinkedEntry,
        [&](const std::filesystem::path& path, const bool) {
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
        std::move(unsupportedExplicitPaths), nestedArchivePaths.size(), std::move(diagnostics));
}
}  // namespace cao::run
