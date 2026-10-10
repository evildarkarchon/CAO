//! Run Executor scenarios: phase traversal, Preparing failures, Safety Cleanup
//! and terminal classification at the synchronous seam beneath the service.
//!
//! Each scenario names the C++ test whose intent it ports, mostly from
//! `tests/RunExecutorTests.cpp`. Asset work runs the production
//! `execute_asset_run` through [`ControlledWork`], the Rust stand-in for C++
//! `ControlledAssetWork`; a junction stands in wherever C++ needed a symlink,
//! which takes a privilege a junction does not.
//!
//! Ported elsewhere:
//! - The Several Mods exclusion and resolution scenarios: `several_mods.rs`.
//! - `cancelledRecoveryPreservesUnattemptedArtifacts`:
//!   `durable_staging.rs`, `a_cancelled_preparation_preserves_the_durable_sibling`.
//! - `linkedStagingIsPreserved` and `unverifiableStagingIsPreserved`:
//!   `staging_recovery.rs` (`unverifiable_ownership_names_the_path_and_deletes_nothing`,
//!   with its junction and hard-link rows, and `a_malformed_manifest_is_never_trusted`),
//!   `durable_staging.rs` (`an_unknown_staging_like_name_fails_closed`, the
//!   look-alike row) and `staged_publication.rs`
//!   (`apply_preparing_fails_closed_on_unverifiable_staging`).
//! - `dryRunLeavesStagingUntouched`: `staged_publication.rs`,
//!   `a_dry_run_never_recovers_leftover_staging`.
//! - `productionWorkRetainsArchiveCollisions`,
//!   `throwingPreflightFailureObserverRetainsEvidence`, the fatal Archive
//!   preflight row of `productionWorkApplicability`,
//!   `mixedExtractionAttemptsAdvanceProgress`,
//!   `committedExtractionSurvivesDiscoveryInterruption`, and the Archive half
//!   of `discoveryDiagnosticsSurviveInterruption`: `archive_discovery.rs`,
//!   each naming its origin.
//!
//! Not ported, with reasons:
//! - `terminalResultOwnsItsDataAfterTheRunEnds`: the result is an owned
//!   value, so outliving the executor, request and services is guaranteed by
//!   the borrow checker rather than by a test.

mod common;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use cao_core::Error;
use cao_core::execution::{AssetExecutionFailure, AssetExecutionResult, MutationState};
use cao_core::routing::{AssetOperation, ExecutionMode, PolicyValidationError, RequestedWork};
use cao_core::run::{
    ArchiveFinalizationAttempt, ArchiveFinalizationFailure, ArchiveFinalizationResult,
    ArchivePrecedence, CancellationToken, ModSelection, MutationKind, OptimizationRunResult,
    PhaseSkipReason, RunConfiguration, RunConfigurationProvider, RunDiagnosticCode,
    RunEvidenceInvariantPanic, RunExecutor, RunFailure, RunFailureCode, RunObservationSink,
    RunOutcome, RunPhase, RunPhaseRecord, RunPhaseStatus, RunPreparation, RunProgress, RunRequest,
    RunServices, RunWorkEvidence, RunWorkMilestones, RunWorkService, SafetyCleanupService,
    SelectedProfileFacts, TemporaryArtifactRegistry, create_run_id,
};
use common::{
    CallbackConfiguration, ControlledWork, CountingCleanup, FixedConfiguration, Observed,
    RecordingSink, ScriptedCleanup, ScriptedWork, canonical, no_work_request, request, scratch_dir,
    test_configuration,
};

/// Executes one request with the given services and a fresh run identity.
fn execute(
    request: &RunRequest,
    cleanup: &mut CountingCleanup,
    configuration: Option<&dyn RunConfigurationProvider>,
    work: Option<&dyn RunWorkService>,
    stop: &CancellationToken,
) -> cao_core::run::OptimizationRunResult {
    RunExecutor.execute(
        request,
        RunServices {
            safety_cleanup: cleanup,
            observations: None,
            configuration,
            work,
        },
        stop,
        create_run_id(),
    )
}

/// Origin: RunExecutorTests::noWorkApplyRunTraversesTheStablePhaseSequence,
/// noWorkRunReportsTheSameReasonsInEveryExecutionMode and
/// noWorkRunSucceedsWithoutInventingProgressTotals.
#[test]
fn a_no_work_run_traverses_every_phase_skipping_work_for_the_one_known_reason() {
    for mode in [ExecutionMode::Apply, ExecutionMode::DryRun] {
        let request = RunRequest::new(
            "SkyrimSE",
            mode,
            no_work_request().mod_selection().clone(),
            Vec::new(),
        );
        let mut cleanup = CountingCleanup::default();
        let result = execute(
            &request,
            &mut cleanup,
            Some(&*test_configuration()),
            None,
            &CancellationToken::new(),
        );

        assert_eq!(result.outcome(), RunOutcome::Succeeded);
        assert_eq!(result.final_phase(), RunPhase::ArchiveFinalization);
        let phases: Vec<_> = result
            .phases()
            .iter()
            .map(|record| record.phase())
            .collect();
        assert_eq!(phases, RunPhase::SEQUENCE);
        for record in &result.phases()[1..6] {
            assert_eq!(record.status(), RunPhaseStatus::Skipped);
            assert_eq!(
                record.skip_reason(),
                Some(PhaseSkipReason::NoRequestedWork),
                "{mode:?}"
            );
            assert_eq!(record.progress(), None, "skipped phases invent no totals");
        }
        assert_eq!(result.phases()[0].status(), RunPhaseStatus::Executed);
        assert_eq!(result.phases()[0].skip_reason(), None);
        assert_eq!(result.phases()[6].status(), RunPhaseStatus::Executed);
        assert!(
            result
                .phases()
                .iter()
                .all(|record| record.progress().is_none()),
            "no phase invents a progress total"
        );
        assert_eq!(result.mod_roots().len(), 1);
    }
}

/// Origin: RunExecutorTests::safetyCleanupRunsOnEveryTerminalPath and
/// safetyCleanupRunsExactlyOnceBeforeTheTerminalResult.
#[test]
fn safety_cleanup_runs_exactly_once_on_every_terminal_path() {
    let root = scratch_dir("executor-cleanup-paths");
    let missing = root.join("missing");
    let working = test_configuration();
    let failing = CallbackConfiguration(Box::new(|_| {
        Err(Error::ConfigurationLoading("unreadable".to_owned()))
    }));
    let cancelled = CancellationToken::new();
    cancelled.cancel();

    let paths: [(
        &str,
        RunRequest,
        &dyn RunConfigurationProvider,
        CancellationToken,
        RunOutcome,
    ); 4] = [
        (
            "success",
            no_work_request(),
            &*working,
            CancellationToken::new(),
            RunOutcome::Succeeded,
        ),
        (
            "configuration failure",
            no_work_request(),
            &failing,
            CancellationToken::new(),
            RunOutcome::Failed,
        ),
        (
            "missing Mod Root",
            RunRequest::new(
                "SkyrimSE",
                ExecutionMode::Apply,
                ModSelection::SingleModRoot(missing),
                Vec::new(),
            ),
            &*working,
            CancellationToken::new(),
            RunOutcome::Failed,
        ),
        (
            "cancellation",
            no_work_request(),
            &*working,
            cancelled,
            RunOutcome::Cancelled,
        ),
    ];
    for (name, request, configuration, stop, outcome) in paths {
        let mut cleanup = CountingCleanup::default();
        let result = execute(&request, &mut cleanup, Some(configuration), None, &stop);

        assert_eq!(result.outcome(), outcome, "{name}");
        assert_eq!(cleanup.passes, 1, "{name}: one cleanup pass");
        let last = result.phases().last().unwrap();
        assert_eq!(
            (last.phase(), last.status()),
            (RunPhase::SafetyCleanup, RunPhaseStatus::Executed),
            "{name}"
        );
    }
}

/// Origin: RunExecutorTests::configurationLoadingFailuresAreTerminal and
/// OptimizationRunServiceTests::configurationLoadingFailuresAreTerminalFailures.
#[test]
fn a_configuration_loading_failure_fails_preparing_with_its_detail() {
    let provider = CallbackConfiguration(Box::new(|_| {
        Err(Error::ConfigurationLoading(
            "profile.ini is unreadable".to_owned(),
        ))
    }));
    let mut cleanup = CountingCleanup::default();
    let result = execute(
        &no_work_request(),
        &mut cleanup,
        Some(&provider),
        None,
        &CancellationToken::new(),
    );

    assert_eq!(result.outcome(), RunOutcome::Failed);
    assert_eq!(result.final_phase(), RunPhase::Preparing);
    assert_eq!(result.failures().len(), 1);
    assert_eq!(
        result.failures()[0].code,
        RunFailureCode::ConfigurationLoadingFailed
    );
    assert_eq!(result.failures()[0].detail, "profile.ini is unreadable");
    assert!(
        result.preparation().is_none(),
        "a failed Preparing exposes no partial facts"
    );
}

/// A panicking provider fails Preparing the way a throwing one did in C++.
#[test]
fn a_panicking_configuration_provider_fails_preparing() {
    let provider = CallbackConfiguration(Box::new(|_| panic!("the provider exploded")));
    let mut cleanup = CountingCleanup::default();
    let result = execute(
        &no_work_request(),
        &mut cleanup,
        Some(&provider),
        None,
        &CancellationToken::new(),
    );

    assert_eq!(result.outcome(), RunOutcome::Failed);
    assert_eq!(
        result.failures()[0].code,
        RunFailureCode::ConfigurationLoadingFailed
    );
    assert!(
        result.failures()[0]
            .detail
            .contains("the provider exploded")
    );
    assert_eq!(cleanup.passes, 1);
}

/// A run without a configuration provider fails Preparing, not its start.
#[test]
fn a_missing_configuration_provider_fails_preparing() {
    let mut cleanup = CountingCleanup::default();
    let result = execute(
        &no_work_request(),
        &mut cleanup,
        None,
        None,
        &CancellationToken::new(),
    );

    assert_eq!(result.outcome(), RunOutcome::Failed);
    assert_eq!(
        result.failures()[0].code,
        RunFailureCode::ConfigurationLoadingFailed
    );
}

/// Origin: RunExecutorTests::policyConflictsFailPreparing.
#[test]
fn a_request_contradicting_the_profile_fails_preparing_with_every_conflict() {
    let provider = CallbackConfiguration(Box::new(|_| {
        Ok(RunConfiguration {
            profile: SelectedProfileFacts {
                archive_extension: Some(".ba2".to_owned()),
                ..SelectedProfileFacts::default()
            },
            ..RunConfiguration::default()
        })
    }));
    let root = scratch_dir("executor-policy-conflict");
    let request = request(
        ExecutionMode::Apply,
        &root,
        &[RequestedWork::StandardMeshOptimization],
    );
    let mut cleanup = CountingCleanup::default();
    let result = execute(
        &request,
        &mut cleanup,
        Some(&provider),
        None,
        &CancellationToken::new(),
    );

    assert_eq!(result.outcome(), RunOutcome::Failed);
    let failure = &result.failures()[0];
    assert_eq!(failure.code, RunFailureCode::PolicyConflict);
    assert_eq!(
        failure.policy_conflicts,
        vec![PolicyValidationError::UnsupportedRequestedAssetKind {
            request: RequestedWork::StandardMeshOptimization,
            kind: cao_core::routing::AssetKind::Mesh,
        }]
    );
}

/// Origin: RunExecutorTests::aNonDirectorySelectionFailsPreparing and
/// OptimizationRunServiceTests::missingModRootsAreTerminalFailures.
#[test]
fn a_selection_that_is_not_an_existing_directory_fails_preparing() {
    let root = scratch_dir("executor-non-directory");
    std::fs::write(root.join("file.txt"), b"not a directory").unwrap();
    for selection in [root.join("missing"), root.join("file.txt")] {
        let request = RunRequest::new(
            "SkyrimSE",
            ExecutionMode::Apply,
            ModSelection::SingleModRoot(selection),
            Vec::new(),
        );
        let mut cleanup = CountingCleanup::default();
        let result = execute(
            &request,
            &mut cleanup,
            Some(&*test_configuration()),
            None,
            &CancellationToken::new(),
        );

        assert_eq!(result.outcome(), RunOutcome::Failed);
        assert_eq!(
            result.failures()[0].code,
            RunFailureCode::ModSelectionResolutionFailed
        );
        assert!(result.mod_roots().is_empty());
    }
}

/// Origin: RunExecutorTests::filesystemRootSelectionFailsPreparing (both rows).
#[test]
fn a_filesystem_root_cannot_be_a_mod_root() {
    let drive = std::path::PathBuf::from(format!("{}\\", &env!("CARGO_MANIFEST_DIR")[..2]));
    for selection in [drive.clone(), drive.join(".")] {
        let request = RunRequest::new(
            "SkyrimSE",
            ExecutionMode::DryRun,
            ModSelection::SingleModRoot(selection.clone()),
            Vec::new(),
        );
        let mut cleanup = CountingCleanup::default();
        let result = execute(
            &request,
            &mut cleanup,
            Some(&*test_configuration()),
            None,
            &CancellationToken::new(),
        );

        assert_eq!(
            result.outcome(),
            RunOutcome::Failed,
            "{}",
            selection.display()
        );
        assert_eq!(result.final_phase(), RunPhase::Preparing);
        assert_eq!(
            result.failures()[0].code,
            RunFailureCode::ModSelectionResolutionFailed
        );
        assert!(result.preparation().is_none());
        assert_eq!(cleanup.passes, 1);
    }
}

