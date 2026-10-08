//! The Run lifecycle vocabulary: phases, progress, failures, requests, results and events.
//!
//! Ported from `src/Run/RunLifecycle.h`. Every type here is plain owned data,
//! so a terminal result or event stays readable after the run, its services
//! and its handle are gone.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::routing::{
    ExecutionMode, PolicyValidationError, RequestedWork, RoutingLedger, SkipReason,
};
use crate::run::{ArchivePrecedence, RoutedAssetAttempt, RunEvidence, RunPreparation};

/// The unique identity of one Optimization Run, carried by its events and terminal result.
pub type RunId = String;

/// Generates an identity distinct across runs, including separate application launches.
///
/// A random process nonce separates launches and a counter separates runs
/// within one process, as the C++ `createRunId` did. Staging names derive from
/// it (#491), so it stays filesystem-safe: decimal digits and hyphens only.
pub fn create_run_id() -> RunId {
    static NONCE: OnceLock<String> = OnceLock::new();
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nonce = NONCE.get_or_init(|| {
        let mut bytes = [0u8; 8];
        // An unavailable OS random source still leaves the counter unique within
        // this process; the time-derived fallback only separates launches.
        if getrandom::fill(&mut bytes).is_err() {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos() as u64)
                .unwrap_or_default();
            bytes = (nanos ^ u64::from(std::process::id())).to_le_bytes();
        }
        let high = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let low = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        format!("{high}-{low}-")
    });
    format!("{nonce}{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

/// The stable lifecycle stages every Optimization Run traverses, in traversal order.
///
/// This is distinct from [`crate::routing::RoutedAssetPhase`], which only
/// groups Routed Assets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RunPhase {
    Preparing,
    DiscoveringArchives,
    ExtractingArchives,
    BuildingEffectiveAssetTree,
    ProcessingAssets,
    ArchiveFinalization,
    SafetyCleanup,
}

impl RunPhase {
    /// The canonical Run Phase sequence. It is the contract, not a detail:
    /// adapters render it, skipped phases still appear in it, and Safety Cleanup
    /// always ends it.
    pub const SEQUENCE: [RunPhase; 7] = [
        RunPhase::Preparing,
        RunPhase::DiscoveringArchives,
        RunPhase::ExtractingArchives,
        RunPhase::BuildingEffectiveAssetTree,
        RunPhase::ProcessingAssets,
        RunPhase::ArchiveFinalization,
        RunPhase::SafetyCleanup,
    ];
}

/// Whether a traversed Run Phase performed its work or was reported inapplicable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RunPhaseStatus {
    Executed,
    Skipped,
}

/// Why a traversed Run Phase was inapplicable.
///
/// A reason must be a fact the run already knows when it skips the phase; it
/// never asserts the outcome of a phase that did not run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PhaseSkipReason {
    NoRequestedWork,
    DryRun,
}

/// The terminal classification of one Optimization Run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RunOutcome {
    Succeeded,
    CompletedWithFailures,
    Cancelled,
    Failed,
}

/// Informational categories that never affect the Run Outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RunDiagnosticCode {
    ObserverFailed,
    DispatcherFailed,
    IgnoredModExcluded,
    SeparatorModExcluded,
    LinkedEntryExcluded,
}

/// An informational observation that never determines the Run Outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunDiagnostic {
    pub code: RunDiagnosticCode,
    /// The run's phase when the diagnostic was recorded.
    pub phase: RunPhase,
    pub detail: String,
    /// The affected entry, or empty for observations unrelated to a path.
    pub path: PathBuf,
}

impl RunDiagnostic {
    /// A diagnostic unrelated to any path.
    pub fn new(code: RunDiagnosticCode, phase: RunPhase, detail: impl Into<String>) -> Self {
        Self {
            code,
            phase,
            detail: detail.into(),
            path: PathBuf::new(),
        }
    }

    /// Attaches the affected entry.
    pub fn with_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.path = path.into();
        self
    }
}

