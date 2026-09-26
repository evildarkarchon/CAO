#include "TemporaryArtifactRegistry.h"
#include "StagingRecovery.h"
#include "StagingPaths.h"

#include <algorithm>
#include <array>
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <limits>
#include <stdexcept>
#include <system_error>
#include <utility>
#include <vector>

#ifdef _WIN32
#include <Windows.h>
#else
#include <fcntl.h>
#include <sys/stat.h>
#include <unistd.h>
#endif

namespace cao::run {
namespace {
namespace fs = std::filesystem;
/// Compares normalized ownership names conservatively, including Windows case aliases.
bool sameArtifactPath(const fs::path& left, const fs::path& right) {
#ifdef _WIN32
    return CompareStringOrdinal(left.c_str(), -1, right.c_str(), -1, TRUE) == CSTR_EQUAL;
#else
    return left == right;
#endif
}

struct EntryIdentity {
    std::uint64_t volume{};
    std::array<std::byte, 16> file{};
    bool fullFileId{};
    bool operator==(const EntryIdentity&) const = default;
};

struct DestinationSnapshot {
    EntryIdentity identity;
    std::uint64_t size{};
    std::uint64_t lastWriteTime{};
    std::uint64_t changeTime{};
    bool operator==(const DestinationSnapshot&) const = default;
};

#ifdef _WIN32
/// Rejects DOS devices, which Win32 recognizes even with an extension in any directory.
bool reservedDeviceName(const std::wstring& name) {
    auto stem = name.substr(0, name.find(L'.'));
    std::transform(stem.begin(), stem.end(), stem.begin(), [](wchar_t character) {
        return character >= L'a' && character <= L'z' ? character - (L'a' - L'A')
                                                   : character;
    });
    if (stem == L"CON" || stem == L"PRN" || stem == L"AUX" || stem == L"NUL" ||
        stem == L"CONIN$" || stem == L"CONOUT$")
        return true;
    const auto deviceNumber = [](wchar_t number) {
        return (number >= L'1' && number <= L'9') || number == L'\u00b9' ||
               number == L'\u00b2' || number == L'\u00b3';
    };
    return stem.size() == 4 &&
           (stem.starts_with(L"COM") || stem.starts_with(L"LPT")) &&
           deviceNumber(stem.back());
}
#endif

/// Keeps an opened file or directory identity alive across validation and native publication.
class NativeEntry final {
   public:
#ifdef _WIN32
    /// Takes ownership of a Win32 handle; destruction closes it without deleting its entry.
    explicit NativeEntry(HANDLE handle) : _handle(handle) {}
#else
    /// Takes ownership of a POSIX descriptor; destruction closes it without unlinking its entry.
    explicit NativeEntry(int handle) : _handle(handle) {}
#endif
    NativeEntry(const NativeEntry&) = delete;
    NativeEntry& operator=(const NativeEntry&) = delete;
    /// Transfers the open identity and leaves the source without an OS handle.
    NativeEntry(NativeEntry&& other) noexcept : _handle(std::exchange(other._handle, invalid())) {}
    /// Closes the current pin before taking another open identity.
    NativeEntry& operator=(NativeEntry&& other) noexcept {
        if (this != &other) {
            close();
            _handle = std::exchange(other._handle, invalid());
        }
        return *this;
    }
    /// Releases only the OS handle, keeping every filesystem entry in place.
    ~NativeEntry() { close(); }

#ifdef _WIN32
    [[nodiscard]] HANDLE get() const { return _handle; }
#else
    [[nodiscard]] int get() const { return _handle; }
#endif

