#include "StagingRecovery.h"
#include "StagingPaths.h"

#include <algorithm>
#include <fstream>
#include <iomanip>
#include <map>
#include <stdexcept>
#include <vector>

#ifdef _WIN32
#include <Windows.h>
#else
#include <fcntl.h>
#include <sys/file.h>
#include <sys/stat.h>
#include <unistd.h>
#endif

namespace cao::run {
namespace {
namespace fs = std::filesystem;

struct RecoveryCancelled {};

/// Stops between read-only steps or atomic removals; cancellation is not a preparation failure.
void observeCancellation(std::stop_token stop) {
    if (stop.stop_requested()) throw RecoveryCancelled{};
}

/// Carries the affected entry and stable failure classification through filesystem helpers.
class RecoveryError final : public std::runtime_error {
   public:
    RecoveryError(RunFailureCode code, fs::path path, const std::string& detail)
        : std::runtime_error(detail), code(code), path(std::move(path)) {}
    RunFailureCode code;
    fs::path path;
};

/// Fails closed without transferring ownership of any unverifiable path.
[[noreturn]] void unverified(const fs::path& path, const std::string& detail) {
    throw RecoveryError(RunFailureCode::StagingOwnershipUnverified, path, detail);
}

/// Encodes protocol paths as generic UTF-8, independently of the Windows ANSI code page.
std::string pathText(const fs::path& path) {
    const auto utf8 = path.generic_u8string();
    return std::string(utf8.begin(), utf8.end());
}

/// Inspects without following links, including Windows junctions and every other reparse tag.
fs::file_status inspect(const fs::path& path) {
    const auto status = fs::symlink_status(path);
    if (!fs::exists(status)) return status;
    if (fs::is_symlink(status)) unverified(path, "Staging contains a linked entry");
#ifdef _WIN32
    const auto attributes = GetFileAttributesW(path.c_str());
    if (attributes == INVALID_FILE_ATTRIBUTES)
        throw std::system_error(static_cast<int>(GetLastError()), std::system_category());
    if ((attributes & FILE_ATTRIBUTE_REPARSE_POINT) != 0)
        unverified(path, "Staging contains a reparse point");
#endif
    if (!fs::is_directory(status) && !fs::is_regular_file(status))
        unverified(path, "Staging contains an unsupported entry type");
    if (fs::is_regular_file(status) && fs::hard_link_count(path) != 1)
        unverified(path, "Staging contains a hard-linked file");
    return status;
}

enum class OpenMode { DirectoryPin, OwnershipLock, ManifestPin, TemporaryFile };

/// Holds existing entries; files and directories are pinned against Windows replacement.
/// No open here creates or truncates anything. OS process teardown releases abandoned locks.
class NativeLock final {
   public:
    /// Opens an existing entry with mode-specific sharing and ownership checks. Acquisition
    /// failures throw RecoveryError (or system_error on POSIX); the handle lives until destruction.
    explicit NativeLock(const fs::path& path, OpenMode mode) : _path(path), _mode(mode) {
        const bool directory = mode == OpenMode::DirectoryPin;
#ifdef _WIN32
        const auto access = mode == OpenMode::TemporaryFile
                                ? DELETE | FILE_READ_ATTRIBUTES
                                : (directory ? FILE_READ_ATTRIBUTES : GENERIC_READ);
        _handle = CreateFileW(
            path.c_str(), access,
            directory ? FILE_SHARE_READ | FILE_SHARE_WRITE
                      : (mode == OpenMode::OwnershipLock ? 0 : FILE_SHARE_READ),
            nullptr, OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT | (directory ? FILE_FLAG_BACKUP_SEMANTICS : 0), nullptr);
        if (_handle == INVALID_HANDLE_VALUE) {
            const auto error = GetLastError();
            throw RecoveryError(
                mode == OpenMode::OwnershipLock &&
                        (error == ERROR_SHARING_VIOLATION || error == ERROR_LOCK_VIOLATION)
                    ? RunFailureCode::StagingActive
                    : RunFailureCode::StagingOwnershipUnverified,
                path, std::system_category().message(static_cast<int>(error)));
        }
        BY_HANDLE_FILE_INFORMATION info{};
        const bool valid = GetFileInformationByHandle(_handle, &info) &&
                           !(info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT) &&
                           !!(info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) == directory &&
                           (directory || info.nNumberOfLinks == 1);
        if (!valid) {
            CloseHandle(_handle);
            _handle = INVALID_HANDLE_VALUE;
            unverified(path, "The opened staging entry does not match its expected identity type");
        }
#else
        _handle =
            open(path.c_str(), O_RDONLY | O_NOFOLLOW | O_CLOEXEC | (directory ? O_DIRECTORY : 0));
        if (_handle < 0) throw std::system_error(errno, std::generic_category());
        struct stat info{};
        const bool valid =
            fstat(_handle, &info) == 0 &&
            (directory ? S_ISDIR(info.st_mode) : S_ISREG(info.st_mode) && info.st_nlink == 1);
        if (!valid || (mode == OpenMode::OwnershipLock && flock(_handle, LOCK_EX | LOCK_NB) != 0)) {
            const auto error = errno;
            close(_handle);
            _handle = -1;
            if (!valid) unverified(path, "The opened staging entry has an unexpected type");
            throw RecoveryError(error == EWOULDBLOCK ? RunFailureCode::StagingActive
                                                     : RunFailureCode::StagingOwnershipUnverified,
                                path, std::generic_category().message(error));
        }
#endif
    }

