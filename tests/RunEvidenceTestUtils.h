#pragma once

#include "Run/RunEvidence.h"

#include <optional>
#include <utility>

/// Creates structurally valid immutable evidence for presentation and result-classification tests.
inline cao::run::RunEvidence terminalTestEvidence(
    const cao::run::RunPhase finalPhase, const bool cancellationObserved = false,
    std::optional<cao::run::RunProgress> progress = {}) {
    using namespace cao::run;
    MutableRunEvidence evidence;
    if (finalPhase != RunPhase::SafetyCleanup) {
        evidence.recordPhase(RunPhaseRecord::executed(RunPhase::Preparing));
        if (finalPhase != RunPhase::Preparing) {
            // Determinate phases establish their immutable total at zero before later counts.
            if (progress && progress->completed() != 0)
                evidence.recordPhase(RunPhaseRecord::executed(
                    finalPhase, RunProgress::determinate(progress->total())));
            evidence.recordPhase(RunPhaseRecord::executed(finalPhase, std::move(progress)));
        }
    }
    evidence.recordPhase(RunPhaseRecord::executed(RunPhase::SafetyCleanup));
    if (cancellationObserved) evidence.recordCancellationObservation();
    return std::move(evidence).consume();
}
