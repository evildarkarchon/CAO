//! The Run Executor: the synchronous, deterministic traversal of one run's phases.
//!
//! Ported from `src/Run/RunExecutor.h`. The executor owns phase sequencing,
//! phase applicability, Safety Cleanup and terminal classification. It
//! schedules nothing and dispatches nothing; the Optimization Run Service and
//! its Run Worker sit above it.
//!
//! The configuration provider returns a Run Toolset with the run's Selected
//! Profile, and the executor runs every work phase itself: Asset attempts
//! through the Toolset's Asset attempt port, and Archive discovery,
//! extraction and Archive Finalization as core's own modules over the
//! Toolset's Archive seams. Every Optimization Run has a Run Toolset.
//!
//! The executor owns the run's Run Evidence and lends it mutably to each step
//! in turn. Run Evidence enforces the canonical Run Phase order: the
//! traversal records Preparing, the skips of a request with no work and
//! Safety Cleanup, and the work phases enter, skip and plan their own Run
//! Phases through it.
//!
//! C++ caught exceptions at three boundaries here; they are `Result`s and
//! `catch_unwind` now. A panic in the configuration provider, the run's work
//! or the cleanup service becomes the same Run Failure an exception did. A
//! panic that escapes those boundaries is caught around the whole traversal,
//! so it still fails the run before Safety Cleanup rather than skipping it. A Run
//! Evidence invariant violation is a bug: the executor finishes Safety Cleanup
//! and then panics, and the Run Worker turns that panic into a Failed outcome.

use std::ops::ControlFlow;
use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::Error;
use crate::routing::{ExecutionMode, RoutingPolicyRequest};
use crate::run::mod_selection::resolve_mod_roots;
use crate::run::{
    CancellationToken, MutableRunEvidence, OptimizationRunResult, PhaseSkipReason,
    RunConfigurationProvider, RunEvidence, RunFailure, RunFailureCode, RunId, RunObservationSink,
    RunOutcome, RunPhase, RunPhaseRecord, RunPreparation, RunRequest, RunToolset, SelectedProfile,
    TemporaryArtifactRegistry, run_worker_panic_detail, take_panic_message,
};

mod work_phases;

/// Why the work phases stopped short of finishing, carried by their
/// [`ControlFlow::Break`].
///
/// Most stops need nothing further from the executor: cancellation the
/// run's token shows is sampled once at Safety Cleanup. Only a backend's own
/// report of cancellation, which the token need not show, has to reach that
/// sample, so it is the one stop with a reason of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stop {
    /// The work stopped with nothing more for the executor to record: on
    /// token cancellation, an unsafe attempt, or a failed or cancelled
    /// preflight, each already in the evidence or in the token.
    Halted,
    /// An Asset backend reported cancellation during its initialization. The
    /// executor records cancellation observed at Safety Cleanup for it, as
    /// for the token, without cancelling the caller's token.
    BackendCancelled,
}

/// The work phases of one run, as the executor's traversal calls them: with
/// the run's preparation, its Run Toolset, its evidence lent mutably for the
/// call, its Temporary Ownership registry and its token.
///
/// Production work is the Run Toolset's work phases; the executor's own unit
/// tests inject work that escapes as an error or a panic through
/// [`RunExecutor::execute_with_work`]. `Ok` says whether the work finished
/// or stopped, and why; an error is a Run Evidence invariant violation or
/// escaped work.
pub(crate) trait RunWork:
    FnOnce(
    &RunPreparation,
    RunToolset,
    &mut MutableRunEvidence<'_>,
    &mut TemporaryArtifactRegistry,
    &CancellationToken,
) -> Result<ControlFlow<Stop>, Error>
{
}

impl<F> RunWork for F where
    F: FnOnce(
        &RunPreparation,
        RunToolset,
        &mut MutableRunEvidence<'_>,
        &mut TemporaryArtifactRegistry,
        &CancellationToken,
    ) -> Result<ControlFlow<Stop>, Error>
{
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
    /// A provider always returns a Run Toolset with the Selected Profile.
    pub configuration: Option<&'a dyn RunConfigurationProvider>,
}

/// Loads the Selected Profile and the Run Toolset built from the same profile read,
/// converting a missing, failing or panicking provider into a failure.
fn load_selected_profile(
    request: &RunRequest,
    provider: Option<&dyn RunConfigurationProvider>,
) -> Result<(SelectedProfile, RunToolset), RunFailure> {
    let failure = |detail: String| {
        RunFailure::new(
            RunFailureCode::ConfigurationLoadingFailed,
            RunPhase::Preparing,
            detail,
        )
    };
    let provider =
        provider.ok_or_else(|| failure("No run configuration provider is available".to_owned()))?;
    match catch_unwind(AssertUnwindSafe(|| provider.load(request))) {
        Ok(Ok(loaded)) => Ok(loaded),
        Ok(Err(error)) => Err(failure(error.to_string())),
        Err(payload) => Err(failure(format!(
            "The configuration provider panicked: {}",
            take_panic_message(payload)
        ))),
    }
}