/// Origin: OptimizationRunServiceTests::failureEventsPrecedeCleanupAndTerminal
/// (the RequestedWorkUnavailable row).
#[test]
fn requested_work_without_a_work_service_fails_preparing() {
    let root = scratch_dir("executor-no-work-service");
    let request = request(
        ExecutionMode::Apply,
        &root,
        &[RequestedWork::NativeTextureOptimization],
    );
    let mut cleanup = CountingCleanup::default();
    let result = execute(
        &request,
        &mut cleanup,
        Some(&*test_configuration()),
        None,
        &CancellationToken::new(),
    );

    assert_eq!(result.outcome(), RunOutcome::Failed);
    assert_eq!(
        result.failures()[0].code,
        RunFailureCode::RequestedWorkUnavailable
    );
    assert_eq!(result.failures()[0].phase, RunPhase::Preparing);
    assert!(result.preparation().is_none());
}

/// Origin: RunExecutorTests::cancellationDuringPreparingPublishesNoPreparation.
#[test]
fn cancellation_during_preparing_publishes_no_preparation() {
    let token = CancellationToken::new();
    let cancelling = {
        let token = token.clone();
        CallbackConfiguration(Box::new(move |_| {
            // The provider finishes its atomic read after cancellation arrives.
            token.cancel();
            Ok(RunConfiguration {
                profile: common::sse_profile(),
                ..RunConfiguration::default()
            })
        }))
    };
    let mut cleanup = CountingCleanup::default();
    let result = execute(
        &no_work_request(),
        &mut cleanup,
        Some(&cancelling),
        None,
        &token,
    );

    assert_eq!(result.outcome(), RunOutcome::Cancelled);
    assert!(result.cancellation_observed());
    assert!(result.preparation().is_none());
    let phases: Vec<_> = result
        .phases()
        .iter()
        .map(|record| record.phase())
        .collect();
    assert_eq!(phases, [RunPhase::Preparing, RunPhase::SafetyCleanup]);
    assert_eq!(cleanup.passes, 1);
}

/// Origin: RunExecutorTests::cleanupFailuresPreserveThePrimaryOutcome and
/// cleanupServiceExceptionsAreTerminal.
#[test]
fn cleanup_failures_complete_with_failures_but_a_failed_cleanup_service_is_fatal() {
    let artifact_failure = RunFailure::new(
        RunFailureCode::TemporaryArtifactCleanupFailed,
        RunPhase::SafetyCleanup,
        "a temporary file is still locked",
    );
    let mut cleanup = CountingCleanup {
        failures: vec![artifact_failure.clone()],
        ..CountingCleanup::default()
    };
    let result = execute(
        &no_work_request(),
        &mut cleanup,
        Some(&*test_configuration()),
        None,
        &CancellationToken::new(),
    );
    assert_eq!(result.outcome(), RunOutcome::CompletedWithFailures);
    assert_eq!(result.cleanup_failures(), [artifact_failure]);
    assert!(
        result.failures().is_empty(),
        "cleanup failures are not Run Failures"
    );

    let mut broken = CountingCleanup {
        service_error: Some("the registry is gone".to_owned()),
        ..CountingCleanup::default()
    };
    let result = execute(
        &no_work_request(),
        &mut broken,
        Some(&*test_configuration()),
        None,
        &CancellationToken::new(),
    );
    assert_eq!(result.outcome(), RunOutcome::Failed);
    assert_eq!(
        result.cleanup_failures()[0].code,
        RunFailureCode::SafetyCleanupServiceFailed
    );
}

/// Origin: RunExecutorTests::cleanupExceptionsPreserveCancellation.
#[test]
fn cancellation_wins_over_cleanup_failures() {
    let token = CancellationToken::new();
    token.cancel();
    let mut cleanup = CountingCleanup {
        failures: vec![RunFailure::new(
            RunFailureCode::TemporaryArtifactCleanupFailed,
            RunPhase::SafetyCleanup,
            "locked",
        )],
        ..CountingCleanup::default()
    };
    let result = execute(
        &no_work_request(),
        &mut cleanup,
        Some(&*test_configuration()),
        None,
        &token,
    );
    assert_eq!(result.outcome(), RunOutcome::Cancelled);
}

/// A work service that fails, panics or violates an evidence invariant.
struct FaultyWork(fn() -> Result<(), Error>);

impl RunWorkService for FaultyWork {
    fn execute(
        &self,
        _preparation: &RunPreparation,
        _evidence: &RunWorkEvidence<'_, '_>,
        _artifacts: &mut TemporaryArtifactRegistry,
        milestones: &dyn RunWorkMilestones,
        _stop: &CancellationToken,
    ) -> Result<(), Error> {
        milestones.archive_discovery_started()?;
        (self.0)()
    }
}

/// Origin: OptimizationRunServiceTests::workExceptionRetainsEarlierEvidence.
#[test]
fn a_failing_or_panicking_work_service_fails_at_its_phase_and_still_cleans_up() {
    let root = scratch_dir("executor-faulty-work");
    let request = request(
        ExecutionMode::DryRun,
        &root,
        &[RequestedWork::NativeTextureOptimization],
    );
    type Fault = fn() -> Result<(), Error>;
    let faults: [(&str, Fault); 2] = [
        ("error", || {
            Err(Error::WorkService("the backend is unavailable".to_owned()))
        }),
        ("panic", || panic!("the work service panicked")),
    ];
    for (name, fault) in faults {
        let work = FaultyWork(fault);
        let mut cleanup = CountingCleanup::default();
        let result = execute(
            &request,
            &mut cleanup,
            Some(&*test_configuration()),
            Some(&work),
            &CancellationToken::new(),
        );

        assert_eq!(result.outcome(), RunOutcome::Failed, "{name}");
        assert_eq!(
            result.final_phase(),
            RunPhase::DiscoveringArchives,
            "{name}"
        );
        assert_eq!(
            result.failures()[0].code,
            RunFailureCode::WorkServiceFailed,
            "{name}"
        );
        assert_eq!(
            result.failures()[0].phase,
            RunPhase::DiscoveringArchives,
            "{name}"
        );
        assert!(
            result.preparation().is_some(),
            "{name}: earlier evidence survives"
        );
        assert_eq!(cleanup.passes, 1, "{name}");
    }
}

/// A panic payload whose own drop panics again, which a containment boundary
/// that merely dropped it would let escape.
struct PanicsAgainOnDrop;

impl Drop for PanicsAgainOnDrop {
    fn drop(&mut self) {
        panic!("the panic payload panicked while it was dropped");
    }
}

/// Stages one durable Asset sibling under the run's Temporary Ownership, then
/// panics with the payload `panic` makes.
struct PanickingAfterStaging {
    panic: fn() -> !,
    staged: Mutex<Option<PathBuf>>,
}

impl RunWorkService for PanickingAfterStaging {
    fn execute(
        &self,
        preparation: &RunPreparation,
        _evidence: &RunWorkEvidence<'_, '_>,
        artifacts: &mut TemporaryArtifactRegistry,
        milestones: &dyn RunWorkMilestones,
        _stop: &CancellationToken,
    ) -> Result<(), Error> {
        milestones.archive_discovery_started()?;
        let root = &preparation.mod_roots()[0];
        let receipt = artifacts
            .capture_and_stage_file(root, &root.join("textures").join("a.dds"))
            .expect("the staging sibling is created");
        let staged = receipt.path().unwrap().to_path_buf();
        std::fs::write(&staged, "half-written output").unwrap();
        *self.staged.lock().unwrap() = Some(staged);
        (self.panic)()
    }
}

/// Spec (#476, #468): a panic in the Run Worker becomes a Failed outcome, and
/// Safety Cleanup still runs, removing what the run staged. The second row's
/// payload panics again when dropped, which must not escape the boundary.
#[test]
fn a_run_worker_panic_fails_the_run_and_safety_cleanup_still_removes_its_staging() {
    type Panic = fn() -> !;
    let rows: [(&str, Panic); 2] = [
        ("contained", || panic!("the work service panicked")),
        ("payload panics on drop", || {
            std::panic::panic_any(PanicsAgainOnDrop)
        }),
    ];
    for (name, panic) in rows {
        let root = scratch_dir(&format!("executor-worker-panic-{name}"));
        common::write_tree(&root, &["textures/a.dds"]);
        let work = PanickingAfterStaging {
            panic,
            staged: Mutex::default(),
        };
        let request = request(
            ExecutionMode::Apply,
            &root,
            &[RequestedWork::NativeTextureOptimization],
        );
        let mut cleanup = CountingCleanup::default();
        let configuration = test_configuration();
        let result = catch_unwind(AssertUnwindSafe(|| {
            execute(
                &request,
                &mut cleanup,
                Some(&*configuration),
                Some(&work),
                &CancellationToken::new(),
            )
        }))
        .unwrap_or_else(|_| panic!("{name}: the executor contains the panic"));

        assert_eq!(result.outcome(), RunOutcome::Failed, "{name}");
        assert_eq!(result.failures().len(), 1, "{name}");
        assert_eq!(
            result.failures()[0].code,
            RunFailureCode::WorkServiceFailed,
            "{name}"
        );
        assert_eq!(
            result.failures()[0].phase,
            RunPhase::DiscoveringArchives,
            "{name}"
        );
        assert_eq!(cleanup.passes, 1, "{name}");
        assert_eq!(
            result.phases().last().unwrap().phase(),
            RunPhase::SafetyCleanup,
            "{name}"
        );
        let staged = work.staged.lock().unwrap().clone().unwrap();
        assert!(
            !staged.exists(),
            "{name}: Safety Cleanup removed {}",
            staged.display()
        );
        assert_eq!(
            std::fs::read_to_string(root.join("textures/a.dds")).unwrap(),
            "textures/a.dds",
            "{name}: the original is untouched"
        );
        assert!(result.cleanup_failures().is_empty(), "{name}");
    }
}

/// Origin: RunExecutorTests::evidenceInvariantViolationsRemainProgrammingDefects.
#[test]
fn an_evidence_invariant_violation_panics_only_after_safety_cleanup() {
    let root = scratch_dir("executor-invariant");
    let request = request(
        ExecutionMode::DryRun,
        &root,
        &[RequestedWork::NativeTextureOptimization],
    );
    // Planning Processing Assets before any routing is a producer bug.
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
            milestones.asset_processing_planned(3)?;
            Ok(())
        }
    }
    let mut cleanup = CountingCleanup::default();
    let configuration = test_configuration();
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        execute(
            &request,
            &mut cleanup,
            Some(&*configuration),
            Some(&OutOfOrder),
            &CancellationToken::new(),
        )
    }));

    let payload = outcome.expect_err("the violation panics");
    let sealed = payload
        .downcast::<RunEvidenceInvariantPanic>()
        .expect("the panic carries the sealed result");
    assert!(
        sealed.violation.contains("Run Evidence invariant violated"),
        "{}",
        sealed.violation
    );
    assert_eq!(cleanup.passes, 1, "Safety Cleanup ran before the panic");
    // The sealed result keeps every earlier fact and records the violation.
    let result = &sealed.result;
    assert_eq!(result.outcome(), RunOutcome::Failed);
    assert!(result.preparation().is_some());
    assert_eq!(result.failures().len(), 1);
    assert_eq!(result.failures()[0].code, RunFailureCode::WorkServiceFailed);
    assert_eq!(result.failures()[0].detail, sealed.violation);
    assert_eq!(
        result.phases().last().unwrap().phase(),
        RunPhase::SafetyCleanup
    );
}

/// Origin: RunExecutorTests::cancellationAfterAtomicAssetAttempt (cancellation
/// between Assets keeps what was committed).
#[test]
fn cancelling_between_assets_keeps_committed_mutations_and_cleans_up() {
    let root = scratch_dir("executor-cancel-between-assets");
    common::write_tree(
        &root,
        &["textures/a.dds", "textures/b.dds", "textures/c.dds"],
    );
    let token = CancellationToken::new();
    let work = ScriptedWork {
        cancel_during: Some((1, token.clone())),
        ..ScriptedWork::committing()
    };
    let request = request(
        ExecutionMode::Apply,
        &root,
        &[RequestedWork::NativeTextureOptimization],
    );
    let mut cleanup = CountingCleanup::default();
    let result = execute(
        &request,
        &mut cleanup,
        Some(&*test_configuration()),
        Some(&work),
        &CancellationToken::new(),
    );

    assert_eq!(result.outcome(), RunOutcome::Cancelled);
    assert!(result.cancellation_observed());
    assert_eq!(
        work.attempted.lock().unwrap().len(),
        1,
        "the in-flight attempt finished; no other started"
    );
    assert_eq!(result.asset_attempts().len(), 1);
    let summaries = result.mutation_summaries();
    assert_eq!(summaries.len(), 1);
    assert_eq!(
        (summaries[0].committed, summaries[0].partial_or_unknown),
        (1, 0)
    );
    assert_eq!(cleanup.passes, 1);
    assert_eq!(
        result.phases().last().unwrap().phase(),
        RunPhase::SafetyCleanup
    );
    assert!(
        result.phase(RunPhase::ArchiveFinalization).is_none(),
        "nothing is invented after the stop"
    );
}

