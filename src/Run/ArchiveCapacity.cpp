#include "ArchiveCapacity.h"

#ifdef _WIN32
#include "NativeVolume.h"
#else
#include <sys/stat.h>
#endif

namespace cao::run {
std::optional<std::string> archiveVolumeIdentity(const std::filesystem::path& root) {
#ifdef _WIN32
    const auto volume = volumeGuidPath(root);
    if (volume.empty()) return std::nullopt;
    std::string identity;
    for (const auto character : volume)
        // Volume GUID paths contain only ASCII syntax and hexadecimal digits.
        identity.push_back(static_cast<char>(character));
    return identity;
#else
    struct stat status {};
    if (::stat(root.c_str(), &status) != 0) return std::nullopt;
    return std::to_string(static_cast<std::uintmax_t>(status.st_dev));
#endif
}
}  // namespace cao::run
