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

mod common;

use std::cell::Cell;
use std::path::PathBuf;

use cao_core::Error;
use cao_core::execution::MutationState;
use cao_core::routing::{ExecutionMode, RoutingPolicyRequest};
use cao_core::run::{
    ArchiveExtractionFailure, ArchiveExtractionResult, ArchiveFinalizationAttempt,
    ArchiveFinalizationFailure, ArchiveFinalizationMutation, ArchiveFinalizationMutationKind,
    ArchiveFinalizationResult, ArchivePrecedence, MutableRunEvidence, MutationKind,
    PhaseSkipReason, RunConfiguration, RunDiagnostic, RunDiagnosticCode, RunEvidence, RunFailure,
    RunFailureCode, RunPhase, RunPhaseRecord, RunPreparation, RunProgress, SelectedProfileFacts,
};
use common::{Observed, RecordingSink};

/// The preparation of a Dry Run with no work, as Preparing would retain it.
fn successful_preparation() -> RunPreparation {
    let configuration = RunConfiguration {
        profile: SelectedProfileFacts {
            archive_extension: Some(".BSA".to_owned()),
            ..common::sse_profile()
        },
        ignored_mods: vec!["ignored-mod".to_owned()],
        separator_suffixes: vec!["separator".to_owned()],
    };
    let policy = configuration
        .profile
        .compile_policy(RoutingPolicyRequest::for_work(ExecutionMode::DryRun, &[]))
        .expect("the profile is valid");
    RunPreparation::new(
        vec![PathBuf::from("prepared-root")],
        configuration,
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

/// A finalization attempt at `archive` in `mod_root`.
fn finalization_attempt(
    archive: &str,
    mod_root: &str,
    mutation: MutationState,
    failure: Option<ArchiveFinalizationFailure>,
) -> ArchiveFinalizationAttempt {
    ArchiveFinalizationAttempt {
        mutation,
        failure,
        detail: failure.map_or_else(String::new, |failure| format!("{failure:?}")),
        ..ArchiveFinalizationAttempt::new(archive.into(), mod_root.into())
    }
}

/// Evidence in the executed Archive Finalization phase, optionally reporting
/// to `sink`.
fn finalizing(sink: Option<&dyn cao_core::run::RunObservationSink>) -> MutableRunEvidence<'_> {
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
    assert_eq!(preparation.configuration().ignored_mods, ["ignored-mod"]);
    assert_eq!(
        preparation.configuration().separator_suffixes,
        ["separator"]
    );
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

/// Origin: RunEvidenceTests::laterPhaseRecordsReplaceWithoutReordering.
#[test]
fn a_later_record_of_a_phase_replaces_it_in_place() {
    let mut evidence = MutableRunEvidence::new(None);
    evidence
        .record_phase(executed(RunPhase::Preparing))
        .unwrap();
    evidence
        .record_phase(executed(RunPhase::DiscoveringArchives))
        .unwrap();
    evidence
        .record_phase(progress(RunPhase::ExtractingArchives, 3, 0, 0))
        .unwrap();
    evidence
        .record_phase(progress(RunPhase::ExtractingArchives, 3, 1, 1))
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
        .record_phase(progress(RunPhase::ProcessingAssets, 1, 0, 0))
        .unwrap();
    assert!(is_invariant(
        evidence.record_phase(executed(RunPhase::ExtractingArchives))
    ));
    assert!(is_invariant(
        evidence.record_phase(executed(RunPhase::Preparing))
    ));
}

/// Origin: RunEvidenceTests::progressRegressionsAreRejected.
#[test]
fn phase_progress_cannot_start_late_change_total_regress_or_overflow() {
    let mut evidence = MutableRunEvidence::new(None);
    evidence
        .record_phase(executed(RunPhase::Preparing))
        .unwrap();
    let phase = RunPhase::ExtractingArchives;
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
    let mut retained = finalization_attempt(
        "second.bsa",
        "second-mod",
        MutationState::Committed,
        Some(ArchiveFinalizationFailure::SourceCleanupFailed),
    );
    retained.detail = "source retained".to_owned();
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
        finalization.attempts[1].failure,
        Some(ArchiveFinalizationFailure::SourceCleanupFailed)
    );
    assert!(finalization.attempts[1].safe_to_continue);
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
            .record_archive_finalization_attempt(attempt.clone(), 2)
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
        .record_archive_finalization_attempt(committed.clone(), 2)
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
    assert_eq!(finalization.attempts[0].mutation, MutationState::Committed);
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
        .record_archive_finalization_attempt(
            finalization_attempt("committed.bsa", "first-mod", MutationState::Committed, None),
            2,
        )
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
    let extraction = |archive: &str, root: &str, mutation, failure| ArchiveExtractionResult {
        archive_path: archive.into(),
        mod_root: root.into(),
        mutation,
        failure,
        safe_to_continue: mutation != MutationState::PartialOrUnknown,
        detail: String::new(),
    };
    let mut evidence = MutableRunEvidence::new(None);
    evidence
        .record_phase(executed(RunPhase::Preparing))
        .unwrap();
    evidence
        .record_phase(executed(RunPhase::DiscoveringArchives))
        .unwrap();
    evidence.record_archive_extraction_plan(3).unwrap();
    for attempt in [
        extraction("first.bsa", "alpha", MutationState::Committed, None),
        extraction(
            "second.bsa",
            "alpha",
            MutationState::PartialOrUnknown,
            Some(ArchiveExtractionFailure::MergeFailed),
        ),
        extraction(
            "third.bsa",
            "beta",
            MutationState::None,
            Some(ArchiveExtractionFailure::ExtractionFailed),
        ),
    ] {
        evidence
            .record_archive_extraction_attempt(attempt, 3)
            .unwrap();
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
