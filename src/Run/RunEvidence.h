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
struct ArchiveFinalizationAttempt;
struct ArchiveFinalizationResult;
struct RoutedAssetAttempt;
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

/// Receives typed work milestones and facts after Run Evidence has claimed publication positions.
///
/// The Run Executor translates milestones into lifecycle records. Direct AssetRun observers use
/// the default phase translation for compatibility. Exceptions from publication are isolated by
/// MutableRunEvidence; work milestone contract violations propagate to the executor.
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

    /// Submits the frozen Archive output total before any completed output is published.
    /// The default throws RunEvidenceInvariantViolation when no evidence reporter is installed.
    virtual void recordArchiveFinalizationPlan(std::size_t total);

    /// Submits a completed output before its progress is published; implementations own a copy.
    /// The default throws RunEvidenceInvariantViolation when no evidence reporter is installed.
    virtual void recordArchiveFinalizationAttempt(const ArchiveFinalizationAttempt&,
                                                  std::size_t total);

    /// Reports discovery entry for the executor to translate into the next Run Phase.
    virtual RunPhaseRecord archiveDiscoveryStarted();
    /// Reports the immutable extraction total before any Archive attempt.
    virtual RunPhaseRecord archiveExtractionPlanned(std::size_t total);
    /// Reports that Dry Run excludes Archive extraction.
    virtual RunPhaseRecord dryRunArchiveExtraction();
    /// Reports entry into definitive Effective Asset Tree discovery.
    virtual RunPhaseRecord effectiveAssetTreeStarted();
    /// Reports the definitive routed Asset total before attempts begin.
    virtual RunPhaseRecord assetProcessingPlanned(std::size_t total);
    /// Reports finalizer availability; the executor selects its final work phase record.
    virtual RunPhaseRecord archiveFinalizationAvailable(routing::ExecutionMode mode,
                                                        bool hasFinalizer);
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

    /// Borrows definitive routing, or returns nullptr when discovery never completed routing.
    /// An empty ledger still means routing completed successfully with no Routed Assets.
    [[nodiscard]] const routing::RoutingLedger* routingLedger() const noexcept;

    /// Returns recognized exclusions from authoritative Archive discovery and routing facts.
    [[nodiscard]] std::size_t skippedAssetCount(routing::SkipReason reason) const noexcept;

    /// Borrows counts derived at sealing from completed attempts, ordered by Mod Root and kind.
    [[nodiscard]] std::span<const MutationSummary> mutationSummaries() const noexcept;

    /// Borrows completed Asset attempts in execution order, including failed unsafe attempts.
    [[nodiscard]] std::span<const RoutedAssetAttempt> assetAttempts() const noexcept;

    /// Borrows finalization attempts and phase-level status, or nullptr when it was not attempted.
    [[nodiscard]] const ArchiveFinalizationResult* archiveFinalization() const noexcept;

    /// Borrows final Safety Cleanup failures in attempted order, apart from Run Failures.
    [[nodiscard]] std::span<const RunFailure> safetyCleanupFailures() const noexcept;

    /// Borrows attempt-local cleanup failures followed by final Safety Cleanup failures.
    [[nodiscard]] std::span<const RunFailure> cleanupFailures() const noexcept;

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

    /// Accepts that Archive discovery started and advances the executor-owned lifecycle account.
    void recordArchiveDiscoveryStarted();

    /// Accepts the immutable Archive extraction total and starts determinate phase progress.
    void recordArchiveExtractionPlan(std::size_t total);

    /// Accepts that Dry Run made Archive extraction inapplicable without inventing a work total.
    void recordDryRunArchiveExtraction();

    /// Accepts that definitive Effective Asset Tree discovery started after Archive attempts.
    void recordEffectiveAssetTreeStarted();

    /// Retains the complete collision plan while Archive discovery owns the current phase.
    ///
    /// Discovery reports the plan once before extraction. A second plan or a report outside
    /// Discovering Archives is a programming invariant violation.
    void recordArchiveCollisions(std::span<const ArchiveCollision> collisions);

    /// Retains one completed Archive extraction attempt during the extraction phase.
    ///
    /// The complete result remains attempt-local evidence: its Operation Failure is not copied
    /// into run-level failure storage. The immutable planned total and retained attempts derive the
    /// executor-owned phase progress published after retention.
    void recordArchiveExtractionAttempt(ArchiveExtractionResult attempt, std::size_t total);

    /// Retains the complete non-derived Archive discovery facts from one returned discovery call.
    ///
    /// Early cancellation and discovery failures can still return trustworthy exclusions while
    /// an exception that unwinds discovery records no misleading completed fact set.
    void recordArchiveDiscovery(ArchiveDiscoveryEvidence discovery);

    /// Retains one definitive batch routing result after Effective Asset Tree discovery.
    /// Failed or interrupted discovery must never submit a partial Routing Ledger.
    void recordRoutingLedger(routing::RoutingLedger ledger);

    /// Starts Processing Assets with the immutable routed-only total from the retained ledger.
    void recordAssetProcessingPlan(std::size_t total);

    /// Retains a completed Asset attempt before publishing progress against the routed total.
    /// Its exact operation result remains attached to the Routed Asset in attempted order.
    void recordAssetAttempt(RoutedAssetAttempt attempt, std::size_t total);

    /// Starts determinate Archive Finalization progress with the frozen output count.
    /// Throws RunEvidenceInvariantViolation for a second plan or a plan outside the executed phase.
    void recordArchiveFinalizationPlan(std::size_t total);

    /// Retains one completed output attempt before publishing its progress update.
    /// Throws RunEvidenceInvariantViolation for a missing/mismatched plan, wrong phase, or excess
    /// attempt count.
    void recordArchiveFinalizationAttempt(ArchiveFinalizationAttempt attempt, std::size_t total);

    /// Retains the returned finalization status and any attempts not already streamed.
    /// Previously streamed attempts must match the returned prefix; phase-level failure and
    /// cancellation remain distinct from Operation Failures on individual attempts. Throws
    /// RunEvidenceInvariantViolation for a wrong phase, repeated result, or mismatched attempts.
    void recordArchiveFinalization(ArchiveFinalizationResult result);

    /// Retains one final Safety Cleanup failure without publishing it as a Run Failure.
    /// Cleanup services may return several failures; callers submit each in attempted order.
    /// Throws RunEvidenceInvariantViolation for a wrong phase or non-cleanup failure code.
    void recordSafetyCleanupFailure(RunFailure failure);

    /// Returns the latest accepted record for a phase, or nullptr when it was never reached.
    [[nodiscard]] const RunPhaseRecord* phase(RunPhase phase) const;

    /// Returns the current executor-owned lifecycle position, or nullptr before traversal starts.
    [[nodiscard]] const RunPhaseRecord* currentPhase() const;

    /// Returns informational observations accepted so far without exposing mutable storage.
    [[nodiscard]] std::span<const RunDiagnostic> diagnostics() const;

    /// Returns run-level failures accepted so far without exposing mutable storage.
    [[nodiscard]] std::span<const RunFailure> failures() const;

    /// Borrows finalization already retained by this worker, including streamed attempts.
    [[nodiscard]] const ArchiveFinalizationResult* archiveFinalization() const;

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
