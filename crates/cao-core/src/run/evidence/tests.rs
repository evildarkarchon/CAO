//! The selected Run Evidence scenarios of `tests/RunEvidenceTests.cpp`,
//! against [`MutableRunEvidence`] and the [`RunEvidence`] it seals.
//!
//! Each scenario names its C++ origin. C++ threw `std::logic_error` (or
//! `RunEvidenceInvariantViolation`) for an invariant violation; Rust returns
//! [`Error::EvidenceInvariant`], which the Run Executor raises after cleanup.
//!
//! Not ported, with reasons:
//! - The sink reading the owner in `finalizationProgressPublishesRetainedAttempt`:
//!   a sink cannot borrow the owner it is called from. The retained attempts
//!   and the progress they publish are ported.
//! - The post-consumption half of `phaseOrderViolationsAreRejected` and
//!   `preparationAndPostConsumptionMutationAreRejected`, and the "a failed
//!   consume leaves the owner usable" rule: `consume` takes the owner by value,
//!   so using it afterwards does not compile, and a refused consume drops it.
//! - The sink re-entering the evidence in `liveFactsAreRetainedBeforePublication`:
//!   a sink cannot borrow the owner it is called from, so it cannot observe
//!   a fact before its retention. Only the publication order is ported.
//!
//! These were an integration suite under `tests/` until [`MutableRunEvidence`]
//! became crate-private (#561); they now run as the evidence module's unit
//! tests, so they keep driving the store directly.

use std::cell::Cell;
use std::path::PathBuf;

use super::MutableRunEvidence;
use crate::Error;
use crate::execution::{AssetExecutionFailure, AssetExecutionResult, MutationState};
use crate::routing::{
    AssetRouter, ExecutionMode, RequestedWork, RoutedAsset, RoutingLedger, RoutingPolicyRequest,
};
use crate::run::{
    ArchiveDiscoveryEvidence, ArchiveExtractionFailure, ArchiveExtractionResult,
    ArchiveFinalizationAttempt, ArchiveFinalizationFailure, ArchiveFinalizationMutation,
    ArchiveFinalizationMutationKind, ArchiveFinalizationResult, ArchivePrecedence, MutationKind,
    PhaseSkipReason, RoutedAssetAttempt, RunDiagnostic, RunDiagnosticCode, RunEvidence, RunFailure,
    RunFailureCode, RunPhase, RunPhaseRecord, RunPreparation, RunProgress, SelectedProfile,
};
use crate::test_support::{Observed, RecordingSink, sse_capabilities, sse_profile};
use crate::{AttemptOutcome, CompletedMutation, OperationFailure};

/// The preparation of a Dry Run with no work, as Preparing would retain it.
fn successful_preparation() -> RunPreparation {
    let profile = SelectedProfile::new(sse_capabilities(".BSA"))
        .with_conversion_preference(true)
        .with_ignored_mods(vec!["ignored-mod".to_owned()])
        .with_packing_exclusions(vec!["readme.txt".to_owned()]);
    let policy = profile
        .compile_policy(RoutingPolicyRequest::for_work(ExecutionMode::DryRun, &[]))
        .expect("the profile is valid");
    RunPreparation::new(
        vec![PathBuf::from("prepared-root")],
        profile,
        policy,
        ArchivePrecedence::ExplicitOrder(vec!["winner.bsa".into(), "shadowed.bsa".into()]),
    )
}

/// Records Safety Cleanup and seals the evidence.
fn consume_after_cleanup(mut evidence: MutableRunEvidence<'_>) -> RunEvidence {
    evidence
        .record_phase(RunPhaseRecord::executed(RunPhase::SafetyCleanup, None))
        .unwrap();
    evidence.consume().unwrap()
}

fn executed(phase: RunPhase) -> RunPhaseRecord {
    RunPhaseRecord::executed(phase, None)
}

fn progress(phase: RunPhase, total: usize, succeeded: usize, failed: usize) -> RunPhaseRecord {
    RunPhaseRecord::executed(
        phase,
        Some(RunProgress::determinate(total, succeeded, failed)),
    )
}

fn is_invariant(result: Result<(), Error>) -> bool {
    matches!(result, Err(Error::EvidenceInvariant(_)))
}

/// The Run Evidence invariant `result` violated, or `None` when it was
/// accepted or failed some other way, so a test can pin which rule refused it.
fn invariant(result: Result<(), Error>) -> Option<&'static str> {
    match result {
        Err(Error::EvidenceInvariant(message)) => Some(message),
        _ => None,
    }
}

/// A finalization attempt at `archive` in `mod_root`.
fn finalization_attempt(
    archive: &str,
    mod_root: &str,
    mutation: MutationState,
    failure: Option<ArchiveFinalizationFailure>,
) -> ArchiveFinalizationAttempt {
    // A failure claims safety, as a committed attempt whose verification
    // passed does; a success can only carry no mutation or a Committed one.
    let outcome = match (failure, mutation) {
        (Some(failure), mutation) => AttemptOutcome::failed(
            OperationFailure::new(failure, format!("{failure:?}")),
            mutation,
            true,
        ),
        (None, MutationState::None) => AttemptOutcome::success(CompletedMutation::None),
        (None, MutationState::Committed) => AttemptOutcome::success(CompletedMutation::Committed),
        (None, MutationState::PartialOrUnknown) => {
            panic!("a successful attempt cannot leave partial or unknown mutation")
        }
    };
    ArchiveFinalizationAttempt::new(archive.into(), mod_root.into(), outcome)
}

/// Evidence in the executed Archive Finalization phase, optionally reporting
/// to `sink`.
fn finalizing(sink: Option<&dyn crate::run::RunObservationSink>) -> MutableRunEvidence<'_> {
    let mut evidence = MutableRunEvidence::new(sink);
    evidence
        .record_phase(executed(RunPhase::Preparing))
        .unwrap();
    evidence
        .record_phase(executed(RunPhase::ArchiveFinalization))
        .unwrap();
    evidence
}

