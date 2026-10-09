//! The Optimization Run Service, the Run Handle and the Run Worker.
//!
//! Ported from `src/Run/OptimizationRunService.h` (ADR-0001). The service
//! validates a Run Request's structure, admits one active run per process,
//! schedules its single Run Worker and hands the run back as an owning
//! [`RunHandle`]. Dropping an active handle requests cancellation and joins
//! the worker, so a run is never abandoned while it executes.
//!
//! Run Events go to one adapter-supplied [`RunEventDispatcher`], called on the
//! Run Worker in sequence order. State is published before its event is
//! dispatched, so a dispatcher that reads a snapshot sees what the event
//! describes. This replaces the C++ observer/dispatcher pair and its delivery
//! bookkeeping (#468): the GUI's dispatcher forwards through
//! `Weak::upgrade_in_event_loop` itself.

use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, Weak};

use crate::Error;
use crate::run::{
    CancellationToken, OptimizationRunResult, RunConfigurationProvider, RunDiagnostic,
    RunDiagnosticCode, RunEvent, RunEventPayload, RunEvidenceInvariantPanic, RunExecutor,
    RunFailure, RunFailureCode, RunId, RunObservationSink, RunPhase, RunPhaseRecord, RunProgress,
    RunRequest, RunScheduler, RunServices, RunSnapshot, RunWorkService, SafetyCleanupService,
    ScheduledRunWorker, StandardRunScheduler, create_run_id, panic_message,
};

/// Receives every Run Event of one run, in sequence order, on the Run Worker.
///
/// A dispatcher that panics is disabled: the run records a `DispatcherFailed`
/// diagnostic and carries on without it. It is dropped once the terminal
/// event has been dispatched.
pub type RunEventDispatcher = Box<dyn Fn(RunEvent) + Send>;

/// A structural request conflict or an active run that prevents a start.
///
/// It is detected before any worker exists, so it produces no Run Outcome.
/// Facts that need the filesystem or the profile belong to Preparing instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StartError {
    MissingProfileIdentity,
    MissingModSelectionDirectory,
    ActiveRun,
}

/// Locks a mutex, recovering the data if a panicking holder poisoned it.
///
/// Every lock here guards plain published state, which stays consistent
/// because each holder writes whole values.
fn lock<T: ?Sized>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The one active run of this process. Weak, so a committed run releases it
/// without the service's help.
static ACTIVE_RUN: Mutex<Weak<RunShared>> = Mutex::new(Weak::new());

thread_local! {
    /// The runs whose Run Worker is executing on this thread, innermost last.
    ///
    /// The inline scheduler runs work before the worker object exists, so the
    /// worker alone cannot diagnose a run waiting for, or destroying, itself.
    static EXECUTING: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
}

/// Marks this thread as executing one run until dropped.
struct ExecutionScope(usize);

impl ExecutionScope {
    fn enter(shared: &Arc<RunShared>) -> Self {
        let key = Arc::as_ptr(shared) as usize;
        EXECUTING.with(|runs| runs.borrow_mut().push(key));
        Self(key)
    }

    /// Reports whether this thread is executing `shared`, at any nesting depth.
    fn contains(shared: &Arc<RunShared>) -> bool {
        let key = Arc::as_ptr(shared) as usize;
        EXECUTING.with(|runs| runs.borrow().contains(&key))
    }
}

impl Drop for ExecutionScope {
    fn drop(&mut self) {
        EXECUTING.with(|runs| {
            let mut runs = runs.borrow_mut();
            if let Some(position) = runs.iter().rposition(|key| *key == self.0) {
                runs.remove(position);
            }
        });
    }
}

/// The state a run publishes for snapshots and waiters.
struct Published {
    phase: RunPhase,
    /// The furthest phase published before Safety Cleanup: the final phase a
    /// panicked run reports.
    work_phase: RunPhase,
    progress: Option<RunProgress>,
    diagnostics: Vec<RunDiagnostic>,
    failure_count: usize,
    result: Option<Arc<OptimizationRunResult>>,
    /// Set once the terminal event has been dispatched; `wait` returns then.
    terminal_dispatched: bool,
}

