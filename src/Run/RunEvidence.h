#pragma once

#include "Run/RunLifecycle.h"

#include <cstddef>
#include <filesystem>
#include <functional>
#include <map>
#include <memory>
#include <span>
#include <stdexcept>
#include <string>
#include <vector>

namespace cao::run {
struct ArchiveExtractionResult;
class RunEvidenceStorage;

/// Owns the non-derived Archive facts established when one discovery call returns.
class ArchiveDiscoveryEvidence final {
   public:
    /// Takes ownership of raw Archive exclusions and malformed nested-Archive evidence.
    ArchiveDiscoveryEvidence(std::map<routing::SkipReason, std::size_t> skippedArchiveCounts,
                             std::vector<std::filesystem::path> unsupportedExplicitPaths,
                             std::size_t nestedArchiveCount) noexcept;

    /// Returns the raw recognized-Archive exclusions for one stable reason.
    [[nodiscard]] std::size_t skippedArchiveCount(routing::SkipReason reason) const noexcept;

    /// Borrows explicitly selected unsupported paths in first-observation order.
    [[nodiscard]] std::span<const std::filesystem::path> unsupportedExplicitPaths() const noexcept;

    /// Returns the count of distinct Archives discovered only after extraction.
    [[nodiscard]] std::size_t nestedArchiveCount() const noexcept;

   private:
    std::map<routing::SkipReason, std::size_t> _skippedArchiveCounts;
    std::vector<std::filesystem::path> _unsupportedExplicitPaths;
    std::size_t _nestedArchiveCount{};
};

/// Signals a violated Run Evidence programming invariant rather than a user-facing Run Failure.
class RunEvidenceInvariantViolation final : public std::logic_error {
   public:
    using std::logic_error::logic_error;
};

/// Receives facts after Run Evidence has retained them and claimed their publication position.
///
/// Implementations adapt the synchronous Run Executor to production Run Event delivery or to a
/// test observer. Exceptions are isolated by MutableRunEvidence and become informational evidence.
class RunObservationSink {
   public:
    virtual ~RunObservationSink() = default;

    /// Observes one accepted phase transition or progress update.
    virtual void recordPhase(const RunPhaseRecord& phase) = 0;

    /// Observes one retained run-level failure.
    virtual void recordFailure(const RunFailure& failure) = 0;

    /// Observes one retained informational diagnostic.
    virtual void recordDiagnostic(const RunDiagnostic& diagnostic) = 0;

    /// Accepts a diagnostic that a lower work boundary deliberately publishes later.
    virtual void retainDiagnostic(const RunDiagnostic&) {}

    /// Publishes evidence already owned by a lower work record without retaining it there again.
    virtual void publishRetainedDiagnostic(const RunDiagnostic& diagnostic) {
        recordDiagnostic(diagnostic);
    }

    /// Publishes a failure already owned by lower work without retaining it there again.
    virtual void publishRetainedFailure(const RunFailure& failure) { recordFailure(failure); }
};

/// The immutable factual record consumed from one Optimization Run's mutable evidence owner.
///
/// Copies share immutable storage and remain readable after the Run Executor and every producer
/// have been destroyed. Run Outcome is deliberately absent because classification belongs to the
/// Run Executor rather than factual evidence.
class RunEvidence final {
   public:
    /// Shares the same immutable evidence storage with another value.
    RunEvidence(const RunEvidence&) noexcept = default;
    /// Replaces this immutable view with storage shared from another value.
    RunEvidence& operator=(const RunEvidence&) noexcept = default;
    /// Transfers one immutable evidence view without copying its retained facts.
    RunEvidence(RunEvidence&&) noexcept = default;
    /// Replaces this immutable view by transferring another view's shared storage.
    RunEvidence& operator=(RunEvidence&&) noexcept = default;
    /// Releases this view while preserving storage owned by any remaining copies.
    ~RunEvidence() = default;

    /// Borrows all successful Preparing facts, or nullptr when preparation did not complete.
    [[nodiscard]] const RunPreparation* preparation() const noexcept;

    /// Returns each traversed Run Phase once in first-traversal order with its latest account.
    [[nodiscard]] std::span<const RunPhaseRecord> phases() const noexcept;

    /// Returns the latest record for a traversed phase, or nullptr when it was never reached.
    [[nodiscard]] const RunPhaseRecord* phase(RunPhase phase) const noexcept;

    /// Returns informational observations in the order Run Evidence accepted them.
    [[nodiscard]] std::span<const RunDiagnostic> diagnostics() const noexcept;

    /// Returns run-level failures in the order Run Evidence accepted them.
    [[nodiscard]] std::span<const RunFailure> failures() const noexcept;

    /// Returns preflight Archive Collisions with precedence winners and ordered shadowed Archives.
    [[nodiscard]] std::span<const ArchiveCollision> archiveCollisions() const noexcept;

    /// Returns completed Archive extraction attempts in attempted order.
    [[nodiscard]] std::span<const ArchiveExtractionResult> archiveExtractionAttempts()
        const noexcept;

    /// Returns the completed Archive discovery fact set, or nullptr if discovery unwound.
    [[nodiscard]] const ArchiveDiscoveryEvidence* archiveDiscovery() const noexcept;

