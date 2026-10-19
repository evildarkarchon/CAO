//! The window's presentation of one Optimization Run, ported from the C++
//! `tests/GuiRunTests.cpp` scenarios (#510).
//!
//! Each scenario feeds [`RunView`] the Run Events a run publishes, built with
//! the real lifecycle types, and checks what the window would show: the
//! progress label and counts, the terminal label, the run-detail rows, and
//! whether a close or a new run may proceed. Terminal results come from the
//! real [`RunExecutor`] driven by scripted Run Toolsets over temporary Mod
//! Roots, so the view is only tested against results a real run can produce
//! (#554).
//!
//! `tests/GuiRunDispatchTests.cpp` is dropped with Qt's dispatch (spec Testing
//! Decisions); its `closeWaitsForCleanupAndTerminalDelivery` intent is
//! `run_close.rs`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cao_core::Error;
use cao_core::execution::{AssetExecutionFailure, AssetExecutionResult, MutationState};
use cao_core::routing::{ExecutionMode, RequestedWork};
use cao_core::run::{
    ArchiveExtractionFailure, ArchiveExtractionResult, ArchiveFinalizationAttempt,
    ArchiveFinalizationFailure, ArchiveFinalizationResult, ArchivePrecedence, CancellationToken,
    ModSelection, OptimizationRunResult, PhaseSkipReason, RunConfiguration, RunDiagnostic,
    RunDiagnosticCode, RunEvent, RunEventPayload, RunExecutor, RunFailure, RunFailureCode,
    RunOutcome, RunPhase, RunPhaseRecord, RunProgress, RunRequest, RunServices,
    SelectedProfileFacts,
};
use cao_core::test_support::{
    CountingCleanup, Observed, RecordingSink, ScriptedToolset, ToolsetConfiguration, entry,
    record_finalization_result, write_archive,
};
use cao_gui::RunView;

// ---- Real runs over scripted Run Toolsets, to build real terminal results

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
fn all_work_configuration() -> RunConfiguration {
    RunConfiguration {
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
    }
}

/// Executes `request` through the real Run Executor, its work done by a fresh
/// Run Toolset over `toolset`. Live facts reach `sink`, whose hook may cancel
/// through `stop`, and Safety Cleanup reports `cleanup`.
fn execute(
    request: &RunRequest,
    toolset: &Arc<ScriptedToolset>,
    sink: &RecordingSink,
    cleanup: Vec<RunFailure>,
    stop: &CancellationToken,
) -> Arc<OptimizationRunResult> {
    let provider = ToolsetConfiguration::new(all_work_configuration(), Some(Arc::clone(toolset)));
    let mut cleanup = CountingCleanup {
        failures: cleanup,
        ..CountingCleanup::default()
    };
    Arc::new(RunExecutor.execute(
        request,
        RunServices {
            safety_cleanup: &mut cleanup,
            observations: Some(sink),
            configuration: Some(&provider),
        },
        stop,
        "run".to_owned(),
    ))
}

/// Writes `count` loose Textures, `asset-0.dds` onwards, into `root`.
fn write_textures(root: &Path, count: usize) {
    for index in 0..count {
        std::fs::write(root.join(format!("asset-{index}.dds")), b"texture").unwrap();
    }
}

/// How the classified run turns out (C++ `ClassificationScenario`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scenario {
    AllSuccessful,
    ContainedFailure,
    Cancelled,
    FailingFinalizer,
}

