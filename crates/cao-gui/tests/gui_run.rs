//! The window's presentation of one Optimization Run, ported from the C++
//! `tests/GuiRunTests.cpp` scenarios (#510).
//!
//! Each scenario feeds [`RunView`] the Run Events a run publishes, built with
//! the real lifecycle types, and checks what the window would show: the
//! progress label and counts, the terminal label, the run-detail rows, and
//! whether a close or a new run may proceed. Terminal results come from the
//! real [`RunExecutor`] over fake work, as the C++ scenarios built theirs.
//!
//! `tests/GuiRunDispatchTests.cpp` is dropped with Qt's dispatch (spec Testing
//! Decisions); its `closeWaitsForCleanupAndTerminalDelivery` intent is
//! `run_close.rs`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cao_core::Error;
use cao_core::execution::{AssetExecutionFailure, AssetExecutionResult, MutationState};
use cao_core::routing::{AssetRouter, ExecutionMode, RequestedWork};
use cao_core::run::{
    ArchiveCollision, ArchiveDiscoveryEvidence, ArchiveExtractionFailure, ArchiveExtractionResult,
    ArchiveFinalizationAttempt, ArchiveFinalizationFailure, ArchiveFinalizationResult,
    CancellationToken, ModSelection, OptimizationRunResult, PhaseSkipReason, RoutedAssetAttempt,
    RunConfiguration, RunConfigurationProvider, RunDiagnostic, RunDiagnosticCode, RunEvent,
    RunEventPayload, RunExecutor, RunFailure, RunFailureCode, RunOutcome, RunPhase, RunPhaseRecord,
    RunPreparation, RunProgress, RunRequest, RunServices, RunWorkEvidence, RunWorkMilestones,
    RunWorkService, SafetyCleanupService, SelectedProfileFacts, TemporaryArtifactRegistry,
};
use cao_gui::RunView;

// ---- Fakes of the run seams, to build real terminal results

/// A fresh, empty directory under the target directory for one scenario.
fn scratch_dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("cao-gui")
        .join("gui_run")
        .join(name);
    // A missing directory is the expected case; anything else surfaces below.
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// An SSE-like profile that supports every kind of work.
struct AllWorkProfile;

impl RunConfigurationProvider for AllWorkProfile {
    fn load(&self, _profile_identity: &str) -> Result<RunConfiguration, Error> {
        Ok(RunConfiguration {
            profile: SelectedProfileFacts {
                archive_extension: Some(".bsa".to_owned()),
                supports_native_texture_optimization: true,
                supports_texture_conversion: true,
                supports_standard_mesh_optimization: true,
                supports_terrain_mesh_optimization: true,
                supports_animation_optimization: true,
                supports_archive_extraction: true,
                supports_mesh_reference_maintenance: true,
                supports_archive_creation: true,
            },
            ..RunConfiguration::default()
        })
    }
}

/// A Safety Cleanup service reporting the given failures.
struct Cleanup(Vec<RunFailure>);

impl SafetyCleanupService for Cleanup {
    fn perform_safety_cleanup(&mut self) -> Result<Vec<RunFailure>, Error> {
        Ok(std::mem::take(&mut self.0))
    }
}

/// Executes `request` with `work` and `cleanup` through the real Run Executor.
fn execute(
    request: &RunRequest,
    work: &dyn RunWorkService,
    cleanup: Vec<RunFailure>,
    stop: &CancellationToken,
) -> Arc<OptimizationRunResult> {
    Arc::new(RunExecutor.execute(
        request,
        RunServices {
            safety_cleanup: &mut Cleanup(cleanup),
            observations: None,
            configuration: Some(&AllWorkProfile),
            work: Some(work),
        },
        stop,
        "run".to_owned(),
    ))
}

/// How the classified run's attempts turn out (C++ `ClassificationScenario`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scenario {
    AllSuccessful,
    ContainedFailure,
    Cancelled,
    UnsafeFailure,
}