/// Prepares immutable facts without mutation, with the Run Toolset the
/// provider built from the same profile read; `Ok(None)` means cancellation
/// was observed.
///
/// Mod Exclusions found while resolving the Mod Selection are recorded in
/// `evidence` and published as they are found.
fn prepare_run(
    request: &RunRequest,
    provider: Option<&dyn RunConfigurationProvider>,
    evidence: &mut MutableRunEvidence<'_>,
    stop: &CancellationToken,
) -> Result<Option<(RunPreparation, RunToolset)>, RunFailure> {
    let (selected_profile, toolset) = load_selected_profile(request, provider)?;
    // A provider may finish an atomic read after cancellation; stop before
    // compiling or resolving anything more.
    if stop.is_cancelled() {
        return Ok(None);
    }
    let policy = selected_profile
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
    let mod_roots = resolve_mod_roots(request.mod_selection(), &selected_profile, evidence, stop)?;
    if stop.is_cancelled() {
        return Ok(None);
    }
    Ok(Some((
        RunPreparation::new(
            mod_roots,
            selected_profile,
            policy,
            request.archive_precedence().clone(),
        ),
        toolset,
    )))
}

/// Runs every work phase, from Archive discovery through Archive
/// Finalization, over the Run Toolset.
///
/// Phases are observed only through the run's Run Observation Sink and
/// cancellation only through `stop`. Each panic is contained once, by the
/// module that owns its rule: the work phases contain an Asset attempt's,
/// and Archive extraction and Archive Finalization their own. Errors are
/// Run Evidence invariant violations; a stop says why the work stopped.
fn execute_toolset(
    preparation: &RunPreparation,
    mut toolset: RunToolset,
    evidence: &mut MutableRunEvidence<'_>,
    artifacts: &mut TemporaryArtifactRegistry,
    stop: &CancellationToken,
) -> Result<ControlFlow<Stop>, Error> {
    work_phases::run_work_phases(preparation, &mut toolset, evidence, artifacts, stop)
}

/// The Run Work Escaped detail of work that escaped with `outcome`, or
/// `None` when it finished, stopped or hit a Run Evidence invariant
/// violation, which is noted in `invariant` instead. An escaped error keeps
/// its own message.
fn escaped_work_detail(
    outcome: std::thread::Result<Result<ControlFlow<Stop>, Error>>,
    invariant: &mut DeferredInvariant,
) -> Option<String> {
    match outcome {
        Ok(Ok(_)) => None,
        Ok(Err(error @ Error::EvidenceInvariant(_))) => {
            invariant.note(Err(error));
            None
        }
        Ok(Err(error)) => Some(error.to_string()),
        Err(payload) => Some(format!(
            "The run's work escaped with a panic: {}",
            take_panic_message(payload)
        )),
    }
}