/// The dispatcher and the sequence its events are numbered by.
struct Dispatch {
    dispatcher: Option<RunEventDispatcher>,
    sequence: u64,
}

/// The run's Safety Cleanup Service, and whether its pass has started.
struct TrackedCleanup {
    service: Box<dyn SafetyCleanupService>,
    performed: bool,
}

impl SafetyCleanupService for TrackedCleanup {
    fn perform_safety_cleanup(&mut self) -> Result<Vec<RunFailure>, Error> {
        self.performed = true;
        self.service.perform_safety_cleanup()
    }
}

/// The injected Safety Cleanup Service of a production run.
///
/// The Run Executor owns each run's Temporary Ownership registry and cleans it
/// up itself, as C++ did, so nothing else is registered here.
struct NoTemporaryArtifacts;

impl SafetyCleanupService for NoTemporaryArtifacts {
    fn perform_safety_cleanup(&mut self) -> Result<Vec<RunFailure>, Error> {
        Ok(Vec::new())
    }
}

/// The state one started run shares between its worker and its Run Handle.
struct RunShared {
    run_id: RunId,
    request: RunRequest,
    configuration: Option<Arc<dyn RunConfigurationProvider>>,
    work: Option<Arc<dyn RunWorkService>>,
    cancellation: CancellationToken,
    cleanup: Mutex<TrackedCleanup>,
    published: Mutex<Published>,
    committed: Condvar,
    dispatch: Mutex<Dispatch>,
}

impl RunShared {
    /// Numbers one event and hands it to the dispatcher, containing a panic.
    fn dispatch(&self, payload: RunEventPayload) {
        let mut dispatch = lock(&self.dispatch);
        dispatch.sequence += 1;
        let event = RunEvent {
            run_id: self.run_id.clone(),
            sequence: dispatch.sequence,
            payload,
        };
        let Some(dispatcher) = dispatch.dispatcher.as_ref() else {
            return;
        };
        if let Err(payload) = catch_unwind(AssertUnwindSafe(|| dispatcher(event))) {
            dispatch.dispatcher = None;
            drop(dispatch);
            let detail = panic_message(payload.as_ref());
            log::warn!("The Run Event dispatcher failed and was disabled: {detail}");
            let mut published = lock(&self.published);
            let phase = published.phase;
            published.diagnostics.push(RunDiagnostic::new(
                RunDiagnosticCode::DispatcherFailed,
                phase,
                detail,
            ));
        }
    }

    /// Runs the Optimization Run on the Run Worker and commits its one result.
    ///
    /// A panic anywhere in the run, including a Run Evidence invariant
    /// violation the executor raises after cleanup, still commits a Failed
    /// result, and Safety Cleanup still runs if it had not.
    fn execute(self: &Arc<Self>) {
        let _scope = ExecutionScope::enter(self);
        let executed = catch_unwind(AssertUnwindSafe(|| {
            let mut cleanup = lock(&self.cleanup);
            RunExecutor.execute(
                &self.request,
                RunServices {
                    safety_cleanup: &mut *cleanup,
                    observations: Some(self.as_ref()),
                    configuration: self.configuration.as_deref(),
                    work: self.work.as_deref(),
                },
                &self.cancellation,
                self.run_id.clone(),
            )
        }));
        let result = executed.unwrap_or_else(|payload| {
            // An invariant violation carries the run's sealed result: commit it
            // with every fact it retained.
            let payload = match payload.downcast::<RunEvidenceInvariantPanic>() {
                Ok(sealed) => return sealed.result,
                Err(payload) => payload,
            };
            let detail = panic_message(payload.as_ref());
            log::error!("The Run Worker panicked: {detail}");
            let work_phase = lock(&self.published).work_phase;
            let mut cleanup = lock(&self.cleanup);
            // If the executor already performed and published Safety Cleanup,
            // nothing may be published after it but the terminal event; the
            // failure is still retained in the result.
            let pending = !cleanup.performed;
            RunExecutor::terminal_failure(
                RunFailure::new(
                    RunFailureCode::WorkServiceFailed,
                    work_phase,
                    format!("The Run Worker panicked: {detail}"),
                ),
                pending.then_some(&mut *cleanup as &mut dyn SafetyCleanupService),
                pending.then_some(self.as_ref() as &dyn RunObservationSink),
                work_phase,
                &self.cancellation,
                self.run_id.clone(),
            )
        });
        self.commit(result);
    }