    /// Reports whether cooperative cancellation was observed before evidence was consumed.
    [[nodiscard]] bool cancellationObserved() const noexcept;

   private:
    friend class MutableRunEvidence;

    /// Adopts storage that the mutable owner has permanently relinquished.
    explicit RunEvidence(std::unique_ptr<RunEvidenceStorage> storage);

    std::shared_ptr<const RunEvidenceStorage> _storage;
};

/// The sole mutable owner of factual state for one synchronous Optimization Run.
///
/// Its interface accepts complete domain values and enforces lifecycle structure close to the
/// producer. Consuming it after Safety Cleanup returns a distinct immutable RunEvidence value and
/// permanently invalidates this owner.
class MutableRunEvidence final {
   public:
    /// Creates the sole mutable evidence owner and optionally binds its real publication adapter.
    explicit MutableRunEvidence(RunObservationSink* observations = nullptr);
    /// Prevents a second producer from sharing mutation authority.
    MutableRunEvidence(const MutableRunEvidence&) = delete;
    /// Prevents replacing this run's mutation authority with another owner's state.
    MutableRunEvidence& operator=(const MutableRunEvidence&) = delete;
    /// Keeps mutable evidence at one stable worker-confined address until consumption.
    MutableRunEvidence(MutableRunEvidence&&) = delete;
    /// Prevents transferred mutation authority from invalidating retained producer references.
    MutableRunEvidence& operator=(MutableRunEvidence&&) = delete;
    /// Releases unconsumed mutable storage without producing terminal evidence.
    ~MutableRunEvidence();

    /// Retains one complete successful preparation while Preparing is the current Run Phase.
    ///
    /// The returned reference borrows the owned value until this evidence is consumed. A second
    /// successful preparation is a programming error; failed or cancelled preparation records no
    /// value and therefore cannot expose partial Mod Roots, policy, configuration, or precedence.
    [[nodiscard]] const RunPreparation& recordPreparation(RunPreparation preparation);

    /// Accepts one canonical lifecycle transition or a monotonic update to the current phase.
    ///
    /// Repeated records replace the phase's latest account in place. Phase regression, status or
    /// skip-reason changes, invalid progress, and mutation after consumption throw
    /// `std::logic_error` as programming invariant violations.
    void recordPhase(RunPhaseRecord phase);

    /// Retains one informational diagnostic before publishing it at most once through the adapter.
    void recordDiagnostic(RunDiagnostic diagnostic);

    /// Retains a diagnostic now while preserving its established later publication boundary.
    void retainDiagnostic(RunDiagnostic diagnostic);

    /// Publishes every retained diagnostic not yet claimed, in acceptance order and at most once.
    void publishDiagnostics();

    /// Retains one run-level failure before publishing it at most once through the adapter.
    void recordFailure(RunFailure failure);

    /// Retains the complete collision plan while Archive discovery owns the current phase.
    ///
    /// Discovery reports the plan once before extraction. A second plan or a report outside
    /// Discovering Archives is a programming invariant violation.
    void recordArchiveCollisions(std::span<const ArchiveCollision> collisions);

    /// Retains one completed Archive extraction attempt during the extraction phase.
    ///
    /// The complete result remains attempt-local evidence: its Operation Failure is not copied
    /// into run-level failure storage, and the caller reports phase progress separately.
    void recordArchiveExtractionAttempt(ArchiveExtractionResult attempt);

    /// Retains the complete non-derived Archive discovery facts from one returned discovery call.
    ///
    /// Early cancellation and discovery failures can still return trustworthy exclusions while
    /// an exception that unwinds discovery records no misleading completed fact set.
    void recordArchiveDiscovery(ArchiveDiscoveryEvidence discovery);

    /// Returns the latest accepted record for a phase, or nullptr when it was never reached.
    [[nodiscard]] const RunPhaseRecord* phase(RunPhase phase) const;

    /// Returns informational observations accepted so far without exposing mutable storage.
    [[nodiscard]] std::span<const RunDiagnostic> diagnostics() const;

    /// Returns run-level failures accepted so far without exposing mutable storage.
    [[nodiscard]] std::span<const RunFailure> failures() const;

    /// Retains that cancellation was observed without selecting or changing a Run Outcome.
    void recordCancellationObservation();

    /// Consumes this owner after Safety Cleanup into self-contained immutable Run Evidence.
    ///
    /// Calling this twice, consuming before Safety Cleanup, or mutating afterward throws
    /// `std::logic_error`.
    [[nodiscard]] RunEvidence consume() &&;

   private:
    /// Publishes one retained phase after irreversibly advancing its publication position.
    void publishPhase(const RunPhaseRecord& phase);
    /// Publishes one retained failure after irreversibly advancing its publication position.
    void publishFailure(const RunFailure& failure);
    /// Converts an adapter exception to one non-recursive informational diagnostic.
    void publishSafely(RunPhase phase, const std::function<void()>& publication);
    /// Retains an adapter exception without feeding it back through the same failing path.
    void retainObserverFailure(RunPhase phase, std::string detail);

    std::unique_ptr<RunEvidenceStorage> _storage;
    RunObservationSink* _observations;
    std::size_t _publishedDiagnostics{};
    std::vector<std::size_t> _diagnosticPublications;
};
}  // namespace cao::run