/// Classifies sealed facts into the one Run Outcome, after every producer has stopped.
///
/// Unsafe work wins over cancellation; cancellation wins over cleanup errors;
/// a failed cleanup service is fatal; any contained failure completes with failures.
///
/// Every completed attempt is read through Run Evidence's one
/// completed-attempts view, and its Attempt Outcome alone decides whether it
/// was safe to continue. Run Failures, finalization's phase-level verdict,
/// cancellation and cleanup failures are read separately.
fn classify_terminal_outcome(evidence: &RunEvidence) -> RunOutcome {
    let mut unsafe_work = !evidence.failures().is_empty();
    let mut contained_failure = false;
    if let Some(finalization) = evidence.archive_finalization() {
        unsafe_work |= !finalization.safe_to_continue;
        contained_failure |= finalization.failure.is_some();
    }
    for attempt in evidence.completed_attempts() {
        unsafe_work |= !attempt.facts.safe_to_continue();
        contained_failure |= !attempt.facts.succeeded();
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
        self.execute_with_work(request, services, stop, run_id, execute_toolset)
    }

    /// [`execute`](Self::execute), with `run_work` running in place of the
    /// Run Toolset's work phases.
    ///
    /// This is the crate-private seam beneath the executor's interface: it
    /// lets the executor's own unit tests inject work that escapes as an
    /// error or a panic, which the work phases' own containment stops before
    /// it could. `run_work` is called only when Preparing succeeded for a
    /// request with work, with the Run Toolset Preparing produced.
    ///
    /// # Panics
    ///
    /// As [`execute`](Self::execute).
    pub(crate) fn execute_with_work(
        &self,
        request: &RunRequest,
        services: RunServices<'_>,
        stop: &CancellationToken,
        run_id: RunId,
        run_work: impl RunWork,
    ) -> OptimizationRunResult {
        let RunServices {
            safety_cleanup,
            observations,
            configuration,
        } = services;
        // The run's evidence, owned here for the whole run and lent mutably to
        // each step of the traversal in turn.
        let mut evidence = MutableRunEvidence::new(observations);
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
                &mut evidence,
                &mut artifacts,
                &mut invariant,
                stop,
                run_work,
            )
        }));
        let backend_cancelled = match traversal {
            Ok(backend_cancelled) => backend_cancelled,
            Err(payload) => {
                // The traversal's borrow of the evidence ended with the
                // unwind. The owned evidence keeps every fact recorded before
                // the panic, including whatever the interrupted step had
                // partly written.
                Self::fail_at_current_phase(&mut evidence, run_worker_panic_detail(payload));
                false
            }
        };
        // The work phases retain some Run Diagnostics without publishing them.
        // They are published here, once, whether the work returned, stopped
        // early or panicked, so no exit path has to remember them and a panic
        // cannot leave them unpublished. Escaped work's Run Failure, already
        // published above or inside the traversal, therefore precedes them,
        // and Safety Cleanup follows them.
        evidence.publish_diagnostics();

        // Safety Cleanup runs exactly once on every terminal path, before the
        // terminal result exists, so cancellation and failure cannot leave
        // run-owned artifacts behind. It does not move the final work phase.
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
        // The token never resets, so this one sample covers cancellation the
        // work phases observed through it; they do not record it themselves.
        // Cancellation a backend reported, which the token may not show,
        // reaches this same sample through the work's stop, so cancellation
        // is recorded here alone and the caller's token is never cancelled
        // on the backend's behalf.
        if stop.is_cancelled() || backend_cancelled {
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
            RunFailureCode::RunWorkEscaped,
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
    /// caller cleans up whatever happens here. `run_work` runs the work
    /// phases when Preparing succeeded for a request with work.
    ///
    /// Returns whether the work stopped on a backend's report of
    /// cancellation, which the caller records with its Safety Cleanup sample
    /// of `stop`.
    fn traverse(
        request: &RunRequest,
        configuration: Option<&dyn RunConfigurationProvider>,
        evidence: &mut MutableRunEvidence<'_>,
        artifacts: &mut TemporaryArtifactRegistry,
        invariant: &mut DeferredInvariant,
        stop: &CancellationToken,
        run_work: impl RunWork,
    ) -> bool {
        let mut failed = false;
        let mut backend_cancelled = false;

        // Preparing always executes: it is where the request becomes run-scoped
        // state. It is indeterminate work, so it reports no progress.
        invariant.note(evidence.record_phase(RunPhaseRecord::executed(RunPhase::Preparing, None)));

        let mut preparation = None;
        // The Run Toolset the provider built from Preparing's profile read,
        // present whenever Preparing produced a preparation. The executor
        // runs the work phases itself over it.
        let mut toolset = None;
        if !stop.is_cancelled() {
            match prepare_run(request, configuration, evidence, stop) {
                Ok(prepared) => {
                    (preparation, toolset) = prepared.unzip();
                }
                Err(failure) => {
                    failed = true;
                    evidence.record_failure(failure);
                }
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
                    evidence.record_failure(failure);
                    break;
                }
            }
        }

        let preparation = preparation.filter(|_| !failed && !stop.is_cancelled());
        if let Some(prepared) = &preparation {
            invariant.note(evidence.record_preparation(prepared.clone()));
        }

        if failed {
            // A failed Preparing stops traversal but never bypasses cleanup.
        } else if stop.is_cancelled() {
            // Cancellation before any work phase: only Safety Cleanup remains.
        } else if let (Some(prepared), true, Some(toolset)) =
            (&preparation, request.has_requested_work(), toolset)
        {
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                run_work(prepared, toolset, evidence, artifacts, stop)
            }));
            backend_cancelled =
                matches!(outcome, Ok(Ok(ControlFlow::Break(Stop::BackendCancelled))));
            if let Some(detail) = escaped_work_detail(outcome, invariant) {
                Self::fail_at_current_phase(evidence, detail);
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
                invariant.note(evidence.record_phase(RunPhaseRecord::skipped(
                    *phase,
                    PhaseSkipReason::NoRequestedWork,
                )));
            }
        }
        backend_cancelled
    }

    /// Records work that escaped with `detail` as a fatal Run Failure at the
    /// run's current phase, retaining every earlier fact.
    fn fail_at_current_phase(evidence: &mut MutableRunEvidence<'_>, detail: String) {
        let phase = evidence
            .current_phase()
            .map_or(RunPhase::Preparing, RunPhaseRecord::phase);
        evidence.record_failure(RunFailure::new(
            RunFailureCode::RunWorkEscaped,
            phase,
            detail,
        ));
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

#[cfg(test)]
mod tests;

// Kept apart from the containment tests above: classification needs no run,
// only evidence built through the evidence store's own recording steps.
#[cfg(test)]
mod classification_tests;