    /// Releases the OS handle only; the stable control file must never be unlinked by recovery.
    ~NativeLock() {
#ifdef _WIN32
        if (_handle != INVALID_HANDLE_VALUE) CloseHandle(_handle);
#else
        if (_handle >= 0) close(_handle);
#endif
    }
    NativeLock(const NativeLock&) = delete;
    NativeLock& operator=(const NativeLock&) = delete;

    /// Deletes the pinned temporary file identity on Windows, never a replacement at its name.
    /// POSIX writers must honor owner.lock; identity is rechecked before unlinking there.
    void removeFile() {
        if (_mode != OpenMode::TemporaryFile) throw std::logic_error("Not a temporary file handle");
#ifdef _WIN32
        FILE_DISPOSITION_INFO disposition{TRUE};
        if (!SetFileInformationByHandle(_handle, FileDispositionInfo, &disposition,
                                        sizeof(disposition)))
            throw std::system_error(static_cast<int>(GetLastError()), std::system_category());
#else
        struct stat opened{}, current{};
        if (fstat(_handle, &opened) != 0 || lstat(_path.c_str(), &current) != 0 ||
            opened.st_dev != current.st_dev || opened.st_ino != current.st_ino)
            unverified(_path, "A temporary file identity changed during recovery");
        fs::remove(_path);
#endif
    }

