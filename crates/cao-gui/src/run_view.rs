//! What the window shows of an Optimization Run (C++ `cao::gui::RunViewModel`,
//! `src/GuiRun.cpp`, deleted with the C++ tree in #515).
//!
//! [`RunView`] turns the ordered Run Events of one run into presentation facts:
//! a label naming the phase or the Run Outcome, the phase's attempt counts, and
//! the run-detail rows the Log tab pins below its log rows. It also holds the
//! close-during-run state machine. It knows nothing of Slint or of threads: the
//! window feeds it on the UI thread, in the order the events arrive.

use std::collections::HashMap;
use std::path::Path;

use cao_core::routing::SkipReason;
use cao_core::run::{
    MutationKind, OptimizationRunResult, PhaseSkipReason, RunEvent, RunEventPayload, RunFailure,
    RunId, RunOutcome, RunPhase, RunPhaseStatus, RunProgress,
};

/// Presentation facts copied from one run's ordered events.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunViewState {
    /// The run the view is bound to; events of any other run are ignored.
    pub run_id: RunId,
    /// The phase or terminal label, such as `Processing Assets`, `Cancelling -
    /// Safety Cleanup` or `Completed With Failures`.
    pub label: String,
    /// The current phase's attempt counts; `None` for skipped and
    /// indeterminate phases.
    pub progress: Option<RunProgress>,
    /// The run-detail rows, in arrival order: streamed failures and
    /// diagnostics, then the terminal result's evidence.
    pub details: Vec<String>,
    /// From [`RunView::begin`] until the terminal result arrives.
    pub active: bool,
    pub cancellation_requested: bool,
    /// A close arrived during the run; the window closes once it ends.
    pub close_requested: bool,
    /// The Run Outcome, once the terminal result arrived.
    pub outcome: Option<RunOutcome>,
}

/// The window's view of the latest Optimization Run. UI-thread only.
#[derive(Debug, Default)]
pub struct RunView {
    state: RunViewState,
    /// The sequence of the last accepted event; later events must exceed it.
    sequence: u64,
}

impl RunView {
    /// Clears the previous run's presentation and binds later events to
    /// `run_id`.
    pub fn begin(&mut self, run_id: impl Into<RunId>) {
        self.state = RunViewState {
            run_id: run_id.into(),
            label: "Preparing".to_owned(),
            active: true,
            ..RunViewState::default()
        };
        self.sequence = 0;
    }

    /// Takes in one event of the bound run.
    ///
    /// Returns `false`, changing nothing, for another run's event or one whose
    /// sequence is not above the last accepted one: queued events can outlive
    /// their run and must never rewrite a later run's view.
    pub fn consume(&mut self, event: &RunEvent) -> bool {
        if event.run_id != self.state.run_id || event.sequence <= self.sequence {
            return false;
        }
        self.sequence = event.sequence;
        let state = &mut self.state;
        match &event.payload {
            RunEventPayload::Phase(record) => {
                state.label = phase_name(record.phase()).to_owned();
                state.progress = record.progress();
                if record.status() == RunPhaseStatus::Skipped {
                    state.label.push_str(match record.skip_reason() {
                        Some(PhaseSkipReason::DryRun) => " - Skipped (Dry Run)",
                        _ => " - Skipped (No Requested Work)",
                    });
                }
                if state.cancellation_requested {
                    mark_cancelling(&mut state.label);
                }
            }
            RunEventPayload::Diagnostic(diagnostic) => state.details.push(format!(
                "Diagnostic: {} | {}",
                diagnostic.detail,
                diagnostic.path.display()
            )),
            RunEventPayload::Failure(failure) => state.details.push(failure_row(failure)),
            RunEventPayload::Terminal(result) => {
                state.active = false;
                state.outcome = Some(result.outcome());
                // Safety Cleanup has no attempt counts; show the final work
                // phase's instead.
                if let Some(record) = result.phase(result.final_phase()) {
                    state.progress = record.progress();
                }
                append_result_details(&mut state.details, result);
                state.label = outcome_label(result.outcome()).to_owned();
            }
        }
        true
    }

