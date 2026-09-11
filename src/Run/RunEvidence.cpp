#include "RunEvidence.h"

#include "RunLifecycle.h"

#include <algorithm>
#include <optional>
#include <stdexcept>
#include <utility>
#include <vector>

namespace cao::run {
class RunEvidenceStorage final {
   public:
    std::optional<RunPreparation> preparation;
    std::vector<RunPhaseRecord> phases;
    bool cancellationObserved{};
};

namespace {
/// Returns a phase's stable position in the canonical lifecycle sequence.
std::size_t phasePosition(const RunPhase phase) {
    const auto sequence = runPhaseSequence();
    const auto found = std::find(sequence.begin(), sequence.end(), phase);
    if (found == sequence.end()) throw RunEvidenceInvariantViolation("Unknown Run Phase");
    return static_cast<std::size_t>(std::distance(sequence.begin(), found));
}

/// Rejects progress values that cannot be a valid account for a determinate phase.
void validateProgress(const RunPhaseRecord& phase) {
    if (!phase.progress()) return;
    if (phase.progress()->completed() > phase.progress()->total())
        throw RunEvidenceInvariantViolation("Run Phase progress exceeds its immutable total");
}

/// Rejects a same-phase replacement that would rewrite or regress accepted lifecycle facts.
void validateReplacement(const RunPhaseRecord& previous, const RunPhaseRecord& replacement) {
    if (previous.status() != replacement.status() ||
        previous.skipReason() != replacement.skipReason())
        throw RunEvidenceInvariantViolation(
            "Run Phase status and skip reason are immutable after traversal");

    validateProgress(replacement);
    if (!previous.progress()) {
        if (replacement.progress() && replacement.progress()->completed() != 0)
            throw RunEvidenceInvariantViolation(
                "Determinate Run Phase progress must begin at zero");
        return;
    }
    if (!replacement.progress())
        throw RunEvidenceInvariantViolation("Run Phase progress cannot become indeterminate");
    if (previous.progress()->total() != replacement.progress()->total())
        throw RunEvidenceInvariantViolation("Run Phase progress total cannot change");
    if (replacement.progress()->succeeded() < previous.progress()->succeeded() ||
        replacement.progress()->failed() < previous.progress()->failed())
        throw RunEvidenceInvariantViolation("Run Phase progress cannot regress");
}

/// Returns live mutable storage or rejects use of a consumed evidence owner.
RunEvidenceStorage& requireStorage(std::unique_ptr<RunEvidenceStorage>& storage) {
    if (!storage)
        throw RunEvidenceInvariantViolation("Mutable Run Evidence has already been consumed");
    return *storage;
}
}  // namespace

RunEvidence::RunEvidence(std::unique_ptr<RunEvidenceStorage> storage)
    : _storage(std::move(storage)) {}

const RunPreparation* RunEvidence::preparation() const noexcept {
    return _storage->preparation ? &*_storage->preparation : nullptr;
}

std::span<const RunPhaseRecord> RunEvidence::phases() const noexcept { return _storage->phases; }

const RunPhaseRecord* RunEvidence::phase(const RunPhase phase) const noexcept {
    const auto found =
        std::find_if(_storage->phases.begin(), _storage->phases.end(),
                     [phase](const auto& candidate) { return candidate.phase() == phase; });
    return found == _storage->phases.end() ? nullptr : &*found;
}

bool RunEvidence::cancellationObserved() const noexcept { return _storage->cancellationObserved; }

MutableRunEvidence::MutableRunEvidence() : _storage(std::make_unique<RunEvidenceStorage>()) {
    _storage->phases.reserve(runPhaseSequence().size());
}

MutableRunEvidence::~MutableRunEvidence() = default;

const RunPreparation& MutableRunEvidence::recordPreparation(RunPreparation preparation) {
    auto& storage = requireStorage(_storage);
    if (storage.preparation)
        throw RunEvidenceInvariantViolation(
            "Successful Run preparation can only be recorded once");
    if (storage.phases.empty() || storage.phases.back().phase() != RunPhase::Preparing)
        throw RunEvidenceInvariantViolation(
            "Successful Run preparation must be recorded during Preparing");
    storage.preparation.emplace(std::move(preparation));
    return *storage.preparation;
}

void MutableRunEvidence::recordPhase(RunPhaseRecord phase) {
    auto& storage = requireStorage(_storage);
    validateProgress(phase);
    if (storage.phases.empty()) {
        // A scheduling failure owns no Preparing traversal but still owes mandatory cleanup.
        if (phase.phase() != RunPhase::Preparing && phase.phase() != RunPhase::SafetyCleanup)
            throw RunEvidenceInvariantViolation("Run Phase traversal must begin at Preparing");
        if (phase.progress() && phase.progress()->completed() != 0)
            throw RunEvidenceInvariantViolation(
                "Determinate Run Phase progress must begin at zero");
        storage.phases.push_back(std::move(phase));
        return;
    }

    auto& latest = storage.phases.back();
    if (phase.phase() == latest.phase()) {
        validateReplacement(latest, phase);
        latest = std::move(phase);
        return;
    }
    if (phasePosition(phase.phase()) <= phasePosition(latest.phase()))
        throw RunEvidenceInvariantViolation("Run Phases must follow canonical traversal order");
    if (phase.progress() && phase.progress()->completed() != 0)
        throw RunEvidenceInvariantViolation("Determinate Run Phase progress must begin at zero");
    storage.phases.push_back(std::move(phase));
}

void MutableRunEvidence::recordCancellationObservation() {
    requireStorage(_storage).cancellationObserved = true;
}

RunEvidence MutableRunEvidence::consume() && {
    auto& storage = requireStorage(_storage);
    if (storage.phases.empty() || storage.phases.back().phase() != RunPhase::SafetyCleanup)
        throw RunEvidenceInvariantViolation(
            "Run Evidence can only be consumed after Safety Cleanup");
    return RunEvidence(std::move(_storage));
}
}  // namespace cao::run