/// Completes three routed Texture attempts, the third turning out as the
/// scenario says, and stops in Processing Assets without reaching Archive
/// Finalization, as the C++ fake did. The cancelled scenario routes eight
/// Textures and cancels after the third attempt, leaving five unattempted.
struct ClassifiedWork(Scenario);

impl RunWorkService for ClassifiedWork {
    fn execute(
        &self,
        preparation: &RunPreparation,
        evidence: &RunWorkEvidence<'_, '_>,
        _artifacts: &mut TemporaryArtifactRegistry,
        milestones: &dyn RunWorkMilestones,
        stop: &CancellationToken,
    ) -> Result<(), Error> {
        let scenario = self.0;
        let root = preparation.mod_roots()[0].clone();
        milestones.archive_discovery_started()?;
        evidence.record_archive_discovery(ArchiveDiscoveryEvidence::default())?;
        milestones.dry_run_archive_extraction()?;
        milestones.effective_asset_tree_started()?;
        let total = if scenario == Scenario::Cancelled {
            8
        } else {
            3
        };
        let ledger = AssetRouter::new(preparation.policy().clone())
            .route_all((0..total).map(|index| root.join(format!("asset-{index}.dds"))));
        let assets = ledger.routed_assets().to_vec();
        evidence.record_routing_ledger(ledger)?;
        milestones.asset_processing_planned(total)?;
        let third = match scenario {
            Scenario::AllSuccessful | Scenario::Cancelled => {
                AssetExecutionResult::success(MutationState::None)
            }
            Scenario::ContainedFailure => AssetExecutionResult::failed(
                AssetExecutionFailure::SaveFailed,
                "third attempt failed",
            ),
            Scenario::UnsafeFailure => AssetExecutionResult::failed(
                AssetExecutionFailure::SaveFailed,
                "third attempt failed",
            )
            .with_mutation(MutationState::PartialOrUnknown),
        };
        let results = [
            AssetExecutionResult::success(MutationState::None),
            AssetExecutionResult::success(MutationState::None),
            third,
        ];
        for (asset, result) in assets.into_iter().zip(results) {
            evidence.record_asset_attempt(
                RoutedAssetAttempt {
                    mod_root: root.clone(),
                    asset,
                    result,
                },
                total,
            )?;
        }
        if scenario == Scenario::Cancelled {
            stop.cancel();
        }
        Ok(())
    }
}

/// The terminal result of a Dry Run whose Textures turn out as `scenario`
/// says, with the number of Textures it routed.
fn classified_run(scenario: Scenario) -> (Arc<OptimizationRunResult>, usize) {
    let total = if scenario == Scenario::Cancelled {
        8
    } else {
        3
    };
    let request = RunRequest::new(
        "SkyrimSE",
        ExecutionMode::DryRun,
        ModSelection::SingleModRoot(scratch_dir(&format!("{scenario:?}"))),
        vec![RequestedWork::NativeTextureOptimization],
    );
    let result = execute(
        &request,
        &ClassifiedWork(scenario),
        Vec::new(),
        &CancellationToken::new(),
    );
    (result, total)
}

/// A Run Event of run `run_id` at `sequence`.
fn event(run_id: &str, sequence: u64, payload: RunEventPayload) -> RunEvent {
    RunEvent {
        run_id: run_id.to_owned(),
        sequence,
        payload,
    }
}

/// A Phase event for an executed phase.
fn executed(phase: RunPhase, progress: Option<RunProgress>) -> RunEventPayload {
    RunEventPayload::Phase(RunPhaseRecord::executed(phase, progress))
}

/// Origin: GuiRunTests::rejectsStaleObservations. An old run's events and
/// repeated or out-of-order events never overwrite the current run's view.
#[test]
fn stale_and_foreign_events_never_overwrite_the_current_run() {
    let mut view = RunView::default();
    view.begin("new");
    assert_eq!(view.state().label, "Preparing");

    assert!(!view.consume(&event("old", 9, executed(RunPhase::SafetyCleanup, None))));
    assert!(view.consume(&event("new", 2, executed(RunPhase::ProcessingAssets, None))));
    assert!(!view.consume(&event("new", 1, executed(RunPhase::Preparing, None))));
    assert!(!view.consume(&event("new", 2, executed(RunPhase::Preparing, None))));
    assert_eq!(view.state().label, "Processing Assets");
}