    /// Shows that cancellation was requested, without inventing an outcome.
    /// Does nothing once the run has ended or when already requested.
    pub fn request_cancellation(&mut self) {
        let state = &mut self.state;
        if state.active && !state.cancellation_requested {
            state.cancellation_requested = true;
            mark_cancelling(&mut state.label);
        }
    }

    /// Asks whether the window may close now.
    ///
    /// Returns `true` when no run is active. During a run it records the close,
    /// requests cancellation and returns `false`: the window must stay open
    /// through Safety Cleanup until the terminal result arrives, and then close.
    pub fn request_close(&mut self) -> bool {
        if !self.state.active {
            return true;
        }
        self.state.close_requested = true;
        self.request_cancellation();
        false
    }

    /// Whether a new run may start: none is active, and no close is pending,
    /// which keeps a run from starting between terminal delivery and the
    /// deferred close.
    pub fn can_start(&self) -> bool {
        !self.state.active && !self.state.close_requested
    }

    /// The presentation state.
    pub fn state(&self) -> &RunViewState {
        &self.state
    }

    /// The progress bar's and status bar's text: the label and, when the phase
    /// has counts, `" - <completed> / <total> attempts (<succeeded> succeeded,
    /// <failed> failed)"` (C++ `MainWindow::renderRun`).
    pub fn text(&self) -> String {
        match self.state.progress {
            Some(progress) => format!(
                "{} - {} / {} attempts ({} succeeded, {} failed)",
                self.state.label,
                progress.completed(),
                progress.total(),
                progress.succeeded(),
                progress.failed()
            ),
            None => self.state.label.clone(),
        }
    }

    /// Whether the progress bar is busy rather than filled: an active phase
    /// without counts.
    pub fn indeterminate(&self) -> bool {
        self.state.active && self.state.progress.is_none()
    }

    /// The progress bar's fill, from 0 to 1. A phase planning no attempts, or
    /// one without counts, is empty.
    pub fn fraction(&self) -> f32 {
        match self.state.progress {
            Some(progress) if progress.total() > 0 => {
                (progress.completed() as f64 / progress.total() as f64) as f32
            }
            _ => 0.0,
        }
    }
}

/// The canonical lifecycle vocabulary, rather than anything guessed from log
/// messages.
fn phase_name(phase: RunPhase) -> &'static str {
    match phase {
        RunPhase::Preparing => "Preparing",
        RunPhase::DiscoveringArchives => "Discovering Archives",
        RunPhase::ExtractingArchives => "Extracting Archives",
        RunPhase::BuildingEffectiveAssetTree => "Building the Effective Asset Tree",
        RunPhase::ProcessingAssets => "Processing Assets",
        RunPhase::ArchiveFinalization => "Archive Finalization",
        RunPhase::SafetyCleanup => "Safety Cleanup",
    }
}

/// The terminal label of each Run Outcome; only success reads as done.
fn outcome_label(outcome: RunOutcome) -> &'static str {
    match outcome {
        RunOutcome::Succeeded => "Done",
        RunOutcome::CompletedWithFailures => "Completed With Failures",
        RunOutcome::Cancelled => "Cancelled",
        RunOutcome::Failed => "Failed",
    }
}

/// Names a durable effect independently of the phase that ended the run.
fn mutation_name(kind: MutationKind) -> &'static str {
    match kind {
        MutationKind::ArchiveExtraction => "Archive Extraction",
        MutationKind::AssetProcessing => "Asset Processing",
        MutationKind::ArchiveFinalization => "Archive Finalization",
    }
}

/// `yes` or `no`, as the detail rows spell flags.
fn yes_no(flag: bool) -> &'static str {
    if flag { "yes" } else { "no" }
}

/// The path as the detail rows show it.
fn shown(path: &Path) -> std::path::Display<'_> {
    path.display()
}

/// Prefixes a phase label with the cancellation marker, as both a requested
/// cancellation and every later phase show it.
fn mark_cancelling(label: &mut String) {
    label.insert_str(0, "Cancelling - ");
}

/// The start of every [`failure_row`].
const FAILURE_PREFIX: &str = "Failure: ";

