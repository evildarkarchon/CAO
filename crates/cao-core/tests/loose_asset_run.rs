//! Loose Asset runs through the Optimization Run Service, with a fake Asset
//! Execution Backend over real Mod Root directories.
//!
//! Scenarios that port a C++ test name its origin, mostly from
//! `tests/AssetRunTests.cpp`; the rest pin Rust-side contracts from the spec
//! (#476): panic containment, and the interim behaviour of Apply work whose
//! production work service wires no Archive extraction yet (#497).
//!
//! Ported elsewhere:
//! - `throwingDiagnosticsCancellationSkipsFinalization`: `run_executor.rs`,
//!   the "cancel then panic" row of
//!   `a_discovery_diagnostic_follows_the_attempt_and_can_cancel_finalization`.
//! - `archiveFailuresControlContinuation`,
//!   `archiveExtractionPrecedesDefinitiveRoutedExecution`,
//!   `realExtractionPreservesLooseAssetPrecedence`,
//!   `archiveCancellationSkipsDefinitiveDiscovery`,
//!   `finalArchiveCancellationSkipsDefinitiveDiscovery`,
//!   `nestedArchivesAreReportedWithoutInflatingTheWorkTotal`, the
//!   definitive-mod-tree row of `filesystemTraversalPollsCancellation`,
//!   `unreadableArchiveStopsRunBeforeMutation` and
//!   `reportsCollisionsBeforeOrderedExtraction`: `archive_discovery.rs`, each
//!   naming its origin. C++'s `reportDiscoveryFailure` adapter has no Rust
//!   counterpart: a failing failure observer is the Run Observation Sink's,
//!   in `a_failing_preflight_observer_cannot_lose_the_failure`.
//!
//! Not ported, with reasons:
//! - `cancelledArchiveFinalizationIsReported`, and the Archive Finalization
//!   half of `completeAttemptEvidenceSurvivesAdapters`: Archive Finalization
//!   results are not Run Evidence yet (#498).
//! - The explicit-Archive rows of `filesystemTraversalPollsCancellation`, and
//!   the explicit unsupported path of
//!   `dryRunAggregatesArchiveSkipsAndKeepsDirectoryUnsupportedPathsSilent`: a
//!   Rust Mod Selection is always a directory, so no file can be selected.
//! - The ledger identity check of `executesOriginalLedgerAssetsInTargetOrder`:
//!   it compared C++ pointers; the order it pins is ported.