/// Origin: RunExecutorTests::schedulingFailureRetainsOrderedCleanupEvidence.
#[test]
fn a_scheduling_failure_records_only_safety_cleanup() {
    let token = CancellationToken::new();
    let cleanup_failure = |detail| {
        RunFailure::new(
            RunFailureCode::TemporaryArtifactCleanupFailed,
            RunPhase::SafetyCleanup,
            detail,
        )
    };
    let mut cleanup = ScriptedCleanup {
        cancels: Some(token.clone()),
        failures: vec![
            cleanup_failure("first artifact"),
            cleanup_failure("second artifact"),
        ],
        ..ScriptedCleanup::default()
    };
    let result = RunExecutor.scheduling_failure(
        "scheduler exhausted",
        &mut cleanup,
        None,
        &token,
        create_run_id(),
    );

    assert_eq!(result.outcome(), RunOutcome::Failed);
    assert_eq!(result.final_phase(), RunPhase::Preparing);
    assert!(result.cancellation_observed());
    assert_eq!(result.phases().len(), 1);
    assert_eq!(result.phases()[0].phase(), RunPhase::SafetyCleanup);
    assert_eq!(result.failures().len(), 1);
    assert_eq!(result.failures()[0].code, RunFailureCode::SchedulingFailed);
    let details: Vec<_> = result
        .cleanup_failures()
        .iter()
        .map(|failure| failure.detail.as_str())
        .collect();
    assert_eq!(details, ["first artifact", "second artifact"]);
    assert_eq!(cleanup.passes, 1);
}

/// Origin: RunExecutorTests::requestedWorkIsADeduplicatedClosedSetInEnumerationOrder.
#[test]
fn requested_work_is_a_deduplicated_set_in_enumeration_order() {
    let request = RunRequest::new(
        "SkyrimSE",
        ExecutionMode::Apply,
        ModSelection::SingleModRoot("mods".into()),
        vec![
            RequestedWork::AnimationOptimization,
            RequestedWork::NativeTextureOptimization,
            RequestedWork::AnimationOptimization,
        ],
    );
    assert_eq!(
        request.requested_work(),
        [
            RequestedWork::NativeTextureOptimization,
            RequestedWork::AnimationOptimization
        ]
    );
    assert!(request.requests(RequestedWork::AnimationOptimization));
    assert!(!request.requests(RequestedWork::ArchiveCreation));
}

/// Executes one request with every service supplied by the scenario.
fn execute_with(
    request: &RunRequest,
    cleanup: &mut dyn SafetyCleanupService,
    observations: Option<&dyn RunObservationSink>,
    configuration: Option<&dyn RunConfigurationProvider>,
    work: Option<&dyn RunWorkService>,
    stop: &CancellationToken,
) -> OptimizationRunResult {
    RunExecutor.execute(
        request,
        RunServices {
            safety_cleanup: cleanup,
            observations,
            configuration,
            work,
        },
        stop,
        create_run_id(),
    )
}

/// A fresh, canonical Mod Root holding `files`.
fn mod_root(name: &str, files: &[&str]) -> PathBuf {
    let root = canonical(&scratch_dir(&format!("executor/{name}")));
    common::write_tree(&root, files);
    root
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

/// Asserts the work's staged temporary existed and Safety Cleanup removed it.
fn assert_temporary_removed(work: &ControlledWork) {
    let staged = work.staged_temporary().expect("Apply staged a temporary");
    assert!(!staged.exists(), "{} survived the run", staged.display());
}

fn diagnostic_count(result: &OptimizationRunResult, code: RunDiagnosticCode) -> usize {
    result
        .diagnostics()
        .iter()
        .filter(|diagnostic| diagnostic.code == code)
        .count()
}

/// A provider that ignores the named mods under the SSE-like profile.
fn ignoring(mods: &[&str]) -> FixedConfiguration {
    FixedConfiguration {
        configuration: RunConfiguration {
            profile: common::sse_profile(),
            ignored_mods: mods.iter().map(|name| (*name).to_owned()).collect(),
            ..RunConfiguration::default()
        },
        loads: Default::default(),
    }
}

/// Origin: RunExecutorTests::processingAndEvidenceShareModRoot (all four
/// rows). The Mod Root handed to each attempt is the canonical prepared one,
/// and the attempt records that same root, for one Mod Root or Several Mods,
/// selected by an absolute or a relative path.
#[test]
fn processing_and_evidence_share_the_canonical_mod_root() {
    for (row, several, relative) in [
        ("single-absolute", false, false),
        ("single-relative", false, true),
        ("several-absolute", true, false),
        ("several-relative", true, true),
    ] {
        let selected = canonical(&scratch_dir(&format!("executor/shared-root-{row}")));
        let roots: Vec<PathBuf> = if several {
            vec![selected.join("first"), selected.join("second")]
        } else {
            vec![selected.clone()]
        };
        for root in &roots {
            common::write_tree(root, &["textures/asset.dds"]);
        }
        let path = if relative {
            common::relative_to_working_directory(&selected)
        } else {
            selected.clone()
        };
        let selection = if several {
            ModSelection::ChildModRoots(path)
        } else {
            ModSelection::SingleModRoot(path)
        };
        let request = RunRequest::new(
            "SkyrimSE",
            ExecutionMode::Apply,
            selection,
            vec![RequestedWork::NativeTextureOptimization],
        );
        let work = ControlledWork::default();
        let mut cleanup = CountingCleanup::default();

        let result = execute(
            &request,
            &mut cleanup,
            Some(&*test_configuration()),
            Some(&work),
            &CancellationToken::new(),
        );

        assert_eq!(result.outcome(), RunOutcome::Succeeded, "{row}");
        assert_eq!(work.attempted_roots(), roots, "{row}");
        assert_eq!(result.asset_attempts().len(), roots.len(), "{row}");
        for (attempt, root) in result.asset_attempts().iter().zip(&roots) {
            assert_eq!(&attempt.mod_root, root, "{row}");
            assert_eq!(
                attempt.asset.execution_path(),
                root.join("textures").join("asset.dds"),
                "{row}"
            );
        }
        assert_eq!(cleanup.passes, 1, "{row}");
    }
}

/// Origin: RunExecutorTests::removedConversionRetainsModRoot (both rows). A
/// conversion that removed its source keeps its committed attempt and Mod
/// Root when a later checkpoint fails or cancels, and Archive Finalization is
/// never reached.
#[test]
fn a_removed_conversion_keeps_its_attempt_after_a_later_interruption() {
    for cancels in [false, true] {
        let selected = canonical(&scratch_dir(&format!(
            "executor/removed-conversion-{cancels}"
        )));
        let root = selected.join("mod");
        common::write_tree(&root, &["original.tga"]);
        let token = CancellationToken::new();
        let converted = Arc::new(AtomicBool::new(false));
        let work = ControlledWork {
            execute: Some(Box::new({
                let converted = converted.clone();
                move |asset, _| {
                    let source = asset.execution_path();
                    std::fs::write(source.with_extension("dds"), "converted").unwrap();
                    std::fs::remove_file(source).unwrap();
                    converted.store(true, Ordering::SeqCst);
                    Ok(AssetExecutionResult::success(MutationState::Committed))
                }
            })),
            is_cancelled: Some(Box::new({
                let token = token.clone();
                move || {
                    if !converted.load(Ordering::SeqCst) {
                        return false;
                    }
                    if !cancels {
                        panic!("later orchestration failure");
                    }
                    token.cancel();
                    true
                }
            })),
            finalize: Some(Box::new(|| Ok(()))),
            stage_temporary: true,
            ..ControlledWork::default()
        };
        let request = RunRequest::new(
            "SkyrimSE",
            ExecutionMode::Apply,
            ModSelection::ChildModRoots(common::relative_to_working_directory(&selected)),
            vec![RequestedWork::ConvertibleTextureConversion],
        );
        let mut cleanup = CountingCleanup::default();

        let result = execute(
            &request,
            &mut cleanup,
            Some(&*test_configuration()),
            Some(&work),
            &token,
        );

        let row = if cancels {
            "later cancellation"
        } else {
            "later failure"
        };
        let expected = if cancels {
            RunOutcome::Cancelled
        } else {
            RunOutcome::Failed
        };
        assert_eq!(result.outcome(), expected, "{row}");
        if !cancels {
            assert_eq!(result.failures()[0].code, RunFailureCode::WorkServiceFailed);
            assert!(
                result.failures()[0]
                    .detail
                    .contains("later orchestration failure")
            );
        }
        assert_eq!(work.finalizations.load(Ordering::SeqCst), 0, "{row}");
        assert_eq!(cleanup.passes, 1, "{row}");
        assert_temporary_removed(&work);
        let attempts = result.asset_attempts();
        assert_eq!(attempts.len(), 1, "{row}");
        assert_eq!(attempts[0].mod_root, root, "{row}");
        assert_eq!(
            attempts[0].asset.execution_path(),
            root.join("original.tga")
        );
        assert!(
            attempts[0]
                .asset
                .operations()
                .contains(AssetOperation::Conversion)
        );
        assert_eq!(
            attempts[0].result.mutation_state(),
            MutationState::Committed
        );
        let summaries = result.mutation_summaries();
        assert_eq!(summaries.len(), 1, "{row}");
        assert_eq!(summaries[0].kind, MutationKind::AssetProcessing);
        assert_eq!(
            (summaries[0].committed, summaries[0].partial_or_unknown),
            (1, 0)
        );
        let progress = result.phase(RunPhase::ProcessingAssets).unwrap().progress();
        assert_eq!(progress.unwrap().completed(), 1, "{row}");
        assert!(
            result.phase(RunPhase::ArchiveFinalization).is_none(),
            "{row}"
        );
        assert!(!root.join("original.tga").exists());
        assert_eq!(read(&root.join("original.dds")), "converted");
    }
}

/// Origin: RunExecutorTests::retargetedAssetUsesCanonicalContainment (both
/// rows). The Mod Root is resolved when the attempt starts: an Asset whose
/// folder was swapped for a link after discovery is attributed to the
/// prepared root that really contains it, and one that now resolves outside
/// every prepared root is never offered to the backend. A junction stands in
/// for the C++ file symlink, which needs a privilege a junction does not.
#[test]
fn an_asset_retargeted_after_discovery_is_attributed_to_the_root_containing_it() {
    for outside in [false, true] {
        let row = if outside { "outside" } else { "sibling" };
        let base = canonical(&scratch_dir(&format!("executor/retargeted-{row}")));
        let mods = base.join("mods");
        let (first, second) = (mods.join("first"), mods.join("second"));
        common::write_tree(&first, &["textures/asset.dds"]);
        let target = if outside {
            base.join("outside")
        } else {
            second.join("retarget")
        };
        std::fs::create_dir_all(&second).unwrap();
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("asset.dds"), "target bytes").unwrap();
        let link = first.join("textures");
        let retargeted = Arc::new(AtomicBool::new(false));
        let work = ControlledWork {
            report_phase: Some(Box::new({
                let (link, target, retargeted) = (link.clone(), target.clone(), retargeted.clone());
                move |record| {
                    if record.phase() == RunPhase::ProcessingAssets
                        && !retargeted.swap(true, Ordering::SeqCst)
                    {
                        std::fs::rename(&link, link.with_file_name("moved")).unwrap();
                        common::junction(&link, &target);
                    }
                }
            })),
            ..ControlledWork::default()
        };
        let request = RunRequest::new(
            "SkyrimSE",
            ExecutionMode::Apply,
            ModSelection::ChildModRoots(mods.clone()),
            vec![RequestedWork::NativeTextureOptimization],
        );
        let mut cleanup = CountingCleanup::default();

        let result = execute(
            &request,
            &mut cleanup,
            Some(&*test_configuration()),
            Some(&work),
            &CancellationToken::new(),
        );
        std::fs::remove_dir(&link).unwrap();

        assert!(retargeted.load(Ordering::SeqCst), "{row}");
        let attempt = &result.asset_attempts()[0];
        assert_eq!(
            attempt.asset.execution_path(),
            link.join("asset.dds"),
            "{row}"
        );
        if outside {
            assert_eq!(result.outcome(), RunOutcome::Failed, "{row}");
            assert_eq!(result.asset_attempts().len(), 1, "{row}");
            assert!(work.attempted_roots().is_empty(), "{row}: never offered");
            assert_eq!(attempt.mod_root, PathBuf::new(), "{row}");
            assert!(!attempt.result.safe_to_continue(), "{row}");
        } else {
            assert_eq!(result.outcome(), RunOutcome::Succeeded, "{row}");
            assert_eq!(work.attempted_roots()[0], second, "{row}");
            assert_eq!(attempt.mod_root, second, "{row}");
        }
        assert_eq!(read(&target.join("asset.dds")), "target bytes", "{row}");
        assert_eq!(cleanup.passes, 1, "{row}");
    }
}

