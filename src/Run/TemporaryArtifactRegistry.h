#pragma once

#include "Run/RunExecutor.h"

#include <filesystem>
#include <memory>
#include <optional>
#include <stop_token>
#include <string>
#include <vector>

namespace cao::run {
class StagingRecovery;
/// Selects the native destination rule; no-replace must arbitrate an occupied leaf atomically.
enum class PublicationPolicy { Replace, NoReplace };
/// Records whether the destination became a committed mutation before any later release failure.
enum class PublicationState { NotPublished, PublishedStillOwned, PublishedAndReleased };
/// Gives producers the committed fact and an actionable detail for unsuccessful completion.
struct PublicationResult {
    PublicationState state{PublicationState::NotPublished};
    std::string errorDetail;
};
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

    /// One-use authority to publish a durable staged file within its owning registry's lifetime.
    /// Moving transfers authority; destruction leaves any unpublished temporary path owned.
    class PublicationReceipt final {
       public:
        PublicationReceipt(PublicationReceipt&&) noexcept;
        PublicationReceipt& operator=(PublicationReceipt&&) noexcept;
        PublicationReceipt(const PublicationReceipt&) = delete;
        PublicationReceipt& operator=(const PublicationReceipt&) = delete;
        ~PublicationReceipt();

        /// Returns the writable staged path while its owner scope is live and this receipt is
        /// unconsumed. Throws `logic_error` after either condition ends.
        [[nodiscard]] const std::filesystem::path& path() const;
        /// Consumes authority on every attempt, returning the committed fact even if release fails.
        /// An ended owner scope returns NotPublished; callers must serialize with cleanup.
        [[nodiscard]] PublicationResult publish(const std::filesystem::path& destination,
                                                PublicationPolicy policy);

       private:
        friend class TemporaryArtifactRegistry;
        struct State;
        explicit PublicationReceipt(std::unique_ptr<State> state);
        std::unique_ptr<State> _state;
    };

    /// Recovers existing staging and retains its lock. An absent area is not created. Cancellation
    /// stops between recovery operations without returning a failure. Throws `logic_error` after
    /// Safety Cleanup has closed registration.
    [[nodiscard]] std::optional<RunFailure> prepareRoot(const std::filesystem::path& modRoot,
                                                        std::stop_token stop = {});
    /// Flushes ownership before exclusively creating an empty staging file beside its destination.
    /// Throws on invalid ownership, unavailable locks, or filesystem failures.
    [[nodiscard]] StagedFile stageFile(const std::filesystem::path& modRoot,
                                       const std::filesystem::path& destination);
    /// Flushes ownership before creating a unique empty file in the durable Archive run child.
    /// Throws on invalid ownership, unavailable locks, filesystem failures, or closed registration.
    [[nodiscard]] StagedFile stageArchiveFile(const std::filesystem::path& modRoot);

    /// Creates a durable Asset sibling and binds a move-only receipt to its canonical Mod Root,
    /// intended destination, and ordinary parent identity. Throws on invalid staging or I/O.
    [[nodiscard]] PublicationReceipt stageFileForPublication(
        const std::filesystem::path& modRoot, const std::filesystem::path& destination);
    /// Creates a durable Archive file whose case-resolved destination may be supplied later.
    /// Throws on invalid staging or I/O; the caller retains Archive-specific parent pins.
    [[nodiscard]] PublicationReceipt stageArchiveFileForPublication(
        const std::filesystem::path& modRoot);

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
    /// Validates, flushes, publishes natively, then releases only the temporary ownership name.
    [[nodiscard]] PublicationResult publishReceipt(PublicationReceipt::State& receipt,
                                                   const std::filesystem::path& destination,
                                                   PublicationPolicy policy);
    struct Artifact {
        std::filesystem::path path;
        bool committed{};
        bool durable{};
    };
    std::vector<Artifact> _artifacts;
    bool _cleaned{};
    std::unique_ptr<StagingRecovery> _ownedRecovery;
    StagingRecovery* _recovery;
    // A late receipt can detect scope teardown before touching its non-owning registry pointer.
    std::shared_ptr<int> _lifetime{std::make_shared<int>(0)};
};
}  // namespace cao::run