mod common;

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use cao_core::Error;
use cao_core::execution::{
    AssetExecutionFailure, AssetExecutionResult, ExecutionFailureCategory, MutationState,
};
use cao_core::routing::{AssetOperation, ExecutionMode, RequestedWork, SkipReason};
use cao_core::run::{
    AssetInitializationCancelled, CancellationToken, InlineRunScheduler, OptimizationRunResult,
    OptimizationRunService, PhaseSkipReason, RunDiagnosticCode, RunEvent, RunEventPayload,
    RunFailureCode, RunOutcome, RunPhase, RunPhaseRecord, RunPhaseStatus, RunPreparation,
    RunProgress, RunRequest, RunWorkEvidence, RunWorkMilestones, RunWorkService,
    TemporaryArtifactRegistry,
};
use common::{
    BackendWork, ControlledWork, EventLog, GatedScheduler, HandleSlot, ScriptedWork, canonical,
    junction, reclaim, request, scratch_dir, serial, snapshot_tree, test_configuration, write_tree,
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
    junction(&link, &outside);

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

/// A panic payload whose own drop panics again.
struct PanicsAgainOnDrop;

impl Drop for PanicsAgainOnDrop {
    fn drop(&mut self) {
        panic!("the panic payload panicked while it was dropped");
    }
}

/// Spec (#468): a backend panic stays an Operation Failure on its attempt
/// even when dropping its payload panics too; the second panic never
/// escapes the backend boundary to become a run-level failure.
#[test]
fn a_backend_panic_whose_payload_panics_again_is_still_an_operation_failure() {
    let _serial = serial();
    let root = scratch_dir("loose-backend-panic-payload");
    write_tree(&root, &["textures/a.dds"]);
    let work = ControlledWork {
        execute: Some(Box::new(|_, _| std::panic::panic_any(PanicsAgainOnDrop))),
        ..ControlledWork::default()
    };

    let (result, _) = run(
        Arc::new(work),
        request(
            ExecutionMode::DryRun,
            &root,
            &[RequestedWork::NativeTextureOptimization],
        ),
    );

    assert_eq!(result.outcome(), RunOutcome::Failed);
    assert!(result.failures().is_empty(), "{:?}", result.failures());
    assert_eq!(result.asset_attempts().len(), 1);
    let attempt = &result.asset_attempts()[0].result;
    assert_eq!(
        attempt.failure(),
        Some(AssetExecutionFailure::BackendException)
    );
    assert_eq!(attempt.mutation_state(), MutationState::PartialOrUnknown);
    assert!(!attempt.safe_to_continue());
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
            _artifacts: &mut TemporaryArtifactRegistry,
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

/// Rust-only, interim until the production work service wires Archive
/// extraction (#497): an Apply run whose work supplies no Archive adapters
/// fails when it selects an Archive, before extraction, and routes nothing.
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

/// The Apply request over native Textures and Archive extraction that most
/// C++ Asset Run scenarios ran under.
fn archive_and_textures(root: &Path) -> RunRequest {
    request(
        ExecutionMode::Apply,
        root,
        &[
            RequestedWork::NativeTextureOptimization,
            RequestedWork::ArchiveExtraction,
        ],
    )
}

/// The Apply request over every loose Asset target and no Archive work.
fn all_loose_targets(root: &Path) -> RunRequest {
    request(
        ExecutionMode::Apply,
        root,
        &[
            RequestedWork::NativeTextureOptimization,
            RequestedWork::StandardMeshOptimization,
            RequestedWork::AnimationOptimization,
        ],
    )
}

/// The `ObserverFailed` diagnostics a run retained.
fn observer_failures(result: &OptimizationRunResult) -> usize {
    result
        .diagnostics()
        .iter()
        .filter(|diagnostic| diagnostic.code == RunDiagnosticCode::ObserverFailed)
        .count()
}

/// Origin: AssetRunTests::completeAttemptEvidenceSurvivesAdapters (the Asset
/// half). Successful and failed attempt evidence outlives the adapters that
/// produced it, and a safe failure does not stop the run reaching
/// finalization.
#[test]
fn attempt_evidence_outlives_its_adapters() {
    let _serial = serial();
    let root = canonical(&scratch_dir("loose-evidence-outlives-adapters"));
    write_tree(&root, &["a.dds", "b.dds"]);
    let work = Arc::new(ControlledWork {
        execute: Some(Box::new(|asset, _| {
            Ok(if asset.execution_path().ends_with("a.dds") {
                AssetExecutionResult::success(MutationState::Committed)
            } else {
                AssetExecutionResult::failed(AssetExecutionFailure::LoadFailed, "retained")
            })
        })),
        finalize: Some(Box::new(|| Ok(()))),
        ..ControlledWork::default()
    });

    let (result, _) = run(work.clone(), archive_and_textures(&root));
    let finalized = work.finalizations.load(Ordering::SeqCst);
    drop(work);

    assert_eq!(finalized, 1);
    let attempts = result.asset_attempts();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].mod_root, root);
    assert!(attempts[0].result.succeeded());
    assert_eq!(
        attempts[0].result.mutation_state(),
        MutationState::Committed
    );
    assert_eq!(attempts[0].asset.execution_path(), root.join("a.dds"));
    assert!(!attempts[1].result.succeeded());
    assert_eq!(attempts[1].result.message(), "retained");
    assert!(result.routing_ledger().is_some());
}