   private:
#ifdef _WIN32
    static HANDLE invalid() { return INVALID_HANDLE_VALUE; }
    HANDLE _handle;
#else
    static int invalid() { return -1; }
    int _handle;
#endif
    /// Closes only this object's OS handle and makes repeated calls harmless.
    void close() {
#ifdef _WIN32
        if (_handle != INVALID_HANDLE_VALUE) CloseHandle(_handle);
#else
        if (_handle >= 0) ::close(_handle);
#endif
        _handle = invalid();
    }
};

/// Opens an ordinary directory; Win32 denies rename, while POSIX retains its identity for checks.
NativeEntry pinDirectory(const fs::path& path) {
#ifdef _WIN32
    const auto handle = CreateFileW(path.c_str(), FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
                                    FILE_SHARE_READ | FILE_SHARE_WRITE, nullptr, OPEN_EXISTING,
                                    FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                                    nullptr);
    if (handle == INVALID_HANDLE_VALUE)
        throw std::system_error(static_cast<int>(GetLastError()), std::system_category());
    return NativeEntry(handle);
#else
    const auto handle = ::open(path.c_str(), O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
    if (handle < 0) throw std::system_error(errno, std::generic_category());
    return NativeEntry(handle);
#endif
}

/// Reads the physical volume and stable entry identity, rejecting links and unexpected types.
EntryIdentity ordinaryIdentity(const NativeEntry& entry, bool directory) {
#ifdef _WIN32
    BY_HANDLE_FILE_INFORMATION info{};
    if (!GetFileInformationByHandle(entry.get(), &info))
        throw std::system_error(static_cast<int>(GetLastError()), std::system_category());
    if ((info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT) != 0 ||
        ((info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) != 0) != directory ||
        (!directory && info.nNumberOfLinks != 1))
        throw std::invalid_argument("Publication requires an ordinary staged file and parent");
    EntryIdentity identity;
    FILE_ID_INFO fileId{};
    if (GetFileInformationByHandleEx(entry.get(), FileIdInfo, &fileId, sizeof(fileId))) {
        identity.volume = fileId.VolumeSerialNumber;
        std::memcpy(identity.file.data(), fileId.FileId.Identifier, identity.file.size());
        identity.fullFileId = true;
    } else {
        // Wine and older file systems may lack FileIdInfo; keep the existing 64-bit identity
        // check as a compatibility fallback. ReFS uses the full 128-bit identity above.
        identity.volume = info.dwVolumeSerialNumber;
        const auto index = (static_cast<std::uint64_t>(info.nFileIndexHigh) << 32) |
                           info.nFileIndexLow;
        std::memcpy(identity.file.data(), &index, sizeof(index));
    }
    return identity;
#else
    struct stat info{};
    if (::fstat(entry.get(), &info) != 0)
        throw std::system_error(errno, std::generic_category());
    if ((directory ? !S_ISDIR(info.st_mode) : !S_ISREG(info.st_mode) || info.st_nlink != 1))
        throw std::invalid_argument("Publication requires an ordinary staged file and parent");
    EntryIdentity identity;
    identity.volume = static_cast<std::uint64_t>(info.st_dev);
    const auto inode = static_cast<std::uint64_t>(info.st_ino);
    std::memcpy(identity.file.data(), &inode, sizeof(inode));
    return identity;
#endif
}

struct PinnedDestination {
    std::vector<NativeEntry> directories;
    EntryIdentity parentIdentity;
    [[nodiscard]] const NativeEntry& parent() const { return directories.back(); }
};

/// Rechecks confinement and pins every ordinary directory from the recorded root to the leaf.
PinnedDestination pinDestination(const fs::path& root, const fs::path& destination,
                                  const std::optional<EntryIdentity>& stagedParent = {}) {
    if (!root.is_absolute() || !destination.is_absolute() ||
        destination != destination.lexically_normal() || destination.filename().empty() ||
        destination.filename() == "." || destination.filename() == "..")
        throw std::invalid_argument("Publication requires an absolute ordinary destination");
    const auto relative = destination.lexically_relative(root);
    if (relative.empty() || relative == "." || relative.is_absolute() ||
        *relative.begin() == ".." || hasStagingComponent(relative))
        throw std::invalid_argument("Publication destination is outside its Mod Root or reserved");
#ifdef _WIN32
    // Win32 aliases trailing dots/spaces and colons to another entry or stream.
    for (const auto& component : relative) {
        const auto& name = component.native();
        if (name.empty() || name.back() == L'.' || name.back() == L' ' ||
            name.find_first_of(L"<>:\"|?*") != std::wstring::npos ||
            reservedDeviceName(name) ||
            std::any_of(name.begin(), name.end(), [](wchar_t character) {
                return character < 32;
            }))
            throw std::invalid_argument("Publication destination has an ambiguous Windows name");
    }
#endif
    if (fs::canonical(root) != root || fs::canonical(destination.parent_path()) !=
                                           destination.parent_path())
        throw std::invalid_argument("Publication destination parent changed or is linked");

    PinnedDestination pinned;
    pinned.directories.push_back(pinDirectory(root));
    (void)ordinaryIdentity(pinned.directories.back(), true);
    auto current = root;
    for (const auto& component : relative.parent_path()) {
        current /= component;
        pinned.directories.push_back(pinDirectory(current));
        (void)ordinaryIdentity(pinned.directories.back(), true);
    }
    pinned.parentIdentity = ordinaryIdentity(pinned.parent(), true);
    if (stagedParent && pinned.parentIdentity != *stagedParent)
        throw std::invalid_argument("Publication destination parent changed after staging");
    return pinned;
}

/// Reads a destination leaf's identity and content-relevant metadata without following links.
/// An existing directory remains an invalid publication target but is left for native arbitration.
std::optional<DestinationSnapshot> destinationIdentity(const fs::path& path) {
#ifdef _WIN32
    const auto handle = CreateFileW(path.c_str(), FILE_READ_ATTRIBUTES,
                                    FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, nullptr,
                                    OPEN_EXISTING,
                                    FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
                                    nullptr);
    if (handle == INVALID_HANDLE_VALUE) {
        const auto error = GetLastError();
        if (error == ERROR_FILE_NOT_FOUND) return std::nullopt;
        throw std::system_error(static_cast<int>(error), std::system_category());
    }
    NativeEntry leaf(handle);
    BY_HANDLE_FILE_INFORMATION info{};
    if (!GetFileInformationByHandle(leaf.get(), &info))
        throw std::system_error(static_cast<int>(GetLastError()), std::system_category());
    if ((info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT) != 0)
        throw std::invalid_argument("Publication destination is linked");
    if ((info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) != 0) return std::nullopt;
    FILE_BASIC_INFO basic{};
    if (!GetFileInformationByHandleEx(leaf.get(), FileBasicInfo, &basic, sizeof(basic)))
        throw std::system_error(static_cast<int>(GetLastError()), std::system_category());
    // File IDs survive in-place writes; size, last-write, and change time reveal ordinary edits.
    return DestinationSnapshot{ordinaryIdentity(leaf, false),
                               (static_cast<std::uint64_t>(info.nFileSizeHigh) << 32) |
                                   info.nFileSizeLow,
                               static_cast<std::uint64_t>(basic.LastWriteTime.QuadPart),
                               static_cast<std::uint64_t>(basic.ChangeTime.QuadPart)};
#else
    struct stat info{};
    if (::lstat(path.c_str(), &info) != 0) {
        if (errno == ENOENT) return std::nullopt;
        throw std::system_error(errno, std::generic_category());
    }
    if (S_ISDIR(info.st_mode)) return std::nullopt;
    const auto handle = ::open(path.c_str(), O_RDONLY | O_NOFOLLOW | O_CLOEXEC);
    if (handle < 0) throw std::system_error(errno, std::generic_category());
    return DestinationSnapshot{ordinaryIdentity(NativeEntry(handle), false)};
#endif
}

/// Opens the staged identity exclusively and flushes its bytes before any destination mutation.
NativeEntry flushStagedFile(const fs::path& temporary, const EntryIdentity& destinationParent) {
#ifdef _WIN32
    const auto handle = CreateFileW(
        temporary.c_str(), GENERIC_WRITE | DELETE | FILE_READ_ATTRIBUTES, 0, nullptr,
        OPEN_EXISTING, FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_WRITE_THROUGH, nullptr);
    if (handle == INVALID_HANDLE_VALUE)
        throw std::system_error(static_cast<int>(GetLastError()), std::system_category());
    NativeEntry staged(handle);
    const auto identity = ordinaryIdentity(staged, false);
    if (identity.volume != destinationParent.volume)
        throw std::invalid_argument("Staged bytes and destination are on different volumes");
    if (!FlushFileBuffers(staged.get()))
        throw std::system_error(static_cast<int>(GetLastError()), std::system_category());
#else
    const auto handle = ::open(temporary.c_str(), O_RDWR | O_NOFOLLOW | O_CLOEXEC);
    if (handle < 0) throw std::system_error(errno, std::generic_category());
    NativeEntry staged(handle);
    const auto identity = ordinaryIdentity(staged, false);
    if (identity.volume != destinationParent.volume)
        throw std::invalid_argument("Staged bytes and destination are on different volumes");
    if (::fsync(staged.get()) != 0) throw std::system_error(errno, std::generic_category());
#endif
    return staged;
}

/// Uses the pinned parent and native occupied-leaf rule, without any cross-volume copy fallback.
void publishNative(const NativeEntry& staged, const fs::path& temporary,
                   const PinnedDestination& destinationParent, const fs::path& destination,
                   PublicationPolicy policy, PublicationState& state) {
#ifdef _WIN32
    (void)temporary;
    (void)destinationParent;
    // The supported Win32 runtime rejects a RootDirectory-relative FileRenameInfo name.
    // Every ancestor stays pinned without delete sharing, so this absolute name still reaches
    // the validated parent while the native ReplaceIfExists rule arbitrates the destination.
    const auto name = destination.native();
    const auto nameBytes = name.size() * sizeof(wchar_t);
    if (nameBytes > (std::numeric_limits<DWORD>::max)() - sizeof(FILE_RENAME_INFO))
        throw std::invalid_argument("Publication destination name is too long");
    std::vector<std::byte> buffer(sizeof(FILE_RENAME_INFO) + nameBytes);
    auto* rename = reinterpret_cast<FILE_RENAME_INFO*>(buffer.data());
    rename->ReplaceIfExists = policy == PublicationPolicy::Replace;
    rename->RootDirectory = nullptr;
    rename->FileNameLength = static_cast<DWORD>(nameBytes);
    std::memcpy(rename->FileName, name.data(), nameBytes);
    if (!SetFileInformationByHandle(staged.get(), FileRenameInfo, rename,
                                    static_cast<DWORD>(buffer.size())))
        throw std::system_error(static_cast<int>(GetLastError()), std::system_category(),
                                "Publish staged file");
    state = PublicationState::PublishedStillOwned;
#else
    const auto leaf = destination.filename();
    if (policy == PublicationPolicy::Replace) {
        if (::renameat(AT_FDCWD, temporary.c_str(), destinationParent.parent().get(),
                       leaf.c_str()) != 0)
            throw std::system_error(errno, std::generic_category());
        state = PublicationState::PublishedStillOwned;
    } else {
        if (::linkat(AT_FDCWD, temporary.c_str(), destinationParent.parent().get(), leaf.c_str(),
                     0) != 0)
            throw std::system_error(errno, std::generic_category());
        // Linking publishes the destination even if removal of the old temporary name fails.
        state = PublicationState::PublishedStillOwned;
        if (::unlink(temporary.c_str()) != 0)
            throw std::system_error(errno, std::generic_category());
    }
#endif
}
}  // namespace

struct TemporaryArtifactRegistry::PublicationTarget::State {
    fs::path root;
    fs::path destination;
    EntryIdentity parent;
    std::optional<DestinationSnapshot> leaf;
};

TemporaryArtifactRegistry::PublicationTarget::PublicationTarget(std::unique_ptr<State> state)
    : _state(std::move(state)) {}
TemporaryArtifactRegistry::PublicationTarget::PublicationTarget(PublicationTarget&&) noexcept =
    default;
TemporaryArtifactRegistry::PublicationTarget&
TemporaryArtifactRegistry::PublicationTarget::operator=(PublicationTarget&&) noexcept = default;
TemporaryArtifactRegistry::PublicationTarget::~PublicationTarget() = default;

struct TemporaryArtifactRegistry::PublicationReceipt::State {
    TemporaryArtifactRegistry* owner;
    std::weak_ptr<int> lifetime;
    std::size_t artifactIndex;
    fs::path root;
    fs::path temporary;
    std::optional<fs::path> intendedDestination;
    std::optional<EntryIdentity> stagedParent;
    std::optional<DestinationSnapshot> stagedLeaf;
};

TemporaryArtifactRegistry::PublicationReceipt::PublicationReceipt(std::unique_ptr<State> state)
    : _state(std::move(state)) {}
TemporaryArtifactRegistry::PublicationReceipt::PublicationReceipt(PublicationReceipt&&) noexcept =
    default;
TemporaryArtifactRegistry::PublicationReceipt&
TemporaryArtifactRegistry::PublicationReceipt::operator=(PublicationReceipt&&) noexcept = default;
TemporaryArtifactRegistry::PublicationReceipt::~PublicationReceipt() = default;

const fs::path& TemporaryArtifactRegistry::PublicationReceipt::path() const {
    if (!_state || _state->lifetime.expired())
        throw std::logic_error("The publication receipt is no longer owned");
    return _state->temporary;
}

PublicationResult TemporaryArtifactRegistry::PublicationReceipt::publish(
    const fs::path& destination, PublicationPolicy policy) {
    // Clear authority before validation: a failed preflight must not allow a stale retry.
    auto state = std::move(_state);
    if (!state) return {PublicationState::NotPublished, "The publication receipt was consumed"};
    if (state->lifetime.expired())
        return {PublicationState::NotPublished, "The Temporary Ownership scope ended"};
    return state->owner->publishReceipt(*state, destination, policy);
}

TemporaryArtifactRegistry::TemporaryArtifactRegistry(StagingRecovery* recovery)
    : _ownedRecovery(recovery ? nullptr : std::make_unique<StagingRecovery>()),
      _recovery(recovery ? recovery : _ownedRecovery.get()) {}
TemporaryArtifactRegistry::~TemporaryArtifactRegistry() = default;

std::optional<RunFailure> TemporaryArtifactRegistry::prepareRoot(const std::filesystem::path& root,
                                                                 std::stop_token stop) {
    if (_cleaned) throw std::logic_error("Temporary artifact registration is closed");
    return _recovery->recover(root, stop);
}

TemporaryArtifactRegistry::StagedFile TemporaryArtifactRegistry::stageFile(
    const std::filesystem::path& root, const std::filesystem::path& destination) {
    if (_cleaned) throw std::logic_error("Temporary artifact registration is closed");
    const auto path = _recovery->stageFile(root, destination);
    _artifacts.push_back({path, false, true});
    return {path, Registration(this, _artifacts.size() - 1)};
}

TemporaryArtifactRegistry::StagedFile TemporaryArtifactRegistry::stageArchiveFile(
    const std::filesystem::path& root) {
    if (_cleaned) throw std::logic_error("Temporary artifact registration is closed");
    const auto path = _recovery->stageArchiveFile(root);
    _artifacts.push_back({path, false, true});
    return {path, Registration(this, _artifacts.size() - 1)};
}

TemporaryArtifactRegistry::PublicationTarget TemporaryArtifactRegistry::capturePublicationTarget(
    const fs::path& modRoot, const fs::path& destination) const {
    if (_cleaned) throw std::logic_error("Temporary artifact registration is closed");
    if (!modRoot.is_absolute()) throw std::invalid_argument("A Mod Root must be absolute");
    const auto root = fs::canonical(modRoot);
    const auto parent = pinDestination(root, destination);
    auto target = std::make_unique<PublicationTarget::State>();
    target->root = root;
    target->destination = destination;
    target->parent = parent.parentIdentity;
    target->leaf = destinationIdentity(destination);
    return PublicationTarget(std::move(target));
}

TemporaryArtifactRegistry::PublicationReceipt TemporaryArtifactRegistry::stageFileForPublication(
    PublicationTarget&& target) {
    if (_cleaned) throw std::logic_error("Temporary artifact registration is closed");
    auto expected = std::move(target._state);
    if (!expected) throw std::invalid_argument("The publication target was consumed");
    (void)pinDestination(expected->root, expected->destination, expected->parent);
    if (destinationIdentity(expected->destination) != expected->leaf)
        throw std::invalid_argument("Publication destination changed after input capture");
    const auto path = _recovery->stageFile(expected->root, expected->destination);
    _artifacts.push_back({path, false, true});
    auto state = std::make_unique<PublicationReceipt::State>();
    state->owner = this;
    state->lifetime = _lifetime;
    state->artifactIndex = _artifacts.size() - 1;
    state->root = expected->root;
    state->temporary = path;
    state->intendedDestination = expected->destination;
    state->stagedParent = expected->parent;
    state->stagedLeaf = expected->leaf;
    return PublicationReceipt(std::move(state));
}

TemporaryArtifactRegistry::PublicationReceipt TemporaryArtifactRegistry::stageFileForPublication(
    const fs::path& modRoot, const fs::path& destination) {
    return stageFileForPublication(capturePublicationTarget(modRoot, destination));
}

TemporaryArtifactRegistry::PublicationReceipt
TemporaryArtifactRegistry::stageArchiveFileForPublication(const fs::path& modRoot) {
    if (_cleaned) throw std::logic_error("Temporary artifact registration is closed");
    if (!modRoot.is_absolute()) throw std::invalid_argument("A Mod Root must be absolute");
    const auto root = fs::canonical(modRoot);
    const auto path = _recovery->stageArchiveFile(root);
    _artifacts.push_back({path, false, true});
    auto state = std::make_unique<PublicationReceipt::State>();
    state->owner = this;
    state->lifetime = _lifetime;
    state->artifactIndex = _artifacts.size() - 1;
    state->root = root;
    state->temporary = path;
    return PublicationReceipt(std::move(state));
}

PublicationResult TemporaryArtifactRegistry::publishReceipt(PublicationReceipt::State& receipt,
                                                            const fs::path& destination,
                                                            PublicationPolicy policy) {
    auto published = PublicationState::NotPublished;
    try {
        if (_cleaned || receipt.artifactIndex >= _artifacts.size() ||
            _artifacts[receipt.artifactIndex].committed ||
            !sameArtifactPath(_artifacts[receipt.artifactIndex].path, receipt.temporary))
            throw std::logic_error("The publication receipt is no longer owned");
        if (policy != PublicationPolicy::Replace && policy != PublicationPolicy::NoReplace)
            throw std::invalid_argument("The publication policy is invalid");
        if (receipt.intendedDestination &&
            !sameArtifactPath(destination, *receipt.intendedDestination))
            throw std::invalid_argument("Asset publication changed its staged destination");
        const auto parent = pinDestination(receipt.root, destination, receipt.stagedParent);
        {
            const auto staged = flushStagedFile(receipt.temporary, parent.parentIdentity);
            // The saved bytes came from the leaf captured before loading, not a replacement
            // or an in-place edit of that leaf while the backend was running.
            if (receipt.intendedDestination && destinationIdentity(destination) != receipt.stagedLeaf)
                throw std::invalid_argument("Publication destination changed after input capture");
            publishNative(staged, receipt.temporary, parent, destination, policy, published);
        }
        // The native operation has committed a destination. A failed snapshot release must
        // preserve this fact and leave any old temporary name under durable ownership.
        _recovery->releaseFile(receipt.temporary);
        _artifacts[receipt.artifactIndex].committed = true;
        return {PublicationState::PublishedAndReleased, {}};
    } catch (const std::exception& error) {
        return {published, error.what()};
    }
}

TemporaryArtifactRegistry::Registration TemporaryArtifactRegistry::registerArtifact(
    const std::filesystem::path& path) {
    if (_cleaned) throw std::logic_error("Temporary artifact registration is closed");
    if (!path.is_absolute() || path.filename().empty() || path.filename() == "." ||
        path.filename() == "..")
        throw std::invalid_argument("A temporary artifact needs an absolute entry path");
    // Resolve parent aliases once so cleanup does not depend on a later working directory.
    const auto normalized = std::filesystem::weakly_canonical(path.parent_path()) / path.filename();
#ifdef _WIN32
    // Win32 strips trailing dots/spaces and treats colons as alternate streams. Distinct receipts
    // for those aliases could otherwise reacquire cleanup ownership of a committed output.
    for (const auto& component : normalized.relative_path()) {
        const auto& name = component.native();
        if (!name.empty() &&
            (name.back() == L'.' || name.back() == L' ' || name.find(L':') != std::wstring::npos))
            throw std::invalid_argument("A temporary artifact needs an unambiguous Windows path");
    }
#endif
    std::error_code error;
    const auto status = std::filesystem::symlink_status(normalized, error);
    if (error && error != std::errc::no_such_file_or_directory)
        throw std::filesystem::filesystem_error("Inspect temporary artifact", normalized, error);
    if (std::filesystem::exists(status))
        throw std::invalid_argument("An existing entry cannot become a temporary artifact");
    if (std::any_of(_artifacts.begin(), _artifacts.end(), [&](const Artifact& artifact) {
            return sameArtifactPath(artifact.path, normalized);
        }))
        throw std::invalid_argument("The temporary artifact is already registered");
    _artifacts.push_back({normalized});
    return Registration(this, _artifacts.size() - 1);
}

void TemporaryArtifactRegistry::commit(Registration registration) {
    if (_cleaned || registration._owner != this || _artifacts.at(registration._index).committed)
        throw std::logic_error("The temporary artifact registration is no longer owned");
    auto& artifact = _artifacts[registration._index];
    // Only publication may release a durable claim after committing a destination.
    if (artifact.durable)
        throw std::logic_error("Durable staged files require a publication receipt");
    artifact.committed = true;
}

std::vector<RunFailure> TemporaryArtifactRegistry::performSafetyCleanup() {
    if (_cleaned) return {};
    // Close ownership before filesystem work so no second pass can retry a failed deletion.
    _cleaned = true;
    std::vector<RunFailure> failures;
    for (auto artifact = _artifacts.rbegin(); artifact != _artifacts.rend(); ++artifact) {
        // The durable owner also knows paths whose creation failed before a receipt was returned.
        if (artifact->committed || artifact->durable) continue;
        // Never recurse: unregistered contents may be committed output or retained evidence.
        std::error_code error;
        const auto parent = std::filesystem::weakly_canonical(artifact->path.parent_path(), error);
        // A link substituted after registration must not redirect a child deletion outside its
        // original parent. Staging locks remain the owning operation's responsibility.
        if (error || !sameArtifactPath(parent, artifact->path.parent_path())) {
            failures.emplace_back(RunFailureCode::TemporaryArtifactCleanupFailed,
                                  RunPhase::SafetyCleanup,
                                  error ? error.message() : "The temporary artifact parent changed",
                                  routing::PolicyValidationErrors{}, artifact->path);
            continue;
        }
        std::filesystem::remove(artifact->path, error);
        if (error)
            failures.emplace_back(RunFailureCode::TemporaryArtifactCleanupFailed,
                                  RunPhase::SafetyCleanup, error.message(),
                                  routing::PolicyValidationErrors{}, artifact->path);
    }
    auto durableFailures = _recovery->cleanupArtifacts();
    failures.insert(failures.end(), std::make_move_iterator(durableFailures.begin()),
                    std::make_move_iterator(durableFailures.end()));
    return failures;
}
}  // namespace cao::run