/// Origin: RunEvidenceTests::successfulPreparationIsAtomicallyOwned.
#[test]
fn a_recorded_preparation_is_owned_by_the_sealed_evidence() {
    let terminal = {
        let mut evidence = MutableRunEvidence::new(None);
        evidence
            .record_phase(executed(RunPhase::Preparing))
            .unwrap();
        evidence
            .record_preparation(successful_preparation())
            .unwrap();
        consume_after_cleanup(evidence)
    };

    let preparation = terminal.preparation().expect("Preparing completed");
    assert_eq!(preparation.mod_roots(), [PathBuf::from("prepared-root")]);
    assert_eq!(preparation.policy().archive_extension(), ".bsa");
    assert_eq!(preparation.policy().execution_mode(), ExecutionMode::DryRun);
    let profile = preparation.selected_profile();
    assert!(profile.prefers_conversion());
    assert_eq!(profile.ignored_mods(), ["ignored-mod"]);
    assert_eq!(profile.packing_exclusions(), ["readme.txt"]);
    let ArchivePrecedence::ExplicitOrder(order) = preparation.archive_precedence() else {
        panic!("the explicit order was retained");
    };
    assert_eq!(order.len(), 2);
    assert_eq!(order[0], PathBuf::from("winner.bsa"));
}

/// Origin: RunEvidenceTests::unfinishedPreparationRemainsAbsent.
#[test]
fn an_unfinished_preparation_is_absent() {
    let mut evidence = MutableRunEvidence::new(None);
    evidence
        .record_phase(executed(RunPhase::Preparing))
        .unwrap();

    assert!(consume_after_cleanup(evidence).preparation().is_none());
}

/// Origin: RunEvidenceTests::laterPhaseRecordsReplaceWithoutReordering. The
/// progress each attempt advances replaces its phase's record in place.
#[test]
fn a_later_record_of_a_phase_replaces_it_in_place() {
    let mut evidence = discovering(None);
    let mut extraction = evidence.open_archive_extraction(3).unwrap();
    extraction
        .record(extraction_attempt("first.bsa", None))
        .unwrap();
    extraction
        .record(extraction_attempt(
            "second.bsa",
            Some(ArchiveExtractionFailure::ExtractionFailed),
        ))
        .unwrap();

    let terminal = consume_after_cleanup(evidence);

    let phases: Vec<_> = terminal
        .phases()
        .iter()
        .map(RunPhaseRecord::phase)
        .collect();
    assert_eq!(
        phases,
        [
            RunPhase::Preparing,
            RunPhase::DiscoveringArchives,
            RunPhase::ExtractingArchives,
            RunPhase::SafetyCleanup
        ]
    );
    let extraction = terminal
        .phase(RunPhase::ExtractingArchives)
        .unwrap()
        .progress()
        .unwrap();
    assert_eq!(
        (
            extraction.total(),
            extraction.succeeded(),
            extraction.failed()
        ),
        (3, 1, 1)
    );
}

/// Origin: RunEvidenceTests::phaseOrderViolationsAreRejected (the live half).
#[test]
fn phase_order_violations_are_rejected() {
    let mut evidence = MutableRunEvidence::new(None);
    assert!(is_invariant(
        evidence.record_phase(executed(RunPhase::DiscoveringArchives))
    ));
    evidence
        .record_phase(executed(RunPhase::Preparing))
        .unwrap();
    // Phases may be skipped over, never revisited.
    evidence
        .record_phase(executed(RunPhase::BuildingEffectiveAssetTree))
        .unwrap();
    for revisited in [RunPhase::DiscoveringArchives, RunPhase::Preparing] {
        assert_eq!(
            invariant(evidence.record_phase(executed(revisited))),
            Some("Run Phases must follow canonical traversal order"),
            "{revisited:?}"
        );
    }
}

/// Origin: RunEvidenceTests::progressRegressionsAreRejected. The general
/// transition validates determinate progress for any phase it accepts; the
/// attempt-bearing phases reach it only through their handles, which always
/// advance it by one attempt.
#[test]
fn phase_progress_cannot_start_late_change_total_regress_or_overflow() {
    let mut evidence = MutableRunEvidence::new(None);
    evidence
        .record_phase(executed(RunPhase::Preparing))
        .unwrap();
    let phase = RunPhase::BuildingEffectiveAssetTree;
    assert!(
        is_invariant(evidence.record_phase(progress(phase, 4, 1, 0))),
        "a first record starts at zero"
    );
    evidence.record_phase(progress(phase, 4, 0, 0)).unwrap();
    evidence.record_phase(progress(phase, 4, 2, 1)).unwrap();
    for (case, record) in [
        ("a changed total", progress(phase, 5, 2, 1)),
        ("a regression", progress(phase, 4, 1, 1)),
        ("completion past the total", progress(phase, 4, 3, 2)),
        ("progress becoming indeterminate", executed(phase)),
        (
            "a changed status",
            RunPhaseRecord::skipped(phase, PhaseSkipReason::DryRun),
        ),
    ] {
        assert!(is_invariant(evidence.record_phase(record)), "{case}");
    }
}

/// Origin: RunEvidenceTests::preparationAndPostConsumptionMutationAreRejected
/// (the live half). Sealing needs Safety Cleanup first, and a preparation is
/// retained once.
#[test]
fn sealing_needs_safety_cleanup_and_a_preparation_is_retained_once() {
    let mut evidence = MutableRunEvidence::new(None);
    evidence
        .record_phase(executed(RunPhase::Preparing))
        .unwrap();
    evidence
        .record_preparation(successful_preparation())
        .unwrap();
    assert!(is_invariant(
        evidence.record_preparation(successful_preparation())
    ));
    assert!(matches!(
        evidence.consume(),
        Err(Error::EvidenceInvariant(_))
    ));
}

/// Origin: RunEvidenceTests::cancellationObservationIsSealedIndependently.
#[test]
fn an_observed_cancellation_is_sealed_as_a_fact() {
    let mut evidence = MutableRunEvidence::new(None);
    evidence
        .record_phase(executed(RunPhase::Preparing))
        .unwrap();
    evidence.record_cancellation_observation();

    assert!(consume_after_cleanup(evidence).cancellation_observed());
}

