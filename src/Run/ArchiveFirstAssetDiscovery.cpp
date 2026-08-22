#include "ArchiveFirstAssetDiscovery.h"

#include <btu/bsa/unpack.hpp>

#include <algorithm>
#include <cctype>
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
bool asciiCaseInsensitiveEqual(const std::string &left, const std::string &right)
{
    return left.size() == right.size()
           && std::equal(left.begin(),
                         left.end(),
                         right.begin(),
                         [](const unsigned char leftCharacter,
                            const unsigned char rightCharacter) {
                             return std::tolower(leftCharacter)
                                    == std::tolower(rightCharacter);
                         });
}

bool hasArchiveExtension(const std::filesystem::path &path,
                         const std::string &archiveExtension)
{
    return asciiCaseInsensitiveEqual(path.extension().string(), archiveExtension);
}

/// Visits each regular-file root or recursively visits regular files beneath directory roots.
template<typename Visitor>
void visitRegularFiles(const std::span<const std::filesystem::path> roots,
                       Visitor &&visitor)
{
    for (const auto &root : roots) {
        if (std::filesystem::is_regular_file(root)) {
            visitor(root);
            continue;
        }

        for (const auto &entry : std::filesystem::recursive_directory_iterator(root)) {
            if (entry.is_regular_file())
                visitor(entry.path());
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

ArchiveFirstAssetDiscovery::ArchiveFirstAssetDiscovery(
    routing::RoutingPolicy policy) noexcept
    : _policy(std::move(policy))
{}

EffectiveAssetTree ArchiveFirstAssetDiscovery::discover(
    const std::span<const std::filesystem::path> roots,
    const ArchiveExtractionOperation &extractArchive) const
{
    routing::AssetRouter router(_policy);
    std::unordered_set<std::filesystem::path> seenArchivePaths;
    visitRegularFiles(roots, [&](const std::filesystem::path &path) {
        if (!hasArchiveExtension(path, _policy.archiveExtension())
            || !seenArchivePaths.insert(path.lexically_normal()).second) {
            return;
        }

        const auto decision = router.route(path);
        const auto *archive = std::get_if<routing::RoutedAsset>(&decision);
        if (archive != nullptr && archive->kind() == routing::AssetKind::Archive)
            extractArchive(*archive);
    });

    // Extraction is synchronous so this is the single definitive view of all non-Archive paths.
    std::vector<std::filesystem::path> effectivePaths;
    visitRegularFiles(roots, [&](const std::filesystem::path &path) {
        if (!hasArchiveExtension(path, _policy.archiveExtension()))
            effectivePaths.push_back(path);
    });
    return EffectiveAssetTree(std::move(effectivePaths));
}
}
