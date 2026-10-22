//! Run Outcome classification over sealed Run Evidence, without a filesystem.
//!
//! Classification is private to the executor and is a pure function of the
//! sealed evidence, so these cases build that evidence directly: each row
//! calls the real recording steps of the run's evidence store, in the order a
//! run makes them, then seals the evidence after Safety Cleanup. A row can
//! therefore only describe evidence a real run could produce; there is no
//! other way to construct [`RunEvidence`].
//!
//! The full-run terminal precedence test keeps one row per Run Outcome to
//! show the executor feeds its evidence here; this table pins every
//! combination of work fact and terminal circumstance.

use std::path::PathBuf;

use super::classify_terminal_outcome;
use crate::CompletedMutation;
use crate::Error;
use crate::execution::{AssetExecutionFailure, AssetExecutionResult, MutationState};
use crate::routing::{AssetRouter, ExecutionMode, RequestedWork, RoutingPolicyRequest};
use crate::run::evidence::{
    ArchiveDiscoveryEvidence, MutableRunEvidence, RoutedAssetAttempt, RunEvidence,
};
use crate::run::{
    ArchiveFinalizationAccount, ArchiveFinalizationFailure, ArchivePrecedence, RunFailure,
    RunFailureCode, RunOutcome, RunPhase, RunPhaseRecord, RunPreparation,
};
use crate::test_support::sse_profile;

/// The work fact one row of the classification table establishes.
#[derive(Debug, Clone, Copy, PartialEq)]
enum WorkFact {
    /// The run's one Asset attempt succeeded and Archive Finalization
    /// completed with nothing to do.
    Succeeded,
    /// The Asset attempt ended in a contained Operation Failure that left
    /// nothing mutated, and the run went on to Archive Finalization.
    ContainedFailure,
    /// The Asset attempt left a partial or unknown mutation, so the run
    /// stopped before Archive Finalization.
    UnsafeMutation,
    /// Archive Finalization failed with an Unexpected Exception before
    /// planning, a result that forbids continuation.
    FatalFailure,
}

/// The circumstance a row's run ended in, after its work.
#[derive(Debug, Clone, Copy)]
struct Terminal {
    /// Safety Cleanup could not remove a run-owned artifact.
    cleanup_fails: bool,
    /// Cancellation was observed during Safety Cleanup.
    cancelled: bool,
}

impl Terminal {
    /// The four terminal circumstances, in the order the table's columns
    /// list them: plain, Safety Cleanup failure, cancelled, and cancelled
    /// with Safety Cleanup failure.
    const ALL: [Self; 4] = [
        Self {
            cleanup_fails: false,
            cancelled: false,
        },
        Self {
            cleanup_fails: true,
            cancelled: false,
        },
        Self {
            cleanup_fails: false,
            cancelled: true,
        },
        Self {
            cleanup_fails: true,
            cancelled: true,
        },
    ];
}

/// The Asset attempt result that establishes `fact`. Only the contained
/// failure and the unsafe mutation fail the attempt itself.
fn asset_result(fact: WorkFact) -> AssetExecutionResult {
    match fact {
        WorkFact::ContainedFailure => {
            AssetExecutionResult::failed(AssetExecutionFailure::LoadFailed, "contained")
        }
        WorkFact::UnsafeMutation => {
            AssetExecutionResult::failed(AssetExecutionFailure::CommitFailed, "unknown")
                .with_mutation(MutationState::PartialOrUnknown)
        }
        WorkFact::Succeeded | WorkFact::FatalFailure => {
            AssetExecutionResult::success(CompletedMutation::None)
        }
    }
}

