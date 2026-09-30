#include "Run/ArchiveFinalizationInternals.h"

#include "Run/StagingPaths.h"

#include <algorithm>
#include <set>

#ifdef _WIN32
#include <Windows.h>
#endif

namespace cao::run::archive_finalization {
namespace {
/// Rejects reserved staging paths and entries matched by the profile's FilesToNotPack list.
/// Returns true when the entry may be packed and later deleted as a packed source.
bool isAllowedFile(const btu::Path& dir, const std::filesystem::directory_entry& fileinfo,
                   std::span<const std::u8string> filesToNotPack) {
    // Packing and its source-deletion pass must never consume another lifecycle's temporary data.
    if (hasStagingComponent(fileinfo.path().lexically_relative(dir))) return false;
    const auto& path = fileinfo.path().u8string();
    for (const auto& fileToNotPack : filesToNotPack) {
        if (btu::common::str_contain(path, fileToNotPack, false)) {
            PLOG_VERBOSE << btu::common::as_ascii(path)
                         << " ignored because of filesToNotPack. Rule: "
                         << btu::common::as_ascii(fileToNotPack);
            return false;
        }
    }

    return true;
}
}  // namespace

std::uintmax_t estimatePackedCapacity(const ArchiveFinalizationOutput& output,
                                      const btu::bsa::Settings& settings,
                                      const std::stop_token stop) {
    auto estimate = std::uintmax_t{65536};
    // A Loading Plugin can disappear before publication, requiring the fallback dummy.
    if (!output.loadingPluginPaths.empty())
        estimate = saturatedCapacityAdd(estimate, settings.s_dummy_plugin->size());
    for (const auto& source : output.sources) {
        if (stop.stop_requested()) throw ArchiveFinalizationPlanningCancelled{};
        // Zlib/LZ4 framing, up to four BA2 texture chunks, and BSA directory/name tables
        // need space beyond source bytes. Filesystem allocation and metadata remain estimates.
        const auto payload = saturatedCapacityMultiply(std::filesystem::file_size(source), 2);
        const auto names = saturatedCapacityMultiply(
            source.lexically_relative(output.modRoot).generic_u8string().size(), 3);
        estimate = saturatedCapacityAdd(
            estimate, saturatedCapacityAdd(payload, saturatedCapacityAdd(65536, names)));
    }
    return estimate;
}

ArchiveFinalizationPlan planFinalization(const std::span<const std::filesystem::path> roots,
                                         const ArchiveFinalizationSettings& choices,
                                         btu::bsa::Settings bsaSettings,
                                         const std::span<const std::u8string> filesToNotPack,
                                         const std::stop_token stop) {
    namespace fs = std::filesystem;
    using namespace btu::bsa;
    ArchiveFinalizationPlan plan;
    plan.settings = std::move(bsaSettings);
    plan.compress = choices.compress;
    plan.deleteSources = choices.deleteSources;
    plan.createDummies = choices.createDummyPlugins;
    const auto& settings = plan.settings;
    std::set<fs::path> reserved;
    // Planning must not publish a partial work total after cancellation; no output has mutated.
    const auto checkCancelled = [&] {
        if (stop.stop_requested()) throw ArchiveFinalizationPlanningCancelled{};
    };
    for (const auto& inputRoot : roots) {
        checkCancelled();
        const auto root = fs::canonical(inputRoot);
        plan.roots.push_back(root);
        auto& dummyCapacity = plan.dummyCapacityByRoot[root];
        if (plan.createDummies && settings.s_dummy_plugin) {
            // Existing Archives can also need plugins in the final cleanup pass, even with no
            // new outputs. Count every Archive conservatively without assuming plugin reuse.
            checkCancelled();
            const auto existing = list_archive(fs::directory_iterator(root), {}, settings);
            dummyCapacity = saturatedCapacityAdd(
                dummyCapacity,
                saturatedCapacityMultiply(existing.size(), settings.s_dummy_plugin->size()));
        }
        auto plugins = list_plugins(fs::directory_iterator(root), {}, settings);
        // Ignore only proven Dummy Plugin names while planning; retain their files until all
        // output attempts finish so cancellation cannot strand an existing Archive.
        if (settings.s_dummy_plugin) {
            std::erase_if(plugins, [&](const auto& plugin) {
                checkCancelled();
                return hasExactDummyBytes(plugin.full_path(), *settings.s_dummy_plugin);
            });
        }
        std::sort(plugins.begin(), plugins.end());
        if (plugins.empty())
            plugins.emplace_back(root, root.filename().u8string(), u8"", u8".esp",
                                 FileTypes::Plugin);

        std::vector<fs::path> sources;
        for (auto it = fs::recursive_directory_iterator(root);
             it != fs::recursive_directory_iterator(); ++it) {
            checkCancelled();
            const auto path = it->path();
            bool excluded = hasStagingComponent(path.lexically_relative(root)) ||
                            fs::is_symlink(it->symlink_status());
#ifdef _WIN32
            const auto attributes = GetFileAttributesW(path.c_str());
            if (attributes == INVALID_FILE_ATTRIBUTES)
                throw std::runtime_error("Cannot inspect finalization input.");
            excluded = excluded || (attributes & FILE_ATTRIBUTE_REPARSE_POINT) != 0;
#endif
            // Do not descend into reserved staging or directory links just to filter their files.
            if (excluded) {
                it.disable_recursion_pending();
                continue;
            }
            if (fs::is_regular_file(it->symlink_status()) && default_is_allowed_path(root, *it) &&
                isAllowedFile(root, *it, filesToNotPack))
                sources.push_back(path);
        }
        std::sort(sources.begin(), sources.end());
        auto standard = ArchiveData(settings, ArchiveType::Standard);
        auto incompressible = ArchiveData(settings, ArchiveType::Incompressible);
        auto textures = ArchiveData(settings, ArchiveType::Textures);
        std::vector<ArchiveData> archives;
        for (const auto& source : sources) {
            checkCancelled();
            const auto type = get_filetype(source, root, settings);
            if (type != FileTypes::Standard && type != FileTypes::Texture &&
                type != FileTypes::Incompressible)
                continue;
            auto& archive = type == FileTypes::Texture          ? textures
                            : type == FileTypes::Incompressible ? incompressible
                                                                : standard;
            if (!archive.add_file(source)) {
                const auto archiveType = archive.get_type();
                archives.push_back(std::move(archive));
                archive = ArchiveData(settings, archiveType);
                if (!archive.add_file(source))
                    throw std::runtime_error("An Asset exceeds the output Archive size limit.");
            }
        }
        // The library merge contract requires the three unfinished partitions in this order.
        archives.insert(archives.end(),
                        {std::move(standard), std::move(incompressible), std::move(textures)});
        auto mergeSettings = static_cast<MergeSettings>(0);
        if (choices.mergeIncompressible) mergeSettings |= MergeSettings::MergeIncompressible;
        if (choices.mergeTextures) mergeSettings |= MergeSettings::MergeTextures;
        merge(archives, mergeSettings);
        for (auto& archive : archives) {
            checkCancelled();
            const auto suffix = archive.get_type() == ArchiveType::Textures
                                    ? settings.texture_suffix.value_or(u8"")
                                    : settings.suffix.value_or(u8"");
            const auto available = [&](const FilePath& candidate) {
                const auto path = candidate.full_path();
                std::error_code error;
                const auto status = fs::symlink_status(path, error);
                if (error && error != std::errc::no_such_file_or_directory)
                    throw fs::filesystem_error("Cannot plan output Archive", path, error);
                return !fs::exists(status) && !reserved.contains(path);
            };
            std::optional<FilePath> selected;
            for (auto plugin : plugins) {
                checkCancelled();
                plugin.ext = settings.extension;
                plugin.suffix = suffix;
                if (available(plugin)) {
                    selected = plugin;
                    break;
                }
            }
            if (!selected) {
                auto candidate = plugins.front();
                candidate.ext = settings.extension;
                candidate.suffix = suffix;
                for (std::uint32_t counter = 0; counter < 255; ++counter) {
                    checkCancelled();
                    candidate.counter = counter;
                    if (available(candidate)) {
                        selected = candidate;
                        break;
                    }
                }
            }
            if (!selected) throw std::runtime_error("No available output Archive name.");
            const auto destination = selected->full_path();
            reserved.insert(destination);
            std::optional<fs::path> pluginPath;
            std::vector<fs::path> loadingPluginPaths;
            if (plan.createDummies && settings.s_dummy_plugin) {
                checkCancelled();
                loadingPluginPaths = loadingPluginNames(*selected, settings);
                if (!findLoadingPlugin(loadingPluginPaths, LoadingPluginStrength::Present))
                    pluginPath = loadingPluginPaths.back();
            }
            plan.outputs.push_back({root,
                                    destination,
                                    {archive.begin(), archive.end()},
                                    pluginPath,
                                    0,
                                    std::move(loadingPluginPaths)});
            auto& output = plan.outputs.back();
            output.estimatedCapacityBytes = estimatePackedCapacity(output, settings, stop);
            plan.archives.push_back(std::move(archive));
        }
    }
    checkCancelled();
    return plan;
}
}  // namespace cao::run::archive_finalization