/// Stable failures reachable at scheduling, preparation and execution boundaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RunFailureCode {
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
    StagingRecoveryFailed,
    ArchiveOrderMissing,
    ArchiveOrderExtra,
    ArchiveOrderDuplicate,
    ArchiveOrderOutsideRoot,
    ArchiveUnreadable,
    ArchiveEntryInvalid,
    ArchiveInsufficientCapacity,
    WorkServiceFailed,
}

/// A retained failure of run preparation or orchestration.
///
/// Distinct from an Operation Failure attached to one attempt, and from a Safety
/// Cleanup failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunFailure {
    pub code: RunFailureCode,
    /// The phase in which the run failed.
    pub phase: RunPhase,
    pub detail: String,
    /// Every policy conflict in compiler order; empty for other codes.
    pub policy_conflicts: Vec<PolicyValidationError>,
    /// The affected path, or empty for failures unrelated to one artifact.
    pub path: PathBuf,
}

impl RunFailure {
    /// A failure unrelated to any path or policy conflict.
    pub fn new(code: RunFailureCode, phase: RunPhase, detail: impl Into<String>) -> Self {
        Self {
            code,
            phase,
            detail: detail.into(),
            policy_conflicts: Vec::new(),
            path: PathBuf::new(),
        }
    }

    /// Attaches the affected path.
    pub fn with_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.path = path.into();
        self
    }

    /// Attaches the policy conflicts behind a `PolicyConflict` failure.
    pub fn with_policy_conflicts(mut self, conflicts: Vec<PolicyValidationError>) -> Self {
        self.policy_conflicts = conflicts;
        self
    }
}

/// The phase-local account of determinate work attempted during one Run Phase.
///
/// The total is fixed when the phase begins, and completed is always succeeded
/// plus failed, so unattempted work never advances the account. Indeterminate
/// and skipped phases have no progress at all rather than an invented total.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RunProgress {
    total: usize,
    succeeded: usize,
    failed: usize,
}

impl RunProgress {
    /// A determinate account against its immutable total.
    pub fn determinate(total: usize, succeeded: usize, failed: usize) -> Self {
        Self {
            total,
            succeeded,
            failed,
        }
    }

    /// The attempts the phase planned before it began.
    pub fn total(&self) -> usize {
        self.total
    }

    /// The attempts that finished without an Operation Failure.
    pub fn succeeded(&self) -> usize {
        self.succeeded
    }

    /// The attempts that finished with an Operation Failure.
    pub fn failed(&self) -> usize {
        self.failed
    }

    /// Completed attempts: always succeeded plus failed.
    pub fn completed(&self) -> usize {
        self.succeeded + self.failed
    }
}

/// One traversed Run Phase with its status, skip reason and progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RunPhaseRecord {
    phase: RunPhase,
    status: RunPhaseStatus,
    skip_reason: Option<PhaseSkipReason>,
    progress: Option<RunProgress>,
}

impl RunPhaseRecord {
    /// An executed phase and the determinate progress it accounted, if any.
    pub fn executed(phase: RunPhase, progress: Option<RunProgress>) -> Self {
        Self {
            phase,
            status: RunPhaseStatus::Executed,
            skip_reason: None,
            progress,
        }
    }

    /// An inapplicable phase. It never carries progress, so it cannot contribute
    /// an invented total.
    pub fn skipped(phase: RunPhase, reason: PhaseSkipReason) -> Self {
        Self {
            phase,
            status: RunPhaseStatus::Skipped,
            skip_reason: Some(reason),
            progress: None,
        }
    }

    pub fn phase(&self) -> RunPhase {
        self.phase
    }

    pub fn status(&self) -> RunPhaseStatus {
        self.status
    }

    /// Why the phase was skipped, or `None` for an executed phase.
    pub fn skip_reason(&self) -> Option<PhaseSkipReason> {
        self.skip_reason
    }

    /// Determinate progress; indeterminate and skipped phases have none.
    pub fn progress(&self) -> Option<RunProgress> {
        self.progress
    }
}