/// Origin: AssetRunTests::throwingAttemptRetainsCancellation, under the
/// spec's panic rule. An adapter that requests cancellation and then panics
/// becomes an unsafe attempt with unknown mutation, the cancellation is still
/// observed, and nothing runs after it.
#[test]
fn a_panicking_attempt_keeps_the_cancellation_it_requested() {
    let _serial = serial();
    let root = scratch_dir("loose-panicking-attempt-cancels");
    write_tree(&root, &["a.dds", "b.dds"]);
    let cancelled = Arc::new(AtomicBool::new(false));
    let work = ControlledWork {
        execute: Some(Box::new({
            let cancelled = cancelled.clone();
            move |_, _| {
                cancelled.store(true, Ordering::SeqCst);
                panic!("adapter failed")
            }
        })),
        is_cancelled: Some(Box::new(move || cancelled.load(Ordering::SeqCst))),
        ..ControlledWork::default()
    };

    let (result, _) = run(Arc::new(work), archive_and_textures(&root));

    assert_eq!(result.outcome(), RunOutcome::Failed);
    assert!(result.cancellation_observed());
    assert_eq!(result.asset_attempts().len(), 1);
    let attempt = &result.asset_attempts()[0].result;
    assert_eq!(attempt.mutation_state(), MutationState::PartialOrUnknown);
    assert!(!attempt.safe_to_continue());
    assert_eq!(attempt.message(), "adapter failed");
}

/// Origin: AssetRunTests::throwingFinalizerPropagates. A finalizer's error, or
/// its panic, reaches the Run Executor unchanged: the run fails at Archive
/// Finalization, and no cancellation is invented.
#[test]
fn a_failing_finalizer_fails_the_run_at_archive_finalization() {
    let _serial = serial();
    type Finalize = fn() -> Result<(), Error>;
    let rows: [(&str, Finalize); 2] = [
        ("error", || {
            Err(Error::WorkService("Archive Finalization failed".to_owned()))
        }),
        ("panic", || panic!("Archive Finalization failed")),
    ];
    for (row, finalize) in rows {
        let root = scratch_dir(&format!("loose-failing-finalizer-{row}"));
        let work = ControlledWork {
            finalize: Some(Box::new(finalize)),
            ..ControlledWork::default()
        };

        let (result, _) = run(Arc::new(work), archive_and_textures(&root));

        assert_eq!(result.outcome(), RunOutcome::Failed, "{row}");
        assert!(!result.cancellation_observed(), "{row}");
        let failure = &result.failures()[0];
        assert_eq!(failure.code, RunFailureCode::WorkServiceFailed, "{row}");
        assert_eq!(failure.phase, RunPhase::ArchiveFinalization, "{row}");
        assert!(
            failure.detail.contains("Archive Finalization failed"),
            "{row}"
        );
    }
}

/// Origin: AssetRunTests::throwingObserversPreserveCommittedWork. Panicking
/// presentation adapters are retained as `ObserverFailed` diagnostics; they
/// discard no committed attempt, are never Run Failures, and do not prevent
/// finalization. The Rust adapters report phases rather than diagnostics,
/// so the phase adapter stands in for C++'s diagnostics observer.
#[test]
fn panicking_presentation_adapters_preserve_committed_work() {
    let _serial = serial();
    let root = scratch_dir("loose-panicking-presentation");
    write_tree(&root, &["a.dds"]);
    let work = Arc::new(ControlledWork {
        execute: Some(Box::new(|_, _| {
            Ok(AssetExecutionResult::success(MutationState::Committed))
        })),
        report_progress: Some(Box::new(|_| panic!("the progress adapter panicked"))),
        report_phase: Some(Box::new(|record| {
            if record.phase() == RunPhase::ProcessingAssets
                && record
                    .progress()
                    .is_some_and(|progress| progress.completed() == 0)
            {
                panic!("the phase adapter panicked");
            }
        })),
        finalize: Some(Box::new(|| Ok(()))),
        ..ControlledWork::default()
    });

    let (result, _) = run(work.clone(), archive_and_textures(&root));

    assert_eq!(result.outcome(), RunOutcome::Succeeded);
    assert_eq!(result.asset_attempts().len(), 1);
    assert_eq!(
        result.asset_attempts()[0].result.mutation_state(),
        MutationState::Committed
    );
    assert_eq!(result.diagnostics().len(), 2);
    assert_eq!(observer_failures(&result), 2);
    assert!(result.failures().is_empty());
    assert_eq!(work.finalizations.load(Ordering::SeqCst), 1);
}

