#include "ArchiveFirstAssetDiscovery.h"
#include "PathOrdering.h"
#include "StagingPaths.h"

#include <btu/bsa/unpack.hpp>
#include <bsa/bsa.hpp>

#include <algorithm>
#include <map>
#include <set>
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

/// Reads raw Archive names without extracting, decompressing, or sanitizing away invalid paths.
ArchiveInventory inspectArchiveInventory(const std::filesystem::path& path) {
    ArchiveInventory inventory;
    const auto add = [&](std::string name, std::uintmax_t bytes) {
        // Each staged payload and its ownership record consume space even when shadowed.
        // Allow metadata/path overhead without pretending to predict allocation-unit sizes.
        inventory.estimatedCapacityBytes = saturatedCapacityAdd(inventory.estimatedCapacityBytes,
            saturatedCapacityAdd(bytes, saturatedCapacityAdd(65536,
                saturatedCapacityMultiply(name.size(), 8))));
        inventory.names.push_back(std::move(name));
    };
    const auto format = bsa::guess_file_format(path);
    if (!format) throw std::runtime_error("Unrecognized Archive format.");
    switch (*format) {
        case bsa::file_format::tes3: {
            bsa::tes3::archive archive;
            archive.read(path);
            for (const auto& [key, file] : archive) add(std::string(key.name()), file.size());
            break;
        }
        case bsa::file_format::tes4: {
            bsa::tes4::archive archive;
            archive.read(path);
            for (const auto& [directory, files] : archive)
                for (const auto& [key, file] : files)
                    add(directory.name().empty() ? std::string(key.name())
                                                             : std::string(directory.name()) + "/" +
                                                                   std::string(key.name()),
                        file.compressed() ? file.decompressed_size() : file.size());
            break;
        }
        case bsa::file_format::fo4: {
            bsa::fo4::archive archive;
            const auto archiveFormat = archive.read(path);
            for (const auto& [key, file] : archive) {
                // DX10 payloads reconstruct a DDS header in addition to their manifest chunks.
                std::uintmax_t bytes = archiveFormat == bsa::fo4::format::directx ? 148 : 0;
                for (const auto& chunk : file)
                    bytes = saturatedCapacityAdd(bytes, chunk.compressed()
                        ? chunk.decompressed_size() : chunk.size());
                add(std::string(key.name()), bytes);
            }
            break;
        }
    }
    return inventory;
}