/// Origin: RunEvidenceTests::finalizationAndCleanupFailuresRemainSeparate. An
/// Archive Finalization plan needs the executed phase first; its attempts,
/// including a contained cleanup failure, stay apart from Run Failures and
/// from Safety Cleanup failures.
#[test]
fn a_finalization_plan_needs_its_phase_and_cleanup_failures_stay_separate() {
    let mut evidence = MutableRunEvidence::new(None);
    evidence
        .record_phase(executed(RunPhase::Preparing))
        .unwrap();
    assert!(is_invariant(evidence.record_archive_finalization_plan(2)));
    evidence
        .record_phase(executed(RunPhase::ArchiveFinalization))
        .unwrap();
    evidence.record_archive_finalization_plan(2).unwrap();
    assert!(
        is_invariant(evidence.record_archive_finalization_plan(2)),
        "the plan is immutable"
    );
    let retained = ArchiveFinalizationAttempt::new(
        "second.bsa".into(),
        "second-mod".into(),
        AttemptOutcome::failed(
            OperationFailure::new(
                ArchiveFinalizationFailure::SourceCleanupFailed,
                "source retained",
            ),
            MutationState::Committed,
            true,
        ),
    );
    evidence
        .record_archive_finalization(ArchiveFinalizationResult {
            attempts: vec![
                finalization_attempt("first.bsa", "first-mod", MutationState::Committed, None),
                retained,
            ],
            ..ArchiveFinalizationResult::default()
        })
        .unwrap();
    evidence
        .record_phase(executed(RunPhase::SafetyCleanup))
        .unwrap();
    for (code, detail) in [
        (RunFailureCode::TemporaryArtifactCleanupFailed, "artifact"),
        (RunFailureCode::SafetyCleanupServiceFailed, "service"),
    ] {
        evidence
            .record_safety_cleanup_failure(RunFailure::new(code, RunPhase::SafetyCleanup, detail))
            .unwrap();
    }

    let terminal = evidence.consume().unwrap();

    assert!(terminal.failures().is_empty());
    let details: Vec<_> = terminal
        .safety_cleanup_failures()
        .iter()
        .map(|failure| failure.detail.as_str())
        .collect();
    assert_eq!(details, ["artifact", "service"]);
    assert_eq!(
        terminal.cleanup_failures(),
        terminal.safety_cleanup_failures()
    );
    let plan = terminal
        .phase(RunPhase::ArchiveFinalization)
        .unwrap()
        .progress()
        .unwrap();
    assert_eq!((plan.total(), plan.completed()), (2, 2));
    let finalization = terminal.archive_finalization().unwrap();
    assert_eq!(
        finalization.attempts[0].mod_root,
        PathBuf::from("first-mod")
    );
    assert!(finalization.attempts[0].succeeded());
    assert_eq!(
        finalization.attempts[1].outcome.failure_category(),
        Some(ArchiveFinalizationFailure::SourceCleanupFailed)
    );
    assert!(finalization.attempts[1].safe_to_continue());
}

/// Origin: RunEvidenceTests::finalizationProgressPublishesRetainedAttempt.
/// Each streamed attempt is retained and published as progress, a failed
/// one counting as failed, and the returned result keeps both.
#[test]
fn finalization_progress_publishes_each_retained_attempt() {
    let sink = RecordingSink::default();
    let mut evidence = finalizing(Some(&sink));
    evidence.record_archive_finalization_plan(2).unwrap();
    let attempts = vec![
        finalization_attempt("first.bsa", "first-mod", MutationState::Committed, None),
        finalization_attempt(
            "second.bsa",
            "second-mod",
            MutationState::None,
            Some(ArchiveFinalizationFailure::WriteFailed),
        ),
    ];
    for attempt in &attempts {
        evidence
            .record_archive_finalization_attempt(attempt.clone())
            .unwrap();
        let retained = evidence.archive_finalization().unwrap();
        assert_eq!(retained.attempts.last(), Some(attempt));
    }
    evidence
        .record_archive_finalization(ArchiveFinalizationResult {
            attempts,
            ..ArchiveFinalizationResult::default()
        })
        .unwrap();

    let published: Vec<_> = sink
        .observed()
        .into_iter()
        .filter_map(|observed| match observed {
            Observed::Phase(record) if record.phase() == RunPhase::ArchiveFinalization => {
                record.progress()
            }
            _ => None,
        })
        .map(|progress| {
            (
                progress.completed(),
                progress.succeeded(),
                progress.failed(),
            )
        })
        .collect();
    assert_eq!(published, [(0, 0, 0), (1, 1, 0), (2, 1, 1)]);
    let terminal = consume_after_cleanup(evidence);
    assert_eq!(terminal.archive_finalization().unwrap().attempts.len(), 2);
}

/// Origin: RunEvidenceTests::interruptedFinalizationRetainsCompletedAttempts.
/// A phase-level failure keeps the attempt completed before it.
#[test]
fn an_interrupted_finalization_keeps_its_completed_attempts() {
    let mut evidence = finalizing(None);
    evidence.record_archive_finalization_plan(2).unwrap();
    let committed =
        finalization_attempt("committed.bsa", "first-mod", MutationState::Committed, None);
    evidence
        .record_archive_finalization_attempt(committed.clone())
        .unwrap();
    evidence
        .record_archive_finalization(ArchiveFinalizationResult {
            attempts: vec![committed],
            failure: Some(ArchiveFinalizationFailure::UnexpectedException),
            safe_to_continue: false,
            detail: "packing interrupted".to_owned(),
            ..ArchiveFinalizationResult::default()
        })
        .unwrap();

    let terminal = consume_after_cleanup(evidence);
    let finalization = terminal.archive_finalization().unwrap();
    assert_eq!(finalization.attempts.len(), 1);
    assert_eq!(
        finalization.attempts[0].outcome.mutation(),
        MutationState::Committed
    );
    assert_eq!(
        finalization.failure,
        Some(ArchiveFinalizationFailure::UnexpectedException)
    );
    assert!(!finalization.safe_to_continue);
    let progress = terminal
        .phase(RunPhase::ArchiveFinalization)
        .unwrap()
        .progress()
        .unwrap();
    assert_eq!((progress.total(), progress.completed()), (2, 1));
}

