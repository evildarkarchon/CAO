#pragma once

#include "PathOrdering.h"

#include <limits>
#include <stdexcept>
#include <string_view>
#include <system_error>

#ifdef _WIN32
#include <Windows.h>
#endif

namespace cao::run {
/// Compares normalized UTF-8 game paths using Windows ordinal case-insensitive filename rules.
/// The POSIX fallback uses simple uppercase mappings so it also avoids expanding one character.
inline int compareArchiveGamePaths(std::string_view left, std::string_view right) {
#ifdef _WIN32
    const auto leftNative = pathFromUtf8(left).native();
    const auto rightNative = pathFromUtf8(right).native();
    if (leftNative.size() > static_cast<std::size_t>((std::numeric_limits<int>::max)()) ||
        rightNative.size() > static_cast<std::size_t>((std::numeric_limits<int>::max)()))
        throw std::length_error("Archive game path exceeds Windows comparison limits.");
    const auto result = CompareStringOrdinal(
        leftNative.data(), static_cast<int>(leftNative.size()), rightNative.data(),
        static_cast<int>(rightNative.size()), TRUE);
    if (result == 0)
        throw std::system_error(static_cast<int>(GetLastError()), std::system_category(),
                                "Compare Archive game paths");
    return result - CSTR_EQUAL;
#else
    const auto nextUpper = [](std::string_view name, std::size_t& offset) {
        utf8proc_int32_t codepoint{};
        const auto length = utf8proc_iterate(
            reinterpret_cast<const utf8proc_uint8_t*>(name.data() + offset),
            static_cast<utf8proc_ssize_t>(name.size() - offset), &codepoint);
        if (length < 0) throw std::runtime_error(utf8proc_errmsg(length));
        offset += static_cast<std::size_t>(length);
        return utf8proc_toupper(codepoint);
    };
    std::size_t leftOffset = 0;
    std::size_t rightOffset = 0;
    while (leftOffset < left.size() && rightOffset < right.size()) {
        const auto leftUpper = nextUpper(left, leftOffset);
        const auto rightUpper = nextUpper(right, rightOffset);
        if (leftUpper != rightUpper) return leftUpper < rightUpper ? -1 : 1;
    }
    if (leftOffset == left.size() && rightOffset == right.size()) return 0;
    return leftOffset == left.size() ? -1 : 1;
#endif
}

/// Orders game paths while treating only Windows-equivalent spellings as the same key.
struct ArchiveGamePathLess {
    /// Returns false in both directions when names alias on a case-insensitive Windows volume.
    bool operator()(std::string_view left, std::string_view right) const {
        return compareArchiveGamePaths(left, right) < 0;
    }
};

/// Tests Windows filename equivalence without Unicode full-fold expansions such as sharp s to ss.
inline bool sameArchiveGamePath(std::string_view left, std::string_view right) {
    return compareArchiveGamePaths(left, right) == 0;
}
}  // namespace cao::run