    /// Commits the result of a run whose worker could not be started.
    fn commit_scheduling_failure(self: &Arc<Self>, detail: String) {
        let _scope = ExecutionScope::enter(self);
        let result = {
            let mut cleanup = lock(&self.cleanup);
            RunExecutor.scheduling_failure(
                detail,
                &mut *cleanup,
                Some(self.as_ref()),
                &self.cancellation,
                self.run_id.clone(),
            )
        };
        self.commit(result);
    }

    /// Commits the one terminal result, releases the active slot, dispatches
    /// the terminal event and then releases every waiter.
    fn commit(self: &Arc<Self>, result: OptimizationRunResult) {
        let result = Arc::new(result);
        {
            let mut published = lock(&self.published);
            published.result = Some(Arc::clone(&result));
            // Publish the result and release the slot together: a waiter must
            // never see a terminal result while a new start would still be
            // rejected. Releasing before dispatch also lets the terminal
            // event's dispatcher start the next run.
            let mut active = lock(&ACTIVE_RUN);
            if std::ptr::eq(active.as_ptr(), Arc::as_ptr(self)) {
                *active = Weak::new();
            }
        }
        self.dispatch(RunEventPayload::Terminal(result));
        // No event follows the terminal one. Dropping the dispatcher here also
        // breaks any cycle through state it captured.
        let dispatcher = lock(&self.dispatch).dispatcher.take();
        drop(dispatcher);
        lock(&self.published).terminal_dispatched = true;
        self.committed.notify_all();
    }

    fn snapshot(&self) -> RunSnapshot {
        let published = lock(&self.published);
        RunSnapshot {
            run_id: self.run_id.clone(),
            phase: published.phase,
            progress: published.progress,
            cancellation_requested: self.cancellation.is_cancelled(),
            diagnostic_count: published.diagnostics.len(),
            failure_count: published.failure_count,
            outcome: published.result.as_ref().map(|result| result.outcome()),
        }
    }
}

impl RunObservationSink for RunShared {
    fn record_phase(&self, phase: &RunPhaseRecord) {
        {
            let mut published = lock(&self.published);
            published.phase = phase.phase();
            published.progress = phase.progress();
            if phase.phase() != RunPhase::SafetyCleanup {
                published.work_phase = phase.phase();
            }
        }
        self.dispatch(RunEventPayload::Phase(*phase));
    }

    fn record_failure(&self, failure: &RunFailure) {
        lock(&self.published).failure_count += 1;
        self.dispatch(RunEventPayload::Failure(failure.clone()));
    }

    fn record_diagnostic(&self, diagnostic: &RunDiagnostic) {
        lock(&self.published).diagnostics.push(diagnostic.clone());
        self.dispatch(RunEventPayload::Diagnostic(diagnostic.clone()));
    }
}

/// The join obligation shared by a run's handle and its service.
///
/// The worker captures only [`RunShared`], so this never forms a cycle.
struct RunWorkerLifetime {
    shared: Arc<RunShared>,
    worker: OnceLock<Box<dyn ScheduledRunWorker>>,
    joined: Mutex<bool>,
}

impl RunWorkerLifetime {
    /// Reports whether the caller is this run's own worker.
    fn called_from_worker(&self) -> bool {
        ExecutionScope::contains(&self.shared)
            || self
                .worker
                .get()
                .is_some_and(|worker| worker.is_current_thread())
    }