   private:
    fs::path _path;
    OpenMode _mode;
#ifdef _WIN32
    HANDLE _handle{INVALID_HANDLE_VALUE};
#else
    int _handle{-1};
#endif
};

struct Artifact {
    fs::path relative;
    bool directory;
};

/// Reads a mandatory quoted string; bare tokens and truncated escapes are invalid ownership.
std::string quotedString(std::istream& input, const fs::path& manifest) {
    input >> std::ws;
    std::string value;
    if (input.peek() != '"' || !(input >> std::quoted(value)))
        unverified(manifest, "The ownership manifest contains an invalid quoted field");
    return value;
}

/// Rejects traversal, Windows aliases/streams, and nonportable separators in recorded names.
bool safeRelativeName(const std::string& name) {
    if (name.empty() || name.back() == '/' || name.front() == '/') return false;
    const auto path = fs::path(std::u8string(name.begin(), name.end()));
    if (path.is_absolute() || path.has_root_name()) return false;
    for (const auto& component : path) {
        const auto part = pathText(component);
        if (part.empty() || part == "." || part == ".." || part.back() == '.' ||
            part.back() == ' ' || part.find_first_of("<>:\"\\|?*") != std::string::npos ||
            std::any_of(part.begin(), part.end(), [](unsigned char c) { return c < 32; }))
            return false;
    }
    return pathText(path) == name && name.find("//") == std::string::npos;
}

/// Parses the bounded v1 protocol, proving root/run identity and parent-before-child ownership.
std::vector<Artifact> readManifest(const fs::path& staging, const fs::path& root,
                                   std::stop_token stop) {
    const auto manifest = staging / "ownership.manifest";
    if (!fs::is_regular_file(inspect(manifest)) || fs::file_size(manifest) > 8 * 1024 * 1024)
        unverified(manifest, "The ownership manifest is missing or exceeds the format limit");
    std::ifstream input(manifest, std::ios::binary);
    std::string magic;
    unsigned version{};
    if (!(input >> magic >> version) || magic != "CAO-STAGING" || version != 1)
        unverified(manifest, "The CAO ownership manifest signature or version is invalid");
    if (quotedString(input, manifest) != pathText(root))
        unverified(manifest, "The ownership manifest belongs to a different Mod Root");
    const auto runId = quotedString(input, manifest);
    const auto child = quotedString(input, manifest);
    const auto prefix = "run-" + runId + "-";
    if (runId.empty() || runId.size() > 128 ||
        runId.find_first_not_of(
            "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ-") !=
            std::string::npos ||
        !child.starts_with(prefix) || child.size() != prefix.size() + 32 ||
        child.substr(prefix.size()).find_first_not_of("0123456789abcdef") != std::string::npos)
        unverified(manifest, "The staging child does not match its Run ID and nonce");
    std::size_t count{};
    if (!(input >> count) || count == 0 || count > 100000)
        unverified(manifest, "The ownership manifest has an invalid artifact count");
    std::vector<Artifact> artifacts;
    std::map<std::string, bool> owned;
    for (std::size_t i = 0; i < count; ++i) {
        observeCancellation(stop);
        char kind{};
        input >> kind;
        const auto name = quotedString(input, manifest);
        if ((kind != 'D' && kind != 'F') || !safeRelativeName(name))
            unverified(manifest, "The ownership manifest contains an unsafe artifact record");
        const auto path = fs::path(std::u8string(name.begin(), name.end()));
        if (i == 0
                ? name != child || kind != 'D'
                : !name.starts_with(child + "/") || !owned.contains(pathText(path.parent_path())) ||
                      !owned.at(pathText(path.parent_path())))
            unverified(manifest,
                       "Artifact ownership is not contained beneath the recorded run child");
        if (!owned.emplace(name, kind == 'D').second)
            unverified(manifest, "The ownership manifest contains duplicate artifact paths");
        artifacts.push_back({path, kind == 'D'});
    }
    input >> std::ws;
    if (!input.eof())
        unverified(manifest, "The ownership manifest has trailing or unreadable data");
    return artifacts;
}

/// Validates every present entry before the first deletion, pinning files and directories.
/// Missing registrations are legal: a crash may occur after registration but before creation.
std::map<fs::path, std::unique_ptr<NativeLock>> validateTree(const fs::path& staging,
                                                             const std::vector<Artifact>& artifacts,
                                                             std::stop_token stop) {
    std::map<fs::path, bool> expected;
    for (const auto& artifact : artifacts) {
        observeCancellation(stop);
        expected.emplace(artifact.relative, artifact.directory);
    }
    std::map<fs::path, std::unique_ptr<NativeLock>> pins;
    for (const auto& entry : fs::recursive_directory_iterator(staging)) {
        observeCancellation(stop);
        const auto relative = entry.path().lexically_relative(staging);
        if (relative == "owner.lock" || relative == "ownership.manifest") continue;
        const auto found = expected.find(relative);
        if (found == expected.end())
            unverified(entry.path(), "Staging contains an unregistered entry");
        const auto status = inspect(entry.path());
        if (fs::is_directory(status) != found->second)
            unverified(entry.path(), "A staging artifact does not match its recorded type");
        pins.emplace(relative, std::make_unique<NativeLock>(
                                   entry.path(), found->second ? OpenMode::DirectoryPin
                                                               : OpenMode::TemporaryFile));
    }
    return pins;
}
}  // namespace

struct StagingRecovery::State {
    std::vector<std::unique_ptr<NativeLock>> locks;
};

StagingRecovery::StagingRecovery() : _state(std::make_unique<State>()) {}
StagingRecovery::~StagingRecovery() = default;

std::optional<RunFailure> StagingRecovery::recover(const std::filesystem::path& modRoot,
                                                   std::stop_token stop) {
    const auto staging = modRoot / ".cao-staging";
    bool deleting = false;
    auto affected = staging;
    try {
        observeCancellation(stop);
        for (const auto& entry : fs::directory_iterator(modRoot)) {
            observeCancellation(stop);
            if (isStagingName(entry.path()) && entry.path().filename() != ".cao-staging")
                unverified(entry.path(),
                           "An unknown staging-like name collides with the reserved namespace");
        }
        const auto status = inspect(staging);
        if (!fs::exists(status)) return {};
        if (!fs::is_directory(status))
            unverified(staging, "The reserved staging name is not a directory");
        auto rootPin = std::make_unique<NativeLock>(modRoot, OpenMode::DirectoryPin);
        auto stagingPin = std::make_unique<NativeLock>(staging, OpenMode::DirectoryPin);
        const auto lockPath = staging / "owner.lock";
        if (!fs::is_regular_file(inspect(lockPath)))
            unverified(staging, "The staging ownership lock is missing");
        auto lock = std::make_unique<NativeLock>(lockPath, OpenMode::OwnershipLock);
        // Deny manifest writes/renames while the parser and deletion pass rely on its ownership.
        auto manifestPin =
            std::make_unique<NativeLock>(staging / "ownership.manifest", OpenMode::ManifestPin);
        const auto artifacts = readManifest(staging, modRoot, stop);
        auto pins = validateTree(staging, artifacts, stop);
        // Retain the same lock through work and Safety Cleanup. Never delete/recreate its path:
        // otherwise another process could own a new lock while this run still uses the old one.
        _state->locks.push_back(std::move(rootPin));
        _state->locks.push_back(std::move(stagingPin));
        _state->locks.push_back(std::move(lock));
        _state->locks.push_back(std::move(manifestPin));
        deleting = true;
        for (auto artifact = artifacts.rbegin(); artifact != artifacts.rend(); ++artifact) {
            observeCancellation(stop);
            affected = staging / artifact->relative;
            const auto pinned = pins.find(artifact->relative);
            if (pinned == pins.end()) continue;
            if (fs::canonical(affected.parent_path()) != affected.parent_path())
                unverified(affected, "A staging artifact parent changed during recovery");
            if (!artifact->directory) {
                pinned->second->removeFile();
                pins.erase(pinned);
                continue;
            }
            pins.erase(artifact->relative);
            // Non-recursive removal preserves unregistered children, including newly added ones.
            fs::remove(affected);
        }
        return {};
    } catch (const RecoveryCancelled&) {
        // The executor observes the same token and still completes mandatory Safety Cleanup.
        return {};
    } catch (const RecoveryError& error) {
        const auto guidance =
            error.code == RunFailureCode::StagingActive
                ? " Wait for the owning CAO run to finish, then retry."
                : " Leave the contents in place. Inspect ownership.manifest and move "
                  "unrecognized material outside .cao-staging before retrying.";
        return RunFailure{
            error.code, RunPhase::Preparing, std::string(error.what()) + guidance, {}, error.path};
    } catch (const std::exception& error) {
        return RunFailure{deleting ? RunFailureCode::StagingRecoveryFailed
                                   : RunFailureCode::StagingOwnershipUnverified,
                          RunPhase::Preparing,
                          std::string(error.what()) +
                              " Leave remaining staging in place; check permissions and ownership "
                              "before retrying recovery.",
                          {},
                          affected};
    }
}
}  // namespace cao::run
