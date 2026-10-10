//! The Run Executor: the synchronous, deterministic traversal of one run's phases.
//!
//! Ported from `src/Run/RunExecutor.h`. The executor owns phase sequencing,
//! phase applicability, Safety Cleanup and terminal classification. It
//! schedules nothing and dispatches nothing; the Optimization Run Service and
//! its Run Worker sit above it.
//!
//! C++ caught exceptions at three boundaries here; they are `Result`s and
//! `catch_unwind` now. A panic in the configuration provider, the work service
//! or the cleanup service becomes the same Run Failure an exception did. A
//! panic that escapes those boundaries is caught around the whole traversal,
//! so it still fails the run before Safety Cleanup rather than skipping it. A Run
//! Evidence invariant violation is a bug: the executor finishes Safety Cleanup
//! and then panics, and the Run Worker turns that panic into a Failed outcome.

use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::Error;
use crate::execution::MutationState;
use crate::routing::{ExecutionMode, RoutingPolicyRequest};
use crate::run::mod_selection::resolve_mod_roots;
use crate::run::{
    CancellationToken, MutableRunEvidence, OptimizationRunResult, PhaseSkipReason,
    RunConfiguration, RunConfigurationProvider, RunEvidence, RunFailure, RunFailureCode, RunId,
    RunObservationSink, RunOutcome, RunPhase, RunPhaseRecord, RunPreparation, RunRequest,
    RunWorkEvidence, TemporaryArtifactRegistry, take_panic_message,
};

/// The typed work milestones through which a Run Work Service moves the lifecycle.
///
/// The executor alone selects Run Phases; work reports where it has got to.
/// Each method returns the accepted current phase for optional presentation.
pub trait RunWorkMilestones {
    /// Enters Archive discovery, before any collision or extraction facts.
    fn archive_discovery_started(&self) -> Result<RunPhaseRecord, Error>;
    /// Starts Archive extraction with its immutable attempt total.
    fn archive_extraction_planned(&self, total: usize) -> Result<RunPhaseRecord, Error>;
    /// Records that Dry Run excluded Archive extraction.
    fn dry_run_archive_extraction(&self) -> Result<RunPhaseRecord, Error>;
    /// Enters definitive Effective Asset Tree discovery after Archive work.
    fn effective_asset_tree_started(&self) -> Result<RunPhaseRecord, Error>;
    /// Starts Asset processing with the retained Routing Ledger's routed total.
    fn asset_processing_planned(&self, total: usize) -> Result<RunPhaseRecord, Error>;
    /// Selects the final work phase from the execution mode and whether a
    /// finalizer exists. A mode that differs from the request's is an invariant
    /// violation.
    fn archive_finalization_available(
        &self,
        mode: ExecutionMode,
        has_finalizer: bool,
    ) -> Result<RunPhaseRecord, Error>;
}

/// Performs a run's requested work, recording owned evidence as it goes.
///
/// The executor retains every completed fact if the work later fails, and
/// always owns Safety Cleanup: work never cleans up after itself.
pub trait RunWorkService: Send + Sync {
    /// Loads work-specific configuration during Preparing, before any mutation.
    /// An error fails Preparing with `ConfigurationLoadingFailed`.
    fn prepare(&self) -> Result<(), Error> {
        Ok(())
    }

    /// Performs the work, submitting facts through `evidence` and moving the
    /// lifecycle through `milestones`, and checking `stop` between atomic
    /// attempts. An error, or a panic, becomes a fatal `WorkServiceFailed` Run
    /// Failure without discarding earlier evidence.
    ///
    /// Every temporary artifact goes through `artifacts`, the run's Temporary
    /// Ownership scope. The executor owns it and performs its Safety Cleanup;
    /// work never cleans the registry itself.
    fn execute(
        &self,
        preparation: &RunPreparation,
        evidence: &RunWorkEvidence<'_, '_>,
        artifacts: &mut TemporaryArtifactRegistry,
        milestones: &dyn RunWorkMilestones,
        stop: &CancellationToken,
    ) -> Result<(), Error>;
}

/// Removes the temporary artifacts one Optimization Run registered.
///
/// It never rolls back Committed Mutations and never removes backups or
/// failed-output evidence. The executor invokes it exactly once on every
/// terminal path, after the last work phase and before the terminal result,
/// and it cannot be cancelled.
pub trait SafetyCleanupService: Send {
    /// Attempts every remaining artifact, returning per-artifact failures in
    /// cleanup order. An error means the service itself failed.
    fn perform_safety_cleanup(&mut self) -> Result<Vec<RunFailure>, Error>;
}

