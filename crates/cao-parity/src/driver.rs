//! The Rust driver behind `cao-parity run` (#472).
//!
//! It fills `cao-profiles`' options model from a case's spec, as the GUI fills
//! it from its widgets, hands that model to the composition root, and turns the
//! run's typed Run Events and final Run Evidence into raw [`RunFacts`]. It never
//! builds a `RunRequest` itself, so option validation and profile-forced choices
//! are compared with the oracle's too.
//!
//! The raw facts keep absolute paths, the Run ID and message text; the shared
//! normaliser removes what may differ between the builds.

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use cao_core::routing::SkipReason as CoreSkipReason;
use cao_core::run::{
    MutationKind as CoreMutationKind, OptimizationRunResult, PhaseSkipReason as CorePhaseSkip,
    RunEvent, RunEventPayload as CorePayload, RunFailure, RunFailureCode as CoreFailureCode,
    RunOutcome as CoreOutcome, RunPhase as CorePhase, RunPhaseRecord, StartError as CoreStartError,
};
use cao_optimizers::composition::ApplicationRun;
use cao_profiles::{OptimizationMode, Options, Profiles};

use crate::HarnessError;
use crate::case::{CaseSpec, ModSelection, selected_folder};
use crate::facts::{
    ArchiveCollision, ArchiveFailure, AssetFailure, CommittedMutations, DetailedPath, MutationKind,
    PhaseSkipReason, PhaseStatus, Progress, RunEventFact, RunEventPayload, RunFacts,
    RunFailureCode, RunOutcome, RunPhase, SkipReason, SkippedAssets, StartError, StartedRun,
    TerminalFacts,
};

/// Fills the options model for `spec` in the install at `app_dir`.
///
/// Like the GUI, it starts from the profile's `settings.ini` and then sets every
/// option a widget controls. Options no widget sets, such as debug logging,
/// keep the file's values. The folder is the spec's selection under `app_dir`.
///
/// # Errors
/// [`HarnessError::InvalidCase`] for a spec the builds cannot express, and
/// [`HarnessError::Driver`] when `settings.ini` cannot be read.
pub fn options(spec: &CaseSpec, app_dir: &Path) -> Result<Options, HarnessError> {
    let folder = selected_folder(spec, app_dir)?;
    let mut options = Profiles::new(app_dir)
        .open(&spec.profile)
        .load_options(&Options::default())
        .map_err(|error| HarnessError::Driver(error.to_string()))?;

    options.dry_run = spec.dry_run;
    options.mode = match spec.mod_selection {
        ModSelection::OneMod { .. } => OptimizationMode::SingleMod,
        ModSelection::SeveralMods { .. } => OptimizationMode::SeveralMods,
    };
    options.user_path = folder.to_string_lossy().into_owned();

    let textures = &spec.textures;
    options.textures_necessary = textures.necessary;
    options.textures_compress = textures.compress;
    options.textures_mipmaps = textures.mipmaps;
    options.textures_resize_ratio = textures.resize_by_ratio;
    options.textures_target_width_ratio = textures.ratio_width;
    options.textures_target_height_ratio = textures.ratio_height;
    options.textures_resize_size = textures.resize_by_size;
    options.textures_target_width = textures.target_width;
    options.textures_target_height = textures.target_height;

    options.meshes_optimization_level = i32::from(spec.meshes.level);
    options.meshes_headparts = spec.meshes.headparts;
    options.meshes_resave = spec.meshes.resave;
    options.animations_optimization = spec.animations;

    let archives = &spec.archives;
    options.bsa_extract = archives.extract;
    options.bsa_create = archives.create;
    options.bsa_delete_backup = archives.delete_backup;
    options.bsa_compress = archives.compress;
    options.bsa_create_dummies = archives.create_dummies;
    options.bsa_merge_incompressible = archives.merge_incompressible;
    options.bsa_merge_textures = archives.merge_textures;
    options.bsa_delete_source = archives.delete_sources;
    Ok(options)
}

/// Runs `spec` through the composition root in the install at `app_dir` and
/// returns its raw facts.
///
/// A Start Error is a fact, not a failure. The run's events are collected on
/// the Run Worker in sequence order; the terminal result is read once the run
/// has committed it.
///
/// # Errors
/// [`HarnessError`] when the spec cannot be turned into a run.
pub fn drive(spec: &CaseSpec, app_dir: &Path) -> Result<RunFacts, HarnessError> {
    let options = options(spec, app_dir)?;
    let run = ApplicationRun::new(app_dir, &spec.profile, &options)
        .map_err(|error| HarnessError::Driver(error.to_string()))?;
    let events = Arc::new(Mutex::new(Vec::new()));
    let collected = Arc::clone(&events);
    let dispatcher = Box::new(move |event: RunEvent| lock(&collected).push(event));
    let handle = match run.start(Some(dispatcher)) {
        Ok(handle) => handle,
        Err(error) => return Ok(RunFacts::StartError(start_error(error))),
    };
    let result = handle.wait();
    let events = lock(&events);
    Ok(started_facts(&events, &result))
}