/// Origin: RunEvidenceTests::finalizationResultCannotDropStreamedAttempts.
#[test]
fn a_finalization_result_cannot_drop_a_streamed_attempt() {
    let mut evidence = finalizing(None);
    evidence.record_archive_finalization_plan(2).unwrap();
    evidence
        .record_archive_finalization_attempt(finalization_attempt(
            "committed.bsa",
            "first-mod",
            MutationState::Committed,
            None,
        ))
        .unwrap();
    assert!(is_invariant(evidence.record_archive_finalization(
        ArchiveFinalizationResult {
            failure: Some(ArchiveFinalizationFailure::UnexpectedException),
            safe_to_continue: false,
            ..ArchiveFinalizationResult::default()
        }
    )));
}

/// Origin: RunEvidenceTests::finalizationAttemptsRequireRecordedTotal. An
/// attempt needs an output total, but a phase-level result may come without
/// one, and a cancelled one is sealed as observed cancellation.
#[test]
fn finalization_attempts_need_a_recorded_total() {
    let mut evidence = finalizing(None);
    assert!(is_invariant(evidence.record_archive_finalization(
        ArchiveFinalizationResult {
            attempts: vec![finalization_attempt(
                "packed.bsa",
                "first-mod",
                MutationState::Committed,
                None
            )],
            ..ArchiveFinalizationResult::default()
        }
    )));
    evidence
        .record_archive_finalization(ArchiveFinalizationResult {
            cancelled: true,
            ..ArchiveFinalizationResult::default()
        })
        .unwrap();

    let terminal = consume_after_cleanup(evidence);
    assert!(terminal.archive_finalization().is_some());
    assert!(
        terminal
            .phase(RunPhase::ArchiveFinalization)
            .unwrap()
            .progress()
            .is_none()
    );
    assert!(terminal.cancellation_observed());
}

/// Origin: RunEvidenceTests::sealedMutationSummariesReflectCompletedAttempts.
/// Extraction and finalization attempts are summarized per Mod Root and
/// kind; an attempt that mutated nothing adds nothing.
#[test]
fn sealed_mutation_summaries_count_completed_attempts() {
    let extraction = |archive: &str, root: &str, outcome| ArchiveExtractionResult {
        archive_path: archive.into(),
        mod_root: root.into(),
        outcome,
    };
    let failed = |category, mutation| {
        AttemptOutcome::failed(
            OperationFailure::new(category, String::new()),
            mutation,
            mutation != MutationState::PartialOrUnknown,
        )
    };
    let mut evidence = discovering(None);
    let mut extracting = evidence.open_archive_extraction(3).unwrap();
    for attempt in [
        extraction(
            "first.bsa",
            "alpha",
            AttemptOutcome::success(CompletedMutation::Committed),
        ),
        extraction(
            "second.bsa",
            "alpha",
            failed(
                ArchiveExtractionFailure::MergeFailed,
                MutationState::PartialOrUnknown,
            ),
        ),
        extraction(
            "third.bsa",
            "beta",
            failed(
                ArchiveExtractionFailure::ExtractionFailed,
                MutationState::None,
            ),
        ),
    ] {
        extracting.record(attempt).unwrap();
    }
    evidence
        .record_phase(executed(RunPhase::ArchiveFinalization))
        .unwrap();
    evidence.record_archive_finalization_plan(2).unwrap();
    evidence
        .record_archive_finalization(ArchiveFinalizationResult {
            attempts: vec![
                finalization_attempt("packed-a.bsa", "alpha", MutationState::Committed, None),
                finalization_attempt(
                    "packed-b.bsa",
                    "beta",
                    MutationState::Committed,
                    Some(ArchiveFinalizationFailure::SourceCleanupFailed),
                ),
            ],
            ..ArchiveFinalizationResult::default()
        })
        .unwrap();

    let terminal = consume_after_cleanup(evidence);
    let summaries: Vec<_> = terminal
        .mutation_summaries()
        .iter()
        .map(|summary| {
            (
                summary.mod_root.to_str().unwrap(),
                summary.kind,
                summary.committed,
                summary.partial_or_unknown,
            )
        })
        .collect();
    assert_eq!(
        summaries,
        [
            ("alpha", MutationKind::ArchiveExtraction, 1, 1),
            ("alpha", MutationKind::ArchiveFinalization, 1, 0),
            ("beta", MutationKind::ArchiveFinalization, 1, 0),
        ]
    );
}

/// Origin: RunEvidenceTests::pluginOnlyFinalizationMutationsAreSealed. Plugin
/// actions with no output attempt still count, by their Mod Root.
#[test]
fn plugin_only_finalization_mutations_are_sealed() {
    let plugin = |root: &str, path: &str, kind| ArchiveFinalizationMutation {
        mod_root: root.into(),
        path: path.into(),
        kind,
        mutation: MutationState::Committed,
        count: 1,
    };
    let mut evidence = finalizing(None);
    evidence.record_archive_finalization_plan(0).unwrap();
    evidence
        .record_archive_finalization(ArchiveFinalizationResult {
            mutations: vec![
                plugin(
                    "first-mod",
                    "first-mod/existing.esp",
                    ArchiveFinalizationMutationKind::PluginCreation,
                ),
                plugin(
                    "second-mod",
                    "second-mod/old.esp",
                    ArchiveFinalizationMutationKind::PluginRemoval,
                ),
            ],
            ..ArchiveFinalizationResult::default()
        })
        .unwrap();

    let terminal = consume_after_cleanup(evidence);
    let retained = terminal.archive_finalization().unwrap();
    assert!(retained.attempts.is_empty());
    assert_eq!(retained.mutations.len(), 2);
    assert_eq!(
        terminal
            .phase(RunPhase::ArchiveFinalization)
            .unwrap()
            .progress()
            .unwrap()
            .total(),
        0
    );
    let summaries: Vec<_> = terminal
        .mutation_summaries()
        .iter()
        .map(|summary| {
            (
                summary.mod_root.to_str().unwrap(),
                summary.kind,
                summary.committed,
            )
        })
        .collect();
    assert_eq!(
        summaries,
        [
            ("first-mod", MutationKind::ArchiveFinalization, 1),
            ("second-mod", MutationKind::ArchiveFinalization, 1),
        ]
    );
}

