//! Loose Asset runs through the Optimization Run Service, with a fake Asset
//! Execution Backend over real Mod Root directories.
//!
//! Scenarios that port a C++ test name its origin; the rest pin Rust-side
//! contracts from the spec (#476): panic containment, and the interim
//! behaviour of Apply work whose staging (#491) or Archive extraction (#496)
//! is not ported yet.

mod common;

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use cao_core::Error;
use cao_core::execution::{AssetExecutionFailure, AssetExecutionResult, MutationState};
use cao_core::routing::{ExecutionMode, RequestedWork, SkipReason};
use cao_core::run::{
    AssetInitializationCancelled, CancellationToken, InlineRunScheduler, MutationKind,
    OptimizationRunResult, OptimizationRunService, PhaseSkipReason, RunDiagnosticCode, RunEvent,
    RunEventPayload, RunFailureCode, RunOutcome, RunPhase, RunPhaseRecord, RunPhaseStatus,
    RunPreparation, RunProgress, RunWorkEvidence, RunWorkMilestones, RunWorkService,
};
use common::{
    BackendWork, EventLog, GatedScheduler, HandleSlot, ScriptedWork, reclaim, request, scratch_dir,
    serial, snapshot_tree, test_configuration, write_tree,
};

/// Runs one request to its terminal result on the inline scheduler.
fn run(
    work: Arc<dyn RunWorkService>,
    request: cao_core::run::RunRequest,
) -> (Arc<OptimizationRunResult>, EventLog) {
    let service = OptimizationRunService::with_scheduler(
        Arc::new(InlineRunScheduler),
        Some(test_configuration()),
        Some(work),
    );
    let events = EventLog::default();
    let handle = service
        .start(request, Some(events.dispatcher()))
        .expect("the run starts");
    (handle.wait(), events)
}

/// The canonical form of a scenario's Mod Root, as Preparing resolves it.
fn canonical(root: &Path) -> PathBuf {
    dunce::canonicalize(root).unwrap()
}