/// The terminal result of an Apply run over three loose Textures that turns
/// out as `scenario` says:
/// - all successful: every Asset attempt succeeds, and Archive Finalization
///   packs its one planned output Archive;
/// - contained failure: as all successful, but the `asset-2.dds` attempt
///   fails safely;
/// - cancelled: the run covers eight Textures and is cancelled once the
///   Processing Assets record counts three attempts, leaving five unattempted;
/// - failing finalizer: Archive Finalization plans three outputs, packs one,
///   then fails outside any attempt with an Unexpected Exception.
///
/// Each is an Apply run because a completed Dry Run ends at a skipped Archive
/// Finalization, which has no counts for the terminal view to show.
fn classified_run(scenario: Scenario) -> Arc<OptimizationRunResult> {
    let root = scratch_dir(&format!("{scenario:?}"));
    let textures = match scenario {
        Scenario::Cancelled => 8,
        _ => 3,
    };
    write_textures(&root, textures);
    let toolset = ScriptedToolset::default().with_finalization(move |preparation, recorder, _| {
        let root = preparation.mod_roots()[0].clone();
        let packed = ArchiveFinalizationAttempt {
            mutation: MutationState::Committed,
            ..ArchiveFinalizationAttempt::new(root.join("output.bsa"), root)
        };
        if scenario == Scenario::FailingFinalizer {
            // As production Archive Finalization records its own work's
            // failure: an Unexpected Exception keeping the attempt already
            // made, which forbids continuation and so fails the run.
            return record_finalization_result(
                recorder,
                ArchiveFinalizationResult {
                    attempts: vec![packed],
                    failure: Some(ArchiveFinalizationFailure::UnexpectedException),
                    safe_to_continue: false,
                    detail: "the packer is gone".to_owned(),
                    ..ArchiveFinalizationResult::default()
                },
                Some(3),
            );
        }
        record_finalization_result(
            recorder,
            ArchiveFinalizationResult {
                attempts: vec![packed],
                ..ArchiveFinalizationResult::default()
            },
            None,
        )
    });
    let toolset = if scenario == Scenario::ContainedFailure {
        toolset.with_assets(|asset, _, _| {
            Ok(if asset.execution_path().ends_with("asset-2.dds") {
                AssetExecutionResult::failed(
                    AssetExecutionFailure::SaveFailed,
                    "third attempt failed",
                )
            } else {
                AssetExecutionResult::success(MutationState::None)
            })
        })
    } else {
        toolset
    };
    let stop = CancellationToken::new();
    let token = stop.clone();
    // Every attempt re-fires the Processing Assets record with its counts, so
    // the third attempt's record is where the user cancels.
    let sink = RecordingSink::with_hook(move |observation| {
        if scenario == Scenario::Cancelled
            && let Observed::Phase(record) = observation
            && record.phase() == RunPhase::ProcessingAssets
            && record
                .progress()
                .is_some_and(|progress| progress.completed() == 3)
        {
            token.cancel();
        }
    });
    let request = RunRequest::new(
        "SkyrimSE",
        ExecutionMode::Apply,
        ModSelection::SingleModRoot(root),
        vec![
            RequestedWork::NativeTextureOptimization,
            RequestedWork::ArchiveCreation,
        ],
    );
    execute(&request, &Arc::new(toolset), &sink, Vec::new(), &stop)
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
                    RunFailureCode::RunWorkEscaped,
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
/// cancelling twice, or after the end, changes nothing. The terminal counts
/// are the final work phase's: Archive Finalization's outputs, or Processing
/// Assets' attempts for the run cancelled there.
#[test]
fn the_terminal_result_names_the_outcome_and_cancelling_is_only_intent() {
    for (scenario, outcome, label, (completed, total)) in [
        (
            Scenario::AllSuccessful,
            RunOutcome::Succeeded,
            "Done",
            (1, 1),
        ),
        (
            Scenario::ContainedFailure,
            RunOutcome::CompletedWithFailures,
            "Completed With Failures",
            (1, 1),
        ),
        (
            Scenario::Cancelled,
            RunOutcome::Cancelled,
            "Cancelled",
            (3, 8),
        ),
        (
            Scenario::FailingFinalizer,
            RunOutcome::Failed,
            "Failed",
            (1, 3),
        ),
    ] {
        let result = classified_run(scenario);
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
        assert_eq!(
            (progress.completed(), progress.total()),
            (completed, total),
            "{scenario:?}"
        );

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

    let result = classified_run(Scenario::Cancelled);
    assert!(view.consume(&event("run", 2, RunEventPayload::Terminal(result))));
    assert!(view.request_close());
    assert!(view.state().close_requested);
    assert!(!view.can_start());
}

/// A failed Apply run whose real evidence holds every kind the detail rows
/// show: an Archive Collision a Loose Asset wins, a committed and a failed
/// extraction, a committed failed Asset save, a finalization whose source
/// cleanup failed beside an output left partial, and the primary Run Failure
/// of a finalizer whose work then escapes (simulated, as no real run reaches
/// a Run Failure here). Safety Cleanup reports `cleanup`.
fn evidence_run(cleanup: Vec<RunFailure>) -> Arc<OptimizationRunResult> {
    let root = scratch_dir("evidence");
    for name in ["winner", "shadowed"] {
        write_archive(
            &root.join(format!("{name}.bsa")),
            &[entry("textures/a.dds", name.as_bytes())],
        );
    }
    std::fs::create_dir_all(root.join("textures")).unwrap();
    std::fs::write(root.join("textures/a.dds"), b"loose").unwrap();

    let toolset =
        ScriptedToolset::default()
            .with_extraction(|plan, _| {
                if plan.archive_path.ends_with("winner.bsa") {
                    ArchiveExtractionResult {
                        mutation: MutationState::Committed,
                        ..ArchiveExtractionResult::new(plan)
                    }
                } else {
                    // A failed merge that rolled back is safe, so the run goes
                    // on to the Asset and Archive Finalization evidence; a
                    // partial one would stop it here.
                    ArchiveExtractionResult {
                        failure: Some(ArchiveExtractionFailure::MergeFailed),
                        detail: "merge interrupted".to_owned(),
                        ..ArchiveExtractionResult::new(plan)
                    }
                }
            })
            .with_assets(|asset, _, _| {
                Ok(AssetExecutionResult::failed(
                    AssetExecutionFailure::SaveFailed,
                    "asset save failed",
                )
                .with_mutation(MutationState::Committed)
                .with_path(asset.execution_path())
                .with_operation("save_texture")
                .with_service_detail("backend detail"))
            })
            .with_finalization(|preparation, recorder, _| {
                let root = preparation.mod_roots()[0].clone();
                // The partial output stops finalization after two of five planned
                // outputs; being the last work phase, it still reaches the result.
                let attempts = vec![
                    ArchiveFinalizationAttempt {
                        mutation: MutationState::Committed,
                        failure: Some(ArchiveFinalizationFailure::SourceCleanupFailed),
                        detail: "source remains usable".to_owned(),
                        ..ArchiveFinalizationAttempt::new(root.join("output.bsa"), root.clone())
                    },
                    ArchiveFinalizationAttempt {
                        mutation: MutationState::PartialOrUnknown,
                        failure: Some(ArchiveFinalizationFailure::CommitFailed),
                        safe_to_continue: false,
                        detail: "publication interrupted".to_owned(),
                        ..ArchiveFinalizationAttempt::new(root.join("output-two.bsa"), root.clone())
                    },
                ];
                record_finalization_result(
                    recorder,
                    ArchiveFinalizationResult {
                        attempts,
                        ..ArchiveFinalizationResult::default()
                    },
                    Some(5),
                )?;
                // Simulates the run's work escaping after the phase recorded
                // its result: no real port returns an error that is not a Run
                // Evidence invariant violation, but the executor contains one
                // as Run Work Escaped, which gives the view the primary Run
                // Failure row this scenario needs.
                Err(Error::RunWorkEscaped("primary failure".to_owned()))
            });
    let request = RunRequest::new(
        "SkyrimSE",
        ExecutionMode::Apply,
        ModSelection::SingleModRoot(root),
        vec![
            RequestedWork::ArchiveExtraction,
            RequestedWork::NativeTextureOptimization,
            RequestedWork::ArchiveCreation,
        ],
    )
    // Explicit, so `winner.bsa` outranks `shadowed.bsa` whatever their names.
    .with_archive_precedence(ArchivePrecedence::ExplicitOrder(vec![
        "winner.bsa".into(),
        "shadowed.bsa".into(),
    ]));
    execute(
        &request,
        &Arc::new(toolset),
        &RecordingSink::default(),
        cleanup,
        &CancellationToken::new(),
    )
}

/// Origin: GuiRunTests::presentsTerminalEvidence. A failed run's detail rows
/// show its attempts, collision winners, cleanup failures and retained
/// mutations; a streamed failure is not repeated, and a late diagnostic keeps
/// the terminal label.
#[test]
fn the_terminal_result_lists_its_evidence_once() {
    let cleanup = vec![
        RunFailure::new(
            RunFailureCode::TemporaryArtifactCleanupFailed,
            RunPhase::SafetyCleanup,
            "cleanup failure",
        )
        .with_path("staging.tmp"),
    ];
    let result = evidence_run(cleanup);
    assert_eq!(result.outcome(), RunOutcome::Failed);
    let [failure] = result.failures() else {
        panic!("one Run Failure expected: {:?}", result.failures());
    };
    assert_eq!(failure.phase, RunPhase::ArchiveFinalization);

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
