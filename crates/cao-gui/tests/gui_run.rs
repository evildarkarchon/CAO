//! The window's presentation of one Optimization Run, ported from the C++
//! `tests/GuiRunTests.cpp` scenarios (#510).
//!
//! Each scenario feeds [`RunView`] the Run Events a run publishes, built with
//! the real lifecycle types, and checks what the window would show: the
//! progress label and counts, the terminal label, the run-detail rows, and
//! whether a close or a new run may proceed. Terminal results come from the
//! real [`RunExecutor`] driven by scripted Run Toolsets over temporary Mod
//! Roots, so the view is only tested against results a real run can produce
//! (#554). Their Archive extraction and Archive Finalization are core's real
//! work over the fake Archive reader and packer (#587).
//!
//! `tests/GuiRunDispatchTests.cpp` is dropped with Qt's dispatch (spec Testing
//! Decisions); its `closeWaitsForCleanupAndTerminalDelivery` intent is
//! `run_close.rs`.

use std::fs::{File, OpenOptions};
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use cao_core::execution::{AssetExecutionFailure, AssetExecutionResult, MutationState};
use cao_core::routing::{ExecutionMode, RequestedWork};
use cao_core::run::{
    ArchiveFinalizationFailure, ArchivePrecedence, CancellationToken, ModSelection,
    OptimizationRunResult, PhaseSkipReason, RunDiagnostic, RunDiagnosticCode, RunEvent,
    RunEventPayload, RunFailure, RunFailureCode, RunOutcome, RunPhase, RunPhaseRecord, RunProgress,
    RunRequest,
};
use cao_core::test_support::{
    CountingCleanup, FakeArchivePacker, FakeArchiveReader, Observed, RecordingSink,
    ScriptedToolset, ToolsetConfiguration, entry, execute_run, write_archive,
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

/// Executes `request` through the real Run Executor under an SSE-like profile
/// that supports every kind of work, its work done by a fresh Run Toolset
/// over `toolset`. Live facts reach `sink`, whose hook may cancel through
/// `stop`, and Safety Cleanup reports `cleanup`.
fn execute(
    request: &RunRequest,
    toolset: &Arc<ScriptedToolset>,
    sink: &RecordingSink,
    cleanup: Vec<RunFailure>,
    stop: &CancellationToken,
) -> Arc<OptimizationRunResult> {
    let provider = ToolsetConfiguration::sse(Arc::clone(toolset));
    let mut cleanup = CountingCleanup {
        failures: cleanup,
        ..CountingCleanup::default()
    };
    Arc::new(execute_run(
        request,
        &mut cleanup,
        Some(sink),
        Some(&provider),
        stop,
    ))
}

/// Writes `count` loose Textures, `textures/asset-0.dds` onwards, into
/// `root`: beneath a directory, since files directly in a Mod Root are never
/// packed.
fn write_textures(root: &Path, count: usize) {
    std::fs::create_dir_all(root.join("textures")).unwrap();
    for index in 0..count {
        std::fs::write(root.join(format!("textures/asset-{index}.dds")), b"texture").unwrap();
    }
}

/// A scripted Run Toolset running core's real Archive extraction and Archive
/// Finalization over `reader` and `packer` (#587).
fn real_archive_work(reader: FakeArchiveReader, packer: FakeArchivePacker) -> ScriptedToolset {
    ScriptedToolset {
        reader: Arc::new(reader),
        packer: Arc::new(packer),
        ..ScriptedToolset::default()
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
///   packs the three Textures into its one planned output Archive;
/// - contained failure: as all successful, but the `asset-2.dds` attempt
///   fails safely;
/// - cancelled: the run covers eight Textures and is cancelled once the
///   Processing Assets record counts three attempts, leaving five unattempted;
/// - failing finalizer: Archive Finalization plans three outputs, one
///   Texture each, packs the first, then the packer panics writing the
///   second: an unsafe Unexpected Exception that stops the third.
///
/// Each is an Apply run because a completed Dry Run ends at a planned Archive
/// Finalization, which has no counts for the terminal view to show.
fn classified_run(scenario: Scenario) -> Arc<OptimizationRunResult> {
    let root = scratch_dir(&format!("{scenario:?}"));
    let textures = match scenario {
        Scenario::Cancelled => 8,
        _ => 3,
    };
    write_textures(&root, textures);
    let mut packer = FakeArchivePacker::default();
    if scenario == Scenario::FailingFinalizer {
        // Room for one seven-byte Texture per Archive.
        packer.archive_limit = 10;
        let writes = AtomicUsize::new(0);
        packer.after_write = Some(Box::new(move |_| {
            if writes.fetch_add(1, Ordering::SeqCst) == 1 {
                panic!("the packer is gone");
            }
        }));
    }
    let toolset = real_archive_work(FakeArchiveReader::default(), packer);
    let toolset = if scenario == Scenario::ContainedFailure {
        toolset.with_assets(|asset, _, _| {
            Ok(if asset.execution_path().ends_with("asset-2.dds") {
                AssetExecutionResult::failed(
                    AssetExecutionFailure::SaveFailed,
                    "third attempt failed",
                )
            } else {
                AssetExecutionResult::success(cao_core::CompletedMutation::None)
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

/// A Dry Run's Planned Archive phase reads as planned, not skipped, and
/// shows no attempt counter: a Planned phase attempted nothing.
#[test]
fn a_planned_archive_phase_shows_as_planned_without_attempt_counts() {
    let mut view = RunView::default();
    view.begin("run");

    for (sequence, (phase, label)) in [
        (
            RunPhase::ExtractingArchives,
            "Extracting Archives - Planned (Dry Run)",
        ),
        (
            RunPhase::ArchiveFinalization,
            "Archive Finalization - Planned (Dry Run)",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        assert!(view.consume(&event(
            "run",
            sequence as u64 + 1,
            RunEventPayload::Phase(RunPhaseRecord::planned(phase)),
        )));
        assert_eq!(view.state().label, label);
        assert!(view.state().progress.is_none(), "{phase:?}");
        assert_eq!(view.text(), label);
    }
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
            (2, 3),
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

/// Opens `path` for reading, sharing only reads: while the handle lives, the
/// file cannot be deleted, though other handles may still read it.
fn hold_against_deletion(path: &Path) -> File {
    // `FILE_SHARE_READ`.
    const SHARE_READ: u32 = 0x1;
    OpenOptions::new()
        .read(true)
        .share_mode(SHARE_READ)
        .open(path)
        .unwrap()
}

/// A failed Apply run whose real evidence holds every kind the detail rows
/// show but a Run Failure: an Archive Collision a Loose Asset wins, a
/// committed and a failed extraction, a committed failed Asset save, and an
/// Archive Finalization output whose source cleanup failed. Safety Cleanup
/// reports `cleanup`.
///
/// `winner.bsa` commits its root-level entry, which is never packed;
/// `shadowed.bsa`'s reader panics while staging, a safe failure, so the run
/// goes on to the Asset and Archive Finalization evidence. The Loose Texture
/// is the one packed source. Its handle denies deletion, so its source
/// cleanup fails after the commit, and once its Archive is written a
/// whole-file lock makes it unreadable as recovery material, which forbids
/// continuation and so fails the run.
fn evidence_run(cleanup: Vec<RunFailure>) -> Arc<OptimizationRunResult> {
    let root = scratch_dir("evidence");
    for name in ["winner", "shadowed"] {
        write_archive(
            &root.join(format!("{name}.bsa")),
            &[
                entry("textures/a.dds", name.as_bytes()),
                entry(&format!("{name}.txt"), name.as_bytes()),
            ],
        );
    }
    std::fs::create_dir_all(root.join("textures")).unwrap();
    let loose = root.join("textures/a.dds");
    std::fs::write(&loose, b"loose").unwrap();
    let held = Arc::new(hold_against_deletion(&loose));

    let reader = FakeArchiveReader {
        on_extract: Some(Box::new(|archive, _| {
            if archive.ends_with("shadowed.bsa") {
                panic!("merge interrupted");
            }
        })),
        ..FakeArchiveReader::default()
    };
    let packer = FakeArchivePacker {
        after_write: Some(Box::new({
            let held = Arc::clone(&held);
            move |_| held.try_lock().unwrap()
        })),
        ..FakeArchivePacker::default()
    };
    let toolset = real_archive_work(reader, packer).with_assets(|asset, _, _| {
        Ok(
            AssetExecutionResult::failed(AssetExecutionFailure::SaveFailed, "asset save failed")
                .with_mutation(MutationState::Committed)
                .with_path(asset.execution_path())
                .with_operation("save_texture")
                .with_service_detail("backend detail"),
        )
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
    let result = execute(
        &request,
        &Arc::new(toolset),
        &RecordingSink::default(),
        cleanup,
        &CancellationToken::new(),
    );
    // The packer's clone of the held handle went with the run's Toolset;
    // closing this last one releases the lock and the deletion denial, so the
    // next run can clear the scratch tree.
    drop(held);
    result
}

/// Origin: GuiRunTests::presentsTerminalEvidence. A failed run's detail rows
/// show its attempts, collision winners, cleanup failures and retained
/// mutations, and a late diagnostic keeps the terminal label. The streamed
/// failure half is [`a_streamed_run_failure_is_listed_once`]: no real run
/// reaches a Run Failure after its Archive work.
#[test]
fn the_terminal_result_lists_its_evidence() {
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
    assert!(result.failures().is_empty(), "{:?}", result.failures());
    let root = result.mod_roots()[0].clone();
    let [winner, shadowed] = result.archive_extraction_attempts() else {
        panic!(
            "two extractions: {:?}",
            result.archive_extraction_attempts()
        );
    };
    assert_eq!(winner.outcome.mutation(), MutationState::Committed);
    assert!(!shadowed.succeeded() && shadowed.safe_to_continue());
    let finalization = result.archive_finalization().unwrap();
    let [packed] = finalization.attempts.as_slice() else {
        panic!("one output: {finalization:?}");
    };
    assert_eq!(
        packed.outcome.failure_category(),
        Some(ArchiveFinalizationFailure::SourceCleanupFailed)
    );
    assert_eq!(packed.outcome.mutation(), MutationState::Committed);
    assert!(!finalization.safe_to_continue);
    let source_cleanup_row = format!(
        "Archive Failure: {} | {}",
        packed.archive_path.display(),
        packed.outcome.failure().unwrap().detail()
    );

    let mut view = RunView::default();
    view.begin("run");
    assert!(view.consume(&event("run", 1, executed(RunPhase::SafetyCleanup, None))));
    assert!(view.consume(&event("run", 2, RunEventPayload::Terminal(result))));

    let details = view.state().details.join("\n");
    let shown = |path: &str| root.join(path).display().to_string();
    for expected in [
        "Cleanup Failure: cleanup failure | staging.tmp".to_owned(),
        source_cleanup_row,
        format!(
            "Archive Failure: {} | merge interrupted",
            shown("shadowed.bsa")
        ),
        format!(
            "Asset Failure: {0} | save_texture | asset save failed | {0} | backend detail",
            root.join("textures").join("a.dds").display()
        ),
        format!(
            "Archive Collision: {} | textures/a.dds | winner={} | loose-asset-wins=yes | shadowed={}",
            root.display(),
            shown("winner.bsa"),
            shown("shadowed.bsa")
        ),
        format!(
            "Committed Mutations Retained: {} | Archive Extraction | committed=1 | partial-or-unknown=0",
            root.display()
        ),
        format!(
            "Committed Mutations Retained: {} | Asset Processing | committed=1 | partial-or-unknown=0",
            root.display()
        ),
        "Cancellation Observed: no".to_owned(),
    ] {
        assert!(
            view.state().details.contains(&expected),
            "{expected} missing from\n{details}"
        );
    }
    let progress = view.state().progress.unwrap();
    assert_eq!((progress.completed(), progress.total()), (1, 1));

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

/// Spec (#643): a Dry Run with Archive creation shows Archive Finalization
/// as planned, and its terminal rows list each planned output Archive and
/// each Dummy Plugin it would need, with the note that the plan is an
/// estimate from the Mod Roots as they are now. The plan is a real Dry
/// Run's: two Textures split into two Archives, each with its own plugin.
#[test]
fn a_dry_run_lists_its_planned_archive_creation() {
    let root = scratch_dir("dry-run-plan");
    write_textures(&root, 2);
    let packer = FakeArchivePacker {
        // Room for one seven-byte Texture per Archive.
        archive_limit: 10,
        ..FakeArchivePacker::default()
    };
    let toolset = Arc::new(real_archive_work(FakeArchiveReader::default(), packer));
    let request = RunRequest::new(
        "SkyrimSE",
        ExecutionMode::DryRun,
        ModSelection::SingleModRoot(root),
        vec![
            RequestedWork::NativeTextureOptimization,
            RequestedWork::ArchiveCreation,
        ],
    );
    let result = execute(
        &request,
        &toolset,
        &RecordingSink::default(),
        Vec::new(),
        &CancellationToken::new(),
    );
    assert_eq!(result.outcome(), RunOutcome::Succeeded);
    let root = result.mod_roots()[0].clone();
    let planned_phase = *result.phase(RunPhase::ArchiveFinalization).unwrap();

    let mut view = RunView::default();
    view.begin("run");
    assert!(view.consume(&event("run", 1, RunEventPayload::Phase(planned_phase))));
    assert_eq!(
        view.state().label,
        "Archive Finalization - Planned (Dry Run)"
    );
    assert!(view.consume(&event("run", 2, executed(RunPhase::SafetyCleanup, None))));
    assert!(view.consume(&event("run", 3, RunEventPayload::Terminal(result))));

    let details = view.state().details.join("\n");
    let shown = |name: &str| root.join(name).display().to_string();
    let output = |archive: &str| {
        format!(
            "Planned Archive: {} | {} | Textures | files=1 | bytes=7 | delete-sources=yes",
            shown(archive),
            root.display()
        )
    };
    for expected in [
        output("dry-run-plan - Textures.bsa"),
        format!("Planned Dummy Plugin: {}", shown("dry-run-plan.esp")),
        output("dry-run-plan0 - Textures.bsa"),
        format!("Planned Dummy Plugin: {}", shown("dry-run-plan0.esp")),
        "Planned Archive Creation: an estimate from the Mod Roots as they are now; an Apply \
         run plans after Archive extraction and Loose Asset optimization, so its Archives \
         can differ"
            .to_owned(),
    ] {
        assert!(
            view.state().details.contains(&expected),
            "{expected} missing from\n{details}"
        );
    }
    assert!(
        !root.join("dry-run-plan.esp").exists(),
        "nothing is created"
    );
}

/// Origin: GuiRunTests::presentsTerminalEvidence (the streamed failure). A
/// Run Failure the view already showed as it streamed is not repeated when
/// the terminal result arrives. The failure is real: an unreadable Archive
/// stops the run in Discovering Archives, before any extraction.
#[test]
fn a_streamed_run_failure_is_listed_once() {
    let root = scratch_dir("streamed-failure");
    let broken = root.join("broken.bsa");
    std::fs::write(&broken, "not an archive").unwrap();
    let request = RunRequest::new(
        "SkyrimSE",
        ExecutionMode::Apply,
        ModSelection::SingleModRoot(root),
        vec![RequestedWork::ArchiveExtraction],
    );
    let result = execute(
        &request,
        &Arc::new(real_archive_work(
            FakeArchiveReader::default(),
            FakeArchivePacker::default(),
        )),
        &RecordingSink::default(),
        Vec::new(),
        &CancellationToken::new(),
    );
    assert_eq!(result.outcome(), RunOutcome::Failed);
    let [failure] = result.failures() else {
        panic!("one Run Failure expected: {:?}", result.failures());
    };
    assert_eq!(failure.code, RunFailureCode::ArchiveUnreadable);
    assert_eq!(failure.path, broken);
    let row = format!("Failure: {} | {}", failure.detail, broken.display());

    let mut view = RunView::default();
    view.begin("run");
    assert!(view.consume(&event("run", 1, RunEventPayload::Failure(failure.clone()))));
    assert!(view.consume(&event("run", 2, RunEventPayload::Terminal(result))));

    let details = view.state().details.join("\n");
    let shown = view
        .state()
        .details
        .iter()
        .filter(|detail| **detail == row)
        .count();
    assert_eq!(shown, 1, "{details}");
    assert_eq!(view.state().label, "Failed");
}