/// Records, then seals, the evidence of an Apply run over one Mod Root
/// holding one Texture and no Archives, whose work establishes `fact` and
/// which ends in `terminal`.
///
/// The steps are those the work phases and the executor make, in their
/// order: Preparing, Archive discovery with no Archives, an empty Archive
/// extraction plan, the definitive routing of the one Texture, its Asset
/// attempt, Archive Finalization unless the attempt was unsafe, then Safety
/// Cleanup, its failure and the cancellation observation.
///
/// # Errors
///
/// A Run Evidence invariant violation, should a step stop matching what the
/// evidence store accepts.
fn sealed_evidence(fact: WorkFact, terminal: Terminal) -> Result<RunEvidence, Error> {
    let mod_root = PathBuf::from("mod-root");
    let profile = sse_profile();
    let policy = profile
        .compile_policy(RoutingPolicyRequest::for_work(
            ExecutionMode::Apply,
            &[RequestedWork::NativeTextureOptimization],
        ))
        .expect("the SSE profile supports Native Texture Optimization");
    let router = AssetRouter::new(policy.clone());
    let mut evidence = MutableRunEvidence::new(None);

    evidence.record_phase(RunPhaseRecord::executed(RunPhase::Preparing, None))?;
    evidence.record_preparation(RunPreparation::new(
        vec![mod_root.clone()],
        profile,
        policy,
        ArchivePrecedence::DeterministicDiscovery,
    ))?;

    evidence.record_phase(RunPhaseRecord::executed(
        RunPhase::DiscoveringArchives,
        None,
    ))?;
    evidence.record_archive_collisions(&[])?;
    // The empty extraction plan closes as soon as its handle's borrow ends.
    evidence.open_archive_extraction(0)?;
    evidence.record_phase(RunPhaseRecord::executed(
        RunPhase::BuildingEffectiveAssetTree,
        None,
    ))?;
    evidence.record_archive_discovery(ArchiveDiscoveryEvidence::default())?;
    // Routing decides from the file name alone, so the Texture never has to exist.
    let ledger = router.route_all([mod_root.join("textures/a.dds")]);
    let asset = ledger
        .routed_assets()
        .first()
        .cloned()
        .expect("the Texture is routed for Native Texture Optimization");
    evidence
        .open_processing_assets(ledger)?
        .record(RoutedAssetAttempt {
            mod_root,
            asset,
            result: asset_result(fact),
        })?;

    // An unsafe attempt stops every later attempt and Archive Finalization.
    if fact != WorkFact::UnsafeMutation {
        evidence.publish_diagnostics();
        let mut finalization = evidence.open_archive_finalization()?;
        if fact == WorkFact::FatalFailure {
            // As Archive Finalization finishes its phase after a panic
            // before planning.
            finalization.finish(ArchiveFinalizationAccount {
                failure: Some(ArchiveFinalizationFailure::UnexpectedException),
                safe_to_continue: false,
                detail: "fatal work failure".to_owned(),
                ..ArchiveFinalizationAccount::default()
            })?;
        } else {
            finalization.plan(0)?;
            finalization.finish(ArchiveFinalizationAccount::default())?;
        }
    }

    evidence.record_phase(RunPhaseRecord::executed(RunPhase::SafetyCleanup, None))?;
    if terminal.cleanup_fails {
        evidence.record_safety_cleanup_failure(RunFailure::new(
            RunFailureCode::TemporaryArtifactCleanupFailed,
            RunPhase::SafetyCleanup,
            "retained artifact",
        ))?;
    }
    if terminal.cancelled {
        evidence.record_cancellation_observation();
    }
    evidence.consume()
}

/// Origin: RunExecutorTests::terminalPrecedenceRetainsAllEvidence (all
/// sixteen cases), classified directly. Unsafe work and fatal failures are
/// Failed whatever else happened; otherwise cancellation is Cancelled;
/// otherwise a contained or Safety Cleanup failure completes with failures.
#[test]
fn classification_orders_every_work_fact_against_every_terminal_circumstance() {
    use RunOutcome::{Cancelled, CompletedWithFailures, Failed, Succeeded};
    // (fact, [plain, cleanup failure, cancelled, cancelled with cleanup failure])
    let table = [
        (
            WorkFact::Succeeded,
            [Succeeded, CompletedWithFailures, Cancelled, Cancelled],
        ),
        (
            WorkFact::ContainedFailure,
            [
                CompletedWithFailures,
                CompletedWithFailures,
                Cancelled,
                Cancelled,
            ],
        ),
        (WorkFact::UnsafeMutation, [Failed; 4]),
        (WorkFact::FatalFailure, [Failed; 4]),
    ];
    for (fact, outcomes) in table {
        for (terminal, expected) in Terminal::ALL.into_iter().zip(outcomes) {
            let case = format!("{fact:?}, {terminal:?}");
            let evidence = sealed_evidence(fact, terminal)
                .unwrap_or_else(|error| panic!("{case}: a real run's steps are accepted: {error}"));

            assert_eq!(classify_terminal_outcome(&evidence), expected, "{case}");
        }
    }
}