/// Runs one mandatory cleanup pass, converting a failed or panicking service into a failure.
pub fn collect_safety_cleanup_failures(service: &mut dyn SafetyCleanupService) -> Vec<RunFailure> {
    let service_failure = |detail: String| {
        vec![RunFailure::new(
            RunFailureCode::SafetyCleanupServiceFailed,
            RunPhase::SafetyCleanup,
            detail,
        )]
    };
    match catch_unwind(AssertUnwindSafe(|| service.perform_safety_cleanup())) {
        Ok(Ok(failures)) => failures,
        Ok(Err(error)) => service_failure(error.to_string()),
        Err(payload) => service_failure(format!(
            "The cleanup service panicked: {}",
            take_panic_message(payload)
        )),
    }
}

/// The services the Run Executor borrows for one synchronous run.
pub struct RunServices<'a> {
    /// Mandatory: every terminal path owes exactly one cleanup pass, even over
    /// an empty set, so an absent service would hide whether it happened. It
    /// runs after the executor's own Temporary Ownership registry.
    pub safety_cleanup: &'a mut dyn SafetyCleanupService,
    pub observations: Option<&'a dyn RunObservationSink>,
    /// A missing provider fails Preparing, including for requests with no work.
    pub configuration: Option<&'a dyn RunConfigurationProvider>,
    /// Requested work fails Preparing with `RequestedWorkUnavailable` without one.
    pub work: Option<&'a dyn RunWorkService>,
}

/// Translates work milestones into the executor's lifecycle account.
struct ExecutorMilestones<'e, 'a> {
    evidence: &'e RefCell<MutableRunEvidence<'a>>,
    mode: ExecutionMode,
}

impl ExecutorMilestones<'_, '_> {
    fn current(&self) -> Result<RunPhaseRecord, Error> {
        self.evidence
            .borrow()
            .current_phase()
            .copied()
            .ok_or(Error::EvidenceInvariant(
                "A milestone was accepted without a current phase",
            ))
    }
}

impl RunWorkMilestones for ExecutorMilestones<'_, '_> {
    fn archive_discovery_started(&self) -> Result<RunPhaseRecord, Error> {
        self.evidence
            .borrow_mut()
            .record_archive_discovery_started()?;
        self.current()
    }

    fn archive_extraction_planned(&self, total: usize) -> Result<RunPhaseRecord, Error> {
        self.evidence
            .borrow_mut()
            .record_archive_extraction_plan(total)?;
        self.current()
    }

    fn dry_run_archive_extraction(&self) -> Result<RunPhaseRecord, Error> {
        self.evidence
            .borrow_mut()
            .record_dry_run_archive_extraction()?;
        self.current()
    }

    fn effective_asset_tree_started(&self) -> Result<RunPhaseRecord, Error> {
        self.evidence
            .borrow_mut()
            .record_effective_asset_tree_started()?;
        self.current()
    }

    fn asset_processing_planned(&self, total: usize) -> Result<RunPhaseRecord, Error> {
        self.evidence
            .borrow_mut()
            .record_asset_processing_plan(total)?;
        self.current()
    }

    fn archive_finalization_available(
        &self,
        mode: ExecutionMode,
        has_finalizer: bool,
    ) -> Result<RunPhaseRecord, Error> {
        if mode != self.mode {
            return Err(Error::EvidenceInvariant(
                "Work mode differs from the Run Request",
            ));
        }
        let record = match (self.mode, has_finalizer) {
            (ExecutionMode::DryRun, _) => {
                RunPhaseRecord::skipped(RunPhase::ArchiveFinalization, PhaseSkipReason::DryRun)
            }
            (ExecutionMode::Apply, true) => {
                RunPhaseRecord::executed(RunPhase::ArchiveFinalization, None)
            }
            (ExecutionMode::Apply, false) => RunPhaseRecord::skipped(
                RunPhase::ArchiveFinalization,
                PhaseSkipReason::NoRequestedWork,
            ),
        };
        self.evidence.borrow_mut().record_phase(record)?;
        Ok(record)
    }
}

