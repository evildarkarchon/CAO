#include "RunEvidence.h"

#include "ArchiveExtraction.h"
#include "ArchiveFinalizationResult.h"
#include "AssetRun.h"
#include "RunLifecycle.h"

#include <algorithm>
#include <map>
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
    std::optional<routing::RoutingLedger> routingLedger;
    std::vector<RoutedAssetAttempt> assetAttempts;
    std::optional<ArchiveFinalizationResult> archiveFinalization;
    std::optional<std::size_t> archiveFinalizationTotal;
    bool archiveFinalizationCompleted{};
    std::vector<RunFailure> safetyCleanupFailures;
    std::vector<RunFailure> cleanupFailures;
    std::vector<MutationSummary> mutationSummaries;
    std::map<routing::SkipReason, std::size_t> skippedAssetCounts;
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

/// Checks that a streamed output still describes the same completed attempt on return.
bool sameFinalizationAttempt(const ArchiveFinalizationAttempt& left,
                             const ArchiveFinalizationAttempt& right) {
    return left.archivePath == right.archivePath && left.modRoot == right.modRoot &&
           left.mutation == right.mutation && left.failure == right.failure &&
           left.safeToContinue == right.safeToContinue && left.detail == right.detail;
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

/// Groups durable effects from completed attempts and non-output finalization mutations at sealing.
std::vector<MutationSummary> deriveMutationSummaries(const RunEvidenceStorage& storage) {
    std::map<std::pair<std::filesystem::path, MutationKind>, MutationSummary> grouped;
    const auto account = [&grouped](const std::filesystem::path& root, const MutationKind kind,
                                    const execution::MutationState mutation,
                                    const std::size_t count = 1) {
        if (mutation == execution::MutationState::None) return;
        auto entry = grouped.try_emplace(std::pair{root, kind}, MutationSummary{root, kind}).first;
        if (mutation == execution::MutationState::Committed)
            entry->second.committed += count;
        else
            entry->second.partialOrUnknown += count;
    };
    for (const auto& attempt : storage.assetAttempts)
        account(attempt.modRoot, MutationKind::AssetProcessing, attempt.result.mutationState());
    for (const auto& attempt : storage.archiveExtractionAttempts)
        account(attempt.modRoot, MutationKind::ArchiveExtraction, attempt.mutation);
    if (storage.archiveFinalization) {
        for (const auto& attempt : storage.archiveFinalization->attempts)
            account(attempt.modRoot, MutationKind::ArchiveFinalization, attempt.mutation);
        for (const auto& mutation : storage.archiveFinalization->mutations)
            account(mutation.modRoot, MutationKind::ArchiveFinalization, mutation.mutation,
                    mutation.count);
    }
    std::vector<MutationSummary> summaries;
    summaries.reserve(grouped.size());
    for (auto& [key, summary] : grouped) summaries.push_back(std::move(summary));
    return summaries;
}

/// Preserves attempt-local cleanup evidence before the run's mandatory final cleanup failures.
std::vector<RunFailure> deriveCleanupFailures(const RunEvidenceStorage& storage) {
    std::vector<RunFailure> failures;
    for (const auto& attempt : storage.assetAttempts) {
        const auto local = attempt.result.cleanupFailures();
        failures.insert(failures.end(), local.begin(), local.end());
    }
    failures.insert(failures.end(), storage.safetyCleanupFailures.begin(),
                    storage.safetyCleanupFailures.end());
    return failures;
}

/// Aggregates the closed Skip Reason set from discovery and definitive routing at sealing.
std::map<routing::SkipReason, std::size_t> deriveSkippedAssetCounts(
    const RunEvidenceStorage& storage) {
    std::map<routing::SkipReason, std::size_t> counts;
    for (const auto reason :
         {routing::SkipReason::DisabledPhase, routing::SkipReason::DisabledAssetKind,
          routing::SkipReason::ExcludedAssetVariant}) {
        const auto discovered =
            storage.archiveDiscovery ? storage.archiveDiscovery->skippedArchiveCount(reason) : 0;
        const auto routed =
            storage.routingLedger ? storage.routingLedger->skippedAssetCount(reason) : 0;
        if (const auto total = discovered + routed; total != 0) counts.emplace(reason, total);
    }
    return counts;
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

const routing::RoutingLedger* RunEvidence::routingLedger() const noexcept {
    return _storage->routingLedger ? &*_storage->routingLedger : nullptr;
}

std::size_t RunEvidence::skippedAssetCount(const routing::SkipReason reason) const noexcept {
    const auto found = _storage->skippedAssetCounts.find(reason);
    return found == _storage->skippedAssetCounts.end() ? 0 : found->second;
}

std::span<const MutationSummary> RunEvidence::mutationSummaries() const noexcept {
    return _storage->mutationSummaries;
}

std::span<const RoutedAssetAttempt> RunEvidence::assetAttempts() const noexcept {
    return _storage->assetAttempts;
}

const ArchiveFinalizationResult* RunEvidence::archiveFinalization() const noexcept {
    return _storage->archiveFinalization ? &*_storage->archiveFinalization : nullptr;
}

std::span<const RunFailure> RunEvidence::safetyCleanupFailures() const noexcept {
    return _storage->safetyCleanupFailures;
}

std::span<const RunFailure> RunEvidence::cleanupFailures() const noexcept {
    return _storage->cleanupFailures;
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

void MutableRunEvidence::recordArchiveDiscoveryStarted() {
    recordPhase(RunPhaseRecord::executed(RunPhase::DiscoveringArchives));
}

void MutableRunEvidence::recordArchiveExtractionPlan(const std::size_t total) {
    recordPhase(
        RunPhaseRecord::executed(RunPhase::ExtractingArchives, RunProgress::determinate(total)));
}

void MutableRunEvidence::recordDryRunArchiveExtraction() {
    recordPhase(RunPhaseRecord::skipped(RunPhase::ExtractingArchives, PhaseSkipReason::DryRun));
}

void MutableRunEvidence::recordEffectiveAssetTreeStarted() {
    recordPhase(RunPhaseRecord::executed(RunPhase::BuildingEffectiveAssetTree));
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

void MutableRunEvidence::recordArchiveExtractionAttempt(ArchiveExtractionResult attempt,
                                                        const std::size_t total) {
    auto& storage = requireStorage(_storage);
    if (storage.phases.empty() || storage.phases.back().phase() != RunPhase::ExtractingArchives)
        throw RunEvidenceInvariantViolation(
            "Archive extraction attempts must be recorded during Extracting Archives");
    const auto& extractionPhase = storage.phases.back();
    if (!extractionPhase.progress() || extractionPhase.progress()->total() != total)
        throw RunEvidenceInvariantViolation(
            "Archive extraction attempts must use the immutable planned total");
    if (storage.archiveExtractionAttempts.size() >= total)
        throw RunEvidenceInvariantViolation(
            "Archive extraction attempts cannot exceed the planned total");
    const auto succeeded = extractionPhase.progress()->succeeded() + attempt.succeeded();
    const auto failed = extractionPhase.progress()->failed() + !attempt.succeeded();
    storage.archiveExtractionAttempts.push_back(std::move(attempt));
    recordPhase(RunPhaseRecord::executed(RunPhase::ExtractingArchives,
                                         RunProgress::determinate(total, succeeded, failed)));
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

void MutableRunEvidence::recordRoutingLedger(routing::RoutingLedger ledger) {
    auto& storage = requireStorage(_storage);
    if (storage.phases.empty() ||
        storage.phases.back().phase() != RunPhase::BuildingEffectiveAssetTree ||
        !storage.archiveDiscovery || !storage.failures.empty())
        throw RunEvidenceInvariantViolation(
            "Routing Ledger requires completed, successful Effective Asset Tree discovery");
    if (storage.routingLedger)
        throw RunEvidenceInvariantViolation("Definitive routing can only be recorded once");
    storage.routingLedger.emplace(std::move(ledger));
}

void MutableRunEvidence::recordAssetProcessingPlan(const std::size_t total) {
    auto& storage = requireStorage(_storage);
    if (!storage.routingLedger || storage.routingLedger->routedAssets().size() != total)
        throw RunEvidenceInvariantViolation(
            "Processing Assets total must match the definitive Routing Ledger");
    recordPhase(RunPhaseRecord::executed(RunPhase::ProcessingAssets,
                                         RunProgress::determinate(total)));
}

void MutableRunEvidence::recordAssetAttempt(RoutedAssetAttempt attempt,
                                            const std::size_t total) {
    auto& storage = requireStorage(_storage);
    if (storage.phases.empty() || storage.phases.back().phase() != RunPhase::ProcessingAssets)
        throw RunEvidenceInvariantViolation(
            "Asset attempts must be recorded during Processing Assets");
    const auto& processingPhase = storage.phases.back();
    if (!storage.routingLedger || storage.routingLedger->routedAssets().size() != total ||
        !processingPhase.progress() || processingPhase.progress()->total() != total)
        throw RunEvidenceInvariantViolation(
            "Asset attempts must use the immutable routed-work total");
    if (storage.assetAttempts.size() >= total)
        throw RunEvidenceInvariantViolation(
            "Asset attempts cannot exceed the routed-work total");
    const auto succeeded = processingPhase.progress()->succeeded() + attempt.result.succeeded();
    const auto failed = processingPhase.progress()->failed() + !attempt.result.succeeded();
    storage.assetAttempts.push_back(std::move(attempt));
    recordPhase(RunPhaseRecord::executed(RunPhase::ProcessingAssets,
                                         RunProgress::determinate(total, succeeded, failed)));
}

void MutableRunEvidence::recordArchiveFinalizationPlan(const std::size_t total) {
    auto& storage = requireStorage(_storage);
    if (storage.archiveFinalizationTotal || storage.phases.empty() ||
        storage.phases.back().phase() != RunPhase::ArchiveFinalization ||
        storage.phases.back().status() != RunPhaseStatus::Executed ||
        storage.phases.back().progress())
        throw RunEvidenceInvariantViolation(
            "Archive Finalization needs an executed phase and one immutable plan");
    storage.archiveFinalizationTotal = total;
    storage.archiveFinalization.emplace();
    recordPhase(RunPhaseRecord::executed(RunPhase::ArchiveFinalization,
                                         RunProgress::determinate(total)));
}

void MutableRunEvidence::recordArchiveFinalizationAttempt(ArchiveFinalizationAttempt attempt,
                                                           const std::size_t total) {
    auto& storage = requireStorage(_storage);
    if (!storage.archiveFinalizationTotal || *storage.archiveFinalizationTotal != total ||
        storage.archiveFinalizationCompleted || storage.phases.empty() ||
        storage.phases.back().phase() != RunPhase::ArchiveFinalization ||
        storage.phases.back().status() != RunPhaseStatus::Executed ||
        !storage.phases.back().progress())
        throw RunEvidenceInvariantViolation(
            "Archive Finalization attempts require the immutable executed plan");
    auto& finalization = *storage.archiveFinalization;
    if (finalization.attempts.size() >= total)
        throw RunEvidenceInvariantViolation(
            "Archive Finalization attempts cannot exceed the planned total");
    const auto succeeded = storage.phases.back().progress()->succeeded() + attempt.succeeded();
    const auto failed = storage.phases.back().progress()->failed() + !attempt.succeeded();
    finalization.attempts.push_back(std::move(attempt));
    recordPhase(RunPhaseRecord::executed(RunPhase::ArchiveFinalization,
                                         RunProgress::determinate(total, succeeded, failed)));
}

void MutableRunEvidence::recordArchiveFinalization(ArchiveFinalizationResult result) {
    auto& storage = requireStorage(_storage);
    if (storage.archiveFinalizationCompleted || storage.phases.empty() ||
        storage.phases.back().phase() != RunPhase::ArchiveFinalization ||
        storage.phases.back().status() != RunPhaseStatus::Executed)
        throw RunEvidenceInvariantViolation(
            "Archive Finalization results require the executed work phase exactly once");
    if (!storage.archiveFinalizationTotal && result.attempts.empty()) {
        // A finalizer may fail or cancel before planning establishes a trustworthy total.
        // Retain its phase-level status without inventing zero planned outputs.
        storage.archiveFinalization.emplace(std::move(result));
        storage.archiveFinalizationCompleted = true;
        if (storage.archiveFinalization->cancelled) storage.cancellationObserved = true;
        return;
    }
    if (!storage.archiveFinalizationTotal) recordArchiveFinalizationPlan(result.attempts.size());
    const auto total = *storage.archiveFinalizationTotal;
    const auto& streamed = storage.archiveFinalization->attempts;
    if (result.failure == ArchiveFinalizationFailure::UnexpectedException &&
        !result.safeToContinue && result.attempts.empty())
        result.attempts = streamed;
    if (result.attempts.size() > total || streamed.size() > result.attempts.size() ||
        !std::equal(streamed.begin(), streamed.end(), result.attempts.begin(),
                    sameFinalizationAttempt))
        throw RunEvidenceInvariantViolation(
            "Returned Archive Finalization attempts must match retained attempt order");
    for (std::size_t index = streamed.size(); index < result.attempts.size(); ++index)
        recordArchiveFinalizationAttempt(result.attempts[index], total);
    storage.archiveFinalization = std::move(result);
    storage.archiveFinalizationCompleted = true;
    if (storage.archiveFinalization->cancelled) storage.cancellationObserved = true;
}

void MutableRunEvidence::recordSafetyCleanupFailure(RunFailure failure) {
    auto& storage = requireStorage(_storage);
    if (storage.phases.empty() || storage.phases.back().phase() != RunPhase::SafetyCleanup ||
        failure.phase() != RunPhase::SafetyCleanup ||
        (failure.code() != RunFailureCode::TemporaryArtifactCleanupFailed &&
         failure.code() != RunFailureCode::SafetyCleanupServiceFailed))
        throw RunEvidenceInvariantViolation(
            "Safety Cleanup failures require the cleanup phase and a cleanup failure code");
    storage.safetyCleanupFailures.push_back(std::move(failure));
}

const RunPhaseRecord* MutableRunEvidence::phase(const RunPhase phase) const {
    const auto& storage = requireStorage(_storage);
    const auto found =
        std::find_if(storage.phases.begin(), storage.phases.end(),
                     [phase](const auto& candidate) { return candidate.phase() == phase; });
    return found == storage.phases.end() ? nullptr : &*found;
}

const RunPhaseRecord* MutableRunEvidence::currentPhase() const {
    const auto& storage = requireStorage(_storage);
    return storage.phases.empty() ? nullptr : &storage.phases.back();
}

std::span<const RunDiagnostic> MutableRunEvidence::diagnostics() const {
    return requireStorage(_storage).diagnostics;
}

std::span<const RunFailure> MutableRunEvidence::failures() const {
    return requireStorage(_storage).failures;
}

const ArchiveDiscoveryEvidence* MutableRunEvidence::archiveDiscovery() const {
    const auto& storage = requireStorage(_storage);
    return storage.archiveDiscovery ? &*storage.archiveDiscovery : nullptr;
}

const routing::RoutingLedger* MutableRunEvidence::routingLedger() const {
    const auto& storage = requireStorage(_storage);
    return storage.routingLedger ? &*storage.routingLedger : nullptr;
}

std::size_t MutableRunEvidence::skippedAssetCount(const routing::SkipReason reason) const {
    const auto& storage = requireStorage(_storage);
    return (storage.archiveDiscovery ? storage.archiveDiscovery->skippedArchiveCount(reason) : 0) +
           (storage.routingLedger ? storage.routingLedger->skippedAssetCount(reason) : 0);
}

const ArchiveFinalizationResult* MutableRunEvidence::archiveFinalization() const {
    const auto& storage = requireStorage(_storage);
    return storage.archiveFinalization ? &*storage.archiveFinalization : nullptr;
}

void MutableRunEvidence::publishPhase(const RunPhaseRecord& phase) {
    if (_observations == nullptr) return;
    reportSafely(phase.phase(), [&] { _observations->recordPhase(phase); });
}

void MutableRunEvidence::reportSafely(const RunPhase phase,
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
        reportSafely(retained.phase(), [&] { _observations->recordDiagnostic(retained); });
    }
}

void MutableRunEvidence::publishFailure(const RunFailure& failure) {
    if (_observations == nullptr) return;
    reportSafely(failure.phase(), [&] { _observations->recordFailure(failure); });
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
    // Derive stable summaries from retained facts after every producer has stopped.
    storage.mutationSummaries = deriveMutationSummaries(storage);
    storage.cleanupFailures = deriveCleanupFailures(storage);
    storage.skippedAssetCounts = deriveSkippedAssetCounts(storage);
    return RunEvidence(std::move(_storage));
}

RunWorkEvidence::RunWorkEvidence(MutableRunEvidence& evidence) noexcept : _evidence(evidence) {}

void RunWorkEvidence::recordArchiveCollisions(std::span<const ArchiveCollision> collisions) {
    _evidence.recordArchiveCollisions(collisions);
}

void RunWorkEvidence::recordArchiveExtractionAttempt(ArchiveExtractionResult attempt,
                                                     std::size_t total) {
    _evidence.recordArchiveExtractionAttempt(std::move(attempt), total);
}

void RunWorkEvidence::recordArchiveDiscovery(ArchiveDiscoveryEvidence discovery) {
    _evidence.recordArchiveDiscovery(std::move(discovery));
}

void RunWorkEvidence::recordRoutingLedger(routing::RoutingLedger ledger) {
    _evidence.recordRoutingLedger(std::move(ledger));
}

void RunWorkEvidence::recordAssetAttempt(RoutedAssetAttempt attempt, std::size_t total) {
    _evidence.recordAssetAttempt(std::move(attempt), total);
}

void RunWorkEvidence::recordArchiveFinalizationPlan(std::size_t total) {
    _evidence.recordArchiveFinalizationPlan(total);
}

void RunWorkEvidence::recordArchiveFinalizationAttempt(ArchiveFinalizationAttempt attempt,
                                                       std::size_t total) {
    _evidence.recordArchiveFinalizationAttempt(std::move(attempt), total);
}

void RunWorkEvidence::recordArchiveFinalization(ArchiveFinalizationResult result) {
    _evidence.recordArchiveFinalization(std::move(result));
}

void RunWorkEvidence::recordFailure(RunFailure failure) {
    _evidence.recordFailure(std::move(failure));
}

void RunWorkEvidence::retainDiagnostic(RunDiagnostic diagnostic) {
    _evidence.retainDiagnostic(std::move(diagnostic));
}

void RunWorkEvidence::publishDiagnostics() { _evidence.publishDiagnostics(); }

void RunWorkEvidence::reportSafely(RunPhase phase,
                                   const std::function<void()>& publication) {
    _evidence.reportSafely(phase, publication);
}

void RunWorkEvidence::recordCancellationObservation() {
    _evidence.recordCancellationObservation();
}

const RunPhaseRecord* RunWorkEvidence::currentPhase() const {
    return _evidence.currentPhase();
}

std::span<const RunDiagnostic> RunWorkEvidence::diagnostics() const {
    return _evidence.diagnostics();
}

const ArchiveDiscoveryEvidence* RunWorkEvidence::archiveDiscovery() const {
    return _evidence.archiveDiscovery();
}

const routing::RoutingLedger* RunWorkEvidence::routingLedger() const {
    return _evidence.routingLedger();
}

std::size_t RunWorkEvidence::skippedAssetCount(routing::SkipReason reason) const {
    return _evidence.skippedAssetCount(reason);
}
}  // namespace cao::run
