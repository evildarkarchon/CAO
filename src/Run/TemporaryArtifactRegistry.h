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
/// Backups, completed mutations, and failed-output evidence must never be registered. Commit
/// non-durable retained artifacts; publish durable staged output through a one-use receipt.
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

    /// Read-only snapshot of an Asset destination before its producer loads the original bytes.
    /// Moving transfers the expected parent and leaf identities without creating staging state.
    class PublicationTarget final {
       public:
        PublicationTarget(PublicationTarget&&) noexcept;
        PublicationTarget& operator=(PublicationTarget&&) noexcept;
        PublicationTarget(const PublicationTarget&) = delete;
        PublicationTarget& operator=(const PublicationTarget&) = delete;
        ~PublicationTarget();

       private:
        friend class TemporaryArtifactRegistry;
        struct State;
        explicit PublicationTarget(std::unique_ptr<State> state);
        std::unique_ptr<State> _state;
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
    /// Flushes ownership before creating an empty Asset sibling without publication authority.
    /// For recoverable staging only; use a publication receipt to commit durable output. Throws on
    /// invalid ownership, unavailable locks, or filesystem failures.
    [[nodiscard]] StagedFile stageFile(const std::filesystem::path& modRoot,
                                       const std::filesystem::path& destination);
    /// Flushes ownership before creating an Archive entry without publication authority. Use a
    /// publication receipt to commit durable output. Throws on invalid ownership, unavailable
    /// locks, filesystem failures, or closed registration.
    [[nodiscard]] StagedFile stageArchiveFile(const std::filesystem::path& modRoot);

    /// Captures an Asset destination before loading its original bytes, without disk mutation.
    /// A later stage/publish rejects a changed parent or destination leaf. Throws on invalid
    /// confinement, link aliases, or identity lookup failures.
    [[nodiscard]] PublicationTarget capturePublicationTarget(
        const std::filesystem::path& modRoot, const std::filesystem::path& destination) const;
    /// Creates a durable Asset sibling from a previously captured destination identity. Throws
    /// when the destination changed after capture or staging ownership cannot be established.
    [[nodiscard]] PublicationReceipt stageFileForPublication(PublicationTarget&& target);
    /// Captures and stages an Asset sibling in one step for producers that have already retained
    /// their input identity; the resulting receipt still detects later leaf replacement.
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

    /// Retains a non-durable registered artifact without deleting it. Durable staged files require
    /// receipt publication, even if their temporary name is absent. Throws `logic_error` for a
    /// durable, foreign, already committed, or terminal registration.
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
