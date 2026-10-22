//! The Optimization Run: its lifecycle, evidence, executor, scheduling and service.
//!
//! Ported from the C++ `cao::run` module (`src/Run`). The public entry point is
//! [`OptimizationRunService`]; beneath it, [`RunExecutor`] is the synchronous,
//! deterministic seam (ADR-0001).

mod archives;
mod artifacts;
mod asset_attempt;
mod capacity;
mod evidence;
mod executor;
mod finalization;
mod lifecycle;
mod mod_selection;
mod preparation;
mod probes;
mod scheduler;
mod service;
mod source_pin;
mod staging;
mod toolset;

pub(crate) use archives::ArchiveExtractionPlan;
pub use archives::{
    ArchiveCollision, ArchiveExtractionFailure, ArchiveExtractionResult, SourceCleanup,
};
pub(crate) use artifacts::fingerprint;
pub use artifacts::{
    PublicationPolicy, PublicationReceipt, PublicationResult, PublicationState, PublicationTarget,
    Registration, StagedFile, TemporaryArtifactRegistry,
};
pub use asset_attempt::{
    AssetAttemptAdapter, AssetBackendFactory, BackendCreation, BackendCreationError,
};
pub use evidence::{ArchiveDiscoveryEvidence, RoutedAssetAttempt, RunEvidence, RunObservationSink};
pub use executor::{
    RunEvidenceInvariantPanic, RunExecutor, RunServices, SafetyCleanupService,
    collect_safety_cleanup_failures,
};
pub(crate) use finalization::ArchiveFinalizationAccount;
pub use finalization::{
    ArchiveFinalizationAttempt, ArchiveFinalizationFailure, ArchiveFinalizationMutation,
    ArchiveFinalizationMutationKind, ArchiveFinalizationResult, ArchiveFinalizationSettings,
};
pub use lifecycle::{
    CancellationToken, ModSelection, MutationKind, MutationSummary, OptimizationRunResult,
    PhaseSkipReason, RunDiagnostic, RunDiagnosticCode, RunEvent, RunEventPayload, RunFailure,
    RunFailureCode, RunId, RunOutcome, RunPhase, RunPhaseRecord, RunPhaseStatus, RunProgress,
    RunRequest, RunSnapshot, create_run_id,
};
pub use preparation::{
    ArchivePrecedence, RunConfigurationProvider, RunPreparation, SelectedProfile,
};
pub use probes::{
    ArchiveEntry, ArchiveMerge, ArchiveName, ArchiveNameKind, ArchiveNamingRules, ArchivePacker,
    ArchiveReader, PackedArchiveKind, PackedFile, PlannedArchive, VolumeProbe,
};
pub use scheduler::{
    InlineRunScheduler, RunScheduler, RunWork, ScheduledRunWorker, StandardRunScheduler,
};
pub use service::{OptimizationRunService, RunEventDispatcher, RunHandle, StartError};
pub use source_pin::{LoadingPluginPin, SourceDirectoryPins, SourceFilePin, SourcePinError};
pub use staging::{StagingError, is_staging_name};
pub use toolset::{
    ArchiveSeams, ArchiveWorkSettings, AssetAttemptPort, AssetInitializationCancelled, RunToolset,
};
// Only an Optimization Run may write Run Evidence, so the store's writing
// side stays inside the crate; the sealed `RunEvidence` is the public view.
pub(crate) use evidence::{ArchiveFinalizationHandle, AttemptPhaseHandle, MutableRunEvidence};

/// Extracts the human-readable message from a caught panic payload.
pub(crate) fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "a panic with a non-text payload".to_owned()
    }
}

/// Extracts the message of a caught panic, then discards its payload with
/// [`discard_panic`]. Every containment boundary uses this, so none of them
/// can be bypassed by a payload whose drop panics.
pub(crate) fn take_panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    let message = panic_message(payload.as_ref());
    discard_panic(payload);
    message
}

/// The Run Work Escaped detail of a panic that escaped onto the Run Worker
/// past every narrower containment boundary, logged to the Application Log
/// before it is returned. The payload is discarded as
/// [`take_panic_message`] discards it.
pub(crate) fn run_worker_panic_detail(payload: Box<dyn std::any::Any + Send>) -> String {
    let detail = format!(
        "The run's work escaped with a Run Worker panic: {}",
        take_panic_message(payload)
    );
    log::error!("{detail}");
    detail
}

/// Drops a caught panic payload without letting a payload whose own `Drop`
/// panics unwind out of the boundary that caught it.
///
/// The second panic's payload is leaked rather than dropped: it could panic
/// in the same way, and containment matters more than a few bytes.
fn discard_panic(payload: Box<dyn std::any::Any + Send>) {
    let dropped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(payload)));
    if let Err(nested) = dropped {
        std::mem::forget(nested);
    }
}
