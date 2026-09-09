#include "StagingRecovery.h"
#include "StagingPaths.h"

#include <algorithm>
#include <fstream>
#include <iomanip>
#include <map>
#include <random>
#include <sstream>
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

/// Separates an exclusively rejected name from failures after the writer acquired its file.
class CreationCollision final : public std::runtime_error {
   public:
    explicit CreationCollision(const fs::path& path)
        : std::runtime_error("An unowned entry already occupies a new staging path: " +
                             path.string()) {}
};

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

/// Pins entries against Windows replacement and can exclusively create a new ownership lock.
/// Existing entries are never truncated. OS process teardown releases abandoned locks.
class NativeLock final {
   public:
    /// Opens an entry with mode-specific sharing and identity checks; create exclusively claims a
    /// new owner lock. Acquisition failures throw; the handle lives until destruction.
    explicit NativeLock(const fs::path& path, OpenMode mode, bool create = false)
        : _path(path), _mode(mode) {
        const bool directory = mode == OpenMode::DirectoryPin;
#ifdef _WIN32
        const auto access = mode == OpenMode::TemporaryFile
                                ? DELETE | FILE_READ_ATTRIBUTES
                                : (directory ? FILE_READ_ATTRIBUTES : GENERIC_READ);
        _handle = CreateFileW(
            path.c_str(), access,
            directory ? FILE_SHARE_READ | FILE_SHARE_WRITE
                      : (mode == OpenMode::OwnershipLock ? 0 : FILE_SHARE_READ),
            nullptr, create ? CREATE_NEW : OPEN_EXISTING,
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
        _handle = open(path.c_str(),
                       O_RDONLY | O_NOFOLLOW | O_CLOEXEC | (directory ? O_DIRECTORY : 0) |
                           (create ? O_CREAT | O_EXCL : 0),
                       0600);
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
    bool rootRelative{};
};

/// Resolves a manifest record against the namespace selected by its record kind.
fs::path artifactPath(const fs::path& root, const Artifact& artifact) {
    return (artifact.rootRelative ? root : root / ".cao-staging") / artifact.relative;
}

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

/// Proves a v3 sibling filename carries the manifest's Run ID and one lowercase-hex nonce.
bool safeSiblingTextureName(const fs::path& path, const std::string& runId) {
    const auto filename = pathText(path.filename());
    const auto prefix = ".cao-staging-texture-" + runId + "-";
    constexpr auto suffix = ".dds";
    if (!filename.starts_with(prefix) || !filename.ends_with(suffix) ||
        filename.size() != prefix.size() + 32 + std::char_traits<char>::length(suffix))
        return false;
    const auto nonce = filename.substr(prefix.size(), 32);
    return std::all_of(nonce.begin(), nonce.end(), [](const unsigned char character) {
        return (character >= '0' && character <= '9') || (character >= 'a' && character <= 'f');
    });
}

/// Parses bounded ownership records, proving root/run identity and namespace containment.
std::vector<Artifact> readManifest(const fs::path& staging, const fs::path& root,
                                   std::stop_token stop, unsigned& version) {
    const auto manifest = staging / "ownership.manifest";
    if (!fs::is_regular_file(inspect(manifest)) || fs::file_size(manifest) > 8 * 1024 * 1024)
        unverified(manifest, "The ownership manifest is missing or exceeds the format limit");
    std::ifstream input(manifest, std::ios::binary);
    std::string magic;
    if (!(input >> magic >> version) || magic != "CAO-STAGING" ||
        (version != 1 && version != 2 && version != 3))
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
    std::map<std::string, bool> rootOwned;
    for (std::size_t i = 0; i < count; ++i) {
        observeCancellation(stop);
        char kind{};
        input >> kind;
        const auto name = quotedString(input, manifest);
        const bool rootRelative = kind == 'S';
        if ((kind != 'D' && kind != 'F' && !(version == 3 && rootRelative)) ||
            !safeRelativeName(name))
            unverified(manifest, "The ownership manifest contains an unsafe artifact record");
        const auto path = fs::path(std::u8string(name.begin(), name.end()));
        if (rootRelative) {
            if (i == 0 || !safeSiblingTextureName(path, runId) ||
                hasStagingComponent(path.parent_path()) || !rootOwned.emplace(name, false).second)
                unverified(manifest,
                           "A sibling Texture record is unsafe or duplicates owned output");
        } else {
            if (i == 0 ? name != child || kind != 'D'
                       : !name.starts_with(child + "/") ||
                             !owned.contains(pathText(path.parent_path())) ||
                             !owned.at(pathText(path.parent_path())))
                unverified(manifest,
                           "Artifact ownership is not contained beneath the recorded run child");
            if (!owned.emplace(name, kind == 'D').second)
                unverified(manifest, "The ownership manifest contains duplicate artifact paths");
        }
        artifacts.push_back({path, kind == 'D', rootRelative});
    }
    input >> std::ws;
    if (!input.eof())
        unverified(manifest, "The ownership manifest has trailing or unreadable data");
    return artifacts;
}

/// Validates every present entry before the first deletion, pinning files and directories.
/// Missing registrations are legal: a crash may occur after registration but before creation.
std::map<fs::path, std::unique_ptr<NativeLock>> validateTree(const fs::path& staging,
                                                             const fs::path& root,
                                                             const std::vector<Artifact>& artifacts,
                                                             std::stop_token stop,
                                                             unsigned version) {
    std::map<fs::path, bool> expected;
    for (const auto& artifact : artifacts) {
        observeCancellation(stop);
        if (!artifact.rootRelative) expected.emplace(artifact.relative, artifact.directory);
    }
    std::map<fs::path, std::unique_ptr<NativeLock>> pins;
    for (const auto& entry : fs::recursive_directory_iterator(staging)) {
        observeCancellation(stop);
        const auto relative = entry.path().lexically_relative(staging);
        if (relative == "owner.lock" || relative == "ownership.manifest") continue;
        // A valid v2 or v3 manifest owns this fixed scratch control even if a crash truncated it.
        if (version >= 2 && relative == "ownership.manifest.next") {
            if (!fs::is_regular_file(inspect(entry.path())))
                unverified(entry.path(), "The manifest scratch control is not a regular file");
            pins.emplace(relative,
                         std::make_unique<NativeLock>(entry.path(), OpenMode::TemporaryFile));
            continue;
        }
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
    for (const auto& artifact : artifacts) {
        observeCancellation(stop);
        if (!artifact.rootRelative) continue;
        const auto path = artifactPath(root, artifact);
        if (fs::weakly_canonical(path.parent_path()) != path.parent_path())
            unverified(path, "A sibling Texture staging parent changed during recovery");
        const auto status = inspect(path);
        if (!fs::exists(status)) continue;
        if (!fs::is_regular_file(status))
            unverified(path, "A sibling Texture staging artifact is not a regular file");
        pins.emplace(artifact.relative,
                     std::make_unique<NativeLock>(path, OpenMode::TemporaryFile));
    }
    return pins;
}

/// Generates an unpredictable portable component; exclusive creation still arbitrates collisions.
std::string nonce() {
    std::random_device random;
    std::ostringstream result;
    result << std::hex << std::setfill('0');
    for (unsigned i = 0; i < 4; ++i) result << std::setw(8) << random();
    return result.str();
}

/// Creates without truncation and flushes all bytes before returning; failure preserves the file.
void writeNewFile(const fs::path& path, const std::string& bytes) {
#ifdef _WIN32
    const auto handle = CreateFileW(path.c_str(), GENERIC_WRITE, 0, nullptr, CREATE_NEW,
                                    FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT, nullptr);
    if (handle == INVALID_HANDLE_VALUE) {
        const auto error = GetLastError();
        if (error == ERROR_FILE_EXISTS || error == ERROR_ALREADY_EXISTS)
            throw CreationCollision(path);
        throw std::system_error(static_cast<int>(error), std::system_category());
    }
    DWORD written{};
    const bool success =
        WriteFile(handle, bytes.data(), static_cast<DWORD>(bytes.size()), &written, nullptr) &&
        written == bytes.size() && FlushFileBuffers(handle);
    const auto error = GetLastError();
    CloseHandle(handle);
    if (!success) throw std::system_error(static_cast<int>(error), std::system_category());
#else
    const auto handle =
        open(path.c_str(), O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, 0600);
    if (handle < 0) {
        if (errno == EEXIST) throw CreationCollision(path);
        throw std::system_error(errno, std::generic_category());
    }
    const auto written = write(handle, bytes.data(), bytes.size());
    const bool success =
        written >= 0 && static_cast<std::size_t>(written) == bytes.size() && fsync(handle) == 0;
    const auto error = errno;
    close(handle);
    if (!success) throw std::system_error(error, std::generic_category());
#endif
}

/// Publishes a complete v3 snapshot before producers may create any newly owned entries.
void publishManifest(const fs::path& root, const std::string& runId, const fs::path& child,
                     const std::vector<Artifact>& artifacts) {
    const auto staging = root / ".cao-staging";
    const auto scratch = staging / "ownership.manifest.next";
    const auto manifest = staging / "ownership.manifest";
    std::ostringstream output;
    output << "CAO-STAGING 3\n"
           << std::quoted(pathText(root)) << '\n'
           << std::quoted(runId) << ' ' << std::quoted(pathText(child)) << '\n'
           << artifacts.size() << '\n';
    for (const auto& artifact : artifacts)
        output << (artifact.rootRelative ? 'S'
                   : artifact.directory  ? 'D'
                                         : 'F')
               << ' ' << std::quoted(pathText(artifact.relative)) << '\n';
    if (output.str().size() > 8 * 1024 * 1024 || artifacts.size() > 100000)
        throw std::runtime_error("The staging ownership manifest exceeds its format limit");
    writeNewFile(scratch, output.str());
#ifdef _WIN32
    if (!MoveFileExW(scratch.c_str(), manifest.c_str(),
                     MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH))
        throw std::system_error(static_cast<int>(GetLastError()), std::system_category());
#else
    fs::rename(scratch, manifest);
#endif
}
}  // namespace

struct StagingRecovery::State {
    struct Area {
        std::string runId;
        fs::path child;
        std::vector<Artifact> artifacts;
        std::unique_ptr<NativeLock> childPin;
        bool ready{};
    };
    std::vector<std::unique_ptr<NativeLock>> locks;
    std::map<fs::path, Area> areas;
};

StagingRecovery::StagingRecovery() : _state(std::make_unique<State>()) {}
StagingRecovery::~StagingRecovery() = default;

fs::path StagingRecovery::stageFile(const fs::path& modRoot, const fs::path& destination) {
    const auto root = fs::canonical(modRoot);
    const auto parent = fs::canonical(destination.parent_path());
    const auto destinationParent = parent.lexically_relative(root);
    if (!modRoot.is_absolute() || !destination.is_absolute() || destinationParent.empty() ||
        *destinationParent.begin() == ".." || hasStagingComponent(destinationParent) ||
        destination.filename().empty())
        throw std::invalid_argument("A staged output must belong to its canonical Mod Root");
#ifdef _WIN32
    wchar_t rootMount[MAX_PATH]{}, destinationMount[MAX_PATH]{};
    wchar_t rootVolume[MAX_PATH]{}, destinationVolume[MAX_PATH]{};
    if (!GetVolumePathNameW(root.c_str(), rootMount, MAX_PATH) ||
        !GetVolumePathNameW(parent.c_str(), destinationMount, MAX_PATH) ||
        !GetVolumeNameForVolumeMountPointW(rootMount, rootVolume, MAX_PATH) ||
        !GetVolumeNameForVolumeMountPointW(destinationMount, destinationVolume, MAX_PATH))
        throw std::system_error(static_cast<int>(GetLastError()), std::system_category());
    if (CompareStringOrdinal(rootVolume, -1, destinationVolume, -1, TRUE) != CSTR_EQUAL)
        throw std::invalid_argument("The staged output and Mod Root must be on the same volume");
#endif
    const auto extension = pathText(destination.extension());
    if (!safeRelativeName("temporary" + extension))
        throw std::invalid_argument("The staging output extension is unsafe");
    if (const auto failure = recover(root)) throw std::runtime_error(failure->detail());
    const auto staging = root / ".cao-staging";
    if (!_state->areas.contains(root)) {
        auto rootPin = std::make_unique<NativeLock>(root, OpenMode::DirectoryPin);
        // Claim only a newly created area. A pre-existing unproven directory is never adopted.
        if (!fs::create_directory(staging))
            throw std::runtime_error("The reserved staging area appeared during initialization");
        auto stagingPin = std::make_unique<NativeLock>(staging, OpenMode::DirectoryPin);
        auto lock =
            std::make_unique<NativeLock>(staging / "owner.lock", OpenMode::OwnershipLock, true);
        _state->locks.push_back(std::move(rootPin));
        _state->locks.push_back(std::move(stagingPin));
        _state->locks.push_back(std::move(lock));
        _state->areas.emplace(root, State::Area{});
    }
    auto& area = _state->areas.at(root);
    if (!area.child.empty() && !area.ready)
        throw std::runtime_error("The staging area initialization did not complete");
    if (area.child.empty()) {
        area.runId = nonce();
        area.child = "run-" + area.runId + "-" + nonce();
        area.artifacts = {{area.child, true}};
        // Bootstrap may leave only controls if interrupted here; no Texture bytes exist yet.
        publishManifest(root, area.runId, area.child, area.artifacts);
        if (!fs::create_directory(staging / area.child))
            throw std::runtime_error("The staging run child already exists");
        area.childPin = std::make_unique<NativeLock>(staging / area.child, OpenMode::DirectoryPin);
        area.ready = true;
    }
    const auto filename = ".cao-staging-texture-" + area.runId + "-" + nonce() + extension;
    const auto relativeFile =
        destinationParent == "." ? fs::path(filename) : destinationParent / filename;
    auto registered = area.artifacts;
    registered.push_back({relativeFile, false, true});
    publishManifest(root, area.runId, area.child, registered);
    area.artifacts = std::move(registered);
    const auto path = root / relativeFile;
    try {
        writeNewFile(path, {});
    } catch (const CreationCollision&) {
        // A rejected CREATE_NEW never acquired this entry. Release its name even for current-run
        // cleanup; cooperating writers hold owner.lock, but unrelated writers must be preserved.
        area.artifacts.pop_back();
        try {
            publishManifest(root, area.runId, area.child, area.artifacts);
        } catch (...) {
            // An unregistered conflict control makes future recovery preserve the whole tree if
            // the ownership release could not be published. Never delete the colliding entry.
            writeNewFile(staging / "ownership.conflict",
                         "A staged name collided before creation.\n");
            throw;
        }
        throw;
    }
    return path;
}

void StagingRecovery::releaseFile(const fs::path& temporary) {
    for (auto& [root, area] : _state->areas) {
        auto retained = area.artifacts;
        const auto found =
            std::find_if(retained.begin(), retained.end(), [&](const Artifact& artifact) {
                return !artifact.directory && artifactPath(root, artifact) == temporary;
            });
        if (found == retained.end()) continue;
        if (fs::exists(inspect(temporary)))
            throw std::logic_error(
                "A durable temporary file must be moved before releasing ownership");
        retained.erase(found);
        publishManifest(root, area.runId, area.child, retained);
        area.artifacts = std::move(retained);
        return;
    }
    throw std::logic_error("The durable temporary file is not registered");
}

std::vector<RunFailure> StagingRecovery::cleanupArtifacts() {
    std::vector<RunFailure> failures;
    for (auto& [root, area] : _state->areas) {
        if (area.child.empty()) continue;
        const auto staging = root / ".cao-staging";
        area.childPin.reset();
        // Durable registrations predate creation, so they also cover native write failures
        // or a registry allocation failure before its in-memory receipt could be returned.
        // Unlike stale recovery, current-run cleanup attempts every individually owned artifact.
        for (auto artifact = area.artifacts.rbegin(); artifact != area.artifacts.rend();
             ++artifact) {
            const auto affected = artifactPath(root, *artifact);
            try {
                if (fs::weakly_canonical(affected.parent_path()) != affected.parent_path())
                    unverified(affected, "A staging artifact parent changed during cleanup");
                const auto status = inspect(affected);
                if (!fs::exists(status)) continue;
                if (fs::is_directory(status) != artifact->directory)
                    unverified(affected, "A staging artifact changed its recorded type");
                if (artifact->directory) {
                    // Non-recursive removal preserves any unregistered contents.
                    fs::remove(affected);
                } else {
                    NativeLock(affected, OpenMode::TemporaryFile).removeFile();
                }
            } catch (const std::exception& error) {
                failures.emplace_back(RunFailureCode::TemporaryArtifactCleanupFailed,
                                      RunPhase::SafetyCleanup, error.what(),
                                      routing::PolicyValidationErrors{}, affected);
            }
        }
    }
    return failures;
}

std::optional<RunFailure> StagingRecovery::recover(const std::filesystem::path& modRoot,
                                                   std::stop_token stop) {
    const auto staging = modRoot / ".cao-staging";
    bool deleting = false;
    auto affected = staging;
    try {
        observeCancellation(stop);
        if (_state->areas.contains(modRoot)) return {};
        std::vector<fs::path> unknownStagingNames;
        for (const auto& entry : fs::directory_iterator(modRoot)) {
            observeCancellation(stop);
            if (isStagingName(entry.path()) && entry.path().filename() != ".cao-staging")
                unknownStagingNames.push_back(entry.path());
        }
        const auto status = inspect(staging);
        if (!fs::exists(status)) {
            if (!unknownStagingNames.empty())
                unverified(unknownStagingNames.front(),
                           "An unknown staging-like name collides with the reserved namespace");
            return {};
        }
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
        unsigned version{};
        const auto artifacts = readManifest(staging, modRoot, stop, version);
        for (const auto& path : unknownStagingNames) {
            const auto owned =
                std::any_of(artifacts.begin(), artifacts.end(), [&](const auto& item) {
                    return item.rootRelative && artifactPath(modRoot, item) == path;
                });
            if (!owned)
                unverified(path,
                           "An unknown staging-like name collides with the reserved namespace");
        }
        auto pins = validateTree(staging, modRoot, artifacts, stop, version);
        // Retain the same lock through work and Safety Cleanup. Never delete/recreate its path:
        // otherwise another process could own a new lock while this run still uses the old one.
        _state->locks.push_back(std::move(rootPin));
        _state->locks.push_back(std::move(stagingPin));
        _state->locks.push_back(std::move(lock));
        deleting = true;
        const auto scratch = pins.find("ownership.manifest.next");
        if (scratch != pins.end()) {
            scratch->second->removeFile();
            pins.erase(scratch);
        }
        for (auto artifact = artifacts.rbegin(); artifact != artifacts.rend(); ++artifact) {
            observeCancellation(stop);
            affected = artifactPath(modRoot, *artifact);
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
        // Parsing/deletion needed a stable manifest, but the producer must now replace it.
        manifestPin.reset();
        _state->areas.emplace(modRoot, State::Area{});
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