/// A copy of a run's published state, taken under the run's lock.
///
/// State is published before its event is dispatched, so a dispatcher reading a
/// snapshot sees at least the state its event describes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSnapshot {
    pub run_id: RunId,
    /// The latest published phase, including Safety Cleanup.
    pub phase: RunPhase,
    /// Phase-local progress; skipped and indeterminate phases have none.
    pub progress: Option<RunProgress>,
    pub cancellation_requested: bool,
    /// Recorded diagnostics, including presentation failures after terminal commit.
    pub diagnostic_count: usize,
    /// Run Failures published so far.
    pub failure_count: usize,
    /// `None` until the terminal result is committed.
    pub outcome: Option<RunOutcome>,
}

/// The request to process one Mod Root, or the child Mod Roots of a mods directory.
///
/// It is intent only: Preparing resolves it into the canonical Mod Roots the
/// run processes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModSelection {
    SingleModRoot(PathBuf),
    ChildModRoots(PathBuf),
}

impl ModSelection {
    /// The selected directory: the Mod Root itself, or the mods directory holding them.
    pub fn directory(&self) -> &Path {
        match self {
            Self::SingleModRoot(directory) | Self::ChildModRoots(directory) => directory,
        }
    }
}

/// The immutable intent used to start one Optimization Run.
///
/// It holds the profile identity, execution mode, Mod Selection, Archive
/// Precedence intent and the closed set of requested work; Preparing loads the
/// profile's facts from the identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRequest {
    profile_identity: String,
    execution_mode: ExecutionMode,
    mod_selection: ModSelection,
    requested_work: Vec<RequestedWork>,
    archive_precedence: ArchivePrecedence,
}

impl RunRequest {
    /// Owns one request's intent with deterministic Archive Precedence.
    ///
    /// Adapters assemble work from independent choices, so the request keeps
    /// each choice once in enumeration order and repeated runs see one order.
    pub fn new(
        profile_identity: impl Into<String>,
        execution_mode: ExecutionMode,
        mod_selection: ModSelection,
        mut requested_work: Vec<RequestedWork>,
    ) -> Self {
        requested_work.sort();
        requested_work.dedup();
        Self {
            profile_identity: profile_identity.into(),
            execution_mode,
            mod_selection,
            requested_work,
            archive_precedence: ArchivePrecedence::DeterministicDiscovery,
        }
    }

    /// Replaces the Archive Precedence intent; discovery validates it later.
    pub fn with_archive_precedence(mut self, archive_precedence: ArchivePrecedence) -> Self {
        self.archive_precedence = archive_precedence;
        self
    }

    pub fn profile_identity(&self) -> &str {
        &self.profile_identity
    }

    pub fn execution_mode(&self) -> ExecutionMode {
        self.execution_mode
    }

    pub fn mod_selection(&self) -> &ModSelection {
        &self.mod_selection
    }

    /// The Archive ordering intent; discovery checks it against enabled Archives.
    pub fn archive_precedence(&self) -> &ArchivePrecedence {
        &self.archive_precedence
    }

    /// The deduplicated requested work in enumeration order.
    pub fn requested_work(&self) -> &[RequestedWork] {
        &self.requested_work
    }

    /// Reports whether one work choice was requested.
    pub fn requests(&self, work: RequestedWork) -> bool {
        self.requested_work.contains(&work)
    }

    /// Reports whether the request selects any work at all.
    pub fn has_requested_work(&self) -> bool {
        !self.requested_work.is_empty()
    }
}

/// The durable operation whose attempts contribute to a Mod Root's mutation account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MutationKind {
    ArchiveExtraction,
    AssetProcessing,
    ArchiveFinalization,
}

/// Retained effects per Mod Root and work kind. Uncertain effects never count as committed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MutationSummary {
    pub mod_root: PathBuf,
    pub kind: MutationKind,
    pub committed: usize,
    pub partial_or_unknown: usize,
}

/// The immutable, self-contained terminal result of one Optimization Run.
///
/// It owns everything it exposes, so it outlives the executor, the services and
/// the request. It retains the outcome the Run Executor chose; evidence never
/// reclassifies it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptimizationRunResult {
    run_id: RunId,
    outcome: RunOutcome,
    final_phase: RunPhase,
    evidence: RunEvidence,
}

