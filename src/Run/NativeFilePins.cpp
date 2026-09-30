#include "NativeFilePins.h"

#include <cstring>
#include <limits>
#include <stdexcept>
#include <system_error>
#include <vector>

namespace cao::run {
#ifdef _WIN32
/// Captures a native file ID and change metadata, rejecting aliases to links or directories.
SourceFileFacts inspectSourceFile(HANDLE handle, const std::filesystem::path& source) {
    SourceFileFacts facts;
    if (!GetFileInformationByHandle(handle, &facts.file) ||
        !GetFileInformationByHandleEx(handle, FileBasicInfo, &facts.basic,
                                      sizeof(facts.basic)))
        throw std::filesystem::filesystem_error(
            "Could not inspect source file", source,
            std::error_code(static_cast<int>(GetLastError()), std::system_category()));
    if (facts.file.dwFileAttributes & (FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_DIRECTORY))
        throw std::runtime_error("A source file is no longer an ordinary file.");
    return facts;
}

/// Requires the same file object and unchanged content-relevant metadata after Archive I/O.
bool sameSourceFile(const SourceFileFacts& earlier, const SourceFileFacts& current) {
    return earlier.file.dwVolumeSerialNumber == current.file.dwVolumeSerialNumber &&
           earlier.file.nFileIndexHigh == current.file.nFileIndexHigh &&
           earlier.file.nFileIndexLow == current.file.nFileIndexLow &&
           earlier.file.ftCreationTime.dwHighDateTime ==
               current.file.ftCreationTime.dwHighDateTime &&
           earlier.file.ftCreationTime.dwLowDateTime ==
               current.file.ftCreationTime.dwLowDateTime &&
           earlier.file.ftLastWriteTime.dwHighDateTime ==
               current.file.ftLastWriteTime.dwHighDateTime &&
           earlier.file.ftLastWriteTime.dwLowDateTime ==
               current.file.ftLastWriteTime.dwLowDateTime &&
           earlier.file.nFileSizeHigh == current.file.nFileSizeHigh &&
           earlier.file.nFileSizeLow == current.file.nFileSizeLow &&
           earlier.basic.ChangeTime.QuadPart == current.basic.ChangeTime.QuadPart;
}

/// Pins each directory from the filesystem root through the source parent, rejecting junctions
/// and other reparse points before a pathname-based Archive reader can follow them.
void pinSourceDirectories(const std::filesystem::path& source,
                          const std::filesystem::path& root,
                          std::map<std::filesystem::path, SourceFileHandle>& pins,
                          const DWORD shareMode) {
    const auto relative = source.lexically_relative(root);
    if (relative.empty() || relative.is_absolute() || relative == "." ||
        *relative.begin() == "..")
        throw std::invalid_argument("A source file is outside its Mod Root.");

    auto directory = source.root_path();
    if (directory.empty()) throw std::invalid_argument("A source file needs an absolute path.");
    const auto pinDirectory = [&](const std::filesystem::path& path) {
        if (pins.contains(path)) return;
        // Attribute-only opens do not participate in sharing checks; directory read access
        // makes the missing FILE_SHARE_DELETE actually prevent a parent rename or replacement.
        const auto handle = CreateFileW(path.c_str(), FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
                                        shareMode, nullptr, OPEN_EXISTING,
                                        FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                                        nullptr);
        if (handle == INVALID_HANDLE_VALUE)
            throw std::filesystem::filesystem_error(
                "Could not pin source parent", path,
                std::error_code(static_cast<int>(GetLastError()), std::system_category()));
        SourceFileHandle pin(handle, &CloseHandle);
        BY_HANDLE_FILE_INFORMATION information{};
        if (!GetFileInformationByHandle(handle, &information))
            throw std::filesystem::filesystem_error(
                "Could not inspect source parent", path,
                std::error_code(static_cast<int>(GetLastError()), std::system_category()));
        if (!(information.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) ||
            (information.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT))
            throw std::runtime_error("A source parent is no longer an ordinary directory.");
        pins.emplace(path, std::move(pin));
    };
    // Start at a drive or UNC root that cannot be renamed, then keep each child stable while
    // opening the next; checking just the final parent could still follow an earlier junction.
    pinDirectory(directory);
    for (const auto& part : source.parent_path().lexically_relative(directory)) {
        if (part == ".") continue;
        directory /= part;
        pinDirectory(directory);
    }
}

LoadingPluginPin::LoadingPluginPin(const std::filesystem::path& plugin,
                                   const std::filesystem::path& modRoot) {
    // Keep ordinary ancestors and the entry stable while source cleanup uses pathnames.
    pinSourceDirectories(plugin, modRoot, _directories, FILE_SHARE_READ);
    const auto entry = CreateFileW(plugin.c_str(), GENERIC_READ, FILE_SHARE_READ, nullptr,
                                   OPEN_EXISTING, FILE_FLAG_OPEN_REPARSE_POINT, nullptr);
    if (entry == INVALID_HANDLE_VALUE)
        throw std::filesystem::filesystem_error(
            "Could not pin loading plugin", plugin,
            std::error_code(static_cast<int>(GetLastError()), std::system_category()));
    _entry.reset(entry);
    BY_HANDLE_FILE_INFORMATION information{};
    if (!GetFileInformationByHandle(entry, &information))
        throw std::filesystem::filesystem_error(
            "Could not inspect loading plugin", plugin,
            std::error_code(static_cast<int>(GetLastError()), std::system_category()));
    // Attribute-only access can retarget a symlink despite sharing locks. A nonempty
    // ordinary file cannot become a symlink without a write that this handle excludes.
    if (information.dwFileAttributes & (FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_DIRECTORY) ||
        (information.nFileSizeHigh == 0 && information.nFileSizeLow == 0))
        throw std::runtime_error("The loading plugin is not a nonempty ordinary file.");
}
#endif

/// Publishes a source backup without replacing any directory entry, including dangling links.
/// Retries occupied names; other filesystem failures leave the source or published backup intact.
#ifdef _WIN32
void backupExtractedArchive(const std::filesystem::path& source, HANDLE handle) {
#else
void backupExtractedArchive(const std::filesystem::path& source) {
#endif
    auto destination = source;
    for (;;) {
        destination += ".bak";
        std::error_code error;
#ifdef _WIN32
        // Rename the verified file object, not a pathname that could be replaced after checking.
        const auto& name = destination.native();
        const auto nameBytes = name.size() * sizeof(wchar_t);
        if (nameBytes > (std::numeric_limits<DWORD>::max)() - sizeof(FILE_RENAME_INFO))
            throw std::invalid_argument("Archive backup name is too long.");
        std::vector<std::byte> buffer(sizeof(FILE_RENAME_INFO) + nameBytes);
        auto* rename = reinterpret_cast<FILE_RENAME_INFO*>(buffer.data());
        rename->ReplaceIfExists = FALSE;
        rename->RootDirectory = nullptr;
        rename->FileNameLength = static_cast<DWORD>(nameBytes);
        std::memcpy(rename->FileName, name.data(), nameBytes);
        if (SetFileInformationByHandle(handle, FileRenameInfo, rename,
                                       static_cast<DWORD>(buffer.size())))
            return;
        error = std::error_code(static_cast<int>(GetLastError()), std::system_category());
#else
        // Same-directory hard-link publication is atomic and rejects occupied names.
        std::filesystem::create_hard_link(source, destination, error);
        if (!error) {
            if (!std::filesystem::remove(source))
                throw std::runtime_error("The backed-up source Archive could not be removed.");
            return;
        }
#endif
        std::error_code statusError;
        const auto status = std::filesystem::symlink_status(destination, statusError);
        if (!statusError && std::filesystem::exists(status)) continue;
        throw std::filesystem::filesystem_error("Could not back up extracted Archive", source,
                                                destination, error);
    }
}
}  // namespace cao::run

#ifdef _WIN32
struct cao::run::SourceFilePin::DirectoryPins final {
    std::filesystem::path modRoot;
    std::map<std::filesystem::path, SourceFileHandle> handles;