/// Loads configuration, converting a missing, failing or panicking provider into a failure.
fn load_configuration(
    request: &RunRequest,
    provider: Option<&dyn RunConfigurationProvider>,
) -> Result<RunConfiguration, RunFailure> {
    let failure = |detail: String| {
        RunFailure::new(
            RunFailureCode::ConfigurationLoadingFailed,
            RunPhase::Preparing,
            detail,
        )
    };
    let provider =
        provider.ok_or_else(|| failure("No run configuration provider is available".to_owned()))?;
    match catch_unwind(AssertUnwindSafe(|| {
        provider.load(request.profile_identity())
    })) {
        Ok(Ok(configuration)) => Ok(configuration),
        Ok(Err(error)) => Err(failure(error.to_string())),
        Err(payload) => Err(failure(format!(
            "The configuration provider panicked: {}",
            take_panic_message(payload)
        ))),
    }
}

/// Prepares immutable facts without mutation; `Ok(None)` means cancellation was observed.
///
/// Mod Exclusions found while resolving the Mod Selection are recorded in
/// `evidence` and published as they are found.
fn prepare_run(
    request: &RunRequest,
    provider: Option<&dyn RunConfigurationProvider>,
    evidence: &RefCell<MutableRunEvidence<'_>>,
    stop: &CancellationToken,
) -> Result<Option<RunPreparation>, RunFailure> {
    let configuration = load_configuration(request, provider)?;
    // A provider may finish an atomic read after cancellation; stop before
    // compiling or resolving anything more.
    if stop.is_cancelled() {
        return Ok(None);
    }
    let policy = configuration
        .profile
        .compile_policy(RoutingPolicyRequest::for_work(
            request.execution_mode(),
            request.requested_work(),
        ))
        .map_err(|conflicts| {
            RunFailure::new(
                RunFailureCode::PolicyConflict,
                RunPhase::Preparing,
                "The loaded profile conflicts with the requested Routing Policy",
            )
            .with_policy_conflicts(conflicts)
        })?;
    let mod_roots = resolve_mod_roots(request.mod_selection(), &configuration, evidence, stop)?;
    if stop.is_cancelled() {
        return Ok(None);
    }
    Ok(Some(RunPreparation::new(
        mod_roots,
        configuration,
        policy,
        request.archive_precedence().clone(),
    )))
}

/// Classifies sealed facts into the one Run Outcome, after every producer has stopped.
///
/// Unsafe work wins over cancellation; cancellation wins over cleanup errors;
/// a failed cleanup service is fatal; any contained failure completes with failures.
fn classify_terminal_outcome(evidence: &RunEvidence) -> RunOutcome {
    let mut unsafe_work = !evidence.failures().is_empty();
    let mut contained_failure = false;
    let attempts = evidence
        .asset_attempts()
        .iter()
        .map(|attempt| {
            let result = &attempt.result;
            (
                result.mutation_state(),
                result.succeeded(),
                result.safe_to_continue(),
            )
        })
        .chain(
            evidence
                .archive_extraction_attempts()
                .iter()
                .map(|attempt| {
                    (
                        attempt.mutation,
                        attempt.succeeded(),
                        attempt.safe_to_continue,
                    )
                }),
        );
    let finalization = evidence.archive_finalization();
    if let Some(finalization) = finalization {
        unsafe_work |= !finalization.safe_to_continue;
        contained_failure |= finalization.failure.is_some();
    }
    let attempts = attempts.chain(
        finalization
            .into_iter()
            .flat_map(|finalization| &finalization.attempts)
            .map(|attempt| {
                (
                    attempt.mutation,
                    attempt.succeeded(),
                    attempt.safe_to_continue,
                )
            }),
    );
    for (mutation, succeeded, safe_to_continue) in attempts {
        unsafe_work |= !safe_to_continue || mutation == MutationState::PartialOrUnknown;
        contained_failure |= !succeeded;
    }
    if unsafe_work {
        return RunOutcome::Failed;
    }
    if evidence.cancellation_observed() {
        return RunOutcome::Cancelled;
    }
    let cleanup = evidence.cleanup_failures();
    if cleanup
        .iter()
        .any(|failure| failure.code == RunFailureCode::SafetyCleanupServiceFailed)
    {
        return RunOutcome::Failed;
    }
    if contained_failure || !cleanup.is_empty() {
        return RunOutcome::CompletedWithFailures;
    }
    RunOutcome::Succeeded
}

