//! Optimization Runs over Loose Assets through the Optimization Run Service,
//! with a fake Asset Execution Backend over real Mod Root directories.
//!
//! Each run's work goes through a scripted Run Toolset (#553): its Asset
//! attempts either drive the fake backend behind the production Asset attempt
//! adapter or return scripted results. Scenarios whose observer must panic,
//! or cancel at a phase, run through the Run Executor with a recording Run
//! Observation Sink instead, because the Optimization Run Service is its own
//! sink and disables a dispatcher after its first panic.
//!
//! Scenarios that port a C++ test name its origin, mostly from
//! `tests/AssetRunTests.cpp`; the rest pin Rust-side contracts from the spec
//! (#476): panic containment.
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
//! - The diagnostics adapter of `cancelledArchiveFinalizationIsReported`:
//!   Rust has no such adapter; diagnostics reach the Run Observation Sink,
//!   and the Run Executor publishes them before finalization in every path.
//! - The explicit-Archive rows of `filesystemTraversalPollsCancellation`, and
//!   the explicit unsupported path of
//!   `dryRunAggregatesArchiveSkipsAndKeepsDirectoryUnsupportedPathsSilent`: a
//!   Rust Mod Selection is always a directory, so no file can be selected.
//! - The ledger identity check of `executesOriginalLedgerAssetsInTargetOrder`:
//!   it compared C++ pointers; the order it pins is ported.
//!
//! Removed by the move to Run Toolsets (#553):
//! - A Run Worker panic raised by work that called the work milestones out of
//!   order: a Run Toolset gives its ports no milestones to misuse. The
//!   containment guarantees it pinned are tested inside `cao-core` (#555).
//! - An Apply run whose work supplied no Archive adapters: every Run Toolset
//!   carries the Archive reader and probes, so no such run exists.
//! - The per-entry poll count of `filesystemTraversalPollsCancellation`: it
//!   was observed through a cancellation hook, and cancellation now arrives
//!   only through the run's cancellation token.

mod common;

use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use cao_core::execution::{
    AssetExecutionFailure, AssetExecutionResult, ExecutionFailureCategory, MutationState,
};
use cao_core::routing::{AssetOperation, ExecutionMode, RequestedWork, SkipReason};
use cao_core::run::{
    ArchiveFinalizationAttempt, ArchiveFinalizationFailure, ArchiveFinalizationResult,
    AssetInitializationCancelled, CancellationToken, InlineRunScheduler, OptimizationRunResult,
    OptimizationRunService, PhaseSkipReason, RunDiagnosticCode, RunEvent, RunEventPayload,
    RunExecutor, RunFailureCode, RunOutcome, RunPhase, RunPhaseRecord, RunPhaseStatus, RunProgress,
    RunRequest, RunServices, create_run_id,
};
use cao_core::test_support::{
    CountingCleanup, EventLog, FakeBackendFactory, GatedScheduler, Observed, RecordingSink,
    ScriptedToolset, record_finalization_result,
};
use common::{
    HandleSlot, backend_toolset, canonical, junction, reclaim, request, scratch_dir, serial,
    snapshot_tree, toolset_configuration, write_tree,
};

/// Runs one request to its terminal result on the inline scheduler, its work
/// going through a fresh Run Toolset over `toolset`.
fn run(
    toolset: &Arc<ScriptedToolset>,
    request: cao_core::run::RunRequest,
) -> (Arc<OptimizationRunResult>, EventLog) {
    let service = OptimizationRunService::with_scheduler(
        Arc::new(InlineRunScheduler),
        Some(toolset_configuration(Some(Arc::clone(toolset)))),
    );
    let events = EventLog::default();
    let handle = service
        .start(request, Some(events.dispatcher()))
        .expect("the run starts");
    (handle.wait(), events)
}

/// Runs one request through the Run Executor, with `sink` observing every
/// fact, under the cancellation token `stop`.
fn execute_observed(
    toolset: &Arc<ScriptedToolset>,
    request: &RunRequest,
    sink: &RecordingSink,
    stop: &CancellationToken,
) -> OptimizationRunResult {
    let mut cleanup = CountingCleanup::default();
    RunExecutor.execute(
        request,
        RunServices {
            safety_cleanup: &mut cleanup,
            observations: Some(sink),
            configuration: Some(&*toolset_configuration(Some(Arc::clone(toolset)))),
        },
        stop,
        create_run_id(),
    )
}

/// The `(completed, total)` of every Processing Assets phase event, in
/// dispatch order: Run Progress as an observer sees it.
fn processing_progress(events: &EventLog) -> Vec<(usize, usize)> {
    events
        .events()
        .into_iter()
        .filter_map(|event| match event.payload {
            RunEventPayload::Phase(record) if record.phase() == RunPhase::ProcessingAssets => {
                record.progress()
            }
            _ => None,
        })
        .map(|progress| (progress.completed(), progress.total()))
        .collect()
}