    /// Requests cancellation and joins the worker exactly once.
    ///
    /// Joining from the worker would deadlock and cannot fail safely from a
    /// destructor, so it aborts the process with a diagnostic instead.
    fn cancel_and_join(&self) {
        // Checked before taking the join lock: another caller may hold it while
        // waiting for this very worker.
        if self.called_from_worker() {
            // Logged, and also written straight to stderr, because a log sink
            // may never flush before the abort.
            log::error!("An Optimization Run cannot destroy its own worker");
            eprintln!("An Optimization Run cannot destroy its own worker");
            std::process::abort();
        }
        self.shared.cancellation.cancel();
        let mut joined = lock(&self.joined);
        if !*joined {
            if let Some(worker) = self.worker.get() {
                worker.join();
            }
            *joined = true;
        }
    }
}

/// The owning handle of one started Optimization Run.
///
/// There is exactly one owner. Dropping an active handle requests cancellation
/// and joins the Run Worker. It offers no pause, resume, restart, mutable
/// configuration or worker access.
pub struct RunHandle {
    shared: Arc<RunShared>,
    lifetime: Arc<RunWorkerLifetime>,
}

impl RunHandle {
    /// The run's identity, shared by its events and terminal result.
    pub fn run_id(&self) -> &str {
        &self.shared.run_id
    }

    /// Requests cancellation without blocking or interrupting work in progress.
    ///
    /// Idempotent and callable from any thread. A request after terminal
    /// commit shows in snapshots but never changes the result.
    pub fn request_cancellation(&self) {
        self.shared.cancellation.cancel();
    }

    /// The committed terminal result, or `None` while the run is active.
    pub fn terminal_result(&self) -> Option<Arc<OptimizationRunResult>> {
        lock(&self.shared.published).result.clone()
    }

    /// The run's published diagnostics, including a dispatcher failure that
    /// came after the terminal result. Late diagnostics never change it.
    pub fn diagnostics(&self) -> Vec<RunDiagnostic> {
        lock(&self.shared.published).diagnostics.clone()
    }

    /// A copy of the published state, from any thread, including the dispatcher.
    pub fn snapshot(&self) -> RunSnapshot {
        self.shared.snapshot()
    }

    /// Blocks until the terminal result is committed and its event dispatched.
    ///
    /// Every call returns the same result.
    ///
    /// # Panics
    ///
    /// Panics when called from the run's own worker, including from its
    /// dispatcher, which would otherwise deadlock.
    pub fn wait(&self) -> Arc<OptimizationRunResult> {
        if self.lifetime.called_from_worker() {
            panic!("An Optimization Run cannot wait for or destroy its own worker");
        }
        let mut published = lock(&self.shared.published);
        while !published.terminal_dispatched {
            published = self
                .shared
                .committed
                .wait(published)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        Arc::clone(
            published
                .result
                .as_ref()
                .expect("a dispatched terminal event has a committed result"),
        )
    }
}

impl Drop for RunHandle {
    /// Requests cancellation and joins the worker, so the run is never abandoned.
    fn drop(&mut self) {
        self.lifetime.cancel_and_join();
    }
}

impl std::fmt::Debug for RunHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunHandle")
            .field("run_id", &self.shared.run_id)
            .finish_non_exhaustive()
    }
}

/// Starts Optimization Runs and hands each back as an owning [`RunHandle`].
///
/// The public seam adapters use: callers supply intent, and the service owns
/// scheduling, the run services and cleanup. Concurrent `start` calls are
/// safe; dropping the service cancels and joins every run it still retains.
pub struct OptimizationRunService {
    scheduler: Arc<dyn RunScheduler>,
    configuration: Option<Arc<dyn RunConfigurationProvider>>,
    work: Option<Arc<dyn RunWorkService>>,
    runs: Mutex<Vec<Weak<RunWorkerLifetime>>>,
}