/// Origin: GuiRunTests::presentsAuthoritativeProgress. Failed attempts count as
/// completed; skipped and Safety Cleanup phases have no counts.
#[test]
fn progress_shows_the_phase_and_its_attempt_counts() {
    let mut view = RunView::default();
    view.begin("run");

    assert!(view.consume(&event(
        "run",
        1,
        executed(
            RunPhase::ProcessingAssets,
            Some(RunProgress::determinate(9, 2, 1))
        ),
    )));
    assert_eq!(view.state().label, "Processing Assets");
    let progress = view.state().progress.unwrap();
    assert_eq!((progress.completed(), progress.total()), (3, 9));
    assert_eq!(
        view.text(),
        "Processing Assets - 3 / 9 attempts (2 succeeded, 1 failed)"
    );

    assert!(view.consume(&event(
        "run",
        2,
        RunEventPayload::Phase(RunPhaseRecord::skipped(
            RunPhase::ArchiveFinalization,
            PhaseSkipReason::DryRun
        )),
    )));
    assert!(view.state().progress.is_none());
    assert_eq!(
        view.state().label,
        "Archive Finalization - Skipped (Dry Run)"
    );
    assert!(view.consume(&event(
        "run",
        3,
        RunEventPayload::Phase(RunPhaseRecord::skipped(
            RunPhase::ExtractingArchives,
            PhaseSkipReason::NoRequestedWork
        )),
    )));
    assert_eq!(
        view.state().label,
        "Extracting Archives - Skipped (No Requested Work)"
    );

    assert!(view.consume(&event("run", 4, executed(RunPhase::SafetyCleanup, None))));
    assert_eq!(view.state().label, "Safety Cleanup");
    assert_eq!(view.text(), "Safety Cleanup");
    assert!(view.state().active);
    assert!(view.state().outcome.is_none());
}

/// Origin: GuiRunTests::preservesUnicodePaths. A path the ANSI code page
/// cannot encode reaches the detail rows intact.
#[test]
fn detail_rows_keep_unicode_paths() {
    let mut view = RunView::default();
    view.begin("run");
    let path = std::path::PathBuf::from("mods/\u{65e5}\u{672c}/file.bsa");

    assert!(
        view.consume(&event(
            "run",
            1,
            RunEventPayload::Failure(
                RunFailure::new(
                    RunFailureCode::WorkServiceFailed,
                    RunPhase::ExtractingArchives,
                    "archive failure",
                )
                .with_path(&path)
            ),
        ))
    );
    assert_eq!(
        view.state().details,
        ["Failure: archive failure | mods/\u{65e5}\u{672c}/file.bsa"]
    );
}

/// Origin: GuiRunTests::presentsTerminalClassification. Cancellation is only
/// intent until the terminal result arrives, only success reads as Done, and
/// cancelling twice, or after the end, changes nothing.
#[test]
fn the_terminal_result_names_the_outcome_and_cancelling_is_only_intent() {
    for (scenario, outcome, label) in [
        (Scenario::AllSuccessful, RunOutcome::Succeeded, "Done"),
        (
            Scenario::ContainedFailure,
            RunOutcome::CompletedWithFailures,
            "Completed With Failures",
        ),
        (Scenario::Cancelled, RunOutcome::Cancelled, "Cancelled"),
        (Scenario::UnsafeFailure, RunOutcome::Failed, "Failed"),
    ] {
        let (result, total) = classified_run(scenario);
        assert_eq!(result.outcome(), outcome, "{scenario:?}");

        let mut view = RunView::default();
        view.begin("run");
        assert!(view.consume(&event(
            "run",
            1,
            executed(
                RunPhase::ProcessingAssets,
                Some(RunProgress::determinate(total, 0, 0))
            ),
        )));
        view.request_cancellation();
        view.request_cancellation();
        assert!(view.state().active && view.state().cancellation_requested);
        assert_eq!(view.state().label, "Cancelling - Processing Assets");
        assert!(view.state().outcome.is_none());
        assert_eq!(view.state().progress.unwrap().completed(), 0);

        assert!(view.consume(&event("run", 2, RunEventPayload::Terminal(result))));
        assert_eq!(view.state().label, label, "{scenario:?}");
        assert_eq!(view.state().outcome, Some(outcome));
        assert!(!view.state().active);
        // The final work phase's counts, not Safety Cleanup's (it has none).
        let progress = view.state().progress.unwrap();
        assert_eq!((progress.completed(), progress.total()), (3, total));

        view.request_cancellation();
        assert_eq!(view.state().label, label);
    }
}

