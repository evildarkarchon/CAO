#pragma once

#include <utf8proc.h>

#include <cstdlib>
#include <filesystem>
#include <memory>
#include <stdexcept>
#include <string>
#include <string_view>

namespace cao::run {
/// Folds UTF-8 names independently of the process locale; invalid encoding throws to the caller.
inline std::string foldedName(std::string_view name) {
    utf8proc_uint8_t* mapped = nullptr;
    const auto size =
        utf8proc_map(reinterpret_cast<const utf8proc_uint8_t*>(name.data()),
                     static_cast<utf8proc_ssize_t>(name.size()), &mapped, UTF8PROC_CASEFOLD);
    const std::unique_ptr<utf8proc_uint8_t, decltype(&std::free)> owned(mapped, &std::free);
    if (size < 0) throw std::runtime_error(utf8proc_errmsg(size));
    return std::string(reinterpret_cast<const char*>(owned.get()), static_cast<std::size_t>(size));
}

/// Returns normalized generic UTF-8 without depending on the Windows ANSI code page.
inline std::string relativeName(const std::filesystem::path& path) {
    const auto utf8 = path.lexically_normal().generic_u8string();
    return std::string(utf8.begin(), utf8.end());
}
}  // namespace cao::run