impl OptimizationRunService {
    /// A service running each run on its own production thread.
    ///
    /// A missing provider fails each run's Preparing rather than its start; a
    /// missing work service fails runs that request work.
    pub fn new(
        configuration: Option<Arc<dyn RunConfigurationProvider>>,
        work: Option<Arc<dyn RunWorkService>>,
    ) -> Self {
        Self::with_scheduler(Arc::new(StandardRunScheduler), configuration, work)
    }

    /// A service starting runs on an injected scheduler.
    pub fn with_scheduler(
        scheduler: Arc<dyn RunScheduler>,
        configuration: Option<Arc<dyn RunConfigurationProvider>>,
        work: Option<Arc<dyn RunWorkService>>,
    ) -> Self {
        Self {
            scheduler,
            configuration,
            work,
            runs: Mutex::new(Vec::new()),
        }
    }

    /// Validates the request's structure and starts the run.
    ///
    /// A structural conflict or an active run is a synchronous [`StartError`]
    /// and creates no worker. Otherwise the run owns `request`, and anything
    /// that goes wrong later, including a scheduler that cannot start a
    /// worker, ends the run as Failed instead.
    pub fn start(
        &self,
        request: RunRequest,
        dispatcher: Option<RunEventDispatcher>,
    ) -> Result<RunHandle, StartError> {
        // Fixed order, so a request with several conflicts always reports the
        // same one. Only the request's shape is inspected here.
        if request.profile_identity().is_empty() {
            return Err(StartError::MissingProfileIdentity);
        }
        if request.mod_selection().directory().as_os_str().is_empty() {
            return Err(StartError::MissingModSelectionDirectory);
        }

        let shared = {
            let mut active = lock(&ACTIVE_RUN);
            if active.strong_count() != 0 {
                return Err(StartError::ActiveRun);
            }
            let shared = Arc::new(RunShared {
                run_id: create_run_id(),
                request,
                configuration: self.configuration.clone(),
                work: self.work.clone(),
                cancellation: CancellationToken::new(),
                cleanup: Mutex::new(TrackedCleanup {
                    service: Box::new(NoTemporaryArtifacts),
                    performed: false,
                }),
                published: Mutex::new(Published {
                    phase: RunPhase::Preparing,
                    work_phase: RunPhase::Preparing,
                    progress: None,
                    diagnostics: Vec::new(),
                    failure_count: 0,
                    result: None,
                    terminal_dispatched: false,
                }),
                committed: Condvar::new(),
                dispatch: Mutex::new(Dispatch {
                    dispatcher,
                    sequence: 0,
                }),
            });
            *active = Arc::downgrade(&shared);
            shared
        };

        let lifetime = Arc::new(RunWorkerLifetime {
            shared: Arc::clone(&shared),
            worker: OnceLock::new(),
            joined: Mutex::new(false),
        });
        {
            let mut runs = lock(&self.runs);
            runs.retain(|run| run.strong_count() != 0);
            runs.push(Arc::downgrade(&lifetime));
        }

        // The worker holds its own reference, so the run survives a handle
        // dropped while the worker is still executing.
        let worker_state = Arc::clone(&shared);
        match self
            .scheduler
            .schedule(Box::new(move || worker_state.execute()))
        {
            Ok(worker) => {
                // The lifetime was created just above and is set only here, so
                // the slot is always empty and `set` cannot fail.
                let _ = lifetime.worker.set(worker);
            }
            Err(error) => {
                // The run already exists, so it owes a terminal result. A
                // scheduler that failed started nothing, so nothing else will
                // commit it, and there is no worker to join.
                log::error!("The Run Scheduler could not start a Run Worker: {error}");
                shared.commit_scheduling_failure(error.to_string());
            }
        }
        Ok(RunHandle { shared, lifetime })
    }
}

impl Drop for OptimizationRunService {
    /// Cancels and joins every retained run before the service's dependencies go.
    fn drop(&mut self) {
        let runs = std::mem::take(&mut *lock(&self.runs));
        for run in runs {
            if let Some(lifetime) = run.upgrade() {
                lifetime.cancel_and_join();
            }
        }
    }
}
