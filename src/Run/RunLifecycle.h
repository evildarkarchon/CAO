#pragma once

#include "AssetRouting/AssetRouter.h"
#include "Run/RunPreparation.h"

#include <cstddef>
#include <cstdint>
#include <filesystem>
#include <memory>
#include <optional>
#include <span>
#include <string>
#include <utility>
#include <variant>
#include <vector>

namespace cao::run {
/// An opaque identity retained by every observation and terminal result of a run.
using RunId = std::string;

/// Generates an identity distinct across runs, including separate application invocations.
[[nodiscard]] RunId createRunId();

/// The stable lifecycle stages every Optimization Run traverses, in traversal order.
///
/// This is the canonical Run Phase of the project glossary and is distinct from
/// `routing::RoutedAssetPhase`, which only groups Routed Assets into Archive or Loose Asset work.
enum class RunPhase {
    Preparing,
    DiscoveringArchives,
    ExtractingArchives,
    BuildingEffectiveAssetTree,
    ProcessingAssets,
    ArchiveFinalization,
    SafetyCleanup
};

/// Returns the canonical Run Phase sequence in traversal order.
[[nodiscard]] std::span<const RunPhase> runPhaseSequence() noexcept;

/// Whether a traversed Run Phase performed its work or was reported inapplicable.
enum class RunPhaseStatus { Executed, Skipped };

/// Stable structured reasons a traversed Run Phase was inapplicable to the run.
///
/// A reason must be a fact the run already knows when it skips the phase. It may never assert the
/// outcome of a phase that did not run: a Run that skipped discovery cannot claim that no Archives
/// were discovered. Later lifecycle slices add reasons as they add phases that can actually reach
/// a different conclusion.
///
/// This is distinct from `routing::SkipReason`, which explains why Routing Policy excludes one
/// recognized Asset. See "Phase Skip Reason" and "Skip Reason" in the project glossary.
enum class PhaseSkipReason { NoRequestedWork };

/// The terminal classification of one Optimization Run.
enum class RunOutcome { Succeeded, CompletedWithFailures, Cancelled, Failed };

/// Informational categories, including selection exclusions, that never affect Run Outcome.
enum class RunDiagnosticCode {
    ObserverFailed,
    DispatcherFailed,
    IgnoredModExcluded,
    SeparatorModExcluded,
    LinkedEntryExcluded
};

/// An owning informational observation, including presentation failures after terminal commit.
class RunDiagnostic final {
   public:
    /// Retains a stable category, the observed phase, and the boundary's human-readable detail.
    RunDiagnostic(RunDiagnosticCode code, RunPhase phase, std::string detail,
                  std::filesystem::path path = {})
        : _code(code), _phase(phase), _detail(std::move(detail)), _path(std::move(path)) {}

    /// Returns the stable reason for this observation.
    [[nodiscard]] RunDiagnosticCode code() const noexcept { return _code; }
    /// Returns the run's phase when the diagnostic was recorded, including late queued failures.
    [[nodiscard]] RunPhase phase() const noexcept { return _phase; }
    /// Borrows explanatory text for this value's lifetime.
    [[nodiscard]] const std::string& detail() const noexcept { return _detail; }

    /// Borrows the affected entry path, or an empty path for observations unrelated to a path.
    [[nodiscard]] const std::filesystem::path& path() const noexcept { return _path; }

   private:
    RunDiagnosticCode _code;
    RunPhase _phase;
    std::string _detail;
    std::filesystem::path _path;
};

/// Stable failures reachable at scheduling, preparation, and execution boundaries.
enum class RunFailureCode {
    SchedulingFailed,
    RequestedWorkUnavailable,
    PolicyConflict,
    ConfigurationLoadingFailed,
    ModSelectionResolutionFailed,
    ConflictingModRoots,
    TemporaryArtifactCleanupFailed,
    SafetyCleanupServiceFailed,
    StagingOwnershipUnverified,
    StagingActive,
    StagingRecoveryFailed
};

/// An owning run-level failure; Asset/Archive mutation failures belong to their service slices.
class RunFailure final {
   public:
    /// Records the failing boundary and its detail without retaining exception or service objects.
    RunFailure(RunFailureCode code, RunPhase phase, std::string detail,
               routing::PolicyValidationErrors policyConflicts = {},
               std::filesystem::path path = {})
        : _code(code),
          _phase(phase),
          _detail(std::move(detail)),
          _policyConflicts(std::move(policyConflicts)),
          _path(std::move(path)) {}