/// Remembers the first Run Evidence invariant violation, so the executor can
/// finish Safety Cleanup before panicking with it.
#[derive(Default)]
struct DeferredInvariant(Option<Error>);

impl DeferredInvariant {
    fn note(&mut self, result: Result<(), Error>) {
        if let Err(error) = result {
            self.0.get_or_insert(error);
        }
    }

    /// The first violation noted, if any.
    fn into_violation(self) -> Option<Error> {
        self.0
    }
}

/// The panic payload of a run whose Run Evidence rejected a fact.
///
/// The violation is a bug, so the executor panics, but only after Safety
/// Cleanup, and it carries the run's sealed result so the Run Worker can still
/// commit it with every fact retained before the violation.
#[derive(Debug)]
pub struct RunEvidenceInvariantPanic {
    pub violation: String,
    /// The sealed Failed result, with the violation retained as a Run Failure.
    pub result: OptimizationRunResult,
}

/// Executes one Optimization Run synchronously through the canonical Run Phase sequence.
///
/// This is the deepest deterministic seam beneath the Optimization Run Service.
#[derive(Debug, Default, Clone, Copy)]
pub struct RunExecutor;

impl RunExecutor {
    /// Traverses every Run Phase in order, reporting inapplicable phases as
    /// skipped, performs Safety Cleanup exactly once, and returns the terminal
    /// result.
    ///
    /// `stop` is observed between phases and between Assets; Safety Cleanup
    /// always finishes even after cancellation.
    ///
    /// # Panics
    ///
    /// If Run Evidence rejected a fact, panics after Safety Cleanup has run,
    /// with a [`RunEvidenceInvariantPanic`] payload holding the sealed result.
    pub fn execute(
        &self,
        request: &RunRequest,
        services: RunServices<'_>,
        stop: &CancellationToken,
        run_id: RunId,
    ) -> OptimizationRunResult {
        let RunServices {
            safety_cleanup,
            observations,
            configuration,
            work,
        } = services;
        let evidence = RefCell::new(MutableRunEvidence::new(observations));
        let mut invariant = DeferredInvariant::default();
        // The run's Temporary Ownership scope. It lives for the whole run so
        // its root pins and staging lock are held through Safety Cleanup.
        let mut artifacts = TemporaryArtifactRegistry::new(run_id.clone());

        // Every phase before Safety Cleanup runs inside one more containment
        // boundary. The seams' own boundaries contain their panics, but a panic
        // in the executor's own code, such as staging recovery, would otherwise
        // unwind past Safety Cleanup and drop the registry, which never cleans
        // up on drop, leaving the run's staged files behind.
        let traversal = catch_unwind(AssertUnwindSafe(|| {
            Self::traverse(
                request,
                configuration,
                work,
                &evidence,
                &mut artifacts,
                &mut invariant,
                stop,
            )
        }));
        if let Err(payload) = traversal {
            let detail = format!("The Run Worker panicked: {}", take_panic_message(payload));
            log::error!("{detail}");
            // Any evidence borrow the panic interrupted was released by unwinding.
            let mut evidence = evidence.borrow_mut();
            let phase = evidence
                .current_phase()
                .map_or(RunPhase::Preparing, RunPhaseRecord::phase);
            evidence.record_failure(RunFailure::new(
                RunFailureCode::WorkServiceFailed,
                phase,
                detail,
            ));
        }

        // Safety Cleanup runs exactly once on every terminal path, before the
        // terminal result exists, so cancellation and failure cannot leave
        // run-owned artifacts behind. It does not move the final work phase.
        let mut evidence = evidence.into_inner();
        let final_phase = evidence
            .current_phase()
            .map_or(RunPhase::Preparing, RunPhaseRecord::phase);
        invariant
            .note(evidence.record_phase(RunPhaseRecord::executed(RunPhase::SafetyCleanup, None)));
        let mut cleanup_failures = collect_safety_cleanup_failures(&mut artifacts);
        cleanup_failures.extend(collect_safety_cleanup_failures(safety_cleanup));
        for failure in cleanup_failures {
            invariant.note(evidence.record_safety_cleanup_failure(failure));
        }
        if stop.is_cancelled() {
            evidence.record_cancellation_observation();
        }
        let Some(violation) = invariant.into_violation() else {
            return Self::seal(evidence, final_phase, run_id);
        };
        let violation = violation.to_string();
        log::error!("{violation}");
        // Retained without publishing: Safety Cleanup has already been
        // published, and no Run Failure event may follow it.
        evidence.retain_failure(RunFailure::new(
            RunFailureCode::WorkServiceFailed,
            final_phase,
            violation.clone(),
        ));
        let result = Self::seal(evidence, final_phase, run_id);
        std::panic::panic_any(RunEvidenceInvariantPanic { violation, result })
    }