/// A scripted Toolset whose every Asset attempt commits.
fn committing() -> ScriptedToolset {
    ScriptedToolset::default()
        .with_assets(|_, _, _| Ok(AssetExecutionResult::success(MutationState::Committed)))
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
    let factory = FakeBackendFactory::default();
    let calls = factory.calls.clone();

    let (result, events) = run(
        &backend_toolset(factory),
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
        *calls.lock().unwrap(),
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
    assert_eq!(processing_progress(&events), [(0, 2), (1, 2), (2, 2)]);
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
        &backend_toolset(FakeBackendFactory::default()),
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
        &backend_toolset(FakeBackendFactory::default()),
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
        &backend_toolset(FakeBackendFactory::default()),
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
        &backend_toolset(FakeBackendFactory::default()),
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
    let toolset = Arc::new(committing());
    let service = OptimizationRunService::with_scheduler(
        scheduler.clone(),
        Some(toolset_configuration(Some(toolset.clone()))),
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
        toolset.attempted().len(),
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
    let toolset = Arc::new(committing().cancelling_at_attempt(2));

    let (result, _) = run(
        &toolset,
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
    let toolset = Arc::new(
        ScriptedToolset::default().with_assets(|_, _, _| Err(AssetInitializationCancelled)),
    );

    let (result, _) = run(
        &toolset,
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
        &backend_toolset(FakeBackendFactory::default()),
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
    let toolset = Arc::new(ScriptedToolset::default().with_assets(|asset, _, _| {
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
        &toolset,
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
    let toolset = Arc::new(
        ScriptedToolset::default().with_assets(|_, _, _| std::panic::panic_any(PanicsAgainOnDrop)),
    );

    let (result, _) = run(
        &toolset,
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
    let (result, _) = run(
        &backend_toolset(FakeBackendFactory::default()),
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

/// The Apply request over native Textures and Archive extraction that most
/// C++ `AssetRunTests` scenarios ran under.
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

/// Origin: AssetRunTests::completeAttemptEvidenceSurvivesAdapters.
/// Successful and failed attempt evidence, and the Archive Finalization
/// attempt after them, outlive the adapters that produced them, and a safe
/// failure does not stop the run reaching finalization.
#[test]
fn attempt_evidence_outlives_its_adapters() {
    let _serial = serial();
    let root = canonical(&scratch_dir("loose-evidence-outlives-adapters"));
    write_tree(&root, &["a.dds", "b.dds"]);
    let toolset = Arc::new(
        ScriptedToolset::default()
            .with_assets(|asset, _, _| {
                Ok(if asset.execution_path().ends_with("a.dds") {
                    AssetExecutionResult::success(MutationState::Committed)
                } else {
                    AssetExecutionResult::failed(AssetExecutionFailure::LoadFailed, "retained")
                })
            })
            .with_finalization_result(
                ArchiveFinalizationResult {
                    attempts: vec![ArchiveFinalizationAttempt {
                        mutation: MutationState::Committed,
                        ..ArchiveFinalizationAttempt::new(root.join("output.bsa"), root.clone())
                    }],
                    ..ArchiveFinalizationResult::default()
                },
                None,
            ),
    );

    let (result, _) = run(&toolset, archive_and_textures(&root));
    let finalized = toolset.finalizations();
    drop(toolset);

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
    assert_eq!(
        result.archive_finalization().unwrap().attempts[0].mod_root,
        root
    );
    assert!(result.routing_ledger().is_some());
}

/// Origin: AssetRunTests::cancelledArchiveFinalizationIsReported. A
/// finalization that observed cancellation makes the run Cancelled, with no
/// finalization failure.
#[test]
fn a_cancelled_finalization_is_reported() {
    let _serial = serial();
    let root = canonical(&scratch_dir("loose-finalization-cancelled"));
    write_tree(&root, &["textures/native.dds"]);
    let toolset = Arc::new(ScriptedToolset::default().with_finalization_result(
        ArchiveFinalizationResult {
            cancelled: true,
            ..ArchiveFinalizationResult::default()
        },
        None,
    ));

    let (result, _) = run(&toolset, archive_and_textures(&root));

    assert_eq!(toolset.finalizations(), 1);
    assert_eq!(toolset.attempted().len(), 1);
    assert!(result.cancellation_observed());
    assert_eq!(result.outcome(), RunOutcome::Cancelled);
    let finalization = result.archive_finalization().unwrap();
    assert!(finalization.cancelled);
    assert_eq!(finalization.failure, None);
}

/// Origin: AssetRunTests::throwingAttemptRetainsCancellation, under the
/// spec's panic rule. An attempt during which cancellation is requested and
/// which then panics becomes an unsafe attempt with unknown mutation, the
/// cancellation is still observed, and nothing runs after it.
#[test]
fn a_panicking_attempt_keeps_the_cancellation_it_requested() {
    let _serial = serial();
    let root = scratch_dir("loose-panicking-attempt-cancels");
    write_tree(&root, &["a.dds", "b.dds"]);
    let toolset = Arc::new(
        ScriptedToolset::default()
            .cancelling_at_attempt(1)
            .with_assets(|_, _, _| panic!("adapter failed")),
    );

    let (result, _) = run(&toolset, archive_and_textures(&root));

    assert_eq!(result.outcome(), RunOutcome::Failed);
    assert!(result.cancellation_observed());
    assert_eq!(result.asset_attempts().len(), 1);
    let attempt = &result.asset_attempts()[0].result;
    assert_eq!(attempt.mutation_state(), MutationState::PartialOrUnknown);
    assert!(!attempt.safe_to_continue());
    assert_eq!(attempt.message(), "adapter failed");
}

/// Origin: AssetRunTests::throwingFinalizerPropagates. A finalizer's error
/// reaches the Run Executor unchanged: the run fails at Archive Finalization,
/// and no cancellation is invented. A finalizer's panic is contained at the
/// Archive Finalization port instead (#546), as the phase's Unexpected
/// Exception that forbids continuation, so the run fails with no cancellation
/// and no Run Failure.
#[test]
fn a_failing_finalizer_fails_the_run_at_archive_finalization() {
    let _serial = serial();
    for (row, panics) in [("error", false), ("panic", true)] {
        let root = scratch_dir(&format!("loose-failing-finalizer-{row}"));
        let toolset = if panics {
            ScriptedToolset::default()
                .with_finalization(|_, _, _| panic!("Archive Finalization failed"))
        } else {
            ScriptedToolset::default()
                .with_escaping_finalization_work("Archive Finalization failed")
        };

        let (result, _) = run(&Arc::new(toolset), archive_and_textures(&root));

        assert_eq!(result.outcome(), RunOutcome::Failed, "{row}");
        assert!(!result.cancellation_observed(), "{row}");
        if panics {
            assert!(result.failures().is_empty(), "{:?}", result.failures());
            let finalization = result.archive_finalization().unwrap();
            assert_eq!(
                finalization.failure,
                Some(ArchiveFinalizationFailure::UnexpectedException)
            );
            assert!(!finalization.safe_to_continue);
            assert!(finalization.detail.contains("Archive Finalization failed"));
            continue;
        }
        let failure = &result.failures()[0];
        assert_eq!(failure.code, RunFailureCode::RunWorkEscaped, "{row}");
        assert_eq!(failure.phase, RunPhase::ArchiveFinalization, "{row}");
        assert!(
            failure.detail.contains("Archive Finalization failed"),
            "{row}"
        );
    }
}

/// Origin: AssetRunTests::throwingObserversPreserveCommittedWork. A panicking
/// Run Observation Sink is retained as `ObserverFailed` diagnostics; it
/// discards no committed attempt, is never a Run Failure, and does not
/// prevent finalization. The sink panics on the Processing Assets record
/// that plans the work and on the one that reports the attempt's progress,
/// standing in for C++'s diagnostics observer and progress adapter.
#[test]
fn panicking_presentation_adapters_preserve_committed_work() {
    let _serial = serial();
    let root = scratch_dir("loose-panicking-presentation");
    write_tree(&root, &["a.dds"]);
    let toolset = Arc::new(committing());
    let sink = RecordingSink::with_hook(|observed| {
        if let Observed::Phase(record) = observed
            && record.phase() == RunPhase::ProcessingAssets
        {
            match record.progress().map(|progress| progress.completed()) {
                Some(0) => panic!("the phase observer panicked"),
                Some(_) => panic!("the progress observer panicked"),
                None => {}
            }
        }
    });

    let result = execute_observed(
        &toolset,
        &archive_and_textures(&root),
        &sink,
        &CancellationToken::new(),
    );

    assert_eq!(result.outcome(), RunOutcome::Succeeded);
    assert_eq!(result.asset_attempts().len(), 1);
    assert_eq!(
        result.asset_attempts()[0].result.mutation_state(),
        MutationState::Committed
    );
    assert_eq!(result.diagnostics().len(), 2);
    assert_eq!(observer_failures(&result), 2);
    assert!(result.failures().is_empty());
    assert_eq!(toolset.finalizations(), 1);
}

/// Origin: AssetRunTests::relativeSelectionRetainsMutationScope. A Mod Root
/// selected by a relative path is recorded as its canonical absolute root,
/// even when the attempt removes the Asset it ran on.
#[test]
fn a_relative_selection_records_the_canonical_mod_root() {
    let _serial = serial();
    let root = canonical(&scratch_dir("loose-relative-selection"));
    write_tree(&root, &["a.dds"]);
    let toolset = Arc::new(ScriptedToolset::default().with_assets(|asset, _, _| {
        std::fs::remove_file(asset.execution_path()).unwrap();
        Ok(AssetExecutionResult::success(MutationState::Committed))
    }));
    let relative = common::relative_to_working_directory(&root);

    let (result, _) = run(&toolset, archive_and_textures(&relative));

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
        let toolset = Arc::new(ScriptedToolset::default().with_assets({
            let attempts = attempts.clone();
            move |asset, _, _| {
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
        }));

        let (result, events) = run(&toolset, all_loose_targets(&root));

        let attempted = if safe { 2 } else { failed_attempt };
        assert_eq!(result.asset_attempts().len(), attempted, "{row}");
        assert_eq!(toolset.finalizations(), usize::from(safe), "{row}");
        assert!(!result.cancellation_observed(), "{row}");
        let failures: Vec<_> = result
            .asset_attempts()
            .iter()
            .filter(|attempt| !attempt.result.succeeded())
            .collect();
        assert_eq!(failures.len(), 1, "{row}");
        // The plan, then one update per completed attempt against a fixed total.
        let progress = processing_progress(&events);
        assert_eq!(progress.len(), attempted + 1, "{row}");
        assert_eq!(progress.last(), Some(&(attempted, 2)), "{row}");
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
        let (failure, mutation) = if safe {
            (AssetExecutionFailure::OperationFailed, MutationState::None)
        } else {
            (
                AssetExecutionFailure::BackendException,
                MutationState::PartialOrUnknown,
            )
        };
        let toolset = Arc::new(ScriptedToolset::default().with_assets({
            let attempts = attempts.clone();
            move |asset, _, _| {
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
        }));

        let (result, events) = run(&toolset, all_loose_targets(&root));

        let attempted = if safe { 2 } else { failed_attempt };
        assert_eq!(result.asset_attempts().len(), attempted, "{row}");
        assert_eq!(toolset.finalizations(), usize::from(safe), "{row}");
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
        // The plan, then one update per completed attempt against a fixed total.
        let progress = processing_progress(&events);
        assert_eq!(progress.len(), attempted + 1, "{row}");
        assert_eq!(progress[0], (0, 2), "{row}");
        assert_eq!(progress[1], (1, 2), "{row}");
        if attempted == 2 {
            assert_eq!(progress[2], (2, 2), "{row}");
        }
    }
}

/// Origin: AssetRunTests::filesystemTraversalPollsCancellation (the
/// extraction-disabled and no-selected-archives rows). Cancellation
/// requested as Discovering Archives begins stops the walk of a tree of
/// unsupported files: no routing, no attempt, no finalization, and the run
/// never leaves Discovering Archives.
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
        let toolset = Arc::new(ScriptedToolset::default());
        let stop = CancellationToken::new();
        let sink = RecordingSink::with_hook({
            let stop = stop.clone();
            move |observed| {
                if let Observed::Phase(record) = observed
                    && record.phase() == RunPhase::DiscoveringArchives
                {
                    stop.cancel();
                }
            }
        });

        let result = execute_observed(&toolset, &requested(&root), &sink, &stop);

        assert_eq!(result.outcome(), RunOutcome::Cancelled, "{row}");
        assert!(result.cancellation_observed(), "{row}");
        assert!(result.routing_ledger().is_none(), "{row}");
        assert_eq!(result.final_phase(), RunPhase::DiscoveringArchives, "{row}");
        assert!(toolset.attempted().is_empty(), "{row}");
        assert_eq!(toolset.finalizations(), 0, "{row}");
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
    let toolset = ScriptedToolset::default()
        .with_assets({
            let order = order.clone();
            move |_, _, _| {
                order.lock().unwrap().push("execute");
                Ok(AssetExecutionResult::success(MutationState::None))
            }
        })
        .with_finalization({
            let order = order.clone();
            move |_, recorder, _| {
                order.lock().unwrap().push("finalize");
                record_finalization_result(recorder, ArchiveFinalizationResult::default(), None)
            }
        });
    let service = OptimizationRunService::with_scheduler(
        Arc::new(InlineRunScheduler),
        Some(toolset_configuration(Some(Arc::new(toolset)))),
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
    let finalize_target = root.join("content.bsa");
    // A finalizer that ran would visibly change the tree.
    let toolset = Arc::new(
        ScriptedToolset::default().with_finalization(move |_, _, _| {
            std::fs::remove_file(&finalize_target).unwrap();
            Ok(())
        }),
    );

    let (result, events) = run(
        &toolset,
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
    assert_eq!(toolset.finalizations(), 0);
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
    assert_eq!(processing_progress(&events), [(0, 2), (1, 2), (2, 2)]);
}