    /// Returns the stable unsuccessful scheduling or execution category.
    [[nodiscard]] RunFailureCode code() const noexcept { return _code; }
    /// Returns the phase in which the run failed.
    [[nodiscard]] RunPhase phase() const noexcept { return _phase; }
    /// Borrows explanatory text for this value's lifetime.
    [[nodiscard]] const std::string& detail() const noexcept { return _detail; }

    /// Borrows the affected path, or an empty path for failures unrelated to one artifact.
    [[nodiscard]] const std::filesystem::path& path() const noexcept { return _path; }

    /// Borrows all policy conflicts in compiler order; empty for other failure categories.
    [[nodiscard]] std::span<const routing::PolicyValidationError> policyConflicts() const noexcept {
        return _policyConflicts;
    }

   private:
    RunFailureCode _code;
    RunPhase _phase;
    std::string _detail;
    routing::PolicyValidationErrors _policyConflicts;
    std::filesystem::path _path;
};

/// The phase-local account of determinate work attempted during one Run Phase.
///
/// A determinate phase starts at zero against a total that is immutable once the phase begins.
/// Completed attempts are always succeeded plus failed attempts, so failed attempts advance the
/// account while unattempted work never does. Indeterminate and skipped phases have no progress
/// at all rather than an invented total.
///
/// This is the lifecycle-wide account and is distinct from `AssetRunProgress`, which the Asset Run
/// orchestrator reports per Routed Asset attempt until that orchestrator is deleted.
class RunProgress final {
   public:
    /// Records a determinate phase account against its immutable total.
    ///
    /// Attempt counts are supplied rather than advanced through mutators, so the account stays an
    /// immutable value. Passing only a total yields the zeroed account a phase starts with.
    [[nodiscard]] static RunProgress determinate(std::size_t total, std::size_t succeeded = 0,
                                                 std::size_t failed = 0) noexcept;

    /// Returns the immutable count of attempts the phase planned before it began.
    [[nodiscard]] std::size_t total() const noexcept;

    /// Returns the attempts that finished without an Operation Failure.
    [[nodiscard]] std::size_t succeeded() const noexcept;

    /// Returns the attempts that finished with an Operation Failure.
    [[nodiscard]] std::size_t failed() const noexcept;

    /// Returns completed attempts, always the sum of succeeded and failed attempts.
    [[nodiscard]] std::size_t completed() const noexcept;

   private:
    RunProgress(std::size_t total, std::size_t succeeded, std::size_t failed) noexcept;

    std::size_t _total{};
    std::size_t _succeeded{};
    std::size_t _failed{};
};

/// A lightweight immutable copy of authoritative state, published before its corresponding event.
class RunSnapshot final {
   public:
    /// Captures one synchronized observation without borrowing a worker's mutable state.
    RunSnapshot(RunId runId, RunPhase phase, std::optional<RunProgress> progress,
                bool cancellationRequested, std::size_t diagnosticCount, std::size_t failureCount,
                std::optional<RunOutcome> outcome)
        : _runId(std::move(runId)), _phase(phase), _progress(progress),
          _cancellationRequested(cancellationRequested), _diagnosticCount(diagnosticCount),
          _failureCount(failureCount), _outcome(outcome) {}

    /// Borrows the captured run identity for this snapshot's lifetime.
    [[nodiscard]] const RunId& runId() const noexcept { return _runId; }
    /// Returns the latest observed phase, including Safety Cleanup.
    [[nodiscard]] RunPhase phase() const noexcept { return _phase; }
    /// Borrows phase-local progress; skipped and indeterminate phases have no value.
    [[nodiscard]] const std::optional<RunProgress>& progress() const noexcept { return _progress; }
    /// Reports whether cooperative cancellation had been requested when this copy was captured.
    [[nodiscard]] bool cancellationRequested() const noexcept { return _cancellationRequested; }
    /// Counts recorded diagnostics, including presentation failures observed after terminal commit.
    [[nodiscard]] std::size_t diagnosticCount() const noexcept { return _diagnosticCount; }
    /// Counts run failures published before this snapshot was captured.
    [[nodiscard]] std::size_t failureCount() const noexcept { return _failureCount; }
    /// Returns no value until the immutable terminal result is committed.
    [[nodiscard]] std::optional<RunOutcome> outcome() const noexcept { return _outcome; }