/// Origin: RunExecutorTests::productionWorkApplicability (the empty
/// requested work, Dry Run evaluation and empty Dry Run rows). Applicable
/// work with nothing to do still processes Assets against a 0/0 account; Dry
/// Run evaluates Assets but skips extraction and finalization; nothing is
/// extracted or finalized, and only Apply stages anything.
#[test]
fn applicable_work_reports_its_phases_whether_or_not_there_is_anything_to_do() {
    // (row, mode, has an Asset, has a finalizer)
    let rows = [
        ("empty requested work", ExecutionMode::Apply, false, false),
        ("dry run evaluation", ExecutionMode::DryRun, true, true),
        ("empty dry run", ExecutionMode::DryRun, false, false),
    ];
    for (index, (row, mode, has_asset, has_finalizer)) in rows.into_iter().enumerate() {
        let root = mod_root(&format!("applicability-{index}"), &[]);
        if has_asset {
            std::fs::write(root.join("asset.dds"), "untouched").unwrap();
        }
        let work = ControlledWork {
            finalize: has_finalizer.then(|| Box::new(|| Ok(())) as common::Hook<_>),
            stage_temporary: true,
            ..ControlledWork::default()
        };
        let request = request(
            mode,
            &root,
            &[
                RequestedWork::NativeTextureOptimization,
                RequestedWork::ArchiveExtraction,
            ],
        );
        let mut cleanup = CountingCleanup::default();

        let result = execute(
            &request,
            &mut cleanup,
            Some(&*test_configuration()),
            Some(&work),
            &CancellationToken::new(),
        );

        assert_eq!(result.outcome(), RunOutcome::Succeeded, "{row}");
        let total = usize::from(has_asset);
        assert_eq!(work.executions.load(Ordering::SeqCst), total, "{row}");
        assert_eq!(work.finalizations.load(Ordering::SeqCst), 0, "{row}");
        assert!(result.mutation_summaries().is_empty(), "{row}");
        assert_eq!(cleanup.passes, 1, "{row}");
        assert!(result.routing_ledger().is_some(), "{row}");
        assert_eq!(
            result.phase(RunPhase::ProcessingAssets),
            Some(&RunPhaseRecord::executed(
                RunPhase::ProcessingAssets,
                Some(RunProgress::determinate(total, total, 0))
            )),
            "{row}"
        );
        let finalization_skip = if mode == ExecutionMode::DryRun {
            assert_eq!(
                result.phase(RunPhase::ExtractingArchives),
                Some(&RunPhaseRecord::skipped(
                    RunPhase::ExtractingArchives,
                    PhaseSkipReason::DryRun
                )),
                "{row}"
            );
            assert!(work.staged_temporary().is_none(), "{row}");
            assert!(!root.join(".cao-staging").exists(), "{row}");
            PhaseSkipReason::DryRun
        } else {
            assert_temporary_removed(&work);
            PhaseSkipReason::NoRequestedWork
        };
        assert_eq!(
            result
                .phase(RunPhase::ArchiveFinalization)
                .unwrap()
                .skip_reason(),
            Some(finalization_skip),
            "{row}"
        );
        if has_asset {
            assert_eq!(read(&root.join("asset.dds")), "untouched", "{row}");
        }
    }
}

/// Origin: RunExecutorTests::assetWorkPhasesPrecedeAttempts. Processing
/// Assets is published before any attempt and Archive Finalization before
/// its finalizer, and the whole canonical sequence is published in order.
#[test]
fn each_work_phase_is_published_before_its_work_starts() {
    let root = mod_root("phases-precede-work", &["texture.dds"]);
    let published = Arc::new(Mutex::new(Vec::<RunPhase>::new()));
    let sink = RecordingSink::with_hook({
        let published = published.clone();
        move |observed| {
            if let Observed::Phase(record) = observed {
                let mut published = published.lock().unwrap();
                // Progress updates republish the current phase.
                if published.last() != Some(&record.phase()) {
                    published.push(record.phase());
                }
            }
        }
    });
    let last_published = {
        let published = published.clone();
        move || published.lock().unwrap().last().copied()
    };
    let work = ControlledWork {
        execute: Some(Box::new({
            let last_published = last_published.clone();
            move |_, _| {
                assert_eq!(last_published(), Some(RunPhase::ProcessingAssets));
                Ok(AssetExecutionResult::success(MutationState::None))
            }
        })),
        finalize: Some(Box::new(move || {
            assert_eq!(last_published(), Some(RunPhase::ArchiveFinalization));
            Ok(())
        })),
        ..ControlledWork::default()
    };
    let mut cleanup = CountingCleanup::default();

    let result = execute_with(
        &request(
            ExecutionMode::Apply,
            &root,
            &[RequestedWork::NativeTextureOptimization],
        ),
        &mut cleanup,
        Some(&sink),
        Some(&*test_configuration()),
        Some(&work),
        &CancellationToken::new(),
    );

    assert_eq!(
        result.outcome(),
        RunOutcome::Succeeded,
        "{:?}",
        result.failures()
    );
    assert_eq!(result.asset_attempts().len(), 1);
    assert_eq!(work.finalizations.load(Ordering::SeqCst), 1);
    assert_eq!(*published.lock().unwrap(), RunPhase::SEQUENCE);
}

/// Origin: RunExecutorTests::preparingDiagnosticsSurviveWork. A Mod Exclusion
/// found during Preparing is still in the terminal result after work ran,
/// with no observer at all.
#[test]
fn a_preparing_diagnostic_survives_work_without_an_observer() {
    let mods = canonical(&scratch_dir("executor/preparing-diagnostic"));
    std::fs::create_dir_all(mods.join("ignored")).unwrap();
    std::fs::create_dir_all(mods.join("selected")).unwrap();
    let request = RunRequest::new(
        "SkyrimSE",
        ExecutionMode::Apply,
        ModSelection::ChildModRoots(mods.clone()),
        vec![RequestedWork::NativeTextureOptimization],
    );
    let work = ControlledWork::default();
    let mut cleanup = CountingCleanup::default();

    let result = execute(
        &request,
        &mut cleanup,
        Some(&ignoring(&["ignored"])),
        Some(&work),
        &CancellationToken::new(),
    );

    assert_eq!(cleanup.passes, 1);
    assert_eq!(result.outcome(), RunOutcome::Succeeded);
    assert!(result.routing_ledger().is_some());
    assert_eq!(result.diagnostics().len(), 1);
    let diagnostic = &result.diagnostics()[0];
    assert_eq!(diagnostic.code, RunDiagnosticCode::IgnoredModExcluded);
    assert_eq!(diagnostic.phase, RunPhase::Preparing);
    assert_eq!(diagnostic.path, mods.join("ignored"));
    assert!(!diagnostic.detail.is_empty());
}

/// Origin: RunExecutorTests::discoveryDiagnosticCancellationFollowsAssetAttempt
/// (all three rows). A linked-entry diagnostic found during discovery is
/// published after the Asset attempt, before Archive Finalization: cancelling
/// from it skips finalization, and a panicking observer, at that diagnostic
/// or at Preparing's, is one `ObserverFailed` diagnostic and never a Run
/// Failure. A junction stands in for the C++ file symlink.
#[test]
fn a_discovery_diagnostic_follows_the_attempt_and_can_cancel_finalization() {
    // (row, panics at the Preparing diagnostic, panics at the work diagnostic, cancels)
    let rows = [
        ("cancel diagnostic", false, false, true),
        ("panicking diagnostics", true, true, false),
        ("cancel then panic", true, true, true),
    ];
    for (index, (row, panics_preparing, panics_work, cancels)) in rows.into_iter().enumerate() {
        let mods = canonical(&scratch_dir(&format!("executor/diagnostic-order-{index}")));
        let selected = mods.join("selected");
        common::write_tree(&mods, &["selected/asset.dds", "ignored/external.dds"]);
        let link = selected.join("linked");
        common::junction(&link, &mods.join("ignored"));
        let token = CancellationToken::new();
        let order = Arc::new(Mutex::new(Vec::<&'static str>::new()));
        let sink = RecordingSink::with_hook({
            let (order, token) = (order.clone(), token.clone());
            move |observed| match observed {
                Observed::Diagnostic(diagnostic)
                    if diagnostic.code == RunDiagnosticCode::IgnoredModExcluded =>
                {
                    order.lock().unwrap().push("preparing");
                    if panics_preparing {
                        panic!("the Preparing diagnostic observer panicked");
                    }
                }
                Observed::Diagnostic(diagnostic)
                    if diagnostic.code == RunDiagnosticCode::LinkedEntryExcluded =>
                {
                    order.lock().unwrap().push("work diagnostic");
                    if cancels {
                        token.cancel();
                    }
                    if panics_work {
                        panic!("the work diagnostic observer panicked");
                    }
                }
                Observed::Phase(record) if record.phase() == RunPhase::SafetyCleanup => {
                    order.lock().unwrap().push("cleanup");
                }
                _ => {}
            }
        });
        let work = ControlledWork {
            execute: Some(Box::new({
                let order = order.clone();
                move |_, _| {
                    order.lock().unwrap().push("asset");
                    Ok(AssetExecutionResult::success(MutationState::None))
                }
            })),
            finalize: Some(Box::new(|| Ok(()))),
            stage_temporary: true,
            ..ControlledWork::default()
        };
        let request = RunRequest::new(
            "SkyrimSE",
            ExecutionMode::Apply,
            ModSelection::ChildModRoots(mods.clone()),
            vec![RequestedWork::NativeTextureOptimization],
        );
        let mut cleanup = CountingCleanup::default();

        let result = execute_with(
            &request,
            &mut cleanup,
            Some(&sink),
            Some(&ignoring(&["ignored"])),
            Some(&work),
            &token,
        );
        std::fs::remove_dir(&link).unwrap();

        let expected = if cancels {
            RunOutcome::Cancelled
        } else {
            RunOutcome::Succeeded
        };
        assert_eq!(result.outcome(), expected, "{row}");
        assert_eq!(
            *order.lock().unwrap(),
            ["preparing", "asset", "work diagnostic", "cleanup"],
            "{row}"
        );
        assert_eq!(result.asset_attempts().len(), 1, "{row}");
        assert_eq!(
            work.finalizations.load(Ordering::SeqCst),
            usize::from(!cancels),
            "{row}"
        );
        assert_eq!(
            result.phase(RunPhase::ArchiveFinalization).is_none(),
            cancels,
            "{row}"
        );
        assert_eq!(
            diagnostic_count(&result, RunDiagnosticCode::IgnoredModExcluded),
            1
        );
        assert_eq!(
            diagnostic_count(&result, RunDiagnosticCode::LinkedEntryExcluded),
            1
        );
        assert_eq!(
            diagnostic_count(&result, RunDiagnosticCode::ObserverFailed),
            usize::from(panics_preparing) + usize::from(panics_work),
            "{row}"
        );
        assert!(result.failures().is_empty(), "{row}");
        assert_eq!(cleanup.passes, 1, "{row}");
        assert_temporary_removed(&work);
    }
}

/// Origin: RunExecutorTests::throwingWorkObserversRetainEvidence (both rows).
/// An observer panicking on the first attempt's progress keeps the failed
/// attempt; the panic is a diagnostic, and only cancellation skips
/// finalization.
#[test]
fn a_panicking_progress_observer_keeps_the_failed_attempt() {
    for cancels in [false, true] {
        let row = if cancels {
            "cancel then panic"
        } else {
            "panic"
        };
        let root = mod_root(&format!("progress-observer-{cancels}"), &[]);
        std::fs::write(root.join("asset.dds"), "original").unwrap();
        let token = CancellationToken::new();
        let sink = RecordingSink::with_hook({
            let token = token.clone();
            move |observed| {
                if let Observed::Phase(record) = observed
                    && record.phase() == RunPhase::ProcessingAssets
                    && record
                        .progress()
                        .is_some_and(|progress| progress.completed() == 1)
                {
                    if cancels {
                        token.cancel();
                    }
                    panic!("the progress observer panicked");
                }
            }
        });
        let work = ControlledWork {
            execute: Some(Box::new(|_, _| {
                Ok(AssetExecutionResult::failed(
                    AssetExecutionFailure::CommitFailed,
                    "controlled recoverable failure",
                ))
            })),
            finalize: Some(Box::new(|| Ok(()))),
            stage_temporary: true,
            ..ControlledWork::default()
        };
        let mut cleanup = CountingCleanup::default();

        let result = execute_with(
            &request(
                ExecutionMode::Apply,
                &root,
                &[RequestedWork::NativeTextureOptimization],
            ),
            &mut cleanup,
            Some(&sink),
            Some(&*test_configuration()),
            Some(&work),
            &token,
        );

        let expected = if cancels {
            RunOutcome::Cancelled
        } else {
            RunOutcome::CompletedWithFailures
        };
        assert_eq!(result.outcome(), expected, "{row}");
        assert_eq!(result.asset_attempts().len(), 1, "{row}");
        let attempt = &result.asset_attempts()[0].result;
        assert_eq!(attempt.failure(), Some(AssetExecutionFailure::CommitFailed));
        assert_eq!(attempt.message(), "controlled recoverable failure");
        assert!(result.failures().is_empty(), "{row}");
        assert!(
            !sink
                .observed()
                .iter()
                .any(|observed| matches!(observed, Observed::Failure(_))),
            "{row}"
        );
        assert_eq!(
            result.phase(RunPhase::ProcessingAssets).unwrap().progress(),
            Some(RunProgress::determinate(1, 0, 1)),
            "{row}"
        );
        assert_eq!(
            diagnostic_count(&result, RunDiagnosticCode::ObserverFailed),
            1,
            "{row}"
        );
        assert_eq!(
            work.finalizations.load(Ordering::SeqCst),
            usize::from(!cancels),
            "{row}"
        );
        assert_eq!(cleanup.passes, 1, "{row}");
        assert_temporary_removed(&work);
        assert_eq!(read(&root.join("asset.dds")), "original", "{row}");
    }
}

/// Origin: RunExecutorTests::mixedWorkEvidenceOutlivesServices. Mixed Asset
/// and Archive Finalization successes and failures complete with failures,
/// and every fact is owned by the result once the services are gone.
#[test]
fn mixed_asset_evidence_outlives_every_service() {
    let root = mod_root("mixed-evidence", &[]);
    for name in ["a.dds", "b.dds"] {
        std::fs::write(root.join(name), "original").unwrap();
    }
    let output = root.join("packed.bsa");
    let result = {
        let packed = output.clone();
        let failed_root = root.clone();
        let work = ControlledWork {
            execute: Some(Box::new(|asset, _| {
                let path = asset.execution_path();
                Ok(if path.ends_with("a.dds") {
                    std::fs::write(path, "committed").unwrap();
                    AssetExecutionResult::success(MutationState::Committed)
                } else {
                    AssetExecutionResult::failed(
                        AssetExecutionFailure::LoadFailed,
                        "controlled load failure",
                    )
                    .with_path(path)
                    .with_operation("load_texture")
                    .with_service_detail("raw detail")
                })
            })),
            finalization: Some(Box::new(move || {
                std::fs::write(&packed, "packed").unwrap();
                ArchiveFinalizationResult {
                    attempts: vec![
                        finalization_attempt(&packed, &failed_root, MutationState::Committed),
                        ArchiveFinalizationAttempt {
                            failure: Some(ArchiveFinalizationFailure::WriteFailed),
                            detail: "write detail".to_owned(),
                            ..finalization_attempt(
                                &failed_root.join("failed.bsa"),
                                &failed_root,
                                MutationState::None,
                            )
                        },
                    ],
                    ..ArchiveFinalizationResult::default()
                }
            })),
            stage_temporary: true,
            ..ControlledWork::default()
        };
        let mut cleanup = CountingCleanup::default();
        let result = execute(
            &request(
                ExecutionMode::Apply,
                &root,
                &[
                    RequestedWork::NativeTextureOptimization,
                    RequestedWork::ArchiveCreation,
                ],
            ),
            &mut cleanup,
            Some(&*test_configuration()),
            Some(&work),
            &CancellationToken::new(),
        );
        assert_eq!(work.finalizations.load(Ordering::SeqCst), 1);
        assert_eq!(cleanup.passes, 1);
        assert_temporary_removed(&work);
        result
    };

    assert_eq!(result.outcome(), RunOutcome::CompletedWithFailures);
    let ledger = result.routing_ledger().unwrap();
    let routed: Vec<_> = ledger
        .routed_assets()
        .iter()
        .map(|asset| asset.execution_path().to_path_buf())
        .collect();
    assert_eq!(routed, [root.join("a.dds"), root.join("b.dds")]);
    let attempts = result.asset_attempts();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].mod_root, root);
    assert_eq!(attempts[0].asset.execution_path(), root.join("a.dds"));
    assert_eq!(
        attempts[0].result.mutation_state(),
        MutationState::Committed
    );
    let failed = &attempts[1].result;
    assert_eq!(attempts[1].mod_root, root);
    assert_eq!(failed.failure(), Some(AssetExecutionFailure::LoadFailed));
    assert_eq!(failed.mutation_state(), MutationState::None);
    assert_eq!(failed.affected_path(), root.join("b.dds"));
    assert_eq!(failed.operation(), "load_texture");
    assert_eq!(failed.service_detail(), "raw detail");
    assert!(failed.safe_to_continue());
    assert_eq!(
        result.phase(RunPhase::ProcessingAssets).unwrap().progress(),
        Some(RunProgress::determinate(2, 1, 1))
    );
    assert!(result.failures().is_empty());
    let finalization = result.archive_finalization().unwrap();
    assert_eq!(finalization.attempts.len(), 2);
    assert_eq!(finalization.attempts[0].archive_path, output);
    assert_eq!(finalization.attempts[0].mod_root, root);
    assert_eq!(finalization.attempts[0].mutation, MutationState::Committed);
    assert_eq!(
        finalization.attempts[1].failure,
        Some(ArchiveFinalizationFailure::WriteFailed)
    );
    assert_eq!(finalization.attempts[1].detail, "write detail");
    let summaries: Vec<_> = result
        .mutation_summaries()
        .iter()
        .map(|summary| {
            (
                &summary.mod_root,
                summary.kind,
                summary.committed,
                summary.partial_or_unknown,
            )
        })
        .collect();
    assert_eq!(
        summaries,
        [
            (&root, MutationKind::AssetProcessing, 1, 0),
            (&root, MutationKind::ArchiveFinalization, 1, 0),
        ]
    );
    assert!(result.cleanup_failures().is_empty());
    assert_eq!(read(&root.join("a.dds")), "committed");
    assert_eq!(read(&root.join("b.dds")), "original");
    assert_eq!(read(&output), "packed");
    assert_eq!(
        result.phases().last().unwrap().phase(),
        RunPhase::SafetyCleanup
    );
}

