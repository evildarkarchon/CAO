//! The Optimization Run: its lifecycle, evidence, executor, scheduling and service.
//!
//! Ported from the C++ `cao::run` module (`src/Run`). The public entry point is
//! [`OptimizationRunService`]; beneath it, [`RunExecutor`] is the synchronous,
//! deterministic seam (ADR-0001).

mod artifacts;
mod asset_run;
mod evidence;
mod executor;
mod lifecycle;
mod mod_selection;
mod preparation;
mod probes;
mod scheduler;
mod service;
mod staging;

pub use artifacts::{
    PublicationPolicy, PublicationReceipt, PublicationResult, PublicationState, PublicationTarget,
    Registration, StagedFile, TemporaryArtifactRegistry,
};
pub use asset_run::{
    AssetInitializationCancelled, AssetRunAdapters, AssetRunProgress, ExecuteAsset,
    FinalizeArchiveLifecycle, ReportPhase, RoutedAssetAttempt, execute_asset_run, is_staging_name,
};
pub use evidence::{
    ArchiveDiscoveryEvidence, MutableRunEvidence, RunEvidence, RunObservationSink, RunWorkEvidence,
};
pub use executor::{
    RunEvidenceInvariantPanic, RunExecutor, RunServices, RunWorkMilestones, RunWorkService,
    SafetyCleanupService, collect_safety_cleanup_failures,
};
pub use lifecycle::{
    CancellationToken, ModSelection, MutationKind, MutationSummary, OptimizationRunResult,
    PhaseSkipReason, RunDiagnostic, RunDiagnosticCode, RunEvent, RunEventPayload, RunFailure,
    RunFailureCode, RunId, RunOutcome, RunPhase, RunPhaseRecord, RunPhaseStatus, RunProgress,
    RunRequest, RunSnapshot, create_run_id,
};
pub use preparation::{
    ArchivePrecedence, RunConfiguration, RunConfigurationProvider, RunPreparation,
    SelectedProfileFacts,
};
pub use probes::{ArchiveEntry, ArchiveReader, CapacityProbe, VolumeIdentityProbe};
pub use scheduler::{
    InlineRunScheduler, RunScheduler, RunWork, ScheduledRunWorker, StandardRunScheduler,
};
pub use service::{OptimizationRunService, RunEventDispatcher, RunHandle, StartError};
pub use staging::StagingError;

pub(crate) use artifacts::fingerprint;

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