   private:
    RunId _runId;
    RunPhase _phase;
    std::optional<RunProgress> _progress;
    bool _cancellationRequested;
    std::size_t _diagnosticCount;
    std::size_t _failureCount;
    std::optional<RunOutcome> _outcome;
};

/// One traversed Run Phase together with its status, skip reason, and phase-local progress.
class RunPhaseRecord final {
   public:
    /// Records one executed Run Phase and the determinate progress it accounted, if any.
    [[nodiscard]] static RunPhaseRecord executed(RunPhase phase,
                                                 std::optional<RunProgress> progress = {}) noexcept;

    /// Records one inapplicable Run Phase and the stable reason it was skipped.
    [[nodiscard]] static RunPhaseRecord skipped(RunPhase phase, PhaseSkipReason reason) noexcept;

    [[nodiscard]] RunPhase phase() const noexcept;

    [[nodiscard]] RunPhaseStatus status() const noexcept;

    /// Returns the structured reason a phase was skipped, or no value for an executed phase.
    [[nodiscard]] std::optional<PhaseSkipReason> skipReason() const noexcept;

    /// Returns determinate phase-local progress. Indeterminate and skipped phases have none.
    [[nodiscard]] const std::optional<RunProgress>& progress() const noexcept;

   private:
    RunPhaseRecord(RunPhase phase, RunPhaseStatus status, std::optional<PhaseSkipReason> skipReason,
                   std::optional<RunProgress> progress) noexcept;

    RunPhase _phase;
    RunPhaseStatus _status;
    std::optional<PhaseSkipReason> _skipReason;
    std::optional<RunProgress> _progress;
};

/// Whether a Mod Selection names one Mod Root or the mods directory holding several of them.
enum class ModSelectionKind { SingleModRoot, ChildModRoots };

/// The requested scope of mod directories for one Optimization Run.
///
/// A Mod Selection is user intent only. Preparing resolves it into the ordered, canonicalized,
/// non-overlapping Mod Roots the run actually processes.
class ModSelection final {
   public:
    /// Selects one Mod Root processed as a single Archive Precedence scope.
    [[nodiscard]] static ModSelection singleModRoot(std::filesystem::path root);

    /// Selects the immediate child Mod Roots beneath one selected mods directory.
    [[nodiscard]] static ModSelection childModRoots(std::filesystem::path modsDirectory);

    [[nodiscard]] ModSelectionKind kind() const noexcept;

    /// Returns the selected directory: the Mod Root itself, or the mods directory holding them.
    [[nodiscard]] const std::filesystem::path& directory() const noexcept;

   private:
    ModSelection(ModSelectionKind kind, std::filesystem::path directory);

    ModSelectionKind _kind;
    std::filesystem::path _directory;
};

/// The immutable user intent used to start one Optimization Run.
///
/// It owns profile identity, execution mode, Mod Selection, Archive Precedence intent, and the
/// closed set of requested work.
/// It deliberately holds no application singleton, mutable presentation state, or profile object;
/// Preparing loads profile facts from the identity recorded here.
///
/// Archive Precedence is intent only until discovery validates it against enabled Archives.
///
/// This is the canonical Run Request of the project glossary. It is distinct from
/// `routing::RoutingPolicyRequest`, which carries only the facts needed to compile one Routing
/// Policy.
class RunRequest final {
   public:
    /// Owns one request's intent, retaining each requested work choice once in enumeration order.
    [[nodiscard]] static RunRequest create(
        std::string profileIdentity, routing::ExecutionMode executionMode,
        ModSelection modSelection, std::vector<routing::RequestedWork> requestedWork,
        ArchivePrecedence archivePrecedence = ArchivePrecedence::deterministicDiscovery());

    [[nodiscard]] const std::string& profileIdentity() const noexcept;

    [[nodiscard]] routing::ExecutionMode executionMode() const noexcept;

    [[nodiscard]] const ModSelection& modSelection() const noexcept;

    /// Borrows the owned Archive ordering intent for this request's lifetime.
    [[nodiscard]] const ArchivePrecedence& archivePrecedence() const noexcept {
        return _archivePrecedence;
    }

    /// Returns the deduplicated requested work in enumeration order, so runs are reproducible.
    [[nodiscard]] std::span<const routing::RequestedWork> requestedWork() const noexcept;

    /// Reports whether one closed work choice was requested.
    [[nodiscard]] bool requests(routing::RequestedWork work) const noexcept;

    /// Reports whether the request selects any work at all.
    [[nodiscard]] bool hasRequestedWork() const noexcept;

   private:
    RunRequest(std::string profileIdentity, routing::ExecutionMode executionMode,
               ModSelection modSelection, std::vector<routing::RequestedWork> requestedWork,
               ArchivePrecedence archivePrecedence);

