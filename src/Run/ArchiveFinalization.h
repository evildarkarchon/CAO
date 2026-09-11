#pragma once

#include "Run/ArchiveExtraction.h"
#include "Run/ArchiveFinalizationResult.h"
#include "Run/ArchiveCapacity.h"

#include <btu/bsa/archive_data.hpp>

#include <span>

class BSAOptimizer;

namespace cao::run {
/// One frozen output and the complete source set consumed by its atomic attempt.
struct ArchiveFinalizationOutput final {
    std::filesystem::path modRoot;
    std::filesystem::path archivePath;
    std::vector<std::filesystem::path> sources;
    /// A missing loading plugin must commit within this output's attempt before source deletion.
    std::optional<std::filesystem::path> pluginPath;
    /// Conservative content and framing allowance, not a reservation or filesystem quota guarantee.
    std::uintmax_t estimatedCapacityBytes{};
};

/// Owns all output names, partitions, and settings before any finalization mutation.
/// Callers may inspect the plan but cannot change its total or source sets.
class ArchiveFinalizationPlan final {
   public:
    /// Borrows the immutable ordered outputs; its size is the phase's complete progress total.
    [[nodiscard]] std::span<const ArchiveFinalizationOutput> outputs() const noexcept {
        return _outputs;
    }

   private:
    friend class ::BSAOptimizer;
    std::vector<ArchiveFinalizationOutput> _outputs;
    std::vector<btu::bsa::ArchiveData> _archives;
    std::vector<std::filesystem::path> _roots;
    btu::bsa::Settings _settings;
    bool _compress{};
    bool _deleteSources{};
    bool _createDummies{};
    std::uintmax_t _dummyCapacityBytes{};
};

}  // namespace cao::run
