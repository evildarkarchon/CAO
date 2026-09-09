#pragma once

#include <cstdint>
#include <filesystem>
#include <functional>
#include <limits>
#include <optional>
#include <string>

namespace cao::run {
/// Samples caller-available bytes at a Mod Root; nullopt means capacity is unknown, not unlimited.
using CapacityProbe = std::function<std::optional<std::uintmax_t>(const std::filesystem::path&)>;

/// Reads current available space without reserving it. Filesystem errors leave capacity unknown.
[[nodiscard]] inline std::optional<std::uintmax_t> availableArchiveCapacity(
    const std::filesystem::path& root) noexcept {
    std::error_code error;
    const auto space = std::filesystem::space(root, error);
    if (error || space.available == std::numeric_limits<std::uintmax_t>::max()) return std::nullopt;
    return space.available;
}

/// Saturates estimates instead of wrapping a large Archive into an apparently small allocation.
[[nodiscard]] inline std::uintmax_t saturatedCapacityAdd(std::uintmax_t a, std::uintmax_t b) {
    const auto maximum = std::numeric_limits<std::uintmax_t>::max();
    return b > maximum - a ? maximum : a + b;
}

/// Saturates multiplicative overhead allowances before they can understate required capacity.
[[nodiscard]] inline std::uintmax_t saturatedCapacityMultiply(std::uintmax_t a, std::uintmax_t b) {
    const auto maximum = std::numeric_limits<std::uintmax_t>::max();
    return b != 0 && a > maximum / b ? maximum : a * b;
}

/// Explains a rejected estimate without promising a reservation or exact filesystem usage.
[[nodiscard]] inline std::string archiveCapacityDetail(std::uintmax_t required,
                                                      std::uintmax_t available) {
    return "Insufficient Archive staging capacity: estimated " + std::to_string(required) +
           " bytes, available " + std::to_string(available) +
           " bytes. Estimates include staging overhead allowances but do not reserve space or "
           "guarantee filesystem metadata, quotas, or concurrent writes.";
}
}  // namespace cao::run