    std::string _profileIdentity;
    routing::ExecutionMode _executionMode;
    ModSelection _modSelection;
    std::vector<routing::RequestedWork> _requestedWork;
    ArchivePrecedence _archivePrecedence;
};

/// The immutable, self-contained terminal result of one Optimization Run.
///
/// It owns every value it exposes, so it stays readable after the Run Executor, its services, and
/// the originating Run Request have been destroyed.
class OptimizationRunResult final {
   public:
    /// Takes ownership of the traversed phase records once the run reaches its terminal state.
    ///
    /// The result exposes no mutator, so committing it here is what makes it immutable. It is a
    /// named public factory rather than a friendship because the Run Executor, and later the
    /// asynchronous Optimization Run service, both commit terminal results from libraries that
    /// link this one.
    [[nodiscard]] static OptimizationRunResult terminal(
        RunOutcome outcome, RunPhase finalPhase, std::vector<RunPhaseRecord> phases,
        RunId runId = createRunId(), std::vector<RunFailure> failures = {},
        std::shared_ptr<const RunPreparation> preparation = {},
        std::vector<RunFailure> cleanupFailures = {});

    /// Borrows owned preparation facts, or nullptr if preparation did not complete successfully.
    [[nodiscard]] const RunPreparation* preparation() const noexcept { return _preparation.get(); }

    /// Borrows the identity shared with this run's observations for the result's lifetime.
    [[nodiscard]] const RunId& runId() const noexcept { return _runId; }

    /// Returns primary/work failures in observation order; cleanupFailures retains cleanup errors.
    [[nodiscard]] std::span<const RunFailure> failures() const noexcept { return _failures; }

    /// Borrows all cleanup failures in attempted removal order, separately from the primary cause.
    [[nodiscard]] std::span<const RunFailure> cleanupFailures() const noexcept {
        return _cleanupFailures;
    }

    [[nodiscard]] RunOutcome outcome() const noexcept;

    /// Returns the furthest Run Phase the run reached in its work sequence.
    ///
    /// Safety Cleanup is excluded because it executes on every terminal path and so describes no
    /// progress through the lifecycle.
    [[nodiscard]] RunPhase finalPhase() const noexcept;

    /// Returns the traversed Run Phases in canonical order, always ending at Safety Cleanup.
    ///
    /// A run that stops early records only the phases it actually traversed. Phases after the stop
    /// are absent rather than reported as skipped, because the run never reached them and so knows
    /// no reason they were inapplicable; `finalPhase` identifies where it stopped.
    [[nodiscard]] std::span<const RunPhaseRecord> phases() const noexcept;

    /// Returns the record for one Run Phase, or nullptr when the run never traversed it.
    [[nodiscard]] const RunPhaseRecord* phase(RunPhase phase) const noexcept;

   private:
    OptimizationRunResult(RunOutcome outcome, RunPhase finalPhase,
                          std::vector<RunPhaseRecord> phases, RunId runId,
                          std::vector<RunFailure> failures,
                          std::shared_ptr<const RunPreparation> preparation,
                          std::vector<RunFailure> cleanupFailures) noexcept;

    RunId _runId;
    RunOutcome _outcome;
    RunPhase _finalPhase;
    std::vector<RunPhaseRecord> _phases;
    std::vector<RunFailure> _failures;
    std::shared_ptr<const RunPreparation> _preparation;
    std::vector<RunFailure> _cleanupFailures;
};

/// An owning immutable observation; copies keep terminal payloads alive independently of handles.
class RunEvent final {
   public:
    using Payload = std::variant<RunPhaseRecord, RunDiagnostic, RunFailure,
                                 std::shared_ptr<const OptimizationRunResult>>;

    /// Takes an already ordered observation; sequences start at one within each Run ID.
    RunEvent(RunId runId, std::uint64_t sequence, Payload payload)
        : _runId(std::move(runId)), _sequence(sequence), _payload(std::move(payload)) {}

    /// Borrows this observation's run identity for the event value's lifetime.
    [[nodiscard]] const RunId& runId() const noexcept { return _runId; }
    /// Returns its monotonically increasing position in this Run ID's history, starting at one.
    [[nodiscard]] std::uint64_t sequence() const noexcept { return _sequence; }
    /// Borrows the immutable payload; copying the event retains all referenced terminal data.
    [[nodiscard]] const Payload& payload() const noexcept { return _payload; }

   private:
    RunId _runId;
    std::uint64_t _sequence;
    Payload _payload;
};
}  // namespace cao::run
