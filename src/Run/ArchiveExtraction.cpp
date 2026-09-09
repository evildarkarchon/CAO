#include "ArchiveExtraction.h"
#include "PathOrdering.h"
#include "StagingPaths.h"
#include <btu/bsa/archive.hpp>
#include <map>
#include <set>

#ifdef _WIN32
#include <Windows.h>
#endif

namespace cao::run {
namespace {
/// Keeps destination directories stable on Windows until the no-replace commit finishes.
struct MergeTarget {
    std::filesystem::path path;
#ifdef _WIN32
    std::vector<std::unique_ptr<void, decltype(&CloseHandle)>> pins{};
#endif
};

/// Resolves game-path casing without following directory links, creating only missing parents.
/// Throws on ambiguous names, unexpected destinations, or paths outside the selected Mod Root.
MergeTarget prepareMergeTarget(const std::filesystem::path& root,
                               const std::filesystem::path& destination) {
    const auto relative = destination.lexically_relative(root);
    if (relative.empty() || relative.is_absolute() || *relative.begin() == ".." ||
        hasStagingComponent(relative))
        throw std::runtime_error("Archive destination is outside its Mod Root.");
    MergeTarget target{root};
    for (auto part = relative.begin(); part != relative.end(); ++part) {
        const bool leaf = std::next(part) == relative.end();
        const auto folded = foldedName(relativeName(*part));
        std::optional<std::filesystem::path> existing;
        for (const auto& entry : std::filesystem::directory_iterator(target.path)) {
            if (foldedName(relativeName(entry.path().filename())) != folded) continue;
            if (existing) throw std::runtime_error("Archive destination has ambiguous casing.");
            existing = entry.path();
        }
        target.path = existing ? *existing : target.path / *part;
        if (leaf) {
            if (existing)
                throw std::runtime_error("Archive destination appeared after preflight.");
            break;
        }
        if (!existing) std::filesystem::create_directory(target.path);
        const auto status = std::filesystem::symlink_status(target.path);
        if (!std::filesystem::is_directory(status) || std::filesystem::is_symlink(status))
            throw std::runtime_error("Archive destination parent is not an ordinary directory.");
#ifdef _WIN32
        const auto handle = CreateFileW(target.path.c_str(), FILE_READ_ATTRIBUTES,
                                       FILE_SHARE_READ | FILE_SHARE_WRITE, nullptr, OPEN_EXISTING,
                                       FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                                       nullptr);
        if (handle == INVALID_HANDLE_VALUE)
            throw std::system_error(static_cast<int>(GetLastError()), std::system_category());
        auto pin = std::unique_ptr<void, decltype(&CloseHandle)>(handle, &CloseHandle);
        target.pins.push_back(std::move(pin));
        FILE_ATTRIBUTE_TAG_INFO info{};
        if (!GetFileInformationByHandleEx(handle, FileAttributeTagInfo, &info, sizeof(info)))
            throw std::system_error(static_cast<int>(GetLastError()), std::system_category());
        if (info.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT)
            throw std::runtime_error("Archive destination parent is a reparse point.");
#endif
    }
    return target;
}
}  // namespace

ArchiveExtractionResult ArchiveExtractor::extract(const ArchiveExtractionPlan& plan) const {
    ArchiveExtractionResult result{plan.archivePath};
    bool merging = false;
    try {
        const auto root = std::filesystem::canonical(plan.modRoot);
        const auto source = std::filesystem::absolute(plan.archivePath).lexically_normal();
        const auto relativeSource = source.lexically_relative(root);
        if (relativeSource.empty() || relativeSource.is_absolute() ||
            *relativeSource.begin() == ".." || hasStagingComponent(relativeSource))
            throw std::runtime_error("Archive source is outside its Mod Root.");
        // Inventory and free space may have changed since discovery. Check before creating
        // staging; successful preflight reserves nothing and later I/O still uses mutation rules.
        const auto inventory = inspectArchiveInventory(plan.archivePath);
        const auto available = _capacity ? _capacity(root) : std::nullopt;
        const auto required = (std::max)(plan.estimatedCapacityBytes, inventory.estimatedCapacityBytes);
        if (available && *available < required) {
            result.failure = ArchiveExtractionFailure::InsufficientCapacity;
            result.detail = archiveCapacityDetail(required, *available);
            return result;
        }
        auto archive = btu::bsa::read_archive(plan.archivePath);
        if (!archive) throw std::runtime_error("Unrecognized Archive format.");
        const std::set<std::string> expected(plan.entries.begin(), plan.entries.end());
        if (expected.size() != plan.entries.size())
            throw std::runtime_error("Archive manifest contains duplicate canonical entries.");
        for (const auto& entry : expected)
            if (canonicalArchiveEntryPath(entry) != entry)
                throw std::runtime_error("Archive plan contains a noncanonical entry.");
        for (const auto& entry : plan.mergeEntries)
            if (!expected.contains(entry))
                throw std::runtime_error("Archive merge entry is absent from its manifest.");
        std::map<std::string, TemporaryArtifactRegistry::StagedFile> staged;
        for (auto& [name, file] : *archive) {
            const auto entry = canonicalArchiveEntryPath(name);
            if (!expected.contains(entry) || staged.contains(entry))
                throw std::runtime_error("Archive manifest changed after preflight.");
            auto temporary = _artifacts.stageArchiveFile(plan.modRoot);
            file.write(temporary.path);
            staged.emplace(entry, std::move(temporary));
        }
        if (staged.size() != expected.size())
            throw std::runtime_error("Archive manifest changed after preflight.");
        merging = true;
        for (const auto& entry : plan.mergeEntries) {
            auto& temporary = staged.at(entry);
            const auto destination = prepareMergeTarget(
                root, source.parent_path() / std::filesystem::u8path(entry));
#ifdef _WIN32
            // The native no-replace commit also protects Loose Assets created after preflight.
            if (!MoveFileExW(temporary.path.c_str(), destination.path.c_str(), MOVEFILE_WRITE_THROUGH))
                throw std::system_error(static_cast<int>(GetLastError()), std::system_category());
#else
            // Hard-link publication is atomic and refuses existing destinations. An unlink
            // failure must retain the committed destination and report uncertain mutation.
            std::filesystem::create_hard_link(temporary.path, destination.path);
            std::filesystem::remove(temporary.path);
#endif
            _artifacts.commit(temporary.registration);
            result.mutation = execution::MutationState::Committed;
        }
        return result;
    } catch (const std::exception& error) {
        result.detail = error.what();
    } catch (...) {
        // Nonstandard library exceptions still owe the caller precise phase-boundary evidence.
        result.detail = "Unexpected Archive extraction exception.";
    }
    result.failure = merging ? ArchiveExtractionFailure::MergeFailed
                             : ArchiveExtractionFailure::ExtractionFailed;
    result.mutation = merging ? execution::MutationState::PartialOrUnknown
                              : execution::MutationState::None;
    result.safeToContinue = !merging;
    return result;
}
}  // namespace cao::run