/// Locks the event list, recovering it if a panicking holder poisoned it; each
/// holder only appends whole events, so the list stays consistent.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Builds the facts of a started run from its events and terminal result.
///
/// The terminal event itself is left out: its content is the terminal result.
pub fn started_facts(events: &[RunEvent], result: &OptimizationRunResult) -> RunFacts {
    let events = events
        .iter()
        .filter_map(|event| {
            let payload = match &event.payload {
                CorePayload::Phase(record) => RunEventPayload::Phase {
                    phase: phase(record.phase()),
                    status: phase_status(record),
                },
                CorePayload::Diagnostic(diagnostic) => RunEventPayload::Diagnostic {
                    phase: phase(diagnostic.phase),
                    detail: diagnostic.detail.clone(),
                    path: text(&diagnostic.path),
                },
                CorePayload::Failure(failure) => RunEventPayload::Failure {
                    phase: phase(failure.phase),
                    code: failure_code(failure.code),
                    detail: failure.detail.clone(),
                    path: text(&failure.path),
                },
                CorePayload::Terminal(_) => return None,
            };
            Some(RunEventFact {
                sequence: event.sequence,
                payload,
            })
        })
        .collect();
    RunFacts::Started(StartedRun {
        run_id: result.run_id().to_owned(),
        events,
        terminal: terminal(result),
    })
}

/// The terminal facts, in the shape the oracle's terminal event reports them.
fn terminal(result: &OptimizationRunResult) -> TerminalFacts {
    let detailed = |failures: &[RunFailure]| {
        failures
            .iter()
            .map(|failure| DetailedPath {
                detail: failure.detail.clone(),
                path: text(&failure.path),
            })
            .collect()
    };
    TerminalFacts {
        outcome: outcome(result.outcome()),
        final_phase: phase(result.final_phase()),
        cancellation_observed: result.cancellation_observed(),
        mod_roots: result.mod_roots().iter().map(|root| text(root)).collect(),
        run_failures: detailed(result.failures()),
        cleanup_failures: detailed(result.cleanup_failures()),
        asset_failures: result
            .asset_attempts()
            .iter()
            .filter(|attempt| !attempt.result.succeeded())
            .map(|attempt| AssetFailure {
                path: text(attempt.asset.execution_path()),
                operation: attempt.result.operation().to_owned(),
                message: attempt.result.message().to_owned(),
                affected_path: text(attempt.result.affected_path()),
                service_detail: attempt.result.service_detail().to_owned(),
            })
            .collect(),
        // Failed extraction attempts, as the oracle prints them. Archive
        // Finalization is not ported yet (#498), so it adds none.
        archive_failures: result
            .archive_extraction_attempts()
            .iter()
            .filter(|attempt| !attempt.succeeded())
            .map(|attempt| ArchiveFailure {
                archive_path: text(&attempt.archive_path),
                detail: attempt.detail.clone(),
            })
            .collect(),
        finalization_failure: None,
        committed_mutations: result
            .mutation_summaries()
            .iter()
            .map(|summary| CommittedMutations {
                mod_root: text(&summary.mod_root),
                kind: mutation_kind(summary.kind),
                committed: summary.committed as u64,
                partial_or_unknown: summary.partial_or_unknown as u64,
            })
            .collect(),
        archive_collisions: result
            .archive_collisions()
            .iter()
            .map(|collision| ArchiveCollision {
                mod_root: text(&collision.mod_root),
                game_path: text(&collision.game_path),
                winning_archive: text(&collision.winning_archive),
                loose_asset_wins: collision.loose_asset_wins,
                shadowed_archives: collision
                    .shadowed_archives
                    .iter()
                    .map(|archive| text(archive))
                    .collect(),
            })
            .collect(),
        // Only non-zero counts, as the oracle prints them.
        skipped_assets: CoreSkipReason::ALL
            .into_iter()
            .filter_map(|reason| {
                let count = result.skipped_asset_count(reason);
                (count != 0).then(|| SkippedAssets {
                    reason: skip_reason(reason),
                    count: count as u64,
                })
            })
            .collect(),
    }
}

/// A path as the facts carry it; the normaliser makes it comparable.
fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn phase_status(record: &RunPhaseRecord) -> PhaseStatus {
    if let Some(progress) = record.progress() {
        return PhaseStatus::Progress(Progress {
            completed: progress.completed() as u64,
            total: progress.total() as u64,
            succeeded: progress.succeeded() as u64,
            failed: progress.failed() as u64,
        });
    }
    // A skipped record always carries its reason; `RunPhaseRecord::skipped`
    // is the only way to make one.
    match record.skip_reason() {
        Some(reason) => PhaseStatus::Skipped(phase_skip_reason(reason)),
        None => PhaseStatus::Indeterminate,
    }
}

