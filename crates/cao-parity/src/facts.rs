//! The raw `RunFacts` model both sides of a case are turned into (#472).
//!
//! The oracle parser fills it from the C++ CLI's `EVENT:` stream; the Rust
//! driver fills it in-process from typed Run Events and the final Run Evidence
//! and writes it as JSON. Raw facts still carry the Run ID, sequence numbers,
//! absolute paths and message text; [`crate::normalise`] removes what differs
//! legitimately between the two builds.
//!
//! The enums here are the harness's own vocabulary. They carry no `#[repr]` or
//! discriminants tied to the C++ declaration order: the oracle's integers and
//! names reach them only through the hand-written tables in
//! [`crate::oracle::tables`].

use serde::{Deserialize, Serialize};

/// A Run Phase, the canonical lifecycle stage of the glossary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum RunPhase {
    Preparing,
    DiscoveringArchives,
    ExtractingArchives,
    BuildingEffectiveAssetTree,
    ProcessingAssets,
    ArchiveFinalization,
    SafetyCleanup,
}

/// The terminal Run Outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum RunOutcome {
    Succeeded,
    CompletedWithFailures,
    Cancelled,
    Failed,
}

/// Why a traversed Run Phase was inapplicable (a Phase Skip Reason).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum PhaseSkipReason {
    NoRequestedWork,
    DryRun,
}

/// The stable category of a Run Failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
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

/// Why Routing Policy excluded one recognized Asset (a Skip Reason).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum SkipReason {
    DisabledPhase,
    DisabledAssetKind,
    ExcludedAssetVariant,
}

/// Why the Optimization Run Service refused to start a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum StartError {
    MissingProfileIdentity,
    MissingModSelectionDirectory,
    ActiveRun,
}

/// The durable work kind a Committed Mutations Retained count belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum MutationKind {
    ArchiveExtraction,
    AssetProcessing,
    ArchiveFinalization,
}

/// Everything one side reported about one run request.
// One value exists per side of a case, so the variants' size gap costs nothing
// and boxing would only add noise to every match.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunFacts {
    /// The service refused the request synchronously; no run exists.
    StartError(StartError),
    /// The service started a run and reported its events and terminal result.
    Started(StartedRun),
}

/// The facts of a run the service started.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartedRun {
    /// The run's opaque identity, shared by every event.
    pub run_id: String,
    /// Every non-terminal Run Event in sequence order, including diagnostics
    /// published after the terminal event.
    pub events: Vec<RunEventFact>,
    /// The sealed terminal result.
    pub terminal: TerminalFacts,
}

/// One non-terminal Run Event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunEventFact {
    /// The event's position in the run's event stream, starting at 1.
    pub sequence: u64,
    pub payload: RunEventPayload,
}

/// What a non-terminal Run Event reported.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunEventPayload {
    /// A Run Phase record: started, skipped, or a progress update.
    Phase {
        phase: RunPhase,
        status: PhaseStatus,
    },
    /// A Run Diagnostic, which never changes the Run Outcome.
    Diagnostic {
        phase: RunPhase,
        detail: String,
        path: String,
    },
    /// A Run Failure as it was published.
    Failure {
        phase: RunPhase,
        code: RunFailureCode,
        detail: String,
        path: String,
    },
}

/// The state a Run Phase record reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhaseStatus {
    /// Executed, with no determinate progress yet.
    Indeterminate,
    /// Traversed but inapplicable.
    Skipped(PhaseSkipReason),
    /// Executed, with phase-local progress counters.
    Progress(Progress),
}

/// A phase's progress tuple.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Progress {
    pub completed: u64,
    pub total: u64,
    pub succeeded: u64,
    pub failed: u64,
}

/// The sealed terminal result of a started run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalFacts {
    pub outcome: RunOutcome,
    pub final_phase: RunPhase,
    pub cancellation_observed: bool,
    pub mod_roots: Vec<String>,
    /// The terminal result's Run Failures. They carry no phase or code; the
    /// normaliser takes those from the published `Failure` events and checks
    /// the two lists agree.
    pub run_failures: Vec<DetailedPath>,
    pub cleanup_failures: Vec<DetailedPath>,
    pub asset_failures: Vec<AssetFailure>,
    pub archive_failures: Vec<ArchiveFailure>,
    /// The detail of a phase-level Archive Finalization failure, if any.
    pub finalization_failure: Option<String>,
    pub committed_mutations: Vec<CommittedMutations>,
    pub archive_collisions: Vec<ArchiveCollision>,
    pub skipped_assets: Vec<SkippedAssets>,
}

/// A failure recorded as free text plus the path it concerns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetailedPath {
    pub detail: String,
    /// An absolute path, or empty when the failure concerns no path.
    pub path: String,
}

/// A failed Asset attempt (an Operation Failure).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetFailure {
    /// The Asset's execution path.
    pub path: String,
    /// The stable operation identifier, such as `load_texture`.
    pub operation: String,
    pub message: String,
    pub affected_path: String,
    pub service_detail: String,
}

/// A failed Archive extraction or finalization attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveFailure {
    pub archive_path: String,
    pub detail: String,
}

/// Retained effects for one Mod Root and work kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommittedMutations {
    pub mod_root: String,
    pub kind: MutationKind,
    pub committed: u64,
    pub partial_or_unknown: u64,
}

/// One canonical game-path collision within a Mod Root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveCollision {
    pub mod_root: String,
    /// The game path, relative and `/`-separated.
    pub game_path: String,
    pub winning_archive: String,
    pub loose_asset_wins: bool,
    /// The losing Archives, in high-to-low Archive Precedence.
    pub shadowed_archives: Vec<String>,
}

/// How many recognized Assets one Skip Reason excluded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedAssets {
    pub reason: SkipReason,
    pub count: u64,
}