    /// Keeps the canonical output scope with its reusable native directory handles.
    explicit DirectoryPins(std::filesystem::path root) : modRoot(std::move(root)) {}
};

struct cao::run::SourceFilePin::State final {
    std::filesystem::path source;
    std::shared_ptr<DirectoryPins> directories;
    SourceFileHandle pinned{nullptr, &CloseHandle};
    SourceFileFacts facts;

    /// Reopens the recorded path for native cleanup and rejects a substituted file object.
    SourceFileHandle openForCleanup() {
        SourceFileHandle guardian{nullptr, &CloseHandle};
        if (pinned) {
            // Keep the original file ID live while transitioning from a read-only pin to a
            // DELETE-capable handle. The first pin still denies replacement until this opens.
            const auto handle = CreateFileW(source.c_str(), FILE_READ_ATTRIBUTES,
                                            FILE_SHARE_READ | FILE_SHARE_DELETE, nullptr,
                                            OPEN_EXISTING, FILE_FLAG_OPEN_REPARSE_POINT, nullptr);
            if (handle == INVALID_HANDLE_VALUE)
                throw std::filesystem::filesystem_error(
                    "Could not retain source identity for cleanup", source,
                    std::error_code(static_cast<int>(GetLastError()), std::system_category()));
            guardian.reset(handle);
            pinned.reset();
        }
        const auto handle = CreateFileW(source.c_str(), DELETE | FILE_READ_ATTRIBUTES,
                                        FILE_SHARE_READ, nullptr, OPEN_EXISTING,
                                        FILE_FLAG_OPEN_REPARSE_POINT, nullptr);
        if (handle == INVALID_HANDLE_VALUE)
            throw std::filesystem::filesystem_error(
                "Could not reopen source for cleanup", source,
                std::error_code(static_cast<int>(GetLastError()), std::system_category()));
        SourceFileHandle candidate(handle, &CloseHandle);
        if (!sameSourceFile(facts, inspectSourceFile(handle, source)))
            throw std::runtime_error("A source file changed before cleanup.");
        return candidate;
    }
};

std::shared_ptr<cao::run::SourceFilePin::DirectoryPins>
cao::run::SourceFilePin::sharedDirectoryPins(std::filesystem::path modRoot) {
    return std::make_shared<DirectoryPins>(
        std::filesystem::absolute(std::move(modRoot)).lexically_normal());
}

cao::run::SourceFilePin::SourceFilePin(std::filesystem::path source,
                                     std::filesystem::path modRoot,
                                     std::shared_ptr<DirectoryPins> directoryPins)
    : _state(std::make_unique<State>()) {
    _state->source = std::filesystem::absolute(std::move(source)).lexically_normal();
    const auto root = std::filesystem::absolute(std::move(modRoot)).lexically_normal();
    if (!directoryPins) directoryPins = sharedDirectoryPins(root);
    if (directoryPins->modRoot != root)
        throw std::invalid_argument("Source directory pins belong to another Mod Root.");
    pinSourceDirectories(_state->source, root, directoryPins->handles);
    _state->directories = std::move(directoryPins);
    const auto handle = CreateFileW(_state->source.c_str(), GENERIC_READ, FILE_SHARE_READ,
                                    nullptr, OPEN_EXISTING, FILE_FLAG_OPEN_REPARSE_POINT, nullptr);
    if (handle == INVALID_HANDLE_VALUE)
        throw std::filesystem::filesystem_error(
            "Could not pin source file", _state->source,
            std::error_code(static_cast<int>(GetLastError()), std::system_category()));
    _state->pinned.reset(handle);
    _state->facts = inspectSourceFile(handle, _state->source);
}

cao::run::SourceFilePin::~SourceFilePin() = default;
cao::run::SourceFilePin::SourceFilePin(SourceFilePin&&) noexcept = default;
cao::run::SourceFilePin& cao::run::SourceFilePin::operator=(SourceFilePin&&) noexcept =
    default;

void cao::run::SourceFilePin::releaseForCleanup() noexcept {
    if (_state) _state->pinned.reset();
}

void cao::run::SourceFilePin::removeIfUnchanged() {
    if (!_state) throw std::logic_error("A moved source pin cannot remove a source.");
    const auto candidate = _state->openForCleanup();
    FILE_DISPOSITION_INFO disposition{TRUE};
    if (!SetFileInformationByHandle(candidate.get(), FileDispositionInfo, &disposition,
                                    sizeof(disposition)))
        throw std::system_error(static_cast<int>(GetLastError()), std::system_category(),
                                "A source file could not be removed");
}

void cao::run::SourceFilePin::backupIfUnchanged() {
    if (!_state) throw std::logic_error("A moved source pin cannot back up a source.");
    const auto candidate = _state->openForCleanup();
    backupExtractedArchive(_state->source, candidate.get());
}

void cao::run::SourceFilePin::pinUnchangedForRecovery() {
    if (!_state) throw std::logic_error("A moved source pin cannot verify a source.");
    const auto handle = CreateFileW(_state->source.c_str(), GENERIC_READ, FILE_SHARE_READ,
                                    nullptr, OPEN_EXISTING, FILE_FLAG_OPEN_REPARSE_POINT, nullptr);
    if (handle == INVALID_HANDLE_VALUE)
        throw std::filesystem::filesystem_error(
            "Could not pin retained source file", _state->source,
            std::error_code(static_cast<int>(GetLastError()), std::system_category()));
    SourceFileHandle candidate(handle, &CloseHandle);
    if (!sameSourceFile(_state->facts, inspectSourceFile(handle, _state->source)))
        throw std::runtime_error("The retained source file changed after extraction.");
    _state->pinned = std::move(candidate);
}
#endif