/// An Archive Finalization attempt at `archive` in `root`.
fn finalization_attempt(
    archive: &Path,
    root: &Path,
    mutation: MutationState,
) -> ArchiveFinalizationAttempt {
    ArchiveFinalizationAttempt {
        mutation,
        ..ArchiveFinalizationAttempt::new(archive.to_path_buf(), root.to_path_buf())
    }
}

/// Origin: RunExecutorTests::archiveFinalizationCancellationRetainsCommittedOutput.
/// A cancelled finalization keeps its committed output, and the run is
/// Cancelled after Safety Cleanup.
#[test]
fn a_cancelled_finalization_keeps_its_committed_output() {
    let root = mod_root("finalization-cancelled", &[]);
    let output = root.join("committed.bsa");
    let (packed, packed_root) = (output.clone(), root.clone());
    let work = ControlledWork {
        finalization: Some(Box::new(move || {
            std::fs::write(&packed, "committed output").unwrap();
            ArchiveFinalizationResult {
                attempts: vec![finalization_attempt(
                    &packed,
                    &packed_root,
                    MutationState::Committed,
                )],
                cancelled: true,
                ..ArchiveFinalizationResult::default()
            }
        })),
        stage_temporary: true,
        ..ControlledWork::default()
    };
    let mut cleanup = CountingCleanup::default();
    let result = execute(
        &request(
            ExecutionMode::Apply,
            &root,
            &[RequestedWork::ArchiveCreation],
        ),
        &mut cleanup,
        Some(&*test_configuration()),
        Some(&work),
        &CancellationToken::new(),
    );

    assert_eq!(work.finalizations.load(Ordering::SeqCst), 1);
    assert_eq!(cleanup.passes, 1);
    assert_eq!(result.outcome(), RunOutcome::Cancelled);
    assert_eq!(result.final_phase(), RunPhase::ArchiveFinalization);
    assert!(result.cancellation_observed());
    assert!(result.failures().is_empty());
    let finalization = result.archive_finalization().unwrap();
    assert!(finalization.cancelled);
    assert_eq!(finalization.attempts.len(), 1);
    assert_eq!(finalization.attempts[0].mod_root, root);
    assert_eq!(finalization.attempts[0].mutation, MutationState::Committed);
    assert!(finalization.attempts[0].safe_to_continue);
    let progress = result
        .phase(RunPhase::ArchiveFinalization)
        .unwrap()
        .progress()
        .unwrap();
    assert_eq!(progress.completed(), 1);
    assert!(result.cleanup_failures().is_empty());
    assert_temporary_removed(&work);
    assert_eq!(read(&output), "committed output");
}

/// Origin: RunExecutorTests::missingPlannedLoadingPluginFailsRunWithArchiveCommit.
/// An output whose Loading Plugin failed after its Archive committed fails
/// the run, and the committed Archive still counts as a mutation.
#[test]
fn a_committed_archive_without_its_loading_plugin_fails_the_run() {
    let root = mod_root("missing-loading-plugin", &["textures/asset.dds"]);
    let source = root.join("textures/asset.dds");
    let output = root.join("packed.bsa");
    let (packed, packed_root) = (output.clone(), root.clone());
    let work = ControlledWork {
        finalization: Some(Box::new(move || {
            std::fs::write(&packed, "committed Archive").unwrap();
            ArchiveFinalizationResult {
                attempts: vec![ArchiveFinalizationAttempt {
                    failure: Some(ArchiveFinalizationFailure::PluginCreationFailed),
                    safe_to_continue: false,
                    detail: "planned Loading Plugin occupied".to_owned(),
                    ..finalization_attempt(&packed, &packed_root, MutationState::Committed)
                }],
                safe_to_continue: false,
                ..ArchiveFinalizationResult::default()
            }
        })),
        ..ControlledWork::default()
    };
    let mut cleanup = CountingCleanup::default();
    let result = execute(
        &request(
            ExecutionMode::Apply,
            &root,
            &[RequestedWork::ArchiveCreation],
        ),
        &mut cleanup,
        Some(&*test_configuration()),
        Some(&work),
        &CancellationToken::new(),
    );

    assert_eq!(result.outcome(), RunOutcome::Failed);
    assert_eq!(result.final_phase(), RunPhase::ArchiveFinalization);
    let finalization = result.archive_finalization().unwrap();
    assert_eq!(finalization.attempts.len(), 1);
    assert_eq!(
        finalization.attempts[0].failure,
        Some(ArchiveFinalizationFailure::PluginCreationFailed)
    );
    assert_eq!(finalization.attempts[0].mutation, MutationState::Committed);
    assert_eq!(result.mutation_summaries().len(), 1);
    assert_eq!(result.mutation_summaries()[0].committed, 1);
    assert!(output.exists());
    assert!(source.exists());
}

/// Origin: RunExecutorTests::cancellationAfterAtomicAssetAttempt (the unsafe
/// row). Cancellation requested inside an attempt that leaves unknown
/// mutation still lets it finish, but the run is Failed, not Cancelled, and
/// the pending Asset is never touched.
#[test]
fn an_unsafe_attempt_cancelled_mid_flight_fails_and_leaves_pending_work_untouched() {
    let root = mod_root("unsafe-cancelled-attempt", &[]);
    std::fs::write(root.join("a.dds"), "original").unwrap();
    std::fs::write(root.join("b.dds"), "unattempted").unwrap();
    let token = CancellationToken::new();
    let work = ControlledWork {
        execute: Some(Box::new({
            let token = token.clone();
            move |asset, _| {
                token.cancel();
                std::fs::write(asset.execution_path(), "finished atomic attempt").unwrap();
                Ok(AssetExecutionResult::failed(
                    AssetExecutionFailure::CommitFailed,
                    "publication left the output unknown",
                )
                .with_mutation(MutationState::PartialOrUnknown)
                .with_safe_to_continue(false))
            }
        })),
        finalize: Some(Box::new(|| Ok(()))),
        stage_temporary: true,
        ..ControlledWork::default()
    };
    let mut cleanup = CountingCleanup::default();

    let result = execute(
        &request(
            ExecutionMode::Apply,
            &root,
            &[RequestedWork::NativeTextureOptimization],
        ),
        &mut cleanup,
        Some(&*test_configuration()),
        Some(&work),
        &token,
    );

    assert_eq!(result.outcome(), RunOutcome::Failed);
    assert!(result.cancellation_observed());
    assert_eq!(result.asset_attempts().len(), 1);
    assert_eq!(work.finalizations.load(Ordering::SeqCst), 0);
    assert_eq!(result.routing_ledger().unwrap().routed_assets().len(), 2);
    let attempt = &result.asset_attempts()[0];
    assert_eq!(attempt.mod_root, root);
    assert_eq!(attempt.asset.execution_path(), root.join("a.dds"));
    assert_eq!(
        attempt.result.mutation_state(),
        MutationState::PartialOrUnknown
    );
    assert!(!attempt.result.safe_to_continue());
    assert_eq!(
        result.phase(RunPhase::ProcessingAssets).unwrap().progress(),
        Some(RunProgress::determinate(2, 0, 1))
    );
    assert!(result.phase(RunPhase::ArchiveFinalization).is_none());
    assert_eq!(cleanup.passes, 1);
    assert_temporary_removed(&work);
    assert_eq!(read(&root.join("a.dds")), "finished atomic attempt");
    assert_eq!(read(&root.join("b.dds")), "unattempted");
}