    /// Traverses Preparing and every work phase up to, but not including,
    /// Safety Cleanup, recording each fact in `evidence`.
    ///
    /// Failures are recorded, never returned; Run Evidence invariant
    /// violations are noted in `invariant` for the caller to raise after
    /// cleanup. `artifacts` is the run's Temporary Ownership scope, which the
    /// caller cleans up whatever happens here.
    fn traverse(
        request: &RunRequest,
        configuration: Option<&dyn RunConfigurationProvider>,
        work: Option<&dyn RunWorkService>,
        evidence: &RefCell<MutableRunEvidence<'_>>,
        artifacts: &mut TemporaryArtifactRegistry,
        invariant: &mut DeferredInvariant,
        stop: &CancellationToken,
    ) {
        let mut failed = false;

        // Preparing always executes: it is where the request becomes run-scoped
        // state. It is indeterminate work, so it reports no progress.
        invariant.note(
            evidence
                .borrow_mut()
                .record_phase(RunPhaseRecord::executed(RunPhase::Preparing, None)),
        );

        let mut preparation = None;
        if !stop.is_cancelled() {
            match prepare_run(request, configuration, evidence, stop) {
                Ok(prepared) => preparation = prepared,
                Err(failure) => {
                    failed = true;
                    evidence.borrow_mut().record_failure(failure);
                }
            }
        }

        if let (Some(_), true, Some(work)) = (&preparation, request.has_requested_work(), work)
            && !stop.is_cancelled()
        {
            let prepared = catch_unwind(AssertUnwindSafe(|| work.prepare()));
            let detail = match prepared {
                Ok(Ok(())) => None,
                Ok(Err(error)) => Some(error.to_string()),
                Err(payload) => Some(format!(
                    "Work configuration panicked: {}",
                    take_panic_message(payload)
                )),
            };
            if let Some(detail) = detail {
                failed = true;
                evidence.borrow_mut().record_failure(RunFailure::new(
                    RunFailureCode::ConfigurationLoadingFailed,
                    RunPhase::Preparing,
                    detail,
                ));
            }
        }

        // Apply Preparing recovers each Mod Root's leftover staging before any
        // work, and pins the root. Dry Run never inspects, creates, recovers or
        // cleans staging.
        if let Some(prepared) = &preparation
            && !failed
            && request.execution_mode() == ExecutionMode::Apply
        {
            for root in prepared.mod_roots() {
                if stop.is_cancelled() {
                    break;
                }
                let failure = artifacts.prepare_root(root, stop).unwrap_or_else(|error| {
                    Some(RunFailure::new(
                        RunFailureCode::StagingOwnershipUnverified,
                        RunPhase::Preparing,
                        error.to_string(),
                    ))
                });
                if let Some(failure) = failure {
                    failed = true;
                    evidence.borrow_mut().record_failure(failure);
                    break;
                }
            }
        }

        if preparation.is_some()
            && !failed
            && request.has_requested_work()
            && work.is_none()
            && !stop.is_cancelled()
        {
            // Keep the candidate private: a run missing its work service never
            // exposes partial Preparing facts.
            failed = true;
            evidence.borrow_mut().record_failure(RunFailure::new(
                RunFailureCode::RequestedWorkUnavailable,
                RunPhase::Preparing,
                "Requested work requires run services that are not yet available",
            ));
        }

        let preparation = preparation.filter(|_| !failed && !stop.is_cancelled());
        if let Some(prepared) = &preparation {
            invariant.note(evidence.borrow_mut().record_preparation(prepared.clone()));
        }

        if failed {
            // A failed Preparing stops traversal but never bypasses cleanup.
        } else if stop.is_cancelled() {
            // Cancellation before any work phase: only Safety Cleanup remains.
        } else if let (Some(prepared), true, Some(work)) =
            (&preparation, request.has_requested_work(), work)
        {
            let milestones = ExecutorMilestones {
                evidence,
                mode: request.execution_mode(),
            };
            let work_evidence = RunWorkEvidence::new(evidence);
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                work.execute(prepared, &work_evidence, artifacts, &milestones, stop)
            }));
            let detail = match outcome {
                Ok(Ok(())) => None,
                Ok(Err(error @ Error::EvidenceInvariant(_))) => {
                    invariant.note(Err(error));
                    None
                }
                Ok(Err(error)) => Some(error.to_string()),
                Err(payload) => Some(format!(
                    "The work service panicked: {}",
                    take_panic_message(payload)
                )),
            };
            if let Some(detail) = detail {
                let mut evidence = evidence.borrow_mut();
                let phase = evidence
                    .current_phase()
                    .map_or(RunPhase::Preparing, RunPhaseRecord::phase);
                evidence.record_failure(RunFailure::new(
                    RunFailureCode::WorkServiceFailed,
                    phase,
                    detail,
                ));
            }
        } else {
            // A request with no work skips every work phase for the one reason
            // the run knows: nothing was requested. It cannot claim, say, that
            // no Archives were found, and execution mode is not consulted.
            // Every phase between Preparing and Safety Cleanup.
            let work_phases = &RunPhase::SEQUENCE[1..RunPhase::SEQUENCE.len() - 1];
            for phase in work_phases {
                // An inline observer can cancel on a skipped phase; traversal
                // then stops rather than invent further skips.
                if stop.is_cancelled() {
                    break;
                }
                invariant.note(evidence.borrow_mut().record_phase(RunPhaseRecord::skipped(
                    *phase,
                    PhaseSkipReason::NoRequestedWork,
                )));
            }
        }
    }

    /// Commits the terminal result of a run whose worker could not be scheduled.
    ///
    /// No work phase was traversed, so only Safety Cleanup is recorded and
    /// Preparing is the final phase: a phase the run never entered may not
    /// claim an outcome.
    pub fn scheduling_failure(
        &self,
        detail: impl Into<String>,
        cleanup: &mut dyn SafetyCleanupService,
        observations: Option<&dyn RunObservationSink>,
        stop: &CancellationToken,
        run_id: RunId,
    ) -> OptimizationRunResult {
        Self::terminal_failure(
            RunFailure::new(
                RunFailureCode::SchedulingFailed,
                RunPhase::Preparing,
                detail,
            ),
            Some(cleanup),
            observations,
            RunPhase::Preparing,
            stop,
            run_id,
        )
    }

    /// Commits a Failed result carrying one Run Failure and, when a cleanup
    /// service is supplied, its Safety Cleanup pass.
    ///
    /// The Run Worker uses this when the executor panicked outside any
    /// contained boundary. It omits cleanup, and publication, if the executor
    /// had already performed and published Safety Cleanup; `final_phase` is the
    /// furthest work phase the run had published.
    pub(crate) fn terminal_failure(
        failure: RunFailure,
        cleanup: Option<&mut dyn SafetyCleanupService>,
        observations: Option<&dyn RunObservationSink>,
        final_phase: RunPhase,
        stop: &CancellationToken,
        run_id: RunId,
    ) -> OptimizationRunResult {
        let mut evidence = MutableRunEvidence::new(observations);
        evidence.record_failure(failure);
        // An empty evidence owner always accepts Safety Cleanup and its failures.
        let _ = evidence.record_phase(RunPhaseRecord::executed(RunPhase::SafetyCleanup, None));
        if let Some(cleanup) = cleanup {
            for failure in collect_safety_cleanup_failures(cleanup) {
                let _ = evidence.record_safety_cleanup_failure(failure);
            }
        }
        if stop.is_cancelled() {
            evidence.record_cancellation_observation();
        }
        Self::seal(evidence, final_phase, run_id)
    }

    /// Consumes the evidence after cleanup and classifies the outcome.
    fn seal(
        evidence: MutableRunEvidence<'_>,
        final_phase: RunPhase,
        run_id: RunId,
    ) -> OptimizationRunResult {
        let evidence = evidence.consume().unwrap_or_else(|error| panic!("{error}"));
        let outcome = classify_terminal_outcome(&evidence);
        OptimizationRunResult::terminal(outcome, final_phase, evidence, run_id)
    }
}