/// Origin: AssetRunTests::relativeSelectionRetainsMutationScope. A Mod Root
/// selected by a relative path is recorded as its canonical absolute root,
/// even when the attempt removes the Asset it ran on.
#[test]
fn a_relative_selection_records_the_canonical_mod_root() {
    let _serial = serial();
    let root = canonical(&scratch_dir("loose-relative-selection"));
    write_tree(&root, &["a.dds"]);
    let work = ControlledWork {
        execute: Some(Box::new(|asset, _| {
            std::fs::remove_file(asset.execution_path()).unwrap();
            Ok(AssetExecutionResult::success(MutationState::Committed))
        })),
        ..ControlledWork::default()
    };
    let relative = common::relative_to_working_directory(&root);

    let (result, _) = run(Arc::new(work), archive_and_textures(&relative));

    assert_eq!(result.asset_attempts().len(), 1);
    assert_eq!(result.asset_attempts()[0].mod_root, root);
    assert!(!root.join("a.dds").exists());
}

/// Origin: AssetRunTests::mutationAwareFailuresControlContinuation (all three
/// rows). A safe failure moves on to the next Asset; an unsafe one stops
/// every later attempt and Archive Finalization, even as the last Asset. The
/// failure keeps all its facts and the total never changes.
#[test]
fn mutation_aware_failures_control_continuation_and_finalization() {
    let _serial = serial();
    // (row, safe, failed attempt)
    let rows = [
        ("safe failure", true, 1),
        ("unsafe failure", false, 1),
        ("unsafe final attempt", false, 2),
    ];
    for (index, (row, safe, failed_attempt)) in rows.into_iter().enumerate() {
        let root = canonical(&scratch_dir(&format!("loose-continuation-{index}")));
        write_tree(&root, &["first.dds", "second.dds"]);
        let attempts = Arc::new(Mutex::new(0));
        let progress = Arc::new(Mutex::new(Vec::new()));
        let work = Arc::new(ControlledWork {
            execute: Some(Box::new({
                let attempts = attempts.clone();
                move |asset, _| {
                    let mut attempts = attempts.lock().unwrap();
                    *attempts += 1;
                    if *attempts != failed_attempt {
                        return Ok(AssetExecutionResult::success(MutationState::None));
                    }
                    let mutation = if safe {
                        MutationState::None
                    } else {
                        MutationState::PartialOrUnknown
                    };
                    Ok(AssetExecutionResult::failed(
                        AssetExecutionFailure::SaveFailed,
                        "Injected failure",
                    )
                    .with_mutation(mutation)
                    .with_safe_to_continue(safe)
                    .with_path(asset.execution_path())
                    .with_operation("save"))
                }
            })),
            report_progress: Some(Box::new({
                let progress = progress.clone();
                move |update| progress.lock().unwrap().push(update)
            })),
            finalize: Some(Box::new(|| Ok(()))),
            ..ControlledWork::default()
        });

        let (result, _) = run(work.clone(), all_loose_targets(&root));

        let attempted = if safe { 2 } else { failed_attempt };
        assert_eq!(result.asset_attempts().len(), attempted, "{row}");
        assert_eq!(
            work.finalizations.load(Ordering::SeqCst),
            usize::from(safe),
            "{row}"
        );
        assert!(!result.cancellation_observed(), "{row}");
        let failures: Vec<_> = result
            .asset_attempts()
            .iter()
            .filter(|attempt| !attempt.result.succeeded())
            .collect();
        assert_eq!(failures.len(), 1, "{row}");
        let progress = progress.lock().unwrap();
        assert_eq!(progress.len(), attempted, "{row}");
        let last = progress.last().unwrap();
        assert_eq!((last.completed, last.total), (attempted, 2), "{row}");
        assert_eq!(result.routing_ledger().unwrap().routed_assets().len(), 2);
        let failed = failures[0];
        let name = if failed_attempt == 1 {
            "first.dds"
        } else {
            "second.dds"
        };
        assert_eq!(failed.mod_root, root, "{row}");
        assert_eq!(
            failed.result.failure(),
            Some(AssetExecutionFailure::SaveFailed)
        );
        assert_eq!(failed.result.affected_path(), root.join(name), "{row}");
        assert_eq!(failed.result.operation(), "save", "{row}");
        assert_eq!(failed.result.safe_to_continue(), safe, "{row}");
    }
}