// The tables below map `cao-core`'s domain enums onto the harness's own
// vocabulary. They are exhaustive matches, so a new core variant fails to
// compile here instead of reaching a comparison unmapped.

fn phase(phase: CorePhase) -> RunPhase {
    match phase {
        CorePhase::Preparing => RunPhase::Preparing,
        CorePhase::DiscoveringArchives => RunPhase::DiscoveringArchives,
        CorePhase::ExtractingArchives => RunPhase::ExtractingArchives,
        CorePhase::BuildingEffectiveAssetTree => RunPhase::BuildingEffectiveAssetTree,
        CorePhase::ProcessingAssets => RunPhase::ProcessingAssets,
        CorePhase::ArchiveFinalization => RunPhase::ArchiveFinalization,
        CorePhase::SafetyCleanup => RunPhase::SafetyCleanup,
    }
}

fn outcome(outcome: CoreOutcome) -> RunOutcome {
    match outcome {
        CoreOutcome::Succeeded => RunOutcome::Succeeded,
        CoreOutcome::CompletedWithFailures => RunOutcome::CompletedWithFailures,
        CoreOutcome::Cancelled => RunOutcome::Cancelled,
        CoreOutcome::Failed => RunOutcome::Failed,
    }
}

fn failure_code(code: CoreFailureCode) -> RunFailureCode {
    match code {
        CoreFailureCode::SchedulingFailed => RunFailureCode::SchedulingFailed,
        CoreFailureCode::RequestedWorkUnavailable => RunFailureCode::RequestedWorkUnavailable,
        CoreFailureCode::PolicyConflict => RunFailureCode::PolicyConflict,
        CoreFailureCode::ConfigurationLoadingFailed => RunFailureCode::ConfigurationLoadingFailed,
        CoreFailureCode::ModSelectionResolutionFailed => {
            RunFailureCode::ModSelectionResolutionFailed
        }
        CoreFailureCode::ConflictingModRoots => RunFailureCode::ConflictingModRoots,
        CoreFailureCode::TemporaryArtifactCleanupFailed => {
            RunFailureCode::TemporaryArtifactCleanupFailed
        }
        CoreFailureCode::SafetyCleanupServiceFailed => RunFailureCode::SafetyCleanupServiceFailed,
        CoreFailureCode::StagingOwnershipUnverified => RunFailureCode::StagingOwnershipUnverified,
        CoreFailureCode::StagingActive => RunFailureCode::StagingActive,
        CoreFailureCode::StagingRecoveryFailed => RunFailureCode::StagingRecoveryFailed,
        CoreFailureCode::ArchiveOrderMissing => RunFailureCode::ArchiveOrderMissing,
        CoreFailureCode::ArchiveOrderExtra => RunFailureCode::ArchiveOrderExtra,
        CoreFailureCode::ArchiveOrderDuplicate => RunFailureCode::ArchiveOrderDuplicate,
        CoreFailureCode::ArchiveOrderOutsideRoot => RunFailureCode::ArchiveOrderOutsideRoot,
        CoreFailureCode::ArchiveUnreadable => RunFailureCode::ArchiveUnreadable,
        CoreFailureCode::ArchiveEntryInvalid => RunFailureCode::ArchiveEntryInvalid,
        CoreFailureCode::ArchiveInsufficientCapacity => RunFailureCode::ArchiveInsufficientCapacity,
        CoreFailureCode::WorkServiceFailed => RunFailureCode::WorkServiceFailed,
    }
}

fn phase_skip_reason(reason: CorePhaseSkip) -> PhaseSkipReason {
    match reason {
        CorePhaseSkip::NoRequestedWork => PhaseSkipReason::NoRequestedWork,
        CorePhaseSkip::DryRun => PhaseSkipReason::DryRun,
    }
}

fn skip_reason(reason: CoreSkipReason) -> SkipReason {
    match reason {
        CoreSkipReason::DisabledPhase => SkipReason::DisabledPhase,
        CoreSkipReason::DisabledAssetKind => SkipReason::DisabledAssetKind,
        CoreSkipReason::ExcludedAssetVariant => SkipReason::ExcludedAssetVariant,
    }
}

fn mutation_kind(kind: CoreMutationKind) -> MutationKind {
    match kind {
        CoreMutationKind::ArchiveExtraction => MutationKind::ArchiveExtraction,
        CoreMutationKind::AssetProcessing => MutationKind::AssetProcessing,
        CoreMutationKind::ArchiveFinalization => MutationKind::ArchiveFinalization,
    }
}

fn start_error(error: CoreStartError) -> StartError {
    match error {
        CoreStartError::MissingProfileIdentity => StartError::MissingProfileIdentity,
        CoreStartError::MissingModSelectionDirectory => StartError::MissingModSelectionDirectory,
        CoreStartError::ActiveRun => StartError::ActiveRun,
    }
}