/// Origin: RunEvidenceTests::liveFactsAreRetainedBeforePublication (the
/// ordering half). Each phase, diagnostic and failure reaches the sink in the
/// order the evidence accepted it, and the sealed evidence keeps them all.
#[test]
fn live_facts_reach_the_sink_in_acceptance_order() {
    let sink = RecordingSink::default();
    let mut evidence = MutableRunEvidence::new(Some(&sink));
    evidence
        .record_phase(executed(RunPhase::Preparing))
        .unwrap();
    evidence.record_diagnostic(RunDiagnostic::new(
        RunDiagnosticCode::IgnoredModExcluded,
        RunPhase::Preparing,
        "ignored",
    ));
    evidence.record_failure(RunFailure::new(
        RunFailureCode::ConfigurationLoadingFailed,
        RunPhase::Preparing,
        "unreadable",
    ));

    let terminal = consume_after_cleanup(evidence);

    let kinds: Vec<_> = sink.observed().iter().map(Observed::kind).collect();
    assert_eq!(kinds, ["phase", "diagnostic", "failure", "phase"]);
    assert_eq!(terminal.diagnostics().len(), 1);
    assert_eq!(terminal.failures().len(), 1);
}

/// Origin: RunEvidenceTests::throwingPublicationIsClaimedOnce (all three
/// rows). A sink that panics once, on a phase, a diagnostic or a failure,
/// costs exactly one retained `ObserverFailed` diagnostic. That diagnostic
/// is never delivered back to the sink, the failed payload is not retried,
/// and every later payload is still delivered.
#[test]
fn a_panicking_publication_is_claimed_once_and_never_retried() {
    for kind in ["phase", "diagnostic", "failure"] {
        let panicked = Cell::new(false);
        let sink = RecordingSink::with_hook(move |observed| {
            if observed.kind() == kind && !panicked.replace(true) {
                panic!("the {kind} observer panicked");
            }
        });
        let mut evidence = MutableRunEvidence::new(Some(&sink));
        evidence
            .record_phase(executed(RunPhase::Preparing))
            .unwrap();
        evidence.record_diagnostic(RunDiagnostic::new(
            RunDiagnosticCode::IgnoredModExcluded,
            RunPhase::Preparing,
            "ignored",
        ));
        evidence.record_failure(RunFailure::new(
            RunFailureCode::ConfigurationLoadingFailed,
            RunPhase::Preparing,
            "unreadable",
        ));
        evidence.record_diagnostic(RunDiagnostic::new(
            RunDiagnosticCode::SeparatorModExcluded,
            RunPhase::Preparing,
            "separator",
        ));

        let terminal = consume_after_cleanup(evidence);

        let observed = sink.observed();
        let count = |wanted: &str| {
            observed
                .iter()
                .filter(|observed| observed.kind() == wanted)
                .count()
        };
        assert_eq!(
            (count("phase"), count("diagnostic"), count("failure")),
            (2, 2, 1),
            "{kind}: each payload is delivered once"
        );
        let delivered: Vec<_> = observed
            .iter()
            .filter_map(|observed| match observed {
                Observed::Diagnostic(diagnostic) => Some(diagnostic.code),
                _ => None,
            })
            .collect();
        assert_eq!(
            delivered,
            [
                RunDiagnosticCode::IgnoredModExcluded,
                RunDiagnosticCode::SeparatorModExcluded
            ],
            "{kind}"
        );
        let retained_failures = terminal
            .diagnostics()
            .iter()
            .filter(|diagnostic| diagnostic.code == RunDiagnosticCode::ObserverFailed)
            .count();
        assert_eq!(retained_failures, 1, "{kind}");
        assert_eq!(terminal.diagnostics().len(), 3, "{kind}");
        assert_eq!(terminal.failures().len(), 1, "{kind}");
    }
}

/// An extraction attempt at `archive` that mutated nothing, failed with
/// `failure` when one is given.
fn extraction_attempt(
    archive: &str,
    failure: Option<ArchiveExtractionFailure>,
) -> ArchiveExtractionResult {
    ArchiveExtractionResult {
        archive_path: archive.into(),
        mod_root: "extracted-mod".into(),
        outcome: match failure {
            None => AttemptOutcome::success(CompletedMutation::None),
            Some(category) => AttemptOutcome::failed(
                OperationFailure::new(category, String::new()),
                MutationState::None,
                true,
            ),
        },
    }
}

/// Evidence in Discovering Archives, ready to open Archive extraction,
/// optionally reporting to `sink`.
fn discovering(sink: Option<&dyn crate::run::RunObservationSink>) -> MutableRunEvidence<'_> {
    let mut evidence = MutableRunEvidence::new(sink);
    evidence
        .record_phase(executed(RunPhase::Preparing))
        .unwrap();
    evidence
        .record_phase(executed(RunPhase::DiscoveringArchives))
        .unwrap();
    evidence
}

/// The definitive routing of `count` Textures in one Mod Root, each routed
/// for Native Texture Optimization in an Apply run.
fn texture_ledger(count: usize) -> RoutingLedger {
    let policy = sse_profile()
        .compile_policy(RoutingPolicyRequest::for_work(
            ExecutionMode::Apply,
            &[RequestedWork::NativeTextureOptimization],
        ))
        .expect("the SSE profile supports Native Texture Optimization");
    AssetRouter::new(policy)
        .route_all((0..count).map(|index| PathBuf::from(format!("mod-root/textures/{index}.dds"))))
}