/// A Run Failure's detail row. The streamed row and the terminal result's copy
/// must come out identical: the terminal copy is dropped by matching the text.
fn failure_row(failure: &RunFailure) -> String {
    format!(
        "{FAILURE_PREFIX}{} | {}",
        failure.detail,
        shown(&failure.path)
    )
}

/// The detail row of an Archive that failed to extract or to finalize.
fn archive_failure_row(archive_path: &Path, detail: &str) -> String {
    format!("Archive Failure: {} | {detail}", shown(archive_path))
}

/// Appends the terminal result's evidence to the detail rows, including the
/// changes cancellation cannot roll back. A Run Failure already streamed as a
/// `Failure:` row is not repeated when its terminal copy arrives.
fn append_result_details(details: &mut Vec<String>, result: &OptimizationRunResult) {
    details.push(format!(
        "Cancellation Observed: {}",
        yes_no(result.cancellation_observed())
    ));
    details.push(format!("Final Phase: {}", phase_name(result.final_phase())));
    for root in result.mod_roots() {
        details.push(format!("Mod Root: {}", shown(root)));
    }

    // A multiset: each streamed row absorbs one terminal copy.
    let mut streamed: HashMap<String, usize> = HashMap::new();
    for detail in details
        .iter()
        .filter(|detail| detail.starts_with(FAILURE_PREFIX))
    {
        *streamed.entry(detail.clone()).or_default() += 1;
    }
    for failure in result.failures() {
        let detail = failure_row(failure);
        match streamed.get_mut(&detail) {
            Some(count) if *count > 0 => *count -= 1,
            _ => details.push(detail),
        }
    }

    for failure in result.cleanup_failures() {
        details.push(format!(
            "Cleanup Failure: {} | {}",
            failure.detail,
            shown(&failure.path)
        ));
    }
    for attempt in result.asset_attempts() {
        // The presentable detail comes from the Attempt Outcome's Operation
        // Failure, so a success never shows a row and a failure always has one.
        if let Some(failure) = attempt.result.outcome().failure() {
            details.push(format!(
                "Asset Failure: {} | {} | {} | {} | {}",
                shown(attempt.asset.execution_path()),
                attempt.result.operation(),
                failure.detail(),
                shown(attempt.result.affected_path()),
                attempt.result.service_detail()
            ));
        }
    }
    for attempt in result.archive_extraction_attempts() {
        if !attempt.succeeded() {
            details.push(archive_failure_row(&attempt.archive_path, &attempt.detail));
        }
    }
    if let Some(finalization) = result.archive_finalization() {
        if finalization.failure.is_some() {
            details.push(format!("Finalization Failure: {}", finalization.detail));
        }
        for attempt in &finalization.attempts {
            if let Some(failure) = attempt.outcome.failure() {
                details.push(archive_failure_row(&attempt.archive_path, failure.detail()));
            }
        }
    }
    for mutation in result.mutation_summaries() {
        details.push(format!(
            "Committed Mutations Retained: {} | {} | committed={} | partial-or-unknown={}",
            shown(&mutation.mod_root),
            mutation_name(mutation.kind),
            mutation.committed,
            mutation.partial_or_unknown
        ));
    }
    for collision in result.archive_collisions() {
        let mut text = format!(
            "Archive Collision: {} | {} | winner={} | loose-asset-wins={}",
            shown(&collision.mod_root),
            shown(&collision.game_path),
            shown(&collision.winning_archive),
            yes_no(collision.loose_asset_wins)
        );
        for shadowed in &collision.shadowed_archives {
            text.push_str(&format!(" | shadowed={}", shown(shadowed)));
        }
        details.push(text);
    }
    for (reason, name) in [
        (SkipReason::DisabledPhase, "Disabled Phase"),
        (SkipReason::DisabledAssetKind, "Disabled Asset Kind"),
        (SkipReason::ExcludedAssetVariant, "Excluded Asset Variant"),
    ] {
        let count = result.skipped_asset_count(reason);
        if count != 0 {
            details.push(format!("Skipped Assets: {name} | {count}"));
        }
    }
}
