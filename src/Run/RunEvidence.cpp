#include "RunEvidence.h"

#include "ArchiveExtraction.h"
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
    std::vector<RunDiagnostic> diagnostics;
    std::vector<RunFailure> failures;
    std::vector<ArchiveCollision> archiveCollisions;
    std::vector<ArchiveExtractionResult> archiveExtractionAttempts;
    std::optional<ArchiveDiscoveryEvidence> archiveDiscovery;
    bool archiveCollisionsRecorded{};
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

/// Returns readable live storage or rejects inspection of a consumed evidence owner.
const RunEvidenceStorage& requireStorage(const std::unique_ptr<RunEvidenceStorage>& storage) {
    if (!storage)
        throw RunEvidenceInvariantViolation("Mutable Run Evidence has already been consumed");
    return *storage;
}
}  // namespace

ArchiveDiscoveryEvidence::ArchiveDiscoveryEvidence(
    std::map<routing::SkipReason, std::size_t> skippedArchiveCounts,
    std::vector<std::filesystem::path> unsupportedExplicitPaths,
    const std::size_t nestedArchiveCount) noexcept
    : _skippedArchiveCounts(std::move(skippedArchiveCounts)),
      _unsupportedExplicitPaths(std::move(unsupportedExplicitPaths)),
      _nestedArchiveCount(nestedArchiveCount) {}

std::size_t ArchiveDiscoveryEvidence::skippedArchiveCount(
    const routing::SkipReason reason) const noexcept {
    const auto found = _skippedArchiveCounts.find(reason);
    return found == _skippedArchiveCounts.end() ? 0 : found->second;
}

std::span<const std::filesystem::path> ArchiveDiscoveryEvidence::unsupportedExplicitPaths()
    const noexcept {
    return _unsupportedExplicitPaths;
}

std::size_t ArchiveDiscoveryEvidence::nestedArchiveCount() const noexcept {
    return _nestedArchiveCount;
}

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

std::span<const RunDiagnostic> RunEvidence::diagnostics() const noexcept {
    return _storage->diagnostics;
}

std::span<const RunFailure> RunEvidence::failures() const noexcept { return _storage->failures; }

std::span<const ArchiveCollision> RunEvidence::archiveCollisions() const noexcept {
    return _storage->archiveCollisions;
}

std::span<const ArchiveExtractionResult> RunEvidence::archiveExtractionAttempts() const noexcept {
    return _storage->archiveExtractionAttempts;
}

const ArchiveDiscoveryEvidence* RunEvidence::archiveDiscovery() const noexcept {
    return _storage->archiveDiscovery ? &*_storage->archiveDiscovery : nullptr;
}

bool RunEvidence::cancellationObserved() const noexcept { return _storage->cancellationObserved; }

MutableRunEvidence::MutableRunEvidence(RunObservationSink* observations)
    : _storage(std::make_unique<RunEvidenceStorage>()), _observations(observations) {
    _storage->phases.reserve(runPhaseSequence().size());
}

MutableRunEvidence::~MutableRunEvidence() = default;

const RunPreparation& MutableRunEvidence::recordPreparation(RunPreparation preparation) {
    auto& storage = requireStorage(_storage);
    if (storage.preparation)
        throw RunEvidenceInvariantViolation("Successful Run preparation can only be recorded once");
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
    } else {
        auto& latest = storage.phases.back();
        if (phase.phase() == latest.phase()) {
            validateReplacement(latest, phase);
            latest = std::move(phase);
        } else {
            if (phasePosition(phase.phase()) <= phasePosition(latest.phase()))
                throw RunEvidenceInvariantViolation(
                    "Run Phases must follow canonical traversal order");
            if (phase.progress() && phase.progress()->completed() != 0)
                throw RunEvidenceInvariantViolation(
                    "Determinate Run Phase progress must begin at zero");
            storage.phases.push_back(std::move(phase));
        }
    }

    // Copy before calling out so reentrant publication cannot invalidate vector storage.
    const auto retained = storage.phases.back();
    publishPhase(retained);
}

void MutableRunEvidence::recordDiagnostic(RunDiagnostic diagnostic) {
    retainDiagnostic(std::move(diagnostic));
    publishDiagnostics();
}

void MutableRunEvidence::retainDiagnostic(RunDiagnostic diagnostic) {
    auto& storage = requireStorage(_storage);
    storage.diagnostics.push_back(std::move(diagnostic));
    _diagnosticPublications.push_back(storage.diagnostics.size() - 1);
}

