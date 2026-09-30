#pragma once

// Native pins shared by Archive extraction and Archive Finalization. They keep a source, its
// Loading Plugin, and their ordinary ancestors stable while pathname-based Archive I/O runs.

#include <filesystem>
#include <map>
#include <memory>

#ifdef _WIN32
#include <Windows.h>

namespace cao::run {
/// Owns one native handle opened for pinning or identity checks.
using SourceFileHandle = std::unique_ptr<void, decltype(&CloseHandle)>;

/// Native file identity and change metadata captured from an open handle.
struct SourceFileFacts final {
    BY_HANDLE_FILE_INFORMATION file{};
    FILE_BASIC_INFO basic{};
};

/// Captures a native file ID and change metadata, rejecting aliases to links or directories.
SourceFileFacts inspectSourceFile(HANDLE handle, const std::filesystem::path& source);

/// Requires the same file object and unchanged content-relevant metadata after Archive I/O.
bool sameSourceFile(const SourceFileFacts& earlier, const SourceFileFacts& current);

/// Pins each directory from the filesystem root through the source parent, rejecting junctions
/// and other reparse points before a pathname-based Archive reader can follow them.
void pinSourceDirectories(const std::filesystem::path& source,
                          const std::filesystem::path& root,
                          std::map<std::filesystem::path, SourceFileHandle>& pins,
                          DWORD shareMode = FILE_SHARE_READ | FILE_SHARE_WRITE);

/// Publishes a source backup without replacing any directory entry, including dangling links.
/// Retries occupied names; other filesystem failures leave the source or published backup intact.
void backupExtractedArchive(const std::filesystem::path& source, HANDLE handle);

/// Holds a source stable while an Archive is read or written, then cleans only that file.
class SourceFilePin final {
   public:
    /// Opaque shared ownership of ordinary ancestor directories for one Mod Root.
    struct DirectoryPins;
    /// Creates a Mod Root-scoped set of ancestor handles reusable by one output's source pins.
    [[nodiscard]] static std::shared_ptr<DirectoryPins> sharedDirectoryPins(
        std::filesystem::path modRoot);
    /// Opens an ordinary source for reading while denying concurrent writes and renames.
    /// Pins its directory chain within modRoot until cleanup so no ancestor can redirect the path.
    /// Throws when a parent is a reparse point or the source is outside that Mod Root.
    SourceFilePin(std::filesystem::path source, std::filesystem::path modRoot,
                  std::shared_ptr<DirectoryPins> directoryPins = {});
    ~SourceFilePin();
    SourceFilePin(SourceFilePin&&) noexcept;
    SourceFilePin& operator=(SourceFilePin&&) noexcept;
    SourceFilePin(const SourceFilePin&) = delete;
    SourceFilePin& operator=(const SourceFilePin&) = delete;

    /// Releases the read-period handle so a DELETE-capable handle can be opened.
    /// Directory pins remain live to prevent a parent substitution during that transition.
    void releaseForCleanup() noexcept;
    /// Reopens the source without write/delete sharing and deletes only its recorded identity.
    /// Throws if file identity or change metadata differs, or Windows rejects deletion.
    void removeIfUnchanged();
    /// Renames only the recorded identity to an unoccupied .bak name, retrying occupied names.
    /// Throws if the source changed or no backup could be published.
    void backupIfUnchanged();
    /// Pins the unchanged source again while the caller verifies recoverable Archive bytes.
    /// Throws if its path no longer names the recorded source.
    void pinUnchangedForRecovery();

   private:
    struct State;
    std::unique_ptr<State> _state;
};

/// Keeps one ordinary Loading Plugin entry usable until its Archive's source cleanup ends.
class LoadingPluginPin final {
   public:
    /// Rejects links, empty files, and unstable paths before any packed source is deleted.
    LoadingPluginPin(const std::filesystem::path& plugin, const std::filesystem::path& modRoot);

   private:
    std::map<std::filesystem::path, SourceFileHandle> _directories;
    SourceFileHandle _entry{nullptr, &CloseHandle};
};
}  // namespace cao::run
#else
namespace cao::run {
/// Publishes a source backup without replacing any directory entry, including dangling links.
/// Retries occupied names; throws if the backup cannot be published or the source removed.
void backupExtractedArchive(const std::filesystem::path& source);
}  // namespace cao::run
#endif