impl OptimizationRunResult {
    /// Combines the executor's chosen outcome with sealed evidence.
    pub fn terminal(
        outcome: RunOutcome,
        final_phase: RunPhase,
        evidence: RunEvidence,
        run_id: RunId,
    ) -> Self {
        Self {
            run_id,
            outcome,
            final_phase,
            evidence,
        }
    }

    /// The identity shared with this run's events.
    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    pub fn outcome(&self) -> RunOutcome {
        self.outcome
    }

    /// The furthest work phase the run reached. Safety Cleanup is excluded: it
    /// runs on every terminal path and so describes no progress.
    pub fn final_phase(&self) -> RunPhase {
        self.final_phase
    }

    /// The sealed factual record.
    pub fn evidence(&self) -> &RunEvidence {
        &self.evidence
    }

    /// The traversed phases in canonical order, always ending at Safety Cleanup.
    ///
    /// A run that stops early records only the phases it traversed; later ones
    /// are absent rather than skipped, because the run knows no reason they
    /// were inapplicable.
    pub fn phases(&self) -> &[RunPhaseRecord] {
        self.evidence.phases()
    }

    /// The record of one phase, or `None` when the run never reached it.
    pub fn phase(&self, phase: RunPhase) -> Option<&RunPhaseRecord> {
        self.evidence.phase(phase)
    }

    /// Diagnostics accepted before the terminal result was built.
    pub fn diagnostics(&self) -> &[RunDiagnostic] {
        self.evidence.diagnostics()
    }

    /// Run Failures in observation order, apart from Operation Failures.
    pub fn failures(&self) -> &[RunFailure] {
        self.evidence.failures()
    }

    /// Attempt-local and final Safety Cleanup failures.
    pub fn cleanup_failures(&self) -> &[RunFailure] {
        self.evidence.cleanup_failures()
    }

    /// The resolved Mod Roots; empty when Preparing did not resolve any.
    pub fn mod_roots(&self) -> &[PathBuf] {
        self.evidence
            .preparation()
            .map(RunPreparation::mod_roots)
            .unwrap_or_default()
    }

    /// Successful Preparing facts, or `None` when Preparing did not complete.
    pub fn preparation(&self) -> Option<&RunPreparation> {
        self.evidence.preparation()
    }

    /// The definitive routing, or `None` when routing did not complete.
    pub fn routing_ledger(&self) -> Option<&RoutingLedger> {
        self.evidence.routing_ledger()
    }

    /// Completed Asset attempts in execution order.
    pub fn asset_attempts(&self) -> &[RoutedAssetAttempt] {
        self.evidence.asset_attempts()
    }

    /// Recognized-Asset exclusions, including discovery's skipped Archives.
    pub fn skipped_asset_count(&self, reason: SkipReason) -> usize {
        self.evidence.skipped_asset_count(reason)
    }

    /// Mutation counts ordered by Mod Root and kind.
    pub fn mutation_summaries(&self) -> &[MutationSummary] {
        self.evidence.mutation_summaries()
    }

    /// Whether cancellation was observed before classification, even when Failed won.
    pub fn cancellation_observed(&self) -> bool {
        self.evidence.cancellation_observed()
    }
}

/// What one Run Event publishes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunEventPayload {
    Phase(RunPhaseRecord),
    Diagnostic(RunDiagnostic),
    Failure(RunFailure),
    /// The one terminal result, shared with the Run Handle.
    Terminal(Arc<OptimizationRunResult>),
}

/// One immutable, ordered publication from an Optimization Run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunEvent {
    pub run_id: RunId,
    /// The event's position in this run's history, starting at one.
    pub sequence: u64,
    pub payload: RunEventPayload,
}

/// The cooperative cancellation flag of one run, checked between Assets.
///
/// Cloning shares the flag. Requesting cancellation never interrupts work in
/// progress; an atomic attempt or Safety Cleanup always finishes.
#[derive(Debug, Clone, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    /// A token that has not been cancelled.
    pub fn new() -> Self {
        Self::default()
    }

    /// Requests cancellation; idempotent.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    /// Reports whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}
