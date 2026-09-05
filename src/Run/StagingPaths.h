#pragma once

#include <algorithm>
#include <filesystem>

namespace cao::run {
/// Reserves staging-like entry names case-insensitively, including unknown versions and aliases.
/// Discovery excludes the whole namespace; only exact, verified ownership permits recovery.
inline bool isStagingName(const std::filesystem::path& path) {
    auto name = path.filename().generic_u8string();
    std::transform(name.begin(), name.end(), name.begin(), [](char8_t c) {
        return static_cast<char8_t>(c >= u8'A' && c <= u8'Z' ? c + (u8'a' - u8'A') : c);
    });
    return name.starts_with(u8".cao-staging");
}
}  // namespace cao::run
