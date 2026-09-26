#pragma once

#include <cstdint>
#include <filesystem>
#include <functional>
#include <limits>
#include <map>
#include <optional>
#include <string>
#include <utility>

namespace cao::run {
/// Samples caller-available bytes at a Mod Root; nullopt means capacity is unknown, not unlimited.
using CapacityProbe = std::function<std::optional<std::uintmax_t>(const std::filesystem::path&)>;

/// Identifies the filesystem containing a Mod Root; an unknown identity keeps batch checks
/// conservative because that root may share any other root's volume.
using VolumeIdentityProbe =
    std::function<std::optional<std::string>(const std::filesystem::path&)>;

/// Resolves the containing volume after root canonicalization, including mounted filesystems.
/// A failed native query leaves identity unknown so capacity checks retain the whole batch.
[[nodiscard]] std::optional<std::string> archiveVolumeIdentity(
    const std::filesystem::path& root);

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

/// Accumulates staging estimates by filesystem so each root is checked against only the work
/// sharing its volume. Unknown identity for positive work keeps the whole batch as an upper bound.
class ArchiveVolumeCapacityRequirements final {
   public:
    /// Uses the supplied identity probe for every distinct Mod Root added to this batch.
    explicit ArchiveVolumeCapacityRequirements(VolumeIdentityProbe volumeIdentity)
        : _volumeIdentity(std::move(volumeIdentity)) {}

    /// Adds an estimate to the Mod Root's volume, saturating both volume and batch totals.
    void add(const std::filesystem::path& root, std::uintmax_t bytes) {
        const auto [entry, inserted] = _volumes.try_emplace(root);
        if (inserted)
            entry->second = _volumeIdentity ? _volumeIdentity(root) : std::nullopt;
        // A root that needs no bytes cannot consume another volume's capacity, even if its
        // identity is unavailable. A later positive addition still makes it conservative.
        if (!entry->second && bytes != 0) _unknownVolume = true;
        _total = saturatedCapacityAdd(_total, bytes);
        if (entry->second) {
            auto& required = _requiredByVolume[*entry->second];
            required = saturatedCapacityAdd(required, bytes);
        }
    }

    /// Returns the estimate sharing this previously added root's volume; positive work with
    /// unknown identity keeps the whole batch because it could share any known volume.
    [[nodiscard]] std::uintmax_t requiredAt(const std::filesystem::path& root) const {
        const auto& volume = _volumes.at(root);
        if (_unknownVolume || !volume) return _total;
        return _requiredByVolume.at(*volume);
    }

   private:
    VolumeIdentityProbe _volumeIdentity;
    std::map<std::filesystem::path, std::optional<std::string>> _volumes;
    std::map<std::string, std::uintmax_t> _requiredByVolume;
    std::uintmax_t _total{};
    bool _unknownVolume{};
};

/// Explains a rejected estimate without promising a reservation or exact filesystem usage.
[[nodiscard]] inline std::string archiveCapacityDetail(std::uintmax_t required,
                                                       std::uintmax_t available) {
    return "Insufficient Archive staging capacity: estimated " + std::to_string(required) +
           " bytes, available " + std::to_string(available) +
           " bytes. Estimates include staging overhead allowances but do not reserve space or "
           "guarantee filesystem metadata, quotas, or concurrent writes.";
}
}  // namespace cao::run