/// Evidence that has completed a successful Effective Asset Tree discovery,
/// with Building Effective Asset Tree still current, ready to open
/// Processing Assets with its definitive routing, optionally reporting to
/// `sink`.
fn discovered(sink: Option<&dyn crate::run::RunObservationSink>) -> MutableRunEvidence<'_> {
    let mut evidence = MutableRunEvidence::new(sink);
    evidence
        .record_phase(executed(RunPhase::Preparing))
        .unwrap();
    evidence
        .record_phase(executed(RunPhase::BuildingEffectiveAssetTree))
        .unwrap();
    evidence
        .record_archive_discovery(ArchiveDiscoveryEvidence::default())
        .unwrap();
    evidence
}

/// The first Routed Asset of `ledger`.
fn first_routed_asset(ledger: &RoutingLedger) -> RoutedAsset {
    ledger
        .routed_assets()
        .first()
        .cloned()
        .expect("the routing holds a Routed Asset")
}

/// An Asset attempt on `asset` that mutated nothing, ending in a contained
/// Operation Failure when `failed`.
fn asset_attempt(asset: RoutedAsset, failed: bool) -> RoutedAssetAttempt {
    RoutedAssetAttempt {
        mod_root: "mod-root".into(),
        asset,
        result: if failed {
            AssetExecutionResult::failed(AssetExecutionFailure::LoadFailed, "contained")
        } else {
            AssetExecutionResult::success(CompletedMutation::None)
        },
    }
}