/// The file names of the completed attempts, in execution order.
fn attempted_names(result: &OptimizationRunResult) -> Vec<String> {
    result
        .asset_attempts()
        .iter()
        .map(|attempt| {
            attempt
                .asset
                .execution_path()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

/// Origin: AssetRunTests::dryRunLeavesCompleteModTreeUnchangedWhileEvaluatingLooseAssets.
#[test]
fn a_dry_run_evaluates_loose_textures_through_every_phase_without_touching_the_tree() {
    let _serial = serial();
    let root = scratch_dir("loose-dry-run-textures");
    write_tree(
        &root,
        &[
            "textures/armor/cuirass.dds",
            "textures/armor/cuirass_n.dds",
            "readme.txt",
        ],
    );
    let before = snapshot_tree(&root);
    let work = Arc::new(BackendWork::new());

    let (result, events) = run(
        work.clone(),
        request(
            ExecutionMode::DryRun,
            &root,
            &[RequestedWork::NativeTextureOptimization],
        ),
    );

    assert_eq!(result.outcome(), RunOutcome::Succeeded);
    assert_eq!(result.final_phase(), RunPhase::ArchiveFinalization);
    assert_eq!(
        result.phases(),
        [
            RunPhaseRecord::executed(RunPhase::Preparing, None),
            RunPhaseRecord::executed(RunPhase::DiscoveringArchives, None),
            RunPhaseRecord::skipped(RunPhase::ExtractingArchives, PhaseSkipReason::DryRun),
            RunPhaseRecord::executed(RunPhase::BuildingEffectiveAssetTree, None),
            RunPhaseRecord::executed(
                RunPhase::ProcessingAssets,
                Some(RunProgress::determinate(2, 2, 0))
            ),
            RunPhaseRecord::skipped(RunPhase::ArchiveFinalization, PhaseSkipReason::DryRun),
            RunPhaseRecord::executed(RunPhase::SafetyCleanup, None),
        ]
    );
    assert_eq!(attempted_names(&result), ["cuirass.dds", "cuirass_n.dds"]);
    assert!(
        result
            .asset_attempts()
            .iter()
            .all(|attempt| attempt.result.succeeded())
    );
    assert!(
        result.mutation_summaries().is_empty(),
        "a Dry Run never mutates"
    );
    assert_eq!(result.mod_roots(), [canonical(&root)]);
    assert_eq!(
        work.calls(),
        [
            "load_texture cuirass.dds",
            "optimize_texture cuirass.dds",
            "load_texture cuirass_n.dds",
            "optimize_texture cuirass_n.dds"
        ]
    );
    assert_eq!(
        snapshot_tree(&root),
        before,
        "the Mod Root is byte-identical after a Dry Run"
    );

    // Run Progress advances once per completed attempt against a fixed total.
    let progress: Vec<_> = events
        .events()
        .into_iter()
        .filter_map(|event| match event.payload {
            RunEventPayload::Phase(record) if record.phase() == RunPhase::ProcessingAssets => {
                record.progress()
            }
            _ => None,
        })
        .map(|progress| (progress.completed(), progress.total()))
        .collect();
    assert_eq!(progress, [(0, 2), (1, 2), (2, 2)]);
}

/// Origin: AssetRunTests::progressAndSkipSummaryExcludeNonWork and
/// dryRunAggregatesArchiveSkipsAndKeepsDirectoryUnsupportedPathsSilent.
#[test]
fn only_routed_assets_count_as_work_while_recognized_exclusions_are_counted_by_reason() {
    let _serial = serial();
    let root = scratch_dir("loose-skip-summary");
    write_tree(
        &root,
        &[
            "textures/routed.dds",
            "textures/excluded_variant.tga",
            "meshes/disabled_kind.nif",
            "archive.bsa",
            "notes/unsupported.txt",
        ],
    );

    let (result, _) = run(
        Arc::new(BackendWork::new()),
        request(
            ExecutionMode::DryRun,
            &root,
            &[RequestedWork::NativeTextureOptimization],
        ),
    );

    assert_eq!(result.outcome(), RunOutcome::Succeeded);
    assert_eq!(
        result.phase(RunPhase::ProcessingAssets).unwrap().progress(),
        Some(RunProgress::determinate(1, 1, 0))
    );
    assert_eq!(attempted_names(&result), ["routed.dds"]);
    assert_eq!(
        result.skipped_asset_count(SkipReason::ExcludedAssetVariant),
        1
    );
    assert_eq!(result.skipped_asset_count(SkipReason::DisabledAssetKind), 1);
    // A Dry Run never extracts, so the Archive is excluded by its phase.
    assert_eq!(result.skipped_asset_count(SkipReason::DisabledPhase), 1);
    let ledger = result.routing_ledger().expect("routing completed");
    assert_eq!(
        ledger.routed_assets().len(),
        1,
        "Archives never enter the Effective Asset Tree"
    );
}

/// Origin: AssetRunTests::executesOriginalLedgerAssetsInTargetOrder.
#[test]
fn assets_execute_by_target_texture_then_mesh_then_animation() {
    let _serial = serial();
    let root = scratch_dir("loose-target-order");
    write_tree(
        &root,
        &[
            "a/behavior.hkx",
            "b/model.nif",
            "c/skin.dds",
            "d/second.nif",
        ],
    );

    let (result, _) = run(
        Arc::new(BackendWork::new()),
        request(
            ExecutionMode::DryRun,
            &root,
            &[
                RequestedWork::NativeTextureOptimization,
                RequestedWork::StandardMeshOptimization,
                RequestedWork::AnimationOptimization,
            ],
        ),
    );

    assert_eq!(result.outcome(), RunOutcome::Succeeded);
    assert_eq!(
        attempted_names(&result),
        ["skin.dds", "model.nif", "second.nif", "behavior.hkx"]
    );
}

/// Origin: RunExecutorTests::processingAndEvidenceShareModRoot.
#[test]
fn each_attempt_is_attributed_to_the_canonical_mod_root() {
    let _serial = serial();
    let root = scratch_dir("loose-mod-root-attribution");
    write_tree(&root, &["textures/deep/nested/path/skin.dds"]);
    let unnormalized = root.join("textures").join("..");

    let (result, _) = run(
        Arc::new(ScriptedWork::committing()),
        request(
            ExecutionMode::Apply,
            &unnormalized,
            &[RequestedWork::NativeTextureOptimization],
        ),
    );

    assert_eq!(result.mod_roots(), [canonical(&root)]);
    assert_eq!(result.asset_attempts()[0].mod_root, canonical(&root));
    let summary = &result.mutation_summaries()[0];
    assert_eq!(
        (summary.mod_root.clone(), summary.kind, summary.committed),
        (canonical(&root), MutationKind::AssetProcessing, 1)
    );
}

/// Rust-only: the reserved `.cao-staging` namespace is never optimization input,
/// whatever its case or suffix.
#[test]
fn cao_staging_directories_are_never_discovered() {
    let _serial = serial();
    let root = scratch_dir("loose-staging-skipped");
    write_tree(
        &root,
        &[
            ".cao-staging/run/textures/staged.dds",
            "textures/.CAO-Staging-v3/stale.dds",
            "textures/real.dds",
        ],
    );

    let (result, _) = run(
        Arc::new(BackendWork::new()),
        request(
            ExecutionMode::DryRun,
            &root,
            &[RequestedWork::NativeTextureOptimization],
        ),
    );

    assert_eq!(attempted_names(&result), ["real.dds"]);
}

/// Origin: AssetRunTests::linkedAssetsAreReportedBeforeFinalizationWithoutExecution.
/// A directory junction is never followed, and is reported once as a Run Diagnostic.
#[test]
fn a_directory_junction_is_excluded_and_diagnosed_without_changing_the_outcome() {
    let _serial = serial();
    let outside = scratch_dir("loose-junction-target");
    write_tree(&outside, &["textures/outside.dds"]);
    let root = scratch_dir("loose-junction-root");
    write_tree(&root, &["textures/inside.dds"]);
    let link = root.join("linked");
    let created = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&link)
        .arg(&outside)
        .output()
        .expect("cmd runs");
    assert!(
        created.status.success(),
        "mklink /J failed: {}",
        String::from_utf8_lossy(&created.stderr)
    );

    let (result, events) = run(
        Arc::new(BackendWork::new()),
        request(
            ExecutionMode::DryRun,
            &root,
            &[RequestedWork::NativeTextureOptimization],
        ),
    );

    assert_eq!(result.outcome(), RunOutcome::Succeeded);
    assert_eq!(attempted_names(&result), ["inside.dds"]);
    let diagnostics: Vec<_> = result
        .diagnostics()
        .iter()
        .filter(|diagnostic| diagnostic.code == RunDiagnosticCode::LinkedEntryExcluded)
        .collect();
    assert_eq!(
        diagnostics.len(),
        1,
        "one exclusion across both discovery passes"
    );
    assert_eq!(diagnostics[0].path, link);
    assert_eq!(diagnostics[0].phase, RunPhase::DiscoveringArchives);
    // Discovery diagnostics are published after the Asset attempts.
    let events = events.events();
    let diagnostic_at = events
        .iter()
        .position(|event| matches!(event.payload, RunEventPayload::Diagnostic(_)))
        .unwrap();
    let last_progress_at = events
        .iter()
        .rposition(|event| matches!(&event.payload, RunEventPayload::Phase(record) if record.phase() == RunPhase::ProcessingAssets))
        .unwrap();
    assert!(diagnostic_at > last_progress_at);
    std::fs::remove_dir(&link).unwrap();
}

/// Origin: AssetRunTests::cancellationStopsBetweenRoutedAssets, through the Run
/// Handle as a user cancelling mid-run would.
#[test]
fn cancelling_between_assets_is_cancelled_keeps_committed_mutations_and_still_cleans_up() {
    let _serial = serial();
    let root = scratch_dir("loose-cancel-between-assets");
    write_tree(
        &root,
        &["textures/a.dds", "textures/b.dds", "textures/c.dds"],
    );
    let scheduler = Arc::new(GatedScheduler::default());
    let work = Arc::new(ScriptedWork::committing());
    let service = OptimizationRunService::with_scheduler(
        scheduler.clone(),
        Some(test_configuration()),
        Some(work.clone()),
    );
    let slot: HandleSlot = Arc::new(OnceLock::new());
    let events = EventLog::default();
    let dispatcher = {
        let (slot, log) = (Arc::clone(&slot), events.dispatcher());
        Box::new(move |event: RunEvent| {
            // Cancel as soon as the first attempt's progress is published.
            if let RunEventPayload::Phase(record) = &event.payload
                && record
                    .progress()
                    .is_some_and(|progress| progress.completed() == 1)
            {
                slot.get().unwrap().request_cancellation();
            }
            log(event);
        })
    };
    slot.set(
        service
            .start(
                request(
                    ExecutionMode::Apply,
                    &root,
                    &[RequestedWork::NativeTextureOptimization],
                ),
                Some(dispatcher),
            )
            .unwrap(),
    )
    .unwrap();
    scheduler.release();
    let result = slot.get().unwrap().wait();

    assert_eq!(result.outcome(), RunOutcome::Cancelled);
    assert!(result.cancellation_observed());
    assert_eq!(
        work.attempted.lock().unwrap().len(),
        1,
        "no Asset starts after cancellation"
    );
    let summary = &result.mutation_summaries()[0];
    assert_eq!(
        (summary.committed, summary.partial_or_unknown),
        (1, 0),
        "the Committed Mutation is retained"
    );
    assert_eq!(result.final_phase(), RunPhase::ProcessingAssets);
    let cleanup = result.phases().last().unwrap();
    assert_eq!(
        (cleanup.phase(), cleanup.status()),
        (RunPhase::SafetyCleanup, RunPhaseStatus::Executed)
    );
    assert_eq!(events.phases().last(), Some(&RunPhase::SafetyCleanup));
    drop(reclaim(slot));
}

/// Origin: AssetRunTests::cancellationDuringFinalAssetSkipsFinalization.
#[test]
fn cancellation_during_the_final_asset_skips_archive_finalization() {
    let _serial = serial();
    let root = scratch_dir("loose-cancel-final-asset");
    write_tree(&root, &["textures/a.dds", "textures/b.dds"]);
    let token = CancellationToken::new();
    let work = ScriptedWork {
        cancel_during: Some((2, token)),
        ..ScriptedWork::committing()
    };

    let (result, _) = run(
        Arc::new(work),
        request(
            ExecutionMode::Apply,
            &root,
            &[RequestedWork::NativeTextureOptimization],
        ),
    );

    assert_eq!(result.outcome(), RunOutcome::Cancelled);
    assert_eq!(
        result.asset_attempts().len(),
        2,
        "the in-flight attempt completes"
    );
    assert!(result.phase(RunPhase::ArchiveFinalization).is_none());
    assert_eq!(result.mutation_summaries()[0].committed, 2);
}

/// Origin: AssetRunTests::initializationCancellationDoesNotInventAttempt.
#[test]
fn cancellation_during_backend_initialization_invents_no_attempt() {
    let _serial = serial();
    let root = scratch_dir("loose-initialization-cancelled");
    write_tree(&root, &["textures/a.dds", "textures/b.dds"]);
    let work = ScriptedWork::new(Box::new(|_| Err(AssetInitializationCancelled)));

    let (result, _) = run(
        Arc::new(work),
        request(
            ExecutionMode::Apply,
            &root,
            &[RequestedWork::NativeTextureOptimization],
        ),
    );

    assert_eq!(result.outcome(), RunOutcome::Cancelled);
    assert!(result.asset_attempts().is_empty());
    assert_eq!(
        result.phase(RunPhase::ProcessingAssets).unwrap().progress(),
        Some(RunProgress::determinate(2, 0, 0))
    );
}

/// Origin: AssetRunTests::mutationAwareFailuresControlContinuation (the safe row).
#[test]
fn a_contained_load_failure_completes_with_failures_and_the_run_continues() {
    let _serial = serial();
    let root = scratch_dir("loose-load-failure");
    write_tree(&root, &["textures/a_unloadable.dds", "textures/b.dds"]);

    let (result, _) = run(
        Arc::new(BackendWork::new()),
        request(
            ExecutionMode::DryRun,
            &root,
            &[RequestedWork::NativeTextureOptimization],
        ),
    );

    assert_eq!(result.outcome(), RunOutcome::CompletedWithFailures);
    assert_eq!(attempted_names(&result), ["a_unloadable.dds", "b.dds"]);
    let failed = &result.asset_attempts()[0].result;
    assert_eq!(failed.failure(), Some(AssetExecutionFailure::LoadFailed));
    assert_eq!(failed.operation(), "load_texture");
    assert!(failed.safe_to_continue());
    assert_eq!(
        result.phase(RunPhase::ProcessingAssets).unwrap().progress(),
        Some(RunProgress::determinate(2, 1, 1))
    );
}

/// Origin: AssetRunTests::mutationAwareFailuresControlContinuation (the unsafe rows).
#[test]
fn an_unsafe_operation_failure_stops_every_later_attempt_and_fails_the_run() {
    let _serial = serial();
    let root = scratch_dir("loose-unsafe-failure");
    write_tree(
        &root,
        &["textures/a.dds", "textures/b.dds", "textures/c.dds"],
    );
    let work = ScriptedWork::new(Box::new(|asset| {
        Ok(if asset.execution_path().ends_with("a.dds") {
            AssetExecutionResult::success(MutationState::Committed)
        } else {
            AssetExecutionResult::failed(
                AssetExecutionFailure::CommitFailed,
                "publication left the output unknown",
            )
            .with_mutation(MutationState::PartialOrUnknown)
        })
    }));

    let (result, _) = run(
        Arc::new(work),
        request(
            ExecutionMode::Apply,
            &root,
            &[RequestedWork::NativeTextureOptimization],
        ),
    );

    assert_eq!(result.outcome(), RunOutcome::Failed);
    assert_eq!(attempted_names(&result), ["a.dds", "b.dds"]);
    let summary = &result.mutation_summaries()[0];
    assert_eq!((summary.committed, summary.partial_or_unknown), (1, 1));
    assert!(result.phase(RunPhase::ArchiveFinalization).is_none());
}

/// Spec (#468): a panic in an Asset backend call becomes an Operation Failure
/// with unknown mutation that is unsafe to continue.
#[test]
fn a_panicking_backend_becomes_an_unsafe_operation_failure_with_unknown_mutation() {
    let _serial = serial();
    let root = scratch_dir("loose-backend-panic");
    write_tree(
        &root,
        &["textures/a.dds", "textures/b_panics.dds", "textures/c.dds"],
    );
    let work = Arc::new(BackendWork::new());

    let (result, _) = run(
        work.clone(),
        request(
            ExecutionMode::DryRun,
            &root,
            &[RequestedWork::NativeTextureOptimization],
        ),
    );

    assert_eq!(result.outcome(), RunOutcome::Failed);
    assert_eq!(
        attempted_names(&result),
        ["a.dds", "b_panics.dds"],
        "no Asset runs after the panic"
    );
    let panicked = &result.asset_attempts()[1].result;
    assert_eq!(
        panicked.failure(),
        Some(AssetExecutionFailure::BackendException)
    );
    assert_eq!(panicked.mutation_state(), MutationState::PartialOrUnknown);
    assert!(!panicked.safe_to_continue());
    assert!(panicked.message().contains("the backend panicked"));
    assert_eq!(
        result.phases().last().unwrap().phase(),
        RunPhase::SafetyCleanup
    );
}

/// Spec (#468): a panic in the Run Worker, here a Run Evidence invariant
/// violation raised after cleanup, still ends the run in one Failed outcome.
#[test]
fn a_run_worker_panic_ends_the_run_in_one_failed_outcome() {
    let _serial = serial();
    struct OutOfOrder;
    impl RunWorkService for OutOfOrder {
        fn execute(
            &self,
            _preparation: &RunPreparation,
            _evidence: &RunWorkEvidence<'_, '_>,
            milestones: &dyn RunWorkMilestones,
            _stop: &CancellationToken,
        ) -> Result<(), Error> {
            milestones.asset_processing_planned(1)?;
            Ok(())
        }
    }
    let root = scratch_dir("loose-worker-panic");
    let service =
        OptimizationRunService::new(Some(test_configuration()), Some(Arc::new(OutOfOrder)));
    let events = EventLog::default();
    let handle = service
        .start(
            request(
                ExecutionMode::DryRun,
                &root,
                &[RequestedWork::NativeTextureOptimization],
            ),
            Some(events.dispatcher()),
        )
        .unwrap();

    let result = handle.wait();

    assert_eq!(result.outcome(), RunOutcome::Failed);
    assert_eq!(result.failures().len(), 1);
    assert_eq!(result.failures()[0].code, RunFailureCode::WorkServiceFailed);
    assert!(
        result.failures()[0]
            .detail
            .contains("Run Evidence invariant violated")
    );
    // The committed result is the executor's sealed one, earlier facts included.
    assert!(result.preparation().is_some());
    assert_eq!(result.mod_roots(), [canonical(&root)]);
    // Cleanup ran once, inside the executor, before the panic; the event
    // history still ends with Safety Cleanup and then the one terminal event.
    let events = events.events();
    let terminals = events
        .iter()
        .filter(|event| matches!(event.payload, RunEventPayload::Terminal(_)))
        .count();
    assert_eq!(terminals, 1);
    let cleanups = events
        .iter()
        .filter(|event| {
            matches!(&event.payload, RunEventPayload::Phase(record) if record.phase() == RunPhase::SafetyCleanup)
        })
        .count();
    assert_eq!(cleanups, 1);
    assert!(matches!(
        &events[events.len() - 2].payload,
        RunEventPayload::Phase(record) if record.phase() == RunPhase::SafetyCleanup
    ));
}

/// Rust-only, interim until staged publication (#491): an Apply attempt that
/// would persist a change stops at the staging boundary and mutates nothing.
#[test]
fn an_apply_change_stops_at_the_staging_boundary_without_mutating() {
    let _serial = serial();
    let root = scratch_dir("loose-apply-staging-unavailable");
    write_tree(&root, &["textures/a_changes.dds", "textures/b.dds"]);
    let before = snapshot_tree(&root);

    let (result, _) = run(
        Arc::new(BackendWork::new()),
        request(
            ExecutionMode::Apply,
            &root,
            &[RequestedWork::NativeTextureOptimization],
        ),
    );

    assert_eq!(result.outcome(), RunOutcome::Failed);
    let attempt = &result.asset_attempts()[0].result;
    assert_eq!(
        attempt.failure(),
        Some(AssetExecutionFailure::StagingFailed)
    );
    assert_eq!(attempt.mutation_state(), MutationState::None);
    assert!(!attempt.safe_to_continue());
    assert_eq!(result.asset_attempts().len(), 1);
    assert_eq!(snapshot_tree(&root), before);
}

/// Rust-only, interim until Archive extraction (#496, #497): an Apply run that
/// selects an Archive fails before extraction and routes nothing.
#[test]
fn an_apply_run_selecting_an_archive_fails_before_any_extraction() {
    let _serial = serial();
    let root = scratch_dir("loose-apply-archive");
    write_tree(&root, &["archive.bsa", "textures/a.dds"]);
    let before = snapshot_tree(&root);
    let work = Arc::new(BackendWork::new());

    let (result, _) = run(
        work.clone(),
        request(
            ExecutionMode::Apply,
            &root,
            &[
                RequestedWork::NativeTextureOptimization,
                RequestedWork::ArchiveExtraction,
            ],
        ),
    );

    assert_eq!(result.outcome(), RunOutcome::Failed);
    assert_eq!(result.final_phase(), RunPhase::DiscoveringArchives);
    assert_eq!(
        result.failures()[0].code,
        RunFailureCode::RequestedWorkUnavailable
    );
    assert!(result.routing_ledger().is_none());
    assert!(work.calls().is_empty());
    assert_eq!(snapshot_tree(&root), before);
}
