#pragma once

#include "Run/RunExecutor.h"

#include <filesystem>
#include <memory>
#include <optional>
#include <vector>

namespace cao::run {
class StagingRecovery;
/// Owns only explicitly registered temporary paths for one run, on its execution thread.
/// Register each directory before its children; directory cleanup is deliberately non-recursive.
/// Backups, completed mutations, and failed-output evidence must never be registered. If an
/// artifact becomes durable output or retained evidence, explicitly commit its registration.
class TemporaryArtifactRegistry final : public SafetyCleanupService {
   public:
    /// Owns a lazy recovery scope, or borrows one that must outlive this registry.
    explicit TemporaryArtifactRegistry(StagingRecovery* recovery = nullptr);
    /// Releases OS ownership handles; callers must explicitly perform Safety Cleanup.
    ~TemporaryArtifactRegistry();
    /// An opaque receipt bound to this registry's lifetime; copying it does not change ownership.
    class Registration final {
       private:
        friend class TemporaryArtifactRegistry;
        Registration(const TemporaryArtifactRegistry* owner, std::size_t index)
            : _owner(owner), _index(index) {}
        const TemporaryArtifactRegistry* _owner;
        std::size_t _index;
    };

    struct StagedFile {
        std::filesystem::path path;
        Registration registration;
    };

    /// Recovers existing staging and retains its lock. An absent area is not created.
    [[nodiscard]] std::optional<RunFailure> prepareRoot(const std::filesystem::path& modRoot);
    /// Flushes ownership before exclusively creating an empty same-volume staging file.
    /// Throws on invalid ownership, unavailable locks, or filesystem failures.
    [[nodiscard]] StagedFile stageFile(const std::filesystem::path& modRoot,
                                       const std::filesystem::path& destination);

    /// Records an absent absolute path before the operation creates it; performs no mutation.
    /// Throws on existing/duplicate paths, ambiguous Windows names, lookup errors, or registration
    /// after cleanup starts. Callers must keep parent identities stable through cleanup.
    [[nodiscard]] Registration registerArtifact(const std::filesystem::path& path);

    /// Releases an artifact after durable commit, without deleting it. Durable staged files must
    /// first be moved out of staging; in-memory registrations may also retain existing evidence.
    /// Throws logic_error for a foreign, already committed, or terminal registration.
    void commit(Registration registration);

    /// Removes remaining paths once in reverse registration order, without following directory
    /// contents, collecting every error. Repeated calls return no new failures; callers must
    /// serialize this with registration/commit.
    std::vector<RunFailure> performSafetyCleanup() override;

   private:
    struct Artifact {
        std::filesystem::path path;
        bool committed{};
        bool durable{};
    };
    std::vector<Artifact> _artifacts;
    bool _cleaned{};
    std::unique_ptr<StagingRecovery> _ownedRecovery;
    StagingRecovery* _recovery;
};
}  // namespace cao::run