/// Origin: GuiRunTests::defersCloseUntilTerminal. A close during a run keeps
/// the window open through Safety Cleanup until the terminal result arrives,
/// and no new run may start before the window closes.
#[test]
fn a_close_during_a_run_waits_for_its_terminal_result() {
    let mut view = RunView::default();
    assert!(view.request_close(), "no run: close at once");
    assert!(view.can_start());

    view.begin("run");
    assert!(!view.can_start());
    assert!(!view.request_close());
    assert!(view.state().close_requested && view.state().cancellation_requested);
    assert!(view.state().active);

    assert!(view.consume(&event("run", 1, executed(RunPhase::SafetyCleanup, None))));
    assert!(!view.request_close(), "Safety Cleanup has not finished");

    let (result, _) = classified_run(Scenario::Cancelled);
    assert!(view.consume(&event("run", 2, RunEventPayload::Terminal(result))));
    assert!(view.request_close());
    assert!(view.state().close_requested);
    assert!(!view.can_start());
}

/// Records evidence of every kind a failed Apply run can retain: an Archive
/// Collision, a committed and a partial extraction, a committed failed Asset
/// save, a finalization whose source cleanup failed, and a primary Run Failure.
struct EvidenceOnlyWork;

impl RunWorkService for EvidenceOnlyWork {
    fn execute(
        &self,
        preparation: &RunPreparation,
        evidence: &RunWorkEvidence<'_, '_>,
        _artifacts: &mut TemporaryArtifactRegistry,
        milestones: &dyn RunWorkMilestones,
        _stop: &CancellationToken,
    ) -> Result<(), Error> {
        let root = preparation.mod_roots()[0].clone();
        milestones.archive_discovery_started()?;
        evidence.record_archive_collisions(&[ArchiveCollision {
            mod_root: root.clone(),
            game_path: PathBuf::from("textures/a.dds"),
            winning_archive: root.join("winner.bsa"),
            shadowed_archives: vec![root.join("shadowed.bsa")],
            loose_asset_wins: true,
        }])?;
        evidence.record_archive_discovery(ArchiveDiscoveryEvidence::default())?;
        milestones.archive_extraction_planned(2)?;
        evidence.record_archive_extraction_attempt(
            ArchiveExtractionResult {
                archive_path: root.join("committed.bsa"),
                mod_root: root.clone(),
                mutation: MutationState::Committed,
                failure: None,
                safe_to_continue: true,
                detail: String::new(),
            },
            2,
        )?;
        evidence.record_archive_extraction_attempt(
            ArchiveExtractionResult {
                archive_path: root.join("input.bsa"),
                mod_root: root.clone(),
                mutation: MutationState::PartialOrUnknown,
                failure: Some(ArchiveExtractionFailure::MergeFailed),
                safe_to_continue: false,
                detail: "merge interrupted".to_owned(),
            },
            2,
        )?;

        milestones.effective_asset_tree_started()?;
        let ledger =
            AssetRouter::new(preparation.policy().clone()).route_all([root.join("failed.dds")]);
        let asset = ledger.routed_assets()[0].clone();
        evidence.record_routing_ledger(ledger)?;
        milestones.asset_processing_planned(1)?;
        evidence.record_asset_attempt(
            RoutedAssetAttempt {
                mod_root: root.clone(),
                asset,
                result: AssetExecutionResult::failed(
                    AssetExecutionFailure::SaveFailed,
                    "asset save failed",
                )
                .with_mutation(MutationState::Committed)
                .with_path(root.join("failed.dds"))
                .with_operation("save_texture")
                .with_service_detail("backend detail"),
            },
            1,
        )?;

        milestones.archive_finalization_available(ExecutionMode::Apply, true)?;
        let attempts = vec![
            ArchiveFinalizationAttempt {
                mutation: MutationState::Committed,
                failure: Some(ArchiveFinalizationFailure::SourceCleanupFailed),
                detail: "source remains usable".to_owned(),
                ..ArchiveFinalizationAttempt::new(root.join("output.bsa"), root.clone())
            },
            ArchiveFinalizationAttempt::new(root.join("output-two.bsa"), root.clone()),
        ];
        evidence.record_archive_finalization_plan(5)?;
        for attempt in &attempts {
            evidence.record_archive_finalization_attempt(attempt.clone(), 5)?;
        }
        evidence.record_archive_finalization(ArchiveFinalizationResult {
            attempts,
            ..ArchiveFinalizationResult::default()
        })?;
        evidence.record_failure(RunFailure::new(
            RunFailureCode::WorkServiceFailed,
            RunPhase::ArchiveFinalization,
            "primary failure",
        ));
        Ok(())
    }
}

