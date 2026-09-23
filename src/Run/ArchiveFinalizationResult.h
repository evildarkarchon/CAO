#pragma once

#include "AssetExecution/AssetExecutor.h"

namespace cao::run {
enum class ArchiveFinalizationFailure {
    InsufficientCapacity,
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

    /// The frozen plan scope, retained independently of output directory depth.
    std::filesystem::path modRoot{};

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

/// Kinds of finalization effects outside planned Archive output attempts.
enum class ArchiveFinalizationMutationKind {
    EmptyDirectoryPruning,
    PluginCreation,
    PluginRemoval
};

/// Records completed effects that Archive output attempts cannot account for.
struct ArchiveFinalizationMutation final {
    std::filesystem::path modRoot;
    std::filesystem::path path;
    ArchiveFinalizationMutationKind kind;
    execution::MutationState mutation{execution::MutationState::None};
    std::size_t count{1};
};

/// Owns finalization evidence independently of the plan and temporary-artifact lifetime.
struct ArchiveFinalizationResult final {
    std::vector<ArchiveFinalizationAttempt> attempts;
    /// A phase-level failure has no output attempt and does not advance completed progress.
    std::optional<ArchiveFinalizationFailure> failure;
    bool cancelled{};
    bool safeToContinue{true};
    std::string detail;
    std::vector<ArchiveFinalizationMutation> mutations;
};
}  // namespace cao::run