/// Origin: AssetRunTests::animationFailuresPreserveProgressAndEvidence (all
/// four rows). Animation outcomes advance progress and keep the whole
/// structured failure; an unsafe one stops the next Animation and Archive
/// Finalization.
#[test]
fn animation_failures_preserve_progress_and_evidence() {
    let _serial = serial();
    // (row, safe, failed attempt)
    let rows = [
        ("safe failure before success", true, 1),
        ("success before safe failure", true, 2),
        ("unsafe failure stops the next Animation", false, 1),
        ("unsafe final failure stops finalization", false, 2),
    ];
    for (index, (row, safe, failed_attempt)) in rows.into_iter().enumerate() {
        let root = canonical(&scratch_dir(&format!("loose-animation-failure-{index}")));
        write_tree(&root, &["first.hkx", "second.hkx"]);
        let attempts = Arc::new(Mutex::new(0));
        let progress = Arc::new(Mutex::new(Vec::new()));
        let (failure, mutation) = if safe {
            (AssetExecutionFailure::OperationFailed, MutationState::None)
        } else {
            (
                AssetExecutionFailure::BackendException,
                MutationState::PartialOrUnknown,
            )
        };
        let work = Arc::new(ControlledWork {
            execute: Some(Box::new({
                let attempts = attempts.clone();
                move |asset, _| {
                    let mut attempts = attempts.lock().unwrap();
                    *attempts += 1;
                    if *attempts != failed_attempt {
                        return Ok(AssetExecutionResult::success(MutationState::Committed));
                    }
                    Ok(AssetExecutionResult::failed(failure, "Animation failed")
                        .with_mutation(mutation)
                        .with_safe_to_continue(safe)
                        .with_path(asset.execution_path())
                        .with_operation("optimize_animation")
                        .with_service_detail("animation backend diagnostic"))
                }
            })),
            report_progress: Some(Box::new({
                let progress = progress.clone();
                move |update| progress.lock().unwrap().push(update)
            })),
            finalize: Some(Box::new(|| Ok(()))),
            ..ControlledWork::default()
        });

        let (result, _) = run(work.clone(), all_loose_targets(&root));

        let attempted = if safe { 2 } else { failed_attempt };
        assert_eq!(result.asset_attempts().len(), attempted, "{row}");
        assert_eq!(
            work.finalizations.load(Ordering::SeqCst),
            usize::from(safe),
            "{row}"
        );
        assert!(!result.cancellation_observed(), "{row}");
        let failed = &result.asset_attempts()[failed_attempt - 1].result;
        let category = if safe {
            ExecutionFailureCategory::Backend
        } else {
            ExecutionFailureCategory::Contract
        };
        assert_eq!(failed.failure(), Some(failure), "{row}");
        assert_eq!(failed.failure_category(), Some(category), "{row}");
        assert_eq!(failed.mutation_state(), mutation, "{row}");
        assert_eq!(failed.safe_to_continue(), safe, "{row}");
        let name = if failed_attempt == 1 {
            "first.hkx"
        } else {
            "second.hkx"
        };
        assert_eq!(failed.affected_path(), root.join(name), "{row}");
        assert_eq!(failed.operation(), "optimize_animation", "{row}");
        assert_eq!(failed.message(), "Animation failed", "{row}");
        assert_eq!(failed.service_detail(), "animation backend diagnostic");
        let progress = progress.lock().unwrap();
        assert_eq!(progress.len(), attempted, "{row}");
        assert_eq!((progress[0].completed, progress[0].total), (1, 2), "{row}");
        if attempted == 2 {
            assert_eq!((progress[1].completed, progress[1].total), (2, 2), "{row}");
        }
    }
}

