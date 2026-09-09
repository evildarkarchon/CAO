#include "RunLifecycle.h"
#include "RunWorkRecord.h"

#include <algorithm>
#include <array>
#include <atomic>
#include <random>
#include <utility>

namespace cao::run {
RunId createRunId() {
    // A process nonce separates launches; a counter makes identities unique within this process.
    static const auto nonce = [] {
        std::random_device random;
        return std::to_string(random()) + "-" + std::to_string(random()) + "-";
    }();
    static std::atomic<std::uint64_t> next{0};
    return nonce + std::to_string(next.fetch_add(1, std::memory_order_relaxed));
}

std::span<const RunPhase> runPhaseSequence() noexcept {
    // The sequence is the contract, not an implementation detail: adapters render it, skipped
    // phases still appear in it, and Safety Cleanup always terminates it.
    static constexpr std::array sequence{
        RunPhase::Preparing,          RunPhase::DiscoveringArchives,
        RunPhase::ExtractingArchives, RunPhase::BuildingEffectiveAssetTree,
        RunPhase::ProcessingAssets,   RunPhase::ArchiveFinalization,
        RunPhase::SafetyCleanup};
    return sequence;
}

RunProgress::RunProgress(const std::size_t total, const std::size_t succeeded,
                         const std::size_t failed) noexcept
    : _total(total), _succeeded(succeeded), _failed(failed) {}

RunProgress RunProgress::determinate(const std::size_t total, const std::size_t succeeded,
                                     const std::size_t failed) noexcept {
    return RunProgress(total, succeeded, failed);
}

std::size_t RunProgress::total() const noexcept { return _total; }

std::size_t RunProgress::succeeded() const noexcept { return _succeeded; }

std::size_t RunProgress::failed() const noexcept { return _failed; }

std::size_t RunProgress::completed() const noexcept {
    // Deriving completed keeps the invariant structural, so no caller can observe a phase whose
    // completed count disagrees with its succeeded and failed attempts.
    return _succeeded + _failed;
}

RunPhaseRecord::RunPhaseRecord(const RunPhase phase, const RunPhaseStatus status,
                               std::optional<PhaseSkipReason> skipReason,
                               std::optional<RunProgress> progress) noexcept
    : _phase(phase),
      _status(status),
      _skipReason(std::move(skipReason)),
      _progress(std::move(progress)) {}

RunPhaseRecord RunPhaseRecord::executed(const RunPhase phase,
                                        std::optional<RunProgress> progress) noexcept {
    return RunPhaseRecord(phase, RunPhaseStatus::Executed, {}, std::move(progress));
}

RunPhaseRecord RunPhaseRecord::skipped(const RunPhase phase,
                                       const PhaseSkipReason reason) noexcept {
    // A skipped phase never carries progress, so it cannot contribute an invented total.
    return RunPhaseRecord(phase, RunPhaseStatus::Skipped, reason, {});
}

RunPhase RunPhaseRecord::phase() const noexcept { return _phase; }

RunPhaseStatus RunPhaseRecord::status() const noexcept { return _status; }

std::optional<PhaseSkipReason> RunPhaseRecord::skipReason() const noexcept { return _skipReason; }

const std::optional<RunProgress>& RunPhaseRecord::progress() const noexcept { return _progress; }

ModSelection::ModSelection(const ModSelectionKind kind, std::filesystem::path directory)
    : _kind(kind), _directory(std::move(directory)) {}

ModSelection ModSelection::singleModRoot(std::filesystem::path root) {
    return ModSelection(ModSelectionKind::SingleModRoot, std::move(root));
}

ModSelection ModSelection::childModRoots(std::filesystem::path modsDirectory) {
    return ModSelection(ModSelectionKind::ChildModRoots, std::move(modsDirectory));
}

ModSelectionKind ModSelection::kind() const noexcept { return _kind; }

const std::filesystem::path& ModSelection::directory() const noexcept { return _directory; }

RunRequest::RunRequest(std::string profileIdentity, const routing::ExecutionMode executionMode,
                       ModSelection modSelection, std::vector<routing::RequestedWork> requestedWork,
                       ArchivePrecedence archivePrecedence)
    : _profileIdentity(std::move(profileIdentity)),
      _executionMode(executionMode),
      _modSelection(std::move(modSelection)),
      _requestedWork(std::move(requestedWork)),
      _archivePrecedence(std::move(archivePrecedence)) {}

RunRequest RunRequest::create(std::string profileIdentity,
                              const routing::ExecutionMode executionMode, ModSelection modSelection,
                              std::vector<routing::RequestedWork> requestedWork,
                              ArchivePrecedence archivePrecedence) {
    // Callers assemble work from independent GUI and CLI choices, so the request normalizes the
    // sequence into a closed set in enumeration order. Repeated runs then observe one order.
    std::sort(requestedWork.begin(), requestedWork.end());
    requestedWork.erase(std::unique(requestedWork.begin(), requestedWork.end()),
                        requestedWork.end());
    return RunRequest(std::move(profileIdentity), executionMode, std::move(modSelection),
                      std::move(requestedWork), std::move(archivePrecedence));
}

const std::string& RunRequest::profileIdentity() const noexcept { return _profileIdentity; }

routing::ExecutionMode RunRequest::executionMode() const noexcept { return _executionMode; }

const ModSelection& RunRequest::modSelection() const noexcept { return _modSelection; }

std::span<const routing::RequestedWork> RunRequest::requestedWork() const noexcept {
    return _requestedWork;
}

bool RunRequest::requests(const routing::RequestedWork work) const noexcept {
    return std::find(_requestedWork.begin(), _requestedWork.end(), work) != _requestedWork.end();
}

bool RunRequest::hasRequestedWork() const noexcept { return !_requestedWork.empty(); }

OptimizationRunResult::OptimizationRunResult(
    const RunOutcome outcome, const RunPhase finalPhase, std::vector<RunPhaseRecord> phases,
    RunId runId, std::vector<RunFailure> failures,
    std::shared_ptr<const RunPreparation> preparation, std::vector<RunFailure> cleanupFailures,
    const bool cancellationObserved, std::shared_ptr<const RunWorkRecord> work,
    std::vector<MutationSummary> mutationSummaries) noexcept
    : _runId(std::move(runId)),
      _outcome(outcome),
      _finalPhase(finalPhase),
      _phases(std::move(phases)),
      _failures(std::move(failures)),
      _preparation(std::move(preparation)),
      _cleanupFailures(std::move(cleanupFailures)),
      _cancellationObserved(cancellationObserved || outcome == RunOutcome::Cancelled),
      _work(std::move(work)),
      _mutationSummaries(std::move(mutationSummaries)) {}

OptimizationRunResult OptimizationRunResult::terminal(
    RunOutcome outcome, const RunPhase finalPhase, std::vector<RunPhaseRecord> phases,
    RunId runId, std::vector<RunFailure> failures,
    std::shared_ptr<const RunPreparation> preparation, std::vector<RunFailure> cleanupFailures,
    bool cancellationObserved, const RunWorkRecord* work) {
    // Copy instead of sharing caller storage: even a retained mutable service record cannot
    // rewrite evidence already published in a terminal event.
    auto ownedWork = std::make_shared<const RunWorkRecord>(work ? *work : RunWorkRecord{});
    cancellationObserved = cancellationObserved || ownedWork->cancellationObserved;
    failures.insert(failures.end(), ownedWork->failures.begin(), ownedWork->failures.end());
    bool unsafe = !failures.empty();
    bool containedFailure = false;
    std::vector<RunFailure> attemptCleanupFailures;
    std::map<std::pair<std::filesystem::path, MutationKind>, MutationSummary> grouped;
    const auto account = [&](const std::filesystem::path& root, MutationKind kind,
                             execution::MutationState mutation, bool succeeded, bool safe) {
        unsafe = unsafe || !safe || mutation == execution::MutationState::PartialOrUnknown;
        containedFailure = containedFailure || !succeeded;
        if (mutation == execution::MutationState::None) return;
        auto entry = grouped.try_emplace(std::pair{root, kind}, MutationSummary{root, kind}).first;
        if (mutation == execution::MutationState::Committed) ++entry->second.committed;
        else ++entry->second.partialOrUnknown;
    };
    for (const auto& attempt : ownedWork->assetAttempts) {
        account(attempt.modRoot, MutationKind::AssetProcessing, attempt.result.mutationState(),
                attempt.result.succeeded(), attempt.result.safeToContinue());
        const auto errors = attempt.result.cleanupFailures();
        attemptCleanupFailures.insert(attemptCleanupFailures.end(), errors.begin(), errors.end());
    }
    for (const auto& attempt : ownedWork->archiveAttempts)
        account(attempt.modRoot, MutationKind::ArchiveExtraction, attempt.mutation,
                attempt.succeeded(), attempt.safeToContinue);
    for (const auto& finalization : ownedWork->finalizations) {
        cancellationObserved = cancellationObserved || finalization.cancelled;
        unsafe = unsafe || !finalization.safeToContinue;
        containedFailure = containedFailure || finalization.failure.has_value();
        for (const auto& attempt : finalization.attempts)
            account(attempt.modRoot, MutationKind::ArchiveFinalization, attempt.mutation,
                    attempt.succeeded(), attempt.safeToContinue);
    }
    if (unsafe) outcome = RunOutcome::Failed;
    else if (containedFailure && outcome == RunOutcome::Succeeded)
        outcome = RunOutcome::CompletedWithFailures;
    std::vector<MutationSummary> summaries;
    for (auto& [key, summary] : grouped) summaries.push_back(std::move(summary));
    // Attempt-local cleanup precedes the run's final Safety Cleanup pass.
    cleanupFailures.insert(cleanupFailures.begin(), attemptCleanupFailures.begin(),
                           attemptCleanupFailures.end());
    // Work safety is authoritative. Cleanup cannot replace a Failed or Cancelled primary cause,
    // and cancellation observed during cleanup still wins over cleanup errors.
    if (outcome != RunOutcome::Failed) {
        if (cancellationObserved || outcome == RunOutcome::Cancelled) {
            outcome = RunOutcome::Cancelled;
        } else if (std::any_of(cleanupFailures.begin(), cleanupFailures.end(),
                               [](const RunFailure& failure) {
                                   return failure.code() == RunFailureCode::SafetyCleanupServiceFailed;
                               })) {
            // An unexpected service exception cannot establish that all artifacts were attempted.
            outcome = RunOutcome::Failed;
        } else if (outcome == RunOutcome::Succeeded && !cleanupFailures.empty()) {
            outcome = RunOutcome::CompletedWithFailures;
        }
    }
    return OptimizationRunResult(outcome, finalPhase, std::move(phases), std::move(runId),
                                 std::move(failures),
                                 preparation ? std::make_shared<const RunPreparation>(*preparation)
                                             : nullptr,
                                 std::move(cleanupFailures),
                                 cancellationObserved, std::move(ownedWork), std::move(summaries));
}

std::size_t OptimizationRunResult::skippedAssetCount(routing::SkipReason reason) const noexcept {
    const auto count = _work->skippedArchiveCounts.find(reason);
    return (_work->ledger ? _work->ledger->skippedAssetCount(reason) : 0) +
           (count == _work->skippedArchiveCounts.end() ? 0 : count->second);
}

RunOutcome OptimizationRunResult::outcome() const noexcept { return _outcome; }

RunPhase OptimizationRunResult::finalPhase() const noexcept { return _finalPhase; }

std::span<const RunPhaseRecord> OptimizationRunResult::phases() const noexcept { return _phases; }

const RunPhaseRecord* OptimizationRunResult::phase(const RunPhase phase) const noexcept {
    const auto record =
        std::find_if(_phases.begin(), _phases.end(),
                     [phase](const auto& candidate) { return candidate.phase() == phase; });
    return record == _phases.end() ? nullptr : &*record;
}
}  // namespace cao::run