void MutableRunEvidence::recordFailure(RunFailure failure) {
    auto& storage = requireStorage(_storage);
    storage.failures.push_back(std::move(failure));
    const auto retained = storage.failures.back();
    publishFailure(retained);
}

void MutableRunEvidence::recordArchiveCollisions(
    const std::span<const ArchiveCollision> collisions) {
    auto& storage = requireStorage(_storage);
    if (storage.phases.empty() || storage.phases.back().phase() != RunPhase::DiscoveringArchives)
        throw RunEvidenceInvariantViolation(
            "Archive Collisions must be recorded during Discovering Archives");
    if (storage.archiveCollisionsRecorded)
        throw RunEvidenceInvariantViolation("Archive Collisions can only be recorded once");
    storage.archiveCollisions.assign(collisions.begin(), collisions.end());
    storage.archiveCollisionsRecorded = true;
}

void MutableRunEvidence::recordArchiveExtractionAttempt(ArchiveExtractionResult attempt) {
    auto& storage = requireStorage(_storage);
    if (storage.phases.empty() || storage.phases.back().phase() != RunPhase::ExtractingArchives)
        throw RunEvidenceInvariantViolation(
            "Archive extraction attempts must be recorded during Extracting Archives");
    storage.archiveExtractionAttempts.push_back(std::move(attempt));
}

void MutableRunEvidence::recordArchiveDiscovery(ArchiveDiscoveryEvidence discovery) {
    auto& storage = requireStorage(_storage);
    if (storage.phases.empty())
        throw RunEvidenceInvariantViolation(
            "Archive discovery evidence requires a discovery Run Phase");
    const auto position = phasePosition(storage.phases.back().phase());
    if (position < phasePosition(RunPhase::DiscoveringArchives) ||
        position > phasePosition(RunPhase::BuildingEffectiveAssetTree))
        throw RunEvidenceInvariantViolation(
            "Archive discovery evidence must be recorded during Archive discovery");
    if (storage.archiveDiscovery)
        throw RunEvidenceInvariantViolation("Archive discovery evidence can only be recorded once");
    storage.archiveDiscovery.emplace(std::move(discovery));
}

const RunPhaseRecord* MutableRunEvidence::phase(const RunPhase phase) const {
    const auto& storage = requireStorage(_storage);
    const auto found =
        std::find_if(storage.phases.begin(), storage.phases.end(),
                     [phase](const auto& candidate) { return candidate.phase() == phase; });
    return found == storage.phases.end() ? nullptr : &*found;
}

std::span<const RunDiagnostic> MutableRunEvidence::diagnostics() const {
    return requireStorage(_storage).diagnostics;
}

std::span<const RunFailure> MutableRunEvidence::failures() const {
    return requireStorage(_storage).failures;
}

void MutableRunEvidence::publishPhase(const RunPhaseRecord& phase) {
    if (_observations == nullptr) return;
    publishSafely(phase.phase(), [&] { _observations->recordPhase(phase); });
}

void MutableRunEvidence::publishSafely(const RunPhase phase,
                                       const std::function<void()>& publication) {
    try {
        publication();
    } catch (const std::exception& error) {
        retainObserverFailure(phase, error.what());
    } catch (...) {
        retainObserverFailure(phase, "The observer threw a non-standard exception");
    }
}

void MutableRunEvidence::publishDiagnostics() {
    auto& storage = requireStorage(_storage);
    while (_publishedDiagnostics < _diagnosticPublications.size()) {
        // Claim before calling out and copy before ObserverFailed can reallocate diagnostic
        // storage.
        const auto retained = storage.diagnostics[_diagnosticPublications[_publishedDiagnostics++]];
        if (_observations == nullptr) continue;
        publishSafely(retained.phase(),
                      [&] { _observations->publishRetainedDiagnostic(retained); });
    }
}

void MutableRunEvidence::publishFailure(const RunFailure& failure) {
    if (_observations == nullptr) return;
    publishSafely(failure.phase(), [&] { _observations->publishRetainedFailure(failure); });
}

void MutableRunEvidence::retainObserverFailure(const RunPhase phase, std::string detail) {
    auto& storage = requireStorage(_storage);
    storage.diagnostics.emplace_back(RunDiagnosticCode::ObserverFailed, phase, std::move(detail));
    // The generated diagnostic is deliberately absent from _diagnosticPublications, so the same
    // failing path cannot receive it recursively or at a later boundary.
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
