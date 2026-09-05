#include "RunLifecycle.h"

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
                       ModSelection modSelection, std::vector<routing::RequestedWork> requestedWork)
    : _profileIdentity(std::move(profileIdentity)),
      _executionMode(executionMode),
      _modSelection(std::move(modSelection)),
      _requestedWork(std::move(requestedWork)) {}

RunRequest RunRequest::create(std::string profileIdentity,
                              const routing::ExecutionMode executionMode, ModSelection modSelection,
                              std::vector<routing::RequestedWork> requestedWork) {
    // Callers assemble work from independent GUI and CLI choices, so the request normalizes the
    // sequence into a closed set in enumeration order. Repeated runs then observe one order.
    std::sort(requestedWork.begin(), requestedWork.end());
    requestedWork.erase(std::unique(requestedWork.begin(), requestedWork.end()),
                        requestedWork.end());
    return RunRequest(std::move(profileIdentity), executionMode, std::move(modSelection),
                      std::move(requestedWork));
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

OptimizationRunResult::OptimizationRunResult(const RunOutcome outcome, const RunPhase finalPhase,
                                             std::vector<RunPhaseRecord> phases, RunId runId,
                                             std::vector<RunFailure> failures) noexcept
    : _runId(std::move(runId)), _outcome(outcome), _finalPhase(finalPhase),
      _phases(std::move(phases)), _failures(std::move(failures)) {}

OptimizationRunResult OptimizationRunResult::terminal(const RunOutcome outcome,
                                                      const RunPhase finalPhase,
                                                      std::vector<RunPhaseRecord> phases,
                                                      RunId runId,
                                                      std::vector<RunFailure> failures) {
    return OptimizationRunResult(outcome, finalPhase, std::move(phases), std::move(runId),
                                 std::move(failures));
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