/// Origin: RunExecutorTests::discoveryDiagnosticsSurviveInterruption, without
/// its Archive: a linked entry diagnosed by the Archive pass is retained when
/// the definitive discovery pass then fails, and no routing is invented.
#[test]
fn a_discovery_diagnostic_survives_a_failure_later_in_discovery() {
    let root = mod_root("diagnostic-survives-failure", &["textures/asset.dds"]);
    let outside = canonical(&scratch_dir("executor/diagnostic-survives-failure-outside"));
    let link = root.join("linked");
    common::junction(&link, &outside);
    let definitive = Arc::new(AtomicBool::new(false));
    let work = ControlledWork {
        report_phase: Some(Box::new({
            let definitive = definitive.clone();
            move |record| {
                if record.phase() == RunPhase::BuildingEffectiveAssetTree {
                    definitive.store(true, Ordering::SeqCst);
                }
            }
        })),
        is_cancelled: Some(Box::new(move || {
            if definitive.load(Ordering::SeqCst) {
                panic!("the definitive discovery pass failed");
            }
            false
        })),
        stage_temporary: true,
        ..ControlledWork::default()
    };
    let mut cleanup = CountingCleanup::default();

    let result = execute(
        &request(
            ExecutionMode::Apply,
            &root,
            &[RequestedWork::NativeTextureOptimization],
        ),
        &mut cleanup,
        Some(&*test_configuration()),
        Some(&work),
        &CancellationToken::new(),
    );
    std::fs::remove_dir(&link).unwrap();

    assert_eq!(result.outcome(), RunOutcome::Failed);
    assert_eq!(result.failures().len(), 1);
    assert_eq!(result.failures()[0].code, RunFailureCode::WorkServiceFailed);
    assert_eq!(result.diagnostics().len(), 1);
    assert_eq!(
        result.diagnostics()[0].code,
        RunDiagnosticCode::LinkedEntryExcluded
    );
    assert_eq!(result.diagnostics()[0].path, link);
    assert!(result.routing_ledger().is_none());
    assert!(result.phase(RunPhase::ArchiveFinalization).is_none());
    assert_eq!(cleanup.passes, 1);
    assert_temporary_removed(&work);
}

/// Origin: RunExecutorTests::workPreparationFailurePreservesStaleArtifacts.
/// Work configuration that fails during Preparing stops the run before
/// staging recovery: the crashed run's leftover is kept, work never starts,
/// and Safety Cleanup still runs once.
#[test]
fn a_failing_work_configuration_leaves_stale_staging_untouched() {
    let root = mod_root("work-preparation-failure", &["textures/a.dds"]);
    let leftover = common::crashed_run_leftover(&root, "textures/a.dds");
    let work = ControlledWork {
        prepare: Some(Box::new(|| {
            Err(Error::ConfigurationLoading(
                "FilesToNotPack.txt is unreadable".to_owned(),
            ))
        })),
        stage_temporary: true,
        ..ControlledWork::default()
    };
    let mut cleanup = CountingCleanup::default();

    let result = execute(
        &request(
            ExecutionMode::Apply,
            &root,
            &[RequestedWork::NativeTextureOptimization],
        ),
        &mut cleanup,
        Some(&*test_configuration()),
        Some(&work),
        &CancellationToken::new(),
    );

    assert_eq!(result.outcome(), RunOutcome::Failed);
    assert_eq!(result.final_phase(), RunPhase::Preparing);
    assert_eq!(
        result.failures()[0].code,
        RunFailureCode::ConfigurationLoadingFailed
    );
    assert!(work.staged_temporary().is_none(), "work never started");
    assert!(result.preparation().is_none());
    assert_eq!(read(&leftover), "partial output");
    assert_eq!(cleanup.passes, 1);
}

/// Origin: RunExecutorTests::workArtifactsShareRecoveryAndAreCleanedAfterFailure
/// (both rows). Stale staging is recovered before work, the run's own staging
/// shares the same area, the committed Asset survives, and the run's
/// temporaries are cleaned once whether or not the work later panics.
#[test]
fn recovered_and_run_owned_staging_are_cleaned_even_after_a_later_failure() {
    for fails in [false, true] {
        let row = if fails {
            "panic after commit"
        } else {
            "completed"
        };
        let root = mod_root(&format!("shared-recovery-{fails}"), &["texture.dds"]);
        let leftover = common::crashed_run_leftover(&root, "texture.dds");
        let committed = Arc::new(AtomicBool::new(false));
        let work = ControlledWork {
            execute: Some(Box::new({
                let committed = committed.clone();
                move |asset, _| {
                    std::fs::write(asset.execution_path(), "committed asset").unwrap();
                    committed.store(true, Ordering::SeqCst);
                    Ok(AssetExecutionResult::success(MutationState::Committed))
                }
            })),
            is_cancelled: Some(Box::new(move || {
                if fails && committed.load(Ordering::SeqCst) {
                    panic!("orchestration failed after the commit");
                }
                false
            })),
            finalize: Some(Box::new(|| Ok(()))),
            stage_temporary: true,
            ..ControlledWork::default()
        };
        let mut cleanup = CountingCleanup::default();

        let result = execute(
            &request(
                ExecutionMode::Apply,
                &root,
                &[RequestedWork::NativeTextureOptimization],
            ),
            &mut cleanup,
            Some(&*test_configuration()),
            Some(&work),
            &CancellationToken::new(),
        );

        if fails {
            assert_eq!(result.outcome(), RunOutcome::Failed, "{row}");
            assert_eq!(result.failures().len(), 1, "{row}");
            assert_eq!(result.failures()[0].code, RunFailureCode::WorkServiceFailed);
            assert!(
                result.failures()[0]
                    .detail
                    .contains("orchestration failed after the commit")
            );
        } else {
            assert_eq!(result.outcome(), RunOutcome::Succeeded, "{row}");
        }
        assert_eq!(result.asset_attempts().len(), 1, "{row}");
        assert_eq!(result.routing_ledger().unwrap().routed_assets().len(), 1);
        assert_eq!(
            result.phase(RunPhase::ProcessingAssets).unwrap().progress(),
            Some(RunProgress::determinate(1, 1, 0)),
            "{row}"
        );
        assert_eq!(
            work.finalizations.load(Ordering::SeqCst),
            usize::from(!fails),
            "{row}"
        );
        assert_eq!(
            result.phase(RunPhase::ArchiveFinalization).is_some(),
            !fails,
            "{row}"
        );
        assert_eq!(result.mutation_summaries().len(), 1, "{row}");
        assert_eq!(result.mutation_summaries()[0].committed, 1, "{row}");
        assert_temporary_removed(&work);
        assert!(!leftover.exists(), "{row}: the stale sibling was recovered");
        assert_eq!(read(&root.join("texture.dds")), "committed asset", "{row}");
        assert_eq!(cleanup.passes, 1, "{row}");
        assert!(result.cleanup_failures().is_empty(), "{row}");
        assert_eq!(
            result.phases().last().unwrap().phase(),
            RunPhase::SafetyCleanup
        );
    }
}

/// Origin: RunExecutorTests::fatalFailureRetainsConcurrentCancellation.
#[test]
fn a_fatal_preparing_failure_stays_failed_when_cleanup_cancels() {
    let token = CancellationToken::new();
    let mut cleanup = ScriptedCleanup {
        cancels: Some(token.clone()),
        failures: vec![RunFailure::new(
            RunFailureCode::TemporaryArtifactCleanupFailed,
            RunPhase::SafetyCleanup,
            "retained temporary artifact",
        )],
        ..ScriptedCleanup::default()
    };

    let result = execute_with(&no_work_request(), &mut cleanup, None, None, None, &token);

    assert_eq!(result.outcome(), RunOutcome::Failed);
    assert!(result.cancellation_observed());
    assert_eq!(result.failures().len(), 1);
    assert_eq!(
        result.failures()[0].code,
        RunFailureCode::ConfigurationLoadingFailed
    );
    assert_eq!(result.cleanup_failures().len(), 1);
    assert_eq!(
        result.cleanup_failures()[0].detail,
        "retained temporary artifact"
    );
}

/// Origin: RunExecutorTests::cleanupExceptionsPreserveCancellation. A
/// panicking cleanup service that also cancels is a secondary cleanup
/// failure, whether cancellation came before cleanup or during it.
#[test]
fn a_panicking_cleanup_service_never_replaces_cancellation() {
    for cancel_before in [false, true] {
        let token = CancellationToken::new();
        if cancel_before {
            token.cancel();
        }
        let mut cleanup = ScriptedCleanup {
            cancels: Some(token.clone()),
            panics: true,
            ..ScriptedCleanup::default()
        };

        let result = execute_with(
            &no_work_request(),
            &mut cleanup,
            None,
            Some(&*test_configuration()),
            None,
            &token,
        );

        assert_eq!(result.outcome(), RunOutcome::Cancelled, "{cancel_before}");
        assert!(result.cancellation_observed());
        assert_eq!(cleanup.passes, 1);
        assert!(result.failures().is_empty());
        assert_eq!(result.cleanup_failures().len(), 1);
        assert_eq!(
            result.cleanup_failures()[0].code,
            RunFailureCode::SafetyCleanupServiceFailed
        );
    }
}

/// Origin: RunExecutorTests::cleanupServiceExceptionsAreTerminal (the
/// non-standard exception, a panic here).
#[test]
fn a_panicking_cleanup_service_fails_the_run() {
    let mut cleanup = ScriptedCleanup {
        panics: true,
        ..ScriptedCleanup::default()
    };

    let result = execute_with(
        &no_work_request(),
        &mut cleanup,
        None,
        Some(&*test_configuration()),
        None,
        &CancellationToken::new(),
    );

    assert_eq!(result.outcome(), RunOutcome::Failed);
    assert_eq!(cleanup.passes, 1);
    assert!(result.failures().is_empty());
    assert_eq!(result.cleanup_failures().len(), 1);
    assert_eq!(
        result.cleanup_failures()[0].code,
        RunFailureCode::SafetyCleanupServiceFailed
    );
}

/// The work fact one row of the terminal precedence table establishes.
#[derive(Debug, Clone, Copy, PartialEq)]
enum WorkFact {
    Succeeded,
    ContainedFailure,
    UnsafeMutation,
    FatalFailure,
}

/// Origin: RunExecutorTests::terminalPrecedenceRetainsAllEvidence (all
/// sixteen cases). Unsafe work and fatal failures are Failed whatever else
/// happened; otherwise cancellation is Cancelled; otherwise a contained or
/// cleanup failure completes with failures. Every fact is retained.
#[test]
fn terminal_precedence_classifies_every_combination_and_keeps_every_fact() {
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
    let root = mod_root("terminal-precedence", &["textures/a.dds"]);
    for (fact, outcomes) in table {
        for (index, expected) in outcomes.into_iter().enumerate() {
            let (cleanup_fails, cancelled) = (index % 2 == 1, index >= 2);
            let case = format!("{fact:?}, cleanup failure {cleanup_fails}, cancelled {cancelled}");
            let token = CancellationToken::new();
            let work = ControlledWork {
                execute: Some(Box::new(move |_, _| {
                    Ok(match fact {
                        WorkFact::ContainedFailure => AssetExecutionResult::failed(
                            AssetExecutionFailure::LoadFailed,
                            "contained",
                        ),
                        WorkFact::UnsafeMutation => AssetExecutionResult::failed(
                            AssetExecutionFailure::CommitFailed,
                            "unknown",
                        )
                        .with_mutation(MutationState::PartialOrUnknown),
                        _ => AssetExecutionResult::success(MutationState::None),
                    })
                })),
                is_cancelled: (fact == WorkFact::FatalFailure).then(|| {
                    Box::new(|| -> bool { panic!("fatal work failure") }) as common::Hook<bool>
                }),
                ..ControlledWork::default()
            };
            let mut cleanup = ScriptedCleanup {
                cancels: cancelled.then(|| token.clone()),
                failures: if cleanup_fails {
                    vec![RunFailure::new(
                        RunFailureCode::TemporaryArtifactCleanupFailed,
                        RunPhase::SafetyCleanup,
                        "retained artifact",
                    )]
                } else {
                    Vec::new()
                },
                ..ScriptedCleanup::default()
            };

            let result = execute_with(
                &request(
                    ExecutionMode::DryRun,
                    &root,
                    &[RequestedWork::NativeTextureOptimization],
                ),
                &mut cleanup,
                None,
                Some(&*test_configuration()),
                Some(&work),
                &token,
            );

            assert_eq!(result.outcome(), expected, "{case}");
            assert_eq!(result.cancellation_observed(), cancelled, "{case}");
            assert_eq!(
                result.cleanup_failures().len(),
                usize::from(cleanup_fails),
                "{case}"
            );
            if cleanup_fails {
                assert_eq!(result.cleanup_failures()[0].detail, "retained artifact");
            }
            let attempted = matches!(fact, WorkFact::ContainedFailure | WorkFact::UnsafeMutation);
            if attempted {
                assert_eq!(result.asset_attempts().len(), 1, "{case}");
            }
            assert_eq!(
                result.failures().len(),
                usize::from(fact == WorkFact::FatalFailure),
                "{case}"
            );
            assert_eq!(
                result.mutation_summaries().len(),
                usize::from(fact == WorkFact::UnsafeMutation),
                "{case}"
            );
        }
    }
}

/// Origin: RunExecutorTests::recoveryFailureStillPerformsSafetyCleanup. A
/// leftover that recovery cannot delete stops the run in Preparing, naming
/// it, keeps its bytes, and Safety Cleanup still runs once.
#[test]
fn a_failed_staging_recovery_still_performs_safety_cleanup() {
    let root = mod_root("recovery-failure", &["textures/a.dds"]);
    let leftover = common::crashed_run_leftover(&root, "textures/a.dds");
    let mut permissions = std::fs::metadata(&leftover).unwrap().permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(&leftover, permissions.clone()).unwrap();
    let mut cleanup = CountingCleanup::default();

    let result = execute(
        &request(ExecutionMode::Apply, &root, &[]),
        &mut cleanup,
        Some(&*test_configuration()),
        None,
        &CancellationToken::new(),
    );
    // Restored before asserting, so a failing assertion cannot leave a
    // read-only file that blocks the next run from clearing its scratch.
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    std::fs::set_permissions(&leftover, permissions).unwrap();

    assert_eq!(result.outcome(), RunOutcome::Failed);
    assert_eq!(result.final_phase(), RunPhase::Preparing);
    assert_eq!(
        result.failures()[0].code,
        RunFailureCode::StagingRecoveryFailed
    );
    assert_eq!(result.failures()[0].path, leftover);
    assert_eq!(cleanup.passes, 1);
    assert_eq!(read(&leftover), "partial output");
}