/// Origin: GuiRunTests::presentsTerminalEvidence. A failed run's detail rows
/// show its attempts, collision winners, cleanup failures and retained
/// mutations; a streamed failure is not repeated, and a late diagnostic keeps
/// the terminal label.
#[test]
fn the_terminal_result_lists_its_evidence_once() {
    let root = scratch_dir("evidence");
    let request = RunRequest::new(
        "SkyrimSE",
        ExecutionMode::Apply,
        ModSelection::SingleModRoot(root),
        vec![
            RequestedWork::ArchiveExtraction,
            RequestedWork::NativeTextureOptimization,
            RequestedWork::ArchiveCreation,
        ],
    );
    let cleanup = vec![
        RunFailure::new(
            RunFailureCode::TemporaryArtifactCleanupFailed,
            RunPhase::SafetyCleanup,
            "cleanup failure",
        )
        .with_path("staging.tmp"),
    ];
    let result = execute(
        &request,
        &EvidenceOnlyWork,
        cleanup,
        &CancellationToken::new(),
    );
    assert_eq!(result.outcome(), RunOutcome::Failed);

    let mut view = RunView::default();
    view.begin("run");
    assert!(view.consume(&event("run", 1, executed(RunPhase::SafetyCleanup, None))));
    assert!(view.consume(&event(
        "run",
        2,
        RunEventPayload::Failure(result.failures()[0].clone())
    )));
    assert!(view.consume(&event("run", 3, RunEventPayload::Terminal(result))));

    let details = view.state().details.join("\n");
    let primary = view
        .state()
        .details
        .iter()
        .filter(|row| row.contains("Failure: primary failure"))
        .count();
    assert_eq!(primary, 1, "{details}");
    for expected in [
        "primary failure",
        "cleanup failure",
        "staging.tmp",
        "source remains usable",
        "merge interrupted",
        "asset save failed",
        "save_texture",
        "backend detail",
        "winner.bsa",
        "shadowed.bsa",
        "loose-asset-wins=yes",
        "Committed Mutations Retained",
        "committed=1",
        "partial-or-unknown=1",
        "Cancellation Observed: no",
    ] {
        assert!(
            details.contains(expected),
            "{expected} missing from\n{details}"
        );
    }
    let progress = view.state().progress.unwrap();
    assert_eq!((progress.completed(), progress.total()), (2, 5));

    let label = view.state().label.clone();
    assert!(view.consume(&event(
        "run",
        4,
        RunEventPayload::Diagnostic(RunDiagnostic::new(
            RunDiagnosticCode::ObserverFailed,
            RunPhase::SafetyCleanup,
            "late diagnostic",
        )),
    )));
    assert_eq!(view.state().label, label);
    assert!(
        view.state()
            .details
            .last()
            .unwrap()
            .contains("late diagnostic")
    );
}