/// One row of a rejection table: what it records, and the recording steps
/// whose last one the evidence must refuse.
type RejectionCase = (&'static str, fn() -> Result<(), Error>);

/// Every determinate progress `sink` saw published for `phase`, as
/// `(total, succeeded, failed)`.
fn published_progress(sink: &RecordingSink, phase: RunPhase) -> Vec<(usize, usize, usize)> {
    sink.observed()
        .into_iter()
        .filter_map(|observed| match observed {
            Observed::Phase(record) if record.phase() == phase => record.progress(),
            _ => None,
        })
        .map(|progress| (progress.total(), progress.succeeded(), progress.failed()))
        .collect()
}

/// Each Archive extraction attempt is retained and advances the progress
/// planned for Extracting Archives, a failed attempt counting as failed;
/// an attempt beyond the planned total is rejected and not retained.
#[test]
fn an_extraction_attempt_advances_its_planned_progress() {
    let sink = RecordingSink::default();
    let mut evidence = discovering(Some(&sink));
    let mut extraction = evidence.open_archive_extraction(2).unwrap();

    extraction
        .record(extraction_attempt("first.bsa", None))
        .unwrap();
    extraction
        .record(extraction_attempt(
            "second.bsa",
            Some(ArchiveExtractionFailure::ExtractionFailed),
        ))
        .unwrap();
    assert_eq!(
        invariant(extraction.record(extraction_attempt("third.bsa", None))),
        Some("Archive extraction attempts cannot exceed the planned total"),
        "an attempt beyond the planned total"
    );

    assert_eq!(
        published_progress(&sink, RunPhase::ExtractingArchives),
        [(2, 0, 0), (2, 1, 0), (2, 1, 1)]
    );
    let terminal = consume_after_cleanup(evidence);
    let archives: Vec<_> = terminal
        .archive_extraction_attempts()
        .iter()
        .map(|attempt| attempt.archive_path.to_str().unwrap())
        .collect();
    assert_eq!(archives, ["first.bsa", "second.bsa"]);
}

/// Each Asset attempt is retained and advances the progress planned for
/// Processing Assets, a contained failure counting as failed; an attempt
/// beyond the planned total is rejected and not retained.
#[test]
fn an_asset_attempt_advances_its_planned_progress() {
    let sink = RecordingSink::default();
    let mut evidence = discovered(Some(&sink));
    let ledger = texture_ledger(2);
    let asset = first_routed_asset(&ledger);
    let mut assets = evidence.open_processing_assets(ledger).unwrap();

    assets.record(asset_attempt(asset.clone(), true)).unwrap();
    assets.record(asset_attempt(asset.clone(), false)).unwrap();
    assert_eq!(
        invariant(assets.record(asset_attempt(asset, false))),
        Some("Asset attempts cannot exceed the routed-work total"),
        "an attempt beyond the planned total"
    );

    assert_eq!(
        published_progress(&sink, RunPhase::ProcessingAssets),
        [(2, 0, 0), (2, 0, 1), (2, 1, 1)]
    );
    let terminal = consume_after_cleanup(evidence);
    let succeeded: Vec<_> = terminal
        .asset_attempts()
        .iter()
        .map(|attempt| attempt.result.succeeded())
        .collect();
    assert_eq!(succeeded, [false, true]);
}

/// Each Archive Finalization attempt is retained and advances the progress
/// planned from its output total, a failed attempt counting as failed; an
/// attempt beyond the planned total is rejected and not retained.
#[test]
fn a_finalization_attempt_advances_its_planned_progress() {
    let sink = RecordingSink::default();
    let mut evidence = finalizing(Some(&sink));
    evidence.record_archive_finalization_plan(2).unwrap();

    evidence
        .record_archive_finalization_attempt(finalization_attempt(
            "first.bsa",
            "first-mod",
            MutationState::None,
            Some(ArchiveFinalizationFailure::WriteFailed),
        ))
        .unwrap();
    evidence
        .record_archive_finalization_attempt(finalization_attempt(
            "second.bsa",
            "second-mod",
            MutationState::Committed,
            None,
        ))
        .unwrap();
    assert!(
        is_invariant(
            evidence.record_archive_finalization_attempt(finalization_attempt(
                "third.bsa",
                "third-mod",
                MutationState::Committed,
                None,
            ))
        ),
        "an attempt beyond the planned total"
    );

    assert_eq!(
        published_progress(&sink, RunPhase::ArchiveFinalization),
        [(2, 0, 0), (2, 0, 1), (2, 1, 1)]
    );
    let archives: Vec<_> = evidence
        .archive_finalization()
        .unwrap()
        .attempts
        .iter()
        .map(|attempt| attempt.archive_path.to_str().unwrap().to_owned())
        .collect();
    assert_eq!(archives, ["first.bsa", "second.bsa"]);
}

/// An Archive Finalization attempt is rejected when its Run Phase is not the
/// current, executed one. Extraction and Asset attempts reach the evidence
/// only through their open phase's handle, which borrows it exclusively, so
/// they cannot be recorded outside their phase.
#[test]
fn attempts_outside_their_run_phase_are_rejected() {
    let cases: [RejectionCase; 1] = [("a finalization attempt during Processing Assets", || {
        let mut evidence = discovered(None);
        evidence.open_processing_assets(texture_ledger(1))?;
        evidence.record_archive_finalization_attempt(finalization_attempt(
            "a.bsa",
            "mod-root",
            MutationState::Committed,
            None,
        ))
    })];
    for (case, record) in cases {
        assert!(is_invariant(record()), "{case}");
    }
}

/// An Archive Finalization attempt is rejected while its Run Phase is
/// current but has no planned total yet. Opening Archive extraction or
/// Processing Assets always plans its total, so neither phase has an
/// unplanned state.
#[test]
fn attempts_before_their_plan_are_rejected() {
    let cases: [RejectionCase; 1] = [("a finalization attempt", || {
        finalizing(None).record_archive_finalization_attempt(finalization_attempt(
            "a.bsa",
            "mod-root",
            MutationState::Committed,
            None,
        ))
    })];
    for (case, record) in cases {
        assert!(is_invariant(record()), "{case}");
    }
}

/// No Archive Finalization attempt may follow the phase's recorded result,
/// even while its planned total has room.
#[test]
fn a_finalization_attempt_after_its_result_is_rejected() {
    let mut evidence = finalizing(None);
    evidence.record_archive_finalization_plan(1).unwrap();
    evidence
        .record_archive_finalization(ArchiveFinalizationResult::default())
        .unwrap();

    assert!(is_invariant(evidence.record_archive_finalization_attempt(
        finalization_attempt("late.bsa", "mod-root", MutationState::Committed, None)
    )));
}

/// Archive Finalization's attempt rejections keep their own messages: every
/// attempt outside the executed, planned and still open phase breaks its
/// immutable executed plan, and one past the planned total exceeds it.
#[test]
fn finalization_attempt_rejections_keep_their_messages() {
    const PLAN: &str = "Archive Finalization attempts require the immutable executed plan";
    const EXCEEDED: &str = "Archive Finalization attempts cannot exceed the planned total";
    fn attempt() -> ArchiveFinalizationAttempt {
        finalization_attempt("a.bsa", "mod-root", MutationState::Committed, None)
    }
    let cases: [(RejectionCase, &str); 5] = [
        (
            ("an attempt before the phase", || {
                let mut evidence = discovered(None);
                evidence.open_processing_assets(texture_ledger(1))?;
                evidence.record_archive_finalization_attempt(attempt())
            }),
            PLAN,
        ),
        (
            ("an attempt after the phase ended", || {
                let mut evidence = finalizing(None);
                evidence.record_archive_finalization_plan(1)?;
                evidence.record_phase(executed(RunPhase::SafetyCleanup))?;
                evidence.record_archive_finalization_attempt(attempt())
            }),
            PLAN,
        ),
        (
            ("an attempt before the plan", || {
                finalizing(None).record_archive_finalization_attempt(attempt())
            }),
            PLAN,
        ),
        (
            ("an attempt after the result", || {
                let mut evidence = finalizing(None);
                evidence.record_archive_finalization_plan(1)?;
                evidence.record_archive_finalization(ArchiveFinalizationResult::default())?;
                evidence.record_archive_finalization_attempt(attempt())
            }),
            PLAN,
        ),
        (
            ("an attempt past the planned total", || {
                let mut evidence = finalizing(None);
                evidence.record_archive_finalization_plan(1)?;
                evidence.record_archive_finalization_attempt(attempt())?;
                evidence.record_archive_finalization_attempt(attempt())
            }),
            EXCEEDED,
        ),
    ];
    for ((case, record), expected) in cases {
        assert_eq!(invariant(record()), Some(expected), "{case}");
    }
}

/// An Archive Finalization attempt needs the phase's own plan: determinate
/// progress recorded around the plan opens no result to retain it in, so
/// the attempt is refused rather than retained in a result made up for it.
#[test]
fn a_finalization_attempt_without_its_plan_is_rejected() {
    let mut evidence = finalizing(None);
    evidence
        .record_phase(progress(RunPhase::ArchiveFinalization, 1, 0, 0))
        .unwrap();

    assert_eq!(
        invariant(
            evidence.record_archive_finalization_attempt(finalization_attempt(
                "unplanned.bsa",
                "mod-root",
                MutationState::Committed,
                None
            ))
        ),
        Some("Archive Finalization attempts require the immutable executed plan")
    );
    assert!(evidence.archive_finalization().is_none());
    let unchanged = evidence
        .phase(RunPhase::ArchiveFinalization)
        .and_then(RunPhaseRecord::progress)
        .unwrap();
    assert_eq!((unchanged.total(), unchanged.completed()), (1, 0));
}

/// No output total may follow Archive Finalization's recorded result, as
/// when cancelled planning recorded a result without one; the plan would
/// otherwise reopen the closed phase and replace its result.
#[test]
fn a_finalization_plan_after_its_result_is_rejected() {
    let mut evidence = finalizing(None);
    evidence
        .record_archive_finalization(ArchiveFinalizationResult {
            cancelled: true,
            ..ArchiveFinalizationResult::default()
        })
        .unwrap();

    assert_eq!(
        invariant(evidence.record_archive_finalization_plan(1)),
        Some("Archive Finalization needs an executed phase and one immutable plan")
    );
    let terminal = consume_after_cleanup(evidence);
    assert!(terminal.archive_finalization().unwrap().cancelled);
    assert!(
        terminal
            .phase(RunPhase::ArchiveFinalization)
            .unwrap()
            .progress()
            .is_none()
    );
}

/// Opening Processing Assets retains the definitive Routing Ledger it is
/// given and plans the phase's determinate progress from that ledger's
/// Routed Assets, so no caller passes a total. An empty ledger plans zero.
#[test]
fn opening_processing_assets_derives_its_total_from_the_retained_routing_ledger() {
    for routed_assets in [0, 3] {
        let sink = RecordingSink::default();
        let mut evidence = discovered(Some(&sink));
        let ledger = texture_ledger(routed_assets);

        evidence.open_processing_assets(ledger.clone()).unwrap();

        assert_eq!(
            published_progress(&sink, RunPhase::ProcessingAssets),
            [(routed_assets, 0, 0)],
            "{routed_assets} Routed Assets"
        );
        assert_eq!(evidence.routing_ledger(), Some(&ledger));
        let terminal = consume_after_cleanup(evidence);
        assert_eq!(terminal.routing_ledger(), Some(&ledger));
    }
}

/// One row of an opening table: what it describes, and the evidence the
/// opening is then attempted on.
type OpeningCase = (&'static str, fn() -> MutableRunEvidence<'static>);

/// Processing Assets opens only on a completed, successful Effective Asset
/// Tree discovery, and only once; a refused opening retains no Routing
/// Ledger and records no phase.
#[test]
fn processing_assets_opens_only_after_successful_effective_asset_tree_discovery() {
    const UNROUTED: &str =
        "Routing Ledger requires completed, successful Effective Asset Tree discovery";
    let cases: [OpeningCase; 4] = [
        ("during Preparing", || {
            let mut evidence = MutableRunEvidence::new(None);
            evidence
                .record_phase(executed(RunPhase::Preparing))
                .unwrap();
            evidence
        }),
        ("without the Archive discovery facts", || {
            let mut evidence = MutableRunEvidence::new(None);
            evidence
                .record_phase(executed(RunPhase::Preparing))
                .unwrap();
            evidence
                .record_phase(executed(RunPhase::BuildingEffectiveAssetTree))
                .unwrap();
            evidence
        }),
        ("after a Run Failure", || {
            let mut evidence = discovered(None);
            evidence.record_failure(RunFailure::new(
                RunFailureCode::RunWorkEscaped,
                RunPhase::BuildingEffectiveAssetTree,
                "escaped",
            ));
            evidence
        }),
        ("a second time", || {
            let mut evidence = discovered(None);
            evidence.open_processing_assets(texture_ledger(1)).unwrap();
            evidence
        }),
    ];
    for (case, evidence) in cases {
        let mut evidence = evidence();
        let before = evidence.current_phase().copied();
        let retained = evidence.routing_ledger().cloned();

        assert_eq!(
            invariant(evidence.open_processing_assets(texture_ledger(2)).map(drop)),
            Some(UNROUTED),
            "{case}"
        );
        assert_eq!(evidence.current_phase().copied(), before, "{case}");
        assert_eq!(evidence.routing_ledger().cloned(), retained, "{case}");
    }
}

/// The Run Evidence invariant the general phase transition reports for an
/// executed record of an attempt-bearing Run Phase.
const HANDLE_ONLY: &str =
    "An attempt-bearing Run Phase can only be executed through its attempt-phase handle";

/// Asserts that the general phase transition refuses every executed record
/// of `phase`, determinate or not, and records nothing for it, yet still
/// accepts the phase's skip.
fn assert_only_its_handle_executes(phase: RunPhase, skip: PhaseSkipReason) {
    for record in [executed(phase), progress(phase, 1, 0, 0)] {
        let sink = RecordingSink::default();
        let mut evidence = MutableRunEvidence::new(Some(&sink));
        evidence
            .record_phase(executed(RunPhase::Preparing))
            .unwrap();

        assert_eq!(
            invariant(evidence.record_phase(record)),
            Some(HANDLE_ONLY),
            "{record:?}"
        );
        assert!(evidence.phase(phase).is_none(), "{record:?}");
        assert_eq!(sink.phase_order(), [RunPhase::Preparing], "{record:?}");

        evidence
            .record_phase(RunPhaseRecord::skipped(phase, skip))
            .unwrap();
    }
}

/// Extracting Archives executes only through the handle opening Archive
/// extraction, so no producer can account for its attempts or progress
/// outside it. Dry Run still skips it through the general transition.
#[test]
fn the_general_transition_rejects_an_executed_extracting_archives_record() {
    assert_only_its_handle_executes(RunPhase::ExtractingArchives, PhaseSkipReason::DryRun);
}

/// Processing Assets executes only through the handle opening it with the
/// definitive Routing Ledger. A run with no requested work still skips it
/// through the general transition.
#[test]
fn the_general_transition_rejects_an_executed_processing_assets_record() {
    assert_only_its_handle_executes(RunPhase::ProcessingAssets, PhaseSkipReason::NoRequestedWork);
}

/// An Asset attempt's diagnostics are retained through the open Processing
/// Assets handle, which holds the evidence exclusively, and wait for the
/// run's next publication rather than reaching the sink at once.
#[test]
fn a_diagnostic_retained_through_the_asset_handle_waits_for_publication() {
    let sink = RecordingSink::default();
    let mut evidence = discovered(Some(&sink));
    let mut assets = evidence.open_processing_assets(texture_ledger(1)).unwrap();

    assets.retain_diagnostic(RunDiagnostic::new(
        RunDiagnosticCode::PluginUnreadable,
        RunPhase::ProcessingAssets,
        "deferred",
    ));
    let published = |sink: &RecordingSink| {
        sink.observed()
            .iter()
            .filter(|observed| observed.kind() == "diagnostic")
            .count()
    };
    assert_eq!(published(&sink), 0, "retained, not yet published");

    evidence.publish_diagnostics();
    assert_eq!(published(&sink), 1);
    let terminal = consume_after_cleanup(evidence);
    assert_eq!(terminal.diagnostics().len(), 1);
    assert_eq!(terminal.diagnostics()[0].detail, "deferred");
}