/// Origin: RunExecutorTests::activeStagingBlocksUntilItsOwnerExits. Staging
/// another process owns blocks Apply and is untouched; once its owner is
/// gone, the next run recovers it.
#[test]
fn active_staging_blocks_apply_until_its_owner_exits() {
    let root = mod_root("active-until-exit", &["textures/a.dds"]);
    let leftover = common::crashed_run_leftover(&root, "textures/a.dds");
    // Taken first: the owner's lock admits no reader while it is held.
    let before = common::snapshot_tree(&root);
    let owner = cao_winfs::OwnerLock::open_existing(&root.join(".cao-staging/owner.lock")).unwrap();
    let request = request(ExecutionMode::Apply, &root, &[]);

    let blocked = execute(
        &request,
        &mut CountingCleanup::default(),
        Some(&*test_configuration()),
        None,
        &CancellationToken::new(),
    );
    assert_eq!(blocked.outcome(), RunOutcome::Failed);
    assert_eq!(blocked.failures()[0].code, RunFailureCode::StagingActive);
    drop(owner);
    assert_eq!(common::snapshot_tree(&root), before);

    let recovered = execute(
        &request,
        &mut CountingCleanup::default(),
        Some(&*test_configuration()),
        None,
        &CancellationToken::new(),
    );
    assert_eq!(
        recovered.outcome(),
        RunOutcome::Succeeded,
        "{:?}",
        recovered.failures()
    );
    assert!(!leftover.exists());
}

/// A Safety Cleanup Service that starts a second run over the same Mod Root
/// during its pass and records whether staging blocked it.
struct NestedRunCleanup {
    request: RunRequest,
    blocked: Option<bool>,
}

impl SafetyCleanupService for NestedRunCleanup {
    fn perform_safety_cleanup(&mut self) -> Result<Vec<RunFailure>, Error> {
        let nested = execute(
            &self.request,
            &mut CountingCleanup::default(),
            Some(&*test_configuration()),
            None,
            &CancellationToken::new(),
        );
        self.blocked = Some(
            nested
                .failures()
                .first()
                .is_some_and(|failure| failure.code == RunFailureCode::StagingActive),
        );
        Ok(Vec::new())
    }
}

/// Origin: RunExecutorTests::recoveryLockSurvivesThroughSafetyCleanup. The run
/// keeps the staging lock it took during recovery through its whole Safety
/// Cleanup pass, so a nested run is blocked; the next run succeeds.
#[test]
fn the_recovery_lock_is_held_through_safety_cleanup() {
    let root = mod_root("lock-through-cleanup", &["textures/a.dds"]);
    let _leftover = common::crashed_run_leftover(&root, "textures/a.dds");
    let request = request(ExecutionMode::Apply, &root, &[]);
    let mut cleanup = NestedRunCleanup {
        request: request.clone(),
        blocked: None,
    };

    let result = execute_with(
        &request,
        &mut cleanup,
        None,
        Some(&*test_configuration()),
        None,
        &CancellationToken::new(),
    );

    assert_eq!(
        result.outcome(),
        RunOutcome::Succeeded,
        "{:?}",
        result.failures()
    );
    assert_eq!(cleanup.blocked, Some(true));
    let next = execute(
        &request,
        &mut CountingCleanup::default(),
        Some(&*test_configuration()),
        None,
        &CancellationToken::new(),
    );
    assert_eq!(next.outcome(), RunOutcome::Succeeded);
}

/// Origin: RunExecutorTests::verifiedStaleStagingIsRecoveredBeforeWork. Valid
/// leftover staging is recovered during Preparing, before the first work
/// phase is published, and only the control files remain.
#[test]
fn verified_stale_staging_is_recovered_before_the_first_work_phase() {
    let root = mod_root("recovered-before-work", &["textures/a.dds"]);
    let leftover = common::crashed_run_leftover(&root, "textures/a.dds");
    let recovered_first = Arc::new(Mutex::new(None));
    let sink = RecordingSink::with_hook({
        let (leftover, recovered_first) = (leftover.clone(), recovered_first.clone());
        move |observed| {
            if let Observed::Phase(record) = observed
                && record.phase() == RunPhase::DiscoveringArchives
            {
                *recovered_first.lock().unwrap() = Some(!leftover.exists());
            }
        }
    });
    let mut cleanup = CountingCleanup::default();

    let result = execute_with(
        &request(ExecutionMode::Apply, &root, &[]),
        &mut cleanup,
        Some(&sink),
        Some(&*test_configuration()),
        None,
        &CancellationToken::new(),
    );

    assert_eq!(result.outcome(), RunOutcome::Succeeded);
    assert_eq!(*recovered_first.lock().unwrap(), Some(true));
    assert!(!leftover.exists());
    let staging = root.join(".cao-staging");
    assert!(staging.join("owner.lock").exists());
    assert!(staging.join("ownership.manifest").exists());
    assert_eq!(cleanup.passes, 1);
}

/// Origin: RunExecutorTests::unownedStagingBlocksApplyAndRemainsUntouched.
#[test]
fn an_unowned_staging_directory_blocks_apply_and_is_untouched() {
    let root = scratch_dir("executor/unowned-staging");
    let user = root.join(".cao-staging").join("user.dds");
    std::fs::create_dir_all(user.parent().unwrap()).unwrap();
    std::fs::write(&user, "retain me").unwrap();
    let mut cleanup = CountingCleanup::default();

    let result = execute(
        &request(ExecutionMode::Apply, &root, &[]),
        &mut cleanup,
        Some(&*test_configuration()),
        None,
        &CancellationToken::new(),
    );

    assert_eq!(result.outcome(), RunOutcome::Failed);
    assert_eq!(result.final_phase(), RunPhase::Preparing);
    assert_eq!(result.failures().len(), 1);
    assert_eq!(
        result.failures()[0].path,
        canonical(&root.join(".cao-staging"))
    );
    assert!(!result.failures()[0].detail.is_empty());
    assert_eq!(cleanup.passes, 1);
    assert_eq!(read(&user), "retain me");
}

/// Origin: RunExecutorTests::preparingRetainsTheResolvedRootAndPolicy.
/// Preparing loads the requested profile and keeps one canonical Mod Root,
/// the lowercased Archive extension, the execution mode and the ignored mods.
#[test]
fn preparing_retains_the_resolved_root_and_policy() {
    let root = scratch_dir("executor/resolved-root");
    let provider = CallbackConfiguration(Box::new(|identity| {
        if identity != "test-profile" {
            return Err(Error::ConfigurationLoading(format!(
                "unexpected profile {identity}"
            )));
        }
        Ok(RunConfiguration {
            profile: SelectedProfileFacts {
                archive_extension: Some(".BSA".to_owned()),
                ..common::sse_profile()
            },
            ignored_mods: vec!["ignored-mod".to_owned()],
            ..RunConfiguration::default()
        })
    }));
    let request = RunRequest::new(
        "test-profile",
        ExecutionMode::DryRun,
        ModSelection::SingleModRoot(root.join(".")),
        Vec::new(),
    );
    let mut cleanup = CountingCleanup::default();

    let result = execute(
        &request,
        &mut cleanup,
        Some(&provider),
        None,
        &CancellationToken::new(),
    );

    assert_eq!(cleanup.passes, 1);
    assert_eq!(
        result.outcome(),
        RunOutcome::Succeeded,
        "{:?}",
        result.failures()
    );
    let preparation = result.preparation().expect("Preparing completed");
    assert_eq!(preparation.mod_roots(), [canonical(&root)]);
    assert_eq!(preparation.policy().archive_extension(), ".bsa");
    assert_eq!(preparation.policy().execution_mode(), ExecutionMode::DryRun);
    assert_eq!(preparation.configuration().ignored_mods, ["ignored-mod"]);
}

/// Origin: RunExecutorTests::policyConflictsFailPreparing (the ambiguous
/// Archive extension).
#[test]
fn an_ambiguous_archive_extension_fails_preparing_before_any_work_phase() {
    let provider = CallbackConfiguration(Box::new(|_| {
        Ok(RunConfiguration {
            profile: SelectedProfileFacts {
                archive_extension: Some(".dds".to_owned()),
                ..common::sse_profile()
            },
            ..RunConfiguration::default()
        })
    }));
    let mut cleanup = CountingCleanup::default();

    let result = execute(
        &no_work_request(),
        &mut cleanup,
        Some(&provider),
        None,
        &CancellationToken::new(),
    );

    assert_eq!(result.outcome(), RunOutcome::Failed);
    assert_eq!(result.final_phase(), RunPhase::Preparing);
    let failure = &result.failures()[0];
    assert_eq!(failure.code, RunFailureCode::PolicyConflict);
    assert_eq!(failure.policy_conflicts.len(), 1);
    assert!(matches!(
        failure.policy_conflicts[0],
        PolicyValidationError::AmbiguousArchiveExtension { .. }
    ));
    assert!(result.preparation().is_none());
    let phases: Vec<_> = result.phases().iter().map(RunPhaseRecord::phase).collect();
    assert_eq!(phases, [RunPhase::Preparing, RunPhase::SafetyCleanup]);
    assert_eq!(cleanup.passes, 1);
}

/// Origin: RunExecutorTests::archivePrecedenceIntentIsRetained. Explicit
/// Archive Precedence is owned by the request, so a caller changing its own
/// list afterwards cannot reach it, and it survives into the preparation.
#[test]
fn explicit_archive_precedence_is_retained_through_preparing() {
    let mut order = vec![PathBuf::from("winner.bsa"), PathBuf::from("shadowed.bsa")];
    let request =
        no_work_request().with_archive_precedence(ArchivePrecedence::ExplicitOrder(order.clone()));
    order.clear();
    let mut cleanup = CountingCleanup::default();

    let result = execute(
        &request,
        &mut cleanup,
        Some(&*test_configuration()),
        None,
        &CancellationToken::new(),
    );

    assert_eq!(result.outcome(), RunOutcome::Succeeded);
    assert_eq!(
        result.preparation().unwrap().archive_precedence(),
        &ArchivePrecedence::ExplicitOrder(vec!["winner.bsa".into(), "shadowed.bsa".into()])
    );
    assert_eq!(
        no_work_request().archive_precedence(),
        &ArchivePrecedence::DeterministicDiscovery
    );
}

/// Origin: RunExecutorTests::preparingDoesNotMutateAssetsOrArchives (both
/// modes). Requested work without a work service fails Preparing, and the
/// Assets, Archives and directory listing are untouched, mtimes included.
#[test]
fn a_failed_preparing_mutates_no_asset_or_archive() {
    for mode in [ExecutionMode::Apply, ExecutionMode::DryRun] {
        let root = mod_root(&format!("preparing-mutates-nothing-{mode:?}"), &[]);
        for name in ["asset.dds", "source.bsa"] {
            std::fs::write(root.join(name), "untouched sentinel").unwrap();
        }
        let modified = |name: &str| {
            std::fs::metadata(root.join(name))
                .unwrap()
                .modified()
                .unwrap()
        };
        let before = (modified("asset.dds"), modified("source.bsa"));
        let mut cleanup = CountingCleanup::default();

        let result = execute(
            &request(
                mode,
                &root,
                &[
                    RequestedWork::NativeTextureOptimization,
                    RequestedWork::ArchiveExtraction,
                ],
            ),
            &mut cleanup,
            Some(&*test_configuration()),
            None,
            &CancellationToken::new(),
        );

        assert!(result.preparation().is_none(), "{mode:?}");
        assert_eq!(
            result.failures()[0].code,
            RunFailureCode::RequestedWorkUnavailable
        );
        assert_eq!((modified("asset.dds"), modified("source.bsa")), before);
        let mut names: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        names.sort();
        assert_eq!(names, ["asset.dds", "source.bsa"], "{mode:?}");
        for name in ["asset.dds", "source.bsa"] {
            assert_eq!(read(&root.join(name)), "untouched sentinel");
        }
    }
}

/// Origin: RunExecutorTests::aRunThatStopsEarlyRecordsOnlyThePhasesItTraversed.
#[test]
fn a_run_stopped_in_preparing_records_only_the_phases_it_traversed() {
    let mut cleanup = CountingCleanup::default();
    let result = execute(
        &request(
            ExecutionMode::Apply,
            &common::test_mod_root(),
            &[RequestedWork::ArchiveExtraction],
        ),
        &mut cleanup,
        Some(&*test_configuration()),
        None,
        &CancellationToken::new(),
    );

    assert_eq!(result.final_phase(), RunPhase::Preparing);
    let phases: Vec<_> = result.phases().iter().map(RunPhaseRecord::phase).collect();
    assert_eq!(phases, [RunPhase::Preparing, RunPhase::SafetyCleanup]);
}

/// Origin: RunExecutorTests::determinateProgressStartsAtZeroAgainstAnImmutableTotal
/// and failedAttemptsAdvanceCompletedProgress.
#[test]
fn determinate_progress_starts_at_zero_and_failures_advance_it() {
    let fresh = RunProgress::determinate(7, 0, 0);
    assert_eq!(
        (
            fresh.total(),
            fresh.completed(),
            fresh.succeeded(),
            fresh.failed()
        ),
        (7, 0, 0, 0)
    );
    let advanced = RunProgress::determinate(7, 2, 3);
    assert_eq!(
        (
            advanced.total(),
            advanced.succeeded(),
            advanced.failed(),
            advanced.completed()
        ),
        (7, 2, 3, 5)
    );
}