/// Origin: AssetRunTests::filesystemTraversalPollsCancellation (the
/// extraction-disabled and no-selected-archives rows). Discovery polls
/// cancellation per entry, so a tree of unsupported files alone is enough
/// to observe it: no routing, no attempt, no finalization.
#[test]
fn discovery_polls_cancellation_while_it_walks_the_tree() {
    let _serial = serial();
    for (row, requested) in [
        (
            "extraction disabled",
            all_loose_targets as fn(&Path) -> RunRequest,
        ),
        ("no selected Archives", archive_and_textures),
    ] {
        let root = scratch_dir(&format!("loose-traversal-polls-{}", row.replace(' ', "-")));
        let files: Vec<String> = (0..100).map(|index| format!("docs/{index}.txt")).collect();
        let files: Vec<&str> = files.iter().map(String::as_str).collect();
        write_tree(&root, &files);
        let polls = Arc::new(AtomicUsize::new(0));
        let work = Arc::new(ControlledWork {
            is_cancelled: Some(Box::new({
                let polls = polls.clone();
                move || polls.fetch_add(1, Ordering::SeqCst) + 1 >= 4
            })),
            finalize: Some(Box::new(|| Ok(()))),
            ..ControlledWork::default()
        });

        let (result, _) = run(work.clone(), requested(&root));

        assert_eq!(result.outcome(), RunOutcome::Cancelled, "{row}");
        assert!(result.cancellation_observed(), "{row}");
        assert!(result.routing_ledger().is_none(), "{row}");
        assert_eq!(work.executions.load(Ordering::SeqCst), 0, "{row}");
        assert_eq!(work.finalizations.load(Ordering::SeqCst), 0, "{row}");
        assert!(
            polls.load(Ordering::SeqCst) < 100,
            "{row}: stopped mid-walk"
        );
    }
}