/// Canonicalizes a contained game path; rejects names whose extraction could escape or alias.
std::string canonicalArchiveEntryPath(std::string name) {
    std::replace(name.begin(), name.end(), '\\', '/');
    if (name.empty() || name.front() == '/' || name.find('\0') != std::string::npos ||
        name.find_first_of(":*?\"<>|") != std::string::npos)
        throw std::invalid_argument("Archive entry has an invalid game path.");
    const auto path = std::filesystem::u8path(name).lexically_normal();
    if (path.empty() || path == "." || path.filename().empty() || *path.begin() == "..")
        throw std::invalid_argument("Archive entry escapes its extraction directory.");
    for (const auto& part : path) {
        const auto text = relativeName(part);
        if (text.back() == '.' || text.back() == ' ' || isStagingName(part))
            throw std::invalid_argument("Archive entry aliases an unsafe or reserved path.");
    }
    return foldedName(relativeName(path));
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

/// Validates and applies complete high-to-low intent within one already resolved Mod Root.
/// Returns a fatal discovery failure without changing the discovered batch on invalid intent.
std::variant<std::vector<routing::RoutedAsset>, RunFailure> validateArchiveOrder(
    const std::filesystem::path& root, std::span<const routing::RoutedAsset> archives,
    const ArchivePrecedence& precedence) {
    const auto failure = [](RunFailureCode code, const std::filesystem::path& path,
                            const char* detail) {
        return RunFailure(code, RunPhase::DiscoveringArchives, detail,
                          routing::PolicyValidationErrors{}, path);
    };
    std::vector<routing::RoutedAsset> ordered;
    std::unordered_set<std::filesystem::path> used;
    for (const auto& requested : precedence.highToLow()) {
        const auto normalized = requested.lexically_normal();
        if (requested.empty() || requested.has_root_path() || normalized.empty() ||
            *normalized.begin() == "..")
            return failure(
                RunFailureCode::ArchiveOrderOutsideRoot, requested,
                "Archive Precedence paths must be relative and contained in the Mod Root.");
        const auto candidate = root / normalized;
        std::error_code error;
        const auto resolved = std::filesystem::canonical(candidate, error);
        if (!error && !isWithinRoot(resolved, root))
            return failure(RunFailureCode::ArchiveOrderOutsideRoot, candidate,
                           "Required Archive resolves outside the Mod Root.");
        const auto found = std::find_if(archives.begin(), archives.end(), [&](const auto& archive) {
            // Match discovered names, not target identity: two contained hard links
            // remain two enabled Archive entries in this precedence scope.
            const auto left = relativeName(std::filesystem::absolute(archive.executionPath()));
            const auto right = relativeName(candidate);
#ifdef _WIN32
            return foldedName(left) == foldedName(right);
#else
            return left == right;
#endif
        });
        if (found == archives.end())
            return failure(
                RunFailureCode::ArchiveOrderExtra, candidate,
                "Archive Precedence names an Archive that is not enabled in this Mod Root.");
        if (!used.insert(found->executionPath()).second)
            return failure(RunFailureCode::ArchiveOrderDuplicate, candidate,
                           "Archive Precedence names the same enabled Archive more than once.");
        ordered.push_back(*found);
    }
    if (ordered.size() != archives.size())
        return failure(RunFailureCode::ArchiveOrderMissing, root,
                       "Archive Precedence must include every enabled Archive in this Mod Root.");
    return ordered;
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
    // A contained alias can still point into excluded staging; checking only its visible name
    // would let temporary Archives and Assets re-enter discovery through an ordinary filename.
    for (const auto& component : resolved.lexically_relative(canonicalRoot)) {
        if (isStagingName(component)) {
            excluded(path, "Linked entry resolves into excluded CAO staging.");
            return false;
        }
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
        if (isStagingName(root)) continue;
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
            if (isStagingName(entry->path())) {
                // Stale or unverified temporary material must never become optimization input.
                entry.disable_recursion_pending();
            } else if (!entryIsWithinScope(entry->path(), canonicalRoot, excluded)) {
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

ArchiveFirstAssetDiscovery::ArchiveFirstAssetDiscovery(routing::RoutingPolicy policy,
                                                       CapacityProbe capacity) noexcept
    : _policy(std::move(policy)), _capacity(std::move(capacity)) {}

ArchiveFirstAssetDiscoveryResult ArchiveFirstAssetDiscovery::discover(
    const std::span<const std::filesystem::path> roots,
    const ArchiveExtractionOperation& extractArchive,
    const AssetDiscoveryCancellationPredicate& isCancelled, const ArchivePrecedence& precedence,
    const std::function<void(std::span<const ArchiveCollision>)>& reportCollisions,
    const std::function<void(std::span<const ArchiveExtractionPlan>)>& reportExtractionPlan) const {
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
    std::vector<std::filesystem::path> precedenceScopes;
    std::map<std::filesystem::path, std::filesystem::path> archiveRoots;
    std::map<std::filesystem::path, std::set<std::string>> loosePaths;
    std::vector<ArchiveCollision> collisions;
    std::vector<std::filesystem::path> extractionDestinations;
    std::map<routing::SkipReason, std::size_t> skippedArchiveCounts;
    std::vector<std::filesystem::path> unsupportedExplicitPaths;
    std::vector<RunDiagnostic> diagnostics;
    std::unordered_set<std::filesystem::path> diagnosedPaths;
    auto discoveryPhase = RunPhase::DiscoveringArchives;
    const auto failedResult = [&](RunFailureCode code, const std::filesystem::path& path,
                                  const std::string& detail) {
        auto result = ArchiveFirstAssetDiscoveryResult(
            EffectiveAssetTree({}), std::move(skippedArchiveCounts),
            std::move(unsupportedExplicitPaths), 0, std::move(diagnostics));
        result._failures.emplace_back(code, RunPhase::DiscoveringArchives, detail,
                                      routing::PolicyValidationErrors{}, path);
        return result;
    };
    const auto excludeLinkedEntry = [&](const std::filesystem::path& path, const char* detail) {
        // Both discovery passes see unchanged links. Retain their first observation so one skipped
        // entry yields one actionable diagnostic rather than reporting the same exclusion twice.
        if (diagnosedPaths.insert(path.lexically_normal()).second)
            diagnostics.emplace_back(RunDiagnosticCode::LinkedEntryExcluded, discoveryPhase,
                                     detail, path);
    };
    // A partial scan is not a definitive tree and must never become executable work.
    const auto cancelledResult = [&] {
        auto result = ArchiveFirstAssetDiscoveryResult(
            EffectiveAssetTree({}), std::move(skippedArchiveCounts),
            std::move(unsupportedExplicitPaths), 0, std::move(diagnostics), true);
        result._collisions = std::move(collisions);
        return result;
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
            // Freeze a direct file's containing-directory alias as well, but retain the file
            // name so contained file links remain distinct enabled Archive entries.
            auto parent = root.parent_path();
            if (parent.empty()) parent = ".";
            error.clear();
            const auto resolvedParent = std::filesystem::canonical(parent, error);
            resolvedRoots.push_back(error ? root : resolvedParent / root.filename());
        }
    }
    for (const auto& root : resolvedRoots) {
        std::error_code rootError;
        auto boundary = std::filesystem::is_directory(root, rootError) ? root : root.parent_path();
        if (boundary.empty()) boundary = ".";
        const auto canonicalRoot = std::filesystem::weakly_canonical(boundary);
        if (std::find(precedenceScopes.begin(), precedenceScopes.end(), canonicalRoot) ==
            precedenceScopes.end())
            precedenceScopes.push_back(canonicalRoot);
        const auto firstArchive = selectedArchives.size();
        std::map<std::filesystem::path, std::pair<std::string, std::string>> archiveOrder;
        const auto archivePassComplete = visitRegularFiles(
            std::span(&root, 1), isCancelled, excludeLinkedEntry,
            [&](const std::filesystem::path& path, const bool explicitRoot) {
                if (!namesAnArchive(path))
                    loosePaths[canonicalRoot].insert(
                        foldedName(relativeName(path.lexically_relative(canonicalRoot))));
                auto decision = router.route(path);
                if (auto* routedAsset = std::get_if<routing::RoutedAsset>(&decision)) {
                    if (routedAsset->kind() == routing::AssetKind::Archive &&
                        recognizedArchivePaths.insert(path.lexically_normal()).second) {
                        // Extraction writes beside the Archive, so an Archive named directly as a
                        // root needs its containing directory traversed later; re-traversing the
                        // Archive file itself would only rediscover the excluded Archive, or
                        // nothing at all once extraction removed it.
                        if (explicitRoot) {
                            auto destination = path.parent_path();
                            if (destination.empty()) destination = ".";
                            if (std::find(extractionDestinations.begin(),
                                          extractionDestinations.end(),
                                          destination) == extractionDestinations.end()) {
                                extractionDestinations.push_back(std::move(destination));
                            }
                        }
                        auto name = relativeName(explicitRoot ? path.filename()
                                                              : path.lexically_relative(root));
                        archiveOrder.emplace(routedAsset->executionPath(),
                                             std::pair{foldedName(name), name});
                        selectedArchives.push_back(std::move(*routedAsset));
                        archiveRoots.emplace(path, canonicalRoot);
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
        // Root precedence belongs to the caller. Sort only this root's batch, using the original
        // normalized spelling to break case-fold ties without consulting filesystem enumeration.
        std::sort(selectedArchives.begin() + static_cast<std::ptrdiff_t>(firstArchive),
                  selectedArchives.end(), [&](const auto& left, const auto& right) {
                      return archiveOrder.at(left.executionPath()) <
                             archiveOrder.at(right.executionPath());
                  });
    }

    if (_policy.executionMode() == routing::ExecutionMode::Apply &&
        precedence.mode() == ArchivePrecedenceMode::ExplicitOrder) {
        std::vector<routing::RoutedAsset> ordered;
        for (const auto& scope : precedenceScopes) {
            std::vector<routing::RoutedAsset> batch;
            for (const auto& archive : selectedArchives)
                if (archiveRoots.at(archive.executionPath()) == scope) batch.push_back(archive);
            auto validated = validateArchiveOrder(scope, batch, precedence);
            if (const auto* failure = std::get_if<RunFailure>(&validated))
                return failedResult(failure->code(), failure->path(), failure->detail());
            const auto& archives = std::get<std::vector<routing::RoutedAsset>>(validated);
            ordered.insert(ordered.end(), archives.begin(), archives.end());
        }
        selectedArchives = std::move(ordered);
    }

    // A destination directory is only ever reached through the Archive a caller named explicitly,
    // so the Assets it already holds were never requested. Censusing them before extraction is
    // what keeps the definitive pass below limited to what extraction actually produced.
    std::unordered_set<std::filesystem::path> preExistingDestinationPaths;
    const auto censusComplete = visitRegularFiles(
        extractionDestinations, isCancelled, excludeLinkedEntry,
        [&](const std::filesystem::path& path, const bool) {
            preExistingDestinationPaths.insert(path.lexically_normal());
            if (!namesAnArchive(path)) {
                const auto absolute = std::filesystem::absolute(path);
                for (const auto& scope : precedenceScopes)
                    if (isWithinRoot(absolute, scope))
                        loosePaths[scope].insert(
                            foldedName(relativeName(absolute.lexically_relative(scope))));
            }
        });
    if (!censusComplete) return cancelledResult();

    std::map<std::filesystem::path, std::map<std::string, std::vector<std::filesystem::path>>>
        entries;
    std::vector<ArchiveExtractionPlan> extractionPlans;
    for (const auto& archive : selectedArchives) {
        if (isCancelled && isCancelled()) return cancelledResult();
        ArchiveInventory inventory;
        try {
            inventory = inspectArchiveInventory(archive.executionPath());
        } catch (const std::exception& error) {
            return failedResult(RunFailureCode::ArchiveUnreadable, archive.executionPath(),
                                error.what());
        }
        const auto& root = archiveRoots.at(archive.executionPath());
        auto& plan = extractionPlans.emplace_back(ArchiveExtractionPlan{archive.executionPath(), root});
        plan.estimatedCapacityBytes = inventory.estimatedCapacityBytes;
        for (const auto& name : inventory.names) {
            if (isCancelled && isCancelled()) return cancelledResult();
            try {
                // The existing extractor writes beside its Archive, including nested Archives.
                // Compare the actual destination relative to the Mod Root, not just the raw name.
                const auto local = std::filesystem::u8path(canonicalArchiveEntryPath(name));
                plan.entries.push_back(relativeName(local));
                const auto destination =
                    std::filesystem::absolute(archive.executionPath()).parent_path() / local;
                std::error_code error;
                const auto resolved = std::filesystem::weakly_canonical(destination, error);
                if (error || !isWithinRoot(resolved, root))
                    throw std::invalid_argument(
                        "Archive entry resolves outside the Mod Root or cannot be resolved.");
                const auto gamePath =
                    foldedName(relativeName(destination.lexically_relative(root)));
                auto& participants = entries[root][gamePath];
                if (participants.empty() || participants.back() != archive.executionPath())
                    participants.push_back(archive.executionPath());
            } catch (const std::exception& error) {
                return failedResult(RunFailureCode::ArchiveEntryInvalid, archive.executionPath(),
                                    error.what());
            }
        }
    }
    for (auto& plan : extractionPlans) {
        for (const auto& entry : plan.entries) {
            const auto destination = std::filesystem::absolute(plan.archivePath).parent_path() /
                                     std::filesystem::u8path(entry);
            const auto gamePath = foldedName(relativeName(destination.lexically_relative(plan.modRoot)));
            // Ownership is frozen before mutation: a failed winner must not promote a shadowed
            // Archive, and a Loose Asset removed later still retains its preflight precedence.
            if (entries.at(plan.modRoot).at(gamePath).front() == plan.archivePath &&
                !loosePaths[plan.modRoot].contains(gamePath))
                plan.mergeEntries.push_back(entry);
        }
    }
    for (const auto& root : precedenceScopes) {
        const auto found = entries.find(root);
        if (found == entries.end()) continue;
        for (const auto& [gamePath, participants] : found->second) {
            if (participants.size() < 2) continue;
            collisions.emplace_back(root, std::filesystem::u8path(gamePath), participants.front(),
                                    std::vector(participants.begin() + 1, participants.end()),
                                    loosePaths[root].contains(gamePath));
        }
    }
    // Count the whole batch at every root, conservatively covering roots sharing a volume.
    // No credit is taken for source deletion or cleanup of shadowed staging after the phase.
    std::uintmax_t required = 0;
    for (const auto& plan : extractionPlans)
        required = saturatedCapacityAdd(required, plan.estimatedCapacityBytes);
    for (const auto& plan : extractionPlans) {
        const auto available = _capacity ? _capacity(plan.modRoot) : std::nullopt;
        if (available && *available < required)
            return failedResult(RunFailureCode::ArchiveInsufficientCapacity, plan.modRoot,
                                archiveCapacityDetail(required, *available));
    }
    // Publish only the complete plan: a later unreadable Archive must prevent earlier mutations.
    if (reportCollisions && _policy.executionMode() == routing::ExecutionMode::Apply)
        reportCollisions(collisions);
    if (reportExtractionPlan && _policy.executionMode() == routing::ExecutionMode::Apply)
        reportExtractionPlan(extractionPlans);
    if (isCancelled && isCancelled()) return cancelledResult();
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
    auto result = ArchiveFirstAssetDiscoveryResult(
        EffectiveAssetTree(std::move(effectivePaths)), std::move(skippedArchiveCounts),
        std::move(unsupportedExplicitPaths), nestedArchivePaths.size(), std::move(diagnostics));
    result._collisions = std::move(collisions);
    return result;
}
}  // namespace cao::run
