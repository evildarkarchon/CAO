#pragma once

#include "Run/ArchiveFinalizationResult.h"
#include "Run/RunEvidence.h"

#include <cstddef>
#include <optional>
#include <utility>

/// Records a fake finalizer's result the way Archive Finalization does: the output total, each
/// attempt in order, then the complete result. The total defaults to the attempt count; pass a
/// larger one for a result that stopped before every planned output. Throws
/// RunEvidenceInvariantViolation exactly when the real module's recording would.
inline void recordArchiveFinalizationResult(cao::run::RunWorkEvidence& evidence,
                                            cao::run::ArchiveFinalizationResult result,
                                            std::optional<std::size_t> total = std::nullopt) {
    const auto planned = total.value_or(result.attempts.size());
    evidence.recordArchiveFinalizationPlan(planned);
    for (const auto& attempt : result.attempts)
        evidence.recordArchiveFinalizationAttempt(attempt, planned);
    evidence.recordArchiveFinalization(std::move(result));
}

/// Owns Run Evidence that has reached the executed Archive Finalization phase, with the
/// phase-restricted work view Archive Finalization records into.
class ArchiveFinalizationEvidenceFixture final {
   public:
    /// Records Preparing and the executed Archive Finalization phase, optionally binding a sink
    /// that observes every accepted progress update.
    explicit ArchiveFinalizationEvidenceFixture(cao::run::RunObservationSink* observations = nullptr)
        : evidence(observations), workEvidence(evidence) {
        evidence.recordPhase(cao::run::RunPhaseRecord::executed(cao::run::RunPhase::Preparing));
        evidence.recordPhase(
            cao::run::RunPhaseRecord::executed(cao::run::RunPhase::ArchiveFinalization));
    }

    /// Returns the retained finalization, or nullptr before the phase recorded any result.
    [[nodiscard]] const cao::run::ArchiveFinalizationResult* finalization() const {
        return evidence.archiveFinalization();
    }

    /// Returns the phase's latest progress account, or nullptr before an output total exists.
    [[nodiscard]] const cao::run::RunProgress* progress() const {
        const auto* phase = evidence.phase(cao::run::RunPhase::ArchiveFinalization);
        return phase && phase->progress() ? &*phase->progress() : nullptr;
    }

    /// Applies the executor's mandatory Safety Cleanup and consumes the retained facts.
    cao::run::RunEvidence seal() {
        evidence.recordPhase(cao::run::RunPhaseRecord::executed(cao::run::RunPhase::SafetyCleanup));
        return std::move(evidence).consume();
    }

    cao::run::MutableRunEvidence evidence;
    cao::run::RunWorkEvidence workEvidence;
};
