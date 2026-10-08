//! Run Executor scenarios: phase traversal, Preparing failures, Safety Cleanup
//! and terminal classification at the synchronous seam beneath the service.
//!
//! Each scenario names the C++ test whose intent it ports.

mod common;

use std::panic::{AssertUnwindSafe, catch_unwind};

use cao_core::Error;
use cao_core::routing::{ExecutionMode, PolicyValidationError, RequestedWork};
use cao_core::run::{
    CancellationToken, ModSelection, PhaseSkipReason, RunConfiguration, RunConfigurationProvider,
    RunEvidenceInvariantPanic, RunExecutor, RunFailure, RunFailureCode, RunOutcome, RunPhase,
    RunPhaseStatus, RunPreparation, RunRequest, RunServices, RunWorkEvidence, RunWorkMilestones,
    RunWorkService, SelectedProfileFacts, create_run_id,
};
use common::{
    CallbackConfiguration, CountingCleanup, ScriptedWork, no_work_request, request, scratch_dir,
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

/// Origin: RunExecutorTests::noWorkApplyRunTraversesTheStablePhaseSequence and
/// noWorkRunReportsTheSameReasonsInEveryExecutionMode.
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
        assert_eq!(result.phases()[6].status(), RunPhaseStatus::Executed);
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

/// Origin: RunExecutorTests::filesystemRootSelectionFailsPreparing.
#[test]
fn a_filesystem_root_cannot_be_a_mod_root() {
    let drive = std::path::PathBuf::from(format!("{}\\", &env!("CARGO_MANIFEST_DIR")[..2]));
    let request = RunRequest::new(
        "SkyrimSE",
        ExecutionMode::DryRun,
        ModSelection::SingleModRoot(drive),
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
    let mut cleanup = CountingCleanup::default();
    let result = RunExecutor.scheduling_failure(
        "no thread",
        &mut cleanup,
        None,
        &CancellationToken::new(),
        create_run_id(),
    );

    assert_eq!(result.outcome(), RunOutcome::Failed);
    assert_eq!(result.final_phase(), RunPhase::Preparing);
    assert_eq!(result.phases().len(), 1);
    assert_eq!(result.phases()[0].phase(), RunPhase::SafetyCleanup);
    assert_eq!(result.failures()[0].code, RunFailureCode::SchedulingFailed);
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