/// Origin: AssetRunTests::applyFinalizesArchivesAfterRoutedExecution and
/// linkedAssetsAreReportedBeforeFinalizationWithoutExecution (the ordering
/// half). Apply executes Assets, then publishes discovery's diagnostics, then
/// finalizes Archives.
#[test]
fn apply_publishes_diagnostics_between_execution_and_finalization() {
    let _serial = serial();
    let outside = scratch_dir("loose-ordering-outside");
    let root = scratch_dir("loose-ordering-root");
    write_tree(&root, &["textures/native.dds"]);
    let link = root.join("linked");
    junction(&link, &outside);
    let order = Arc::new(Mutex::new(Vec::<&'static str>::new()));
    let work = ControlledWork {
        execute: Some(Box::new({
            let order = order.clone();
            move |_, _| {
                order.lock().unwrap().push("execute");
                Ok(AssetExecutionResult::success(MutationState::None))
            }
        })),
        finalize: Some(Box::new({
            let order = order.clone();
            move || {
                order.lock().unwrap().push("finalize");
                Ok(())
            }
        })),
        ..ControlledWork::default()
    };
    let service = OptimizationRunService::with_scheduler(
        Arc::new(InlineRunScheduler),
        Some(test_configuration()),
        Some(Arc::new(work)),
    );
    let dispatcher = {
        let order = order.clone();
        Box::new(move |event: RunEvent| {
            if matches!(event.payload, RunEventPayload::Diagnostic(_)) {
                order.lock().unwrap().push("diagnostic");
            }
        })
    };

    let result = service
        .start(archive_and_textures(&root), Some(dispatcher))
        .unwrap()
        .wait();
    std::fs::remove_dir(&link).unwrap();

    assert_eq!(result.outcome(), RunOutcome::Succeeded);
    assert_eq!(
        *order.lock().unwrap(),
        ["execute", "diagnostic", "finalize"]
    );
}

/// Origin: AssetRunTests::dryRunLeavesCompleteModTreeUnchangedWhileEvaluatingLooseAssets
/// (the whole tree). A Dry Run never finalizes and leaves every byte and
/// directory as it found them, invalid Archives and staging included, while
/// still evaluating each Routed Asset in Dry Run with its routed operations.
#[test]
fn a_dry_run_leaves_the_complete_mod_tree_unchanged() {
    let _serial = serial();
    let root = scratch_dir("loose-dry-run-complete-tree");
    write_tree(
        &root,
        &[
            "content.bsa",
            "nested/second.bsa",
            ".CAO-Staging-unknown/hidden.bsa",
            "textures/convertible.tga",
            "meshes/actor.nif",
            "animations/walk.hkx",
            "docs/readme.txt",
        ],
    );
    std::fs::create_dir_all(root.join("empty/nested")).unwrap();
    let before = snapshot_tree(&root);
    let progress = Arc::new(Mutex::new(Vec::new()));
    let finalize_target = root.join("content.bsa");
    let work = Arc::new(ControlledWork {
        report_progress: Some(Box::new({
            let progress = progress.clone();
            move |update| {
                progress
                    .lock()
                    .unwrap()
                    .push((update.completed, update.total))
            }
        })),
        // A finalizer that ran would visibly change the tree.
        finalize: Some(Box::new(move || {
            std::fs::remove_file(&finalize_target).unwrap();
            Ok(())
        })),
        ..ControlledWork::default()
    });

    let (result, _) = run(
        work.clone(),
        request(
            ExecutionMode::DryRun,
            &root,
            &[
                RequestedWork::ConvertibleTextureConversion,
                RequestedWork::StandardMeshOptimization,
                RequestedWork::ArchiveExtraction,
            ],
        ),
    );

    assert_eq!(
        result.outcome(),
        RunOutcome::Succeeded,
        "{:?}",
        result.failures()
    );
    assert_eq!(work.finalizations.load(Ordering::SeqCst), 0);
    assert_eq!(snapshot_tree(&root), before);
    assert!(root.join("empty/nested").is_dir());
    let attempts = result.asset_attempts();
    assert_eq!(attempts.len(), 2);
    let texture = &attempts[0].asset;
    assert!(texture.execution_path().ends_with("convertible.tga"));
    assert_eq!(texture.execution_mode(), ExecutionMode::DryRun);
    assert!(texture.operations().contains(AssetOperation::Conversion));
    assert!(!texture.operations().contains(AssetOperation::Optimization));
    assert!(
        !texture
            .operations()
            .contains(AssetOperation::MeshReferenceMaintenance)
    );
    let mesh = &attempts[1].asset;
    assert!(mesh.execution_path().ends_with("actor.nif"));
    assert_eq!(mesh.execution_mode(), ExecutionMode::DryRun);
    assert!(mesh.operations().contains(AssetOperation::Optimization));
    assert!(
        mesh.operations()
            .contains(AssetOperation::MeshReferenceMaintenance)
    );
    assert!(!mesh.operations().contains(AssetOperation::Conversion));
    assert_eq!(result.routing_ledger().unwrap().routed_assets().len(), 2);
    assert_eq!(result.skipped_asset_count(SkipReason::DisabledPhase), 2);
    assert_eq!(result.skipped_asset_count(SkipReason::DisabledAssetKind), 1);
    assert_eq!(*progress.lock().unwrap(), [(1, 2), (2, 2)]);
}
