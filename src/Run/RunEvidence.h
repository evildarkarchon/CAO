#pragma once

#include "Run/RunLifecycle.h"

#include <memory>
#include <span>
#include <stdexcept>

namespace cao::run {
class RunEvidenceStorage;

/// Signals a violated Run Evidence programming invariant rather than a user-facing Run Failure.
class RunEvidenceInvariantViolation final : public std::logic_error {
   public:
    using std::logic_error::logic_error;
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
    /// Creates the sole mutable evidence owner for one synchronous Optimization Run.
    MutableRunEvidence();
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

    /// Retains that cancellation was observed without selecting or changing a Run Outcome.
    void recordCancellationObservation();

    /// Consumes this owner after Safety Cleanup into self-contained immutable Run Evidence.
    ///
    /// Calling this twice, consuming before Safety Cleanup, or mutating afterward throws
    /// `std::logic_error`.
    [[nodiscard]] RunEvidence consume() &&;

   private:
    std::unique_ptr<RunEvidenceStorage> _storage;
};
}  // namespace cao::run
