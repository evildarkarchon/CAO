#pragma once

// Volume identity queries shared by Archive capacity grouping and same-volume staging checks.
// CAO is longPathAware, so a mounted folder may sit deeper than MAX_PATH.

#ifdef _WIN32
#include <Windows.h>

#include <cstddef>
#include <cwchar>
#include <filesystem>
#include <string>

namespace cao::run {
/// Calls a `GetVolumePathNameW`-shaped query with buffers that grow until the mount point fits.
/// Returns an empty string on failure with the native error left in `GetLastError()`.
template <class Query>
[[nodiscard]] std::wstring volumeMountPoint(const std::wstring& path, Query&& query) {
    // UNICODE_STRING lengths cap native paths at 32767 characters plus the terminator.
    constexpr std::size_t limit = 32768;
    // The mount point is normally a prefix of the path plus a separator, but a traversed
    // junction can resolve to another volume's mounted folder, so this is only a first guess.
    const auto guess =
        path.size() + 2 > std::size_t{MAX_PATH} ? path.size() + 2 : std::size_t{MAX_PATH};
    for (auto size = guess < limit ? guess : limit;; size = size * 2 < limit ? size * 2 : limit) {
        std::wstring mount(size, L'\0');
        if (!query(path.c_str(), mount.data(), static_cast<DWORD>(size))) {
            if (GetLastError() != ERROR_FILENAME_EXCED_RANGE || size == limit) return {};
            continue;
        }
        mount.resize(std::wcslen(mount.c_str()));
        // One character short, the query succeeds but drops the trailing separator (`C:` for
        // `C:\`), which GetVolumeNameForVolumeMountPointW then rejects.
        if (mount.size() + 1 < size || mount.ends_with(L'\\') || size == limit) return mount;
    }
}

/// Resolves the volume GUID path (`\\?\Volume{GUID}\`) of the volume containing an existing
/// path, including volumes mounted in folders. Returns an empty string on failure with the
/// native error left in `GetLastError()`.
[[nodiscard]] inline std::wstring volumeGuidPath(const std::filesystem::path& path) {
    const auto mount = volumeMountPoint(path.native(), GetVolumePathNameW);
    if (mount.empty()) return {};
    // Volume GUID paths have a fixed 49-character form however long the mounted folder path
    // is, so MAX_PATH comfortably exceeds the documented 50-character requirement.
    wchar_t volume[MAX_PATH]{};
    if (!GetVolumeNameForVolumeMountPointW(mount.c_str(), volume, MAX_PATH)) return {};
    return volume;
}
}  // namespace cao::run
#endif
