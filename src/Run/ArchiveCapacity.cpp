#include "ArchiveCapacity.h"

#ifdef _WIN32
#include <Windows.h>
#else
#include <sys/stat.h>
#endif

namespace cao::run {
std::optional<std::string> archiveVolumeIdentity(const std::filesystem::path& root) {
#ifdef _WIN32
    wchar_t mount[MAX_PATH]{};
    wchar_t volume[MAX_PATH]{};
    if (!GetVolumePathNameW(root.c_str(), mount, MAX_PATH) ||
        !GetVolumeNameForVolumeMountPointW(mount, volume, MAX_PATH))
        return std::nullopt;
    std::string identity;
    for (const auto* character = volume; *character != L'\0'; ++character)
        // Volume GUID paths contain only ASCII syntax and hexadecimal digits.
        identity.push_back(static_cast<char>(*character));
    return identity;
#else
    struct stat status {};
    if (::stat(root.c_str(), &status) != 0) return std::nullopt;
    return std::to_string(static_cast<std::uintmax_t>(status.st_dev));
#endif
}
}  // namespace cao::run
