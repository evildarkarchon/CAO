#pragma once

#include "Run/ArchiveExtraction.h"

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
};

enum class ArchiveFinalizationFailure {
    WriteFailed,
    CommitFailed,
    PluginCreationFailed,
    SourceCleanupFailed,
    UnexpectedException
};

/// Records one complete output attempt, including durable mutation after a cleanup failure.
struct ArchiveFinalizationAttempt final {
    std::filesystem::path archivePath;
    execution::MutationState mutation{execution::MutationState::None};
    std::optional<ArchiveFinalizationFailure> failure;
    bool safeToContinue{true};
    std::string detail;

    /// Reports success only after staging, commit, and requested source cleanup finish.
    [[nodiscard]] bool succeeded() const noexcept { return !failure; }
};

/// Counts complete output attempts; total is immutable, including on cancellation or failure.
struct ArchiveFinalizationProgress final {
    std::size_t completed{};
    std::size_t total{};
    std::size_t succeeded{};
    std::size_t failed{};
};

/// Owns finalization evidence independently of the plan and temporary-artifact lifetime.
struct ArchiveFinalizationResult final {
    std::vector<ArchiveFinalizationAttempt> attempts;
    bool cancelled{};
    bool safeToContinue{true};
    std::string detail;
};
}  // namespace cao::run