/// Origin: RunExecutorTests::registeredArtifactsAreCleanedAndCommittedOutputSurvives.
/// The run's cleanup service removes what was registered and not committed,
/// keeps the committed output, and a consumed registration cannot be
/// committed afterwards.
#[test]
fn registered_artifacts_are_cleaned_and_committed_output_survives() {
    let root = mod_root("registry-commit", &[]);
    let mut registry = TemporaryArtifactRegistry::new(create_run_id());
    let temporary = registry
        .register_artifact(&root.join("temporary.bin"))
        .unwrap();
    let output = registry
        .register_artifact(&root.join("output.bin"))
        .unwrap();
    assert!(
        !root.join("temporary.bin").exists(),
        "registering creates nothing"
    );
    std::fs::write(root.join("temporary.bin"), "temporary").unwrap();
    std::fs::write(root.join("output.bin"), "output").unwrap();
    registry.commit(output).unwrap();

    let result = execute_with(
        &no_work_request(),
        &mut registry,
        None,
        Some(&*test_configuration()),
        None,
        &CancellationToken::new(),
    );

    assert_eq!(result.outcome(), RunOutcome::Succeeded);
    assert!(!root.join("temporary.bin").exists());
    assert_eq!(read(&root.join("output.bin")), "output");
    assert!(registry.commit(temporary).is_err());
}

/// Origin: RunExecutorTests::cleanupFailuresAreAggregatedWithoutDeletingRetainedMaterial.
/// Every registered artifact is attempted; a registered directory still
/// holding committed or unregistered files is kept and reported, in reverse
/// registration order, and nothing retained is deleted.
#[test]
fn cleanup_failures_are_aggregated_without_deleting_retained_material() {
    let root = mod_root("registry-aggregated", &[]);
    let mut registry = TemporaryArtifactRegistry::new(create_run_id());
    for directory in ["temporary", "staging", "evidence"] {
        registry.register_artifact(&root.join(directory)).unwrap();
        std::fs::create_dir(root.join(directory)).unwrap();
    }
    let committed = registry
        .register_artifact(&root.join("staging/committed.bin"))
        .unwrap();
    std::fs::write(root.join("staging/committed.bin"), "committed").unwrap();
    registry.commit(committed).unwrap();
    std::fs::write(root.join("staging/backup.bsa"), "backup").unwrap();
    std::fs::write(root.join("evidence/failed.bin"), "failed output").unwrap();

    let result = execute_with(
        &no_work_request(),
        &mut registry,
        None,
        Some(&*test_configuration()),
        None,
        &CancellationToken::new(),
    );

    assert_eq!(result.outcome(), RunOutcome::CompletedWithFailures);
    let failures = result.cleanup_failures();
    let paths: Vec<_> = failures
        .iter()
        .map(|failure| failure.path.clone())
        .collect();
    assert_eq!(paths, [root.join("evidence"), root.join("staging")]);
    for failure in failures {
        assert_eq!(failure.code, RunFailureCode::TemporaryArtifactCleanupFailed);
        assert_eq!(failure.phase, RunPhase::SafetyCleanup);
        assert!(!failure.detail.is_empty());
    }
    assert!(result.failures().is_empty());
    assert!(!root.join("temporary").exists());
    assert_eq!(read(&root.join("staging/committed.bin")), "committed");
    assert_eq!(read(&root.join("staging/backup.bsa")), "backup");
    assert_eq!(read(&root.join("evidence/failed.bin")), "failed output");
}

/// Origin: RunExecutorTests::registeredArtifactsAreCleanedOnEveryTerminalPath.
/// Registered nested directories, and a registered file never created, are
/// cleaned in reverse order on every terminal path; the consumed registry
/// then cleans nothing more and accepts no registration.
#[test]
fn registered_artifacts_are_cleaned_on_every_terminal_path() {
    for expected in [
        RunOutcome::Succeeded,
        RunOutcome::Cancelled,
        RunOutcome::Failed,
    ] {
        let root = mod_root(&format!("registry-terminal-{expected:?}"), &[]);
        let mut registry = TemporaryArtifactRegistry::new(create_run_id());
        let staging = root.join("staging");
        let child = staging.join("child");
        registry.register_artifact(&staging).unwrap();
        registry.register_artifact(&child).unwrap();
        registry
            .register_artifact(&child.join("never-created.bin"))
            .unwrap();
        std::fs::create_dir_all(&child).unwrap();
        let token = CancellationToken::new();
        if expected == RunOutcome::Cancelled {
            token.cancel();
        }
        let configuration = test_configuration();
        let provider: Option<&dyn RunConfigurationProvider> =
            (expected != RunOutcome::Failed).then_some(&*configuration);

        let result = execute_with(
            &no_work_request(),
            &mut registry,
            None,
            provider,
            None,
            &token,
        );

        assert_eq!(result.outcome(), expected);
        assert!(result.cleanup_failures().is_empty(), "{expected:?}");
        assert!(!staging.exists(), "{expected:?}");
        let cleanups = result
            .phases()
            .iter()
            .filter(|record| record.phase() == RunPhase::SafetyCleanup)
            .count();
        assert_eq!(cleanups, 1, "{expected:?}");
        std::fs::create_dir_all(&child).unwrap();
        assert!(registry.cleanup().is_empty());
        assert!(child.exists(), "a consumed registry deletes nothing more");
        assert!(registry.register_artifact(&root.join("late.bin")).is_err());
    }
}

/// Origin: RunExecutorTests::cleanupFailuresPreserveThePrimaryOutcome (both
/// paths). A cleanup failure stays secondary to cancellation requested when
/// Safety Cleanup is published, and to a fatal Preparing failure, which alone
/// is published as a Run Failure.
#[test]
fn a_cleanup_failure_never_replaces_the_primary_outcome() {
    for fatal in [false, true] {
        let root = mod_root(&format!("cleanup-secondary-{fatal}"), &[]);
        let mut registry = TemporaryArtifactRegistry::new(create_run_id());
        registry.register_artifact(&root.join("retained")).unwrap();
        std::fs::create_dir(root.join("retained")).unwrap();
        std::fs::write(root.join("retained/unregistered"), "keep").unwrap();
        let token = CancellationToken::new();
        let sink = RecordingSink::with_hook({
            let token = token.clone();
            move |observed| {
                if let Observed::Phase(record) = observed
                    && record.phase() == RunPhase::SafetyCleanup
                {
                    token.cancel();
                }
            }
        });
        let configuration = test_configuration();
        let provider: Option<&dyn RunConfigurationProvider> = (!fatal).then_some(&*configuration);

        let result = execute_with(
            &no_work_request(),
            &mut registry,
            Some(&sink),
            provider,
            None,
            &token,
        );

        let expected = if fatal {
            RunOutcome::Failed
        } else {
            RunOutcome::Cancelled
        };
        assert_eq!(result.outcome(), expected);
        assert_eq!(result.cleanup_failures().len(), 1, "{fatal}");
        assert_eq!(result.failures().len(), usize::from(fatal));
        let published: Vec<_> = sink
            .observed()
            .into_iter()
            .filter_map(|observed| match observed {
                Observed::Failure(failure) => Some(failure.phase),
                _ => None,
            })
            .collect();
        let expected: &[RunPhase] = if fatal { &[RunPhase::Preparing] } else { &[] };
        assert_eq!(published, expected, "cleanup failures are never published");
        assert_eq!(read(&root.join("retained/unregistered")), "keep");
    }
}

/// Origin: RunExecutorTests::artifactRegistrationRejectsUnownedPaths. The
/// registry refuses every path that could transfer cleanup ownership of
/// something it does not own, and committed output survives the run.
#[test]
fn artifact_registration_rejects_paths_it_cannot_own() {
    let root = mod_root("registry-rejections", &[]);
    let mut registry = TemporaryArtifactRegistry::new(create_run_id());
    let mut other = TemporaryArtifactRegistry::new(create_run_id());
    let output = root.join("Output.bin");
    let registration = registry.register_artifact(&output).unwrap();
    std::fs::write(&output, "committed").unwrap();

    for (case, path) in [
        ("the root itself", root.clone()),
        ("a relative path", PathBuf::from("relative.bin")),
        ("a lexical duplicate", root.join(".").join("Output.bin")),
        ("a case alias", root.join("output.bin")),
        ("a trailing dot", root.join("trailing.")),
        ("a trailing space", root.join("trailing ")),
        ("an alternate data stream", root.join("stream.bin:stream")),
    ] {
        assert!(registry.register_artifact(&path).is_err(), "{case}");
    }
    assert!(other.commit(registration).is_err(), "a foreign commit");
    registry.commit(registration).unwrap();
    assert!(registry.commit(registration).is_err(), "a second commit");
    assert!(
        registry.register_artifact(&output).is_err(),
        "a committed path"
    );

    let result = execute_with(
        &no_work_request(),
        &mut registry,
        None,
        Some(&*test_configuration()),
        None,
        &CancellationToken::new(),
    );

    assert_eq!(result.outcome(), RunOutcome::Succeeded);
    assert_eq!(read(&output), "committed");
}

/// Origin: RunExecutorTests::cleanupDoesNotFollowAReplacedParent. A registered
/// directory replaced by a link to unrelated material is never followed:
/// the target's files survive, and the run reports the cleanup failure. A
/// junction stands in for the C++ directory symlink.
#[test]
fn cleanup_does_not_follow_a_replaced_parent() {
    let root = mod_root("registry-replaced-parent", &["unrelated/asset.bin"]);
    std::fs::write(root.join("unrelated/asset.bin"), "original").unwrap();
    let staging = root.join("staging");
    let mut registry = TemporaryArtifactRegistry::new(create_run_id());
    registry.register_artifact(&staging).unwrap();
    registry
        .register_artifact(&staging.join("asset.bin"))
        .unwrap();
    common::junction(&staging, &root.join("unrelated"));

    let result = execute_with(
        &no_work_request(),
        &mut registry,
        None,
        Some(&*test_configuration()),
        None,
        &CancellationToken::new(),
    );

    assert_eq!(read(&root.join("unrelated/asset.bin")), "original");
    assert_eq!(result.outcome(), RunOutcome::CompletedWithFailures);
    // Only the child is refused: its parent is no longer the one registered.
    assert_eq!(
        result.cleanup_failures().len(),
        1,
        "{:?}",
        result.cleanup_failures()
    );
    assert_eq!(result.cleanup_failures()[0].path, staging.join("asset.bin"));
    assert!(!staging.exists(), "the link itself is removed");
}

/// Spec (#493): cancellation at every work milestone. Cancelling as each
/// milestone is reported stops the run at that phase: no later phase is
/// recorded, routing exists only once its pass completed, no Asset is
/// attempted after the milestone, Archive Finalization never runs, and
/// Safety Cleanup still removes what the run staged.
#[test]
fn cancelling_at_each_work_milestone_stops_at_that_phase() {
    let milestones = [
        RunPhase::DiscoveringArchives,
        RunPhase::ExtractingArchives,
        RunPhase::BuildingEffectiveAssetTree,
        RunPhase::ProcessingAssets,
        RunPhase::ArchiveFinalization,
    ];
    for milestone in milestones {
        let root = mod_root(&format!("milestone-{milestone:?}"), &["textures/a.dds"]);
        let token = CancellationToken::new();
        let work = ControlledWork {
            report_phase: Some(Box::new({
                let token = token.clone();
                move |record| {
                    if record.phase() == milestone {
                        token.cancel();
                    }
                }
            })),
            finalize: Some(Box::new(|| Ok(()))),
            stage_temporary: true,
            ..ControlledWork::default()
        };
        let mut cleanup = CountingCleanup::default();

        let result = execute(
            &request(
                ExecutionMode::Apply,
                &root,
                &[RequestedWork::NativeTextureOptimization],
            ),
            &mut cleanup,
            Some(&*test_configuration()),
            Some(&work),
            &token,
        );

        assert_eq!(result.outcome(), RunOutcome::Cancelled, "{milestone:?}");
        assert!(result.cancellation_observed(), "{milestone:?}");
        assert_eq!(result.final_phase(), milestone, "{milestone:?}");
        let last_work_phase = result.phases()[result.phases().len() - 2].phase();
        assert_eq!(last_work_phase, milestone, "{milestone:?}: nothing later");
        assert_eq!(
            result.routing_ledger().is_some(),
            milestone >= RunPhase::ProcessingAssets,
            "{milestone:?}"
        );
        // Only Archive Finalization follows the Asset attempts.
        assert_eq!(
            work.executions.load(Ordering::SeqCst),
            usize::from(milestone == RunPhase::ArchiveFinalization),
            "{milestone:?}"
        );
        assert_eq!(
            work.finalizations.load(Ordering::SeqCst),
            0,
            "{milestone:?}"
        );
        assert!(result.failures().is_empty(), "{milestone:?}");
        assert_eq!(cleanup.passes, 1, "{milestone:?}");
        assert_temporary_removed(&work);
        assert_eq!(read(&root.join("textures/a.dds")), "textures/a.dds");
    }
}
