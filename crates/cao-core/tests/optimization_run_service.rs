//! Optimization Run Service scenarios: the Run Handle subset of
//! `tests/OptimizationRunServiceTests.cpp`, plus the event-ordering scenarios
//! that pin ADR-0001's lifetime rules.
//!
//! Each scenario names its C++ origin. Runs are process-wide (one active run),
//! so every scenario holds the `serial` guard.
//!
//! Not ported, with reasons:
//! - `queuedEventsOutliveTheRunOwners` and the multi-observer half of
//!   `presentationFailuresDisableOnlyTheFailingObserver`: the C++ observer and
//!   queueing dispatcher pair, with its admission bookkeeping, became one
//!   `Box<dyn Fn(RunEvent) + Send>` dispatcher called in order on the worker
//!   (#468). Queueing is the adapter's own business now, and events are owned
//!   values that outlive the run by construction.
//! - The Preparing scenarios (`severalMods…`, `linkedModRoots…`,
//!   `configurationLoads…`, `policyConflicts…`, `missingModRoots…`) are not Run
//!   Handle scenarios; the single-root ones are ported against the Run Executor
//!   in `run_executor.rs`, and the Several Mods ones belong to #486.

mod common;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex, OnceLock, mpsc};

use cao_core::Error;
use cao_core::routing::{ExecutionMode, RequestedWork};
use cao_core::run::{
    ModSelection, OptimizationRunService, PhaseSkipReason, RunDiagnosticCode, RunEventPayload,
    RunFailureCode, RunHandle, RunOutcome, RunPhase, RunPhaseStatus, RunRequest, StartError,
};
use common::{
    CallbackConfiguration, CountingInlineScheduler, DeferredScheduler, EventLog,
    ExhaustedScheduler, GatedScheduler, HandleSlot, no_work_request, reclaim, serial,
    test_configuration, test_mod_root,
};

/// A service on an injected scheduler with the SSE-like configuration and no work service.
fn service_on(scheduler: Arc<dyn cao_core::run::RunScheduler>) -> OptimizationRunService {
    OptimizationRunService::with_scheduler(scheduler, Some(test_configuration()), None)
}

/// Origin: OptimizationRunServiceTests::aRequestWithoutAProfileIdentityIsRejectedWithoutCreatingAWorker.
#[test]
fn a_request_without_a_profile_identity_is_rejected_without_creating_a_worker() {
    let _serial = serial();
    let scheduler = Arc::new(CountingInlineScheduler::default());
    let service = service_on(scheduler.clone());

    let started = service.start(
        RunRequest::new(
            "",
            ExecutionMode::Apply,
            ModSelection::SingleModRoot(test_mod_root()),
            Vec::new(),
        ),
        None,
    );

    assert_eq!(started.unwrap_err(), StartError::MissingProfileIdentity);
    // A Start Error produces no Run Outcome, so nothing may have been scheduled.
    assert_eq!(scheduler.scheduled.load(Ordering::SeqCst), 0);
}

/// Origin: OptimizationRunServiceTests::aModSelectionWithoutADirectoryIsRejectedWithoutCreatingAWorker.
#[test]
fn a_mod_selection_without_a_directory_is_rejected_without_creating_a_worker() {
    let _serial = serial();
    let scheduler = Arc::new(CountingInlineScheduler::default());
    let service = service_on(scheduler.clone());

    let single = service.start(
        RunRequest::new(
            "SkyrimSE",
            ExecutionMode::Apply,
            ModSelection::SingleModRoot("".into()),
            Vec::new(),
        ),
        None,
    );
    let children = service.start(
        RunRequest::new(
            "SkyrimSE",
            ExecutionMode::DryRun,
            ModSelection::ChildModRoots("".into()),
            vec![RequestedWork::NativeTextureOptimization],
        ),
        None,
    );

    assert_eq!(
        single.unwrap_err(),
        StartError::MissingModSelectionDirectory
    );
    assert_eq!(
        children.unwrap_err(),
        StartError::MissingModSelectionDirectory
    );
    assert_eq!(scheduler.scheduled.load(Ordering::SeqCst), 0);
}

/// Origin: OptimizationRunServiceTests::aRejectedStartExposesAStartErrorInsteadOfAHandle and
/// aValidStartSchedulesOneWorkerAndReturnsAHandle.
#[test]
fn a_valid_start_schedules_one_worker_and_returns_a_handle() {
    let _serial = serial();
    let scheduler = Arc::new(CountingInlineScheduler::default());
    let service = service_on(scheduler.clone());

    let rejected = service.start(
        RunRequest::new(
            "",
            ExecutionMode::Apply,
            ModSelection::SingleModRoot("".into()),
            Vec::new(),
        ),
        None,
    );
    let handle = service
        .start(no_work_request(), None)
        .expect("a valid request starts");

    assert!(rejected.is_err());
    assert_eq!(scheduler.scheduled.load(Ordering::SeqCst), 1);
    // The inline seam runs the worker before scheduling returns.
    assert!(handle.terminal_result().is_some());
}

/// Origin: OptimizationRunServiceTests::aRunHandleIsMovableButNotCopyable. Rust
/// moves are the language's; the scenario is that the moved-to owner still
/// observes the run.
#[test]
fn a_moved_handle_carries_its_run() {
    let _serial = serial();
    let service = service_on(Arc::new(CountingInlineScheduler::default()));
    let handle = service.start(no_work_request(), None).unwrap();

    let moved = handle;
    let reseated = moved;

    assert_eq!(reseated.wait().outcome(), RunOutcome::Succeeded);
}

/// Origin: OptimizationRunServiceTests::waitingReturnsTheImmutableTerminalResult.
#[test]
fn waiting_returns_the_terminal_result_with_the_full_phase_sequence() {
    let _serial = serial();
    let service = service_on(Arc::new(CountingInlineScheduler::default()));
    let handle = service.start(no_work_request(), None).unwrap();

    let terminal = handle.wait();

    assert_eq!(terminal.outcome(), RunOutcome::Succeeded);
    assert_eq!(terminal.final_phase(), RunPhase::ArchiveFinalization);
    assert_eq!(terminal.phases().len(), RunPhase::SEQUENCE.len());
    let last = terminal.phases().last().unwrap();
    assert_eq!(
        (last.phase(), last.status()),
        (RunPhase::SafetyCleanup, RunPhaseStatus::Executed)
    );
}

/// Origin: OptimizationRunServiceTests::waitingRepeatedlyObservesTheSameCommittedResult.
#[test]
fn waiting_repeatedly_observes_the_same_committed_result() {
    let _serial = serial();
    let scheduler = Arc::new(CountingInlineScheduler::default());
    let service = service_on(scheduler.clone());
    let handle = service.start(no_work_request(), None).unwrap();

    let first = handle.wait();
    let second = handle.wait();

    // Committed once and never recomputed.
    assert!(Arc::ptr_eq(&first, &second));
    assert!(Arc::ptr_eq(&handle.terminal_result().unwrap(), &first));
    assert_eq!(scheduler.scheduled.load(Ordering::SeqCst), 1);
}

/// Origin: OptimizationRunServiceTests::theRunOwnsItsRequestAfterStartReturns and
/// theDefaultServiceSchedulesAProductionRun.
#[test]
fn a_production_run_owns_its_request_after_start_returns() {
    let _serial = serial();
    let service = OptimizationRunService::new(Some(test_configuration()), None);

    // The request is moved into the run while its worker is still running on
    // another thread; the run must own it, not borrow it.
    let handle = service.start(no_work_request(), None).unwrap();

    assert_eq!(handle.wait().outcome(), RunOutcome::Succeeded);
}

/// Origin: OptimizationRunServiceTests::terminalQueryReportsAnActiveRunAsUncommitted.
#[test]
fn an_active_run_has_no_terminal_result() {
    let _serial = serial();
    let scheduler = Arc::new(DeferredScheduler::default());
    let service = service_on(scheduler.clone());

    let handle = service.start(no_work_request(), None).unwrap();

    assert!(handle.terminal_result().is_none());
    assert_eq!(scheduler.completions.load(Ordering::SeqCst), 0);
    drop(handle);
}

/// Origin: OptimizationRunServiceTests::destroyingAnActiveHandleJoinsItsWorker.
#[test]
fn dropping_an_active_handle_joins_its_worker() {
    let _serial = serial();
    let scheduler = Arc::new(DeferredScheduler::default());
    let service = service_on(scheduler.clone());

    {
        let _handle = service.start(no_work_request(), None).unwrap();
        assert_eq!(scheduler.completions.load(Ordering::SeqCst), 0);
    }

    // The run finished rather than being abandoned mid-flight.
    assert_eq!(scheduler.completions.load(Ordering::SeqCst), 1);
}

/// Origin: OptimizationRunServiceTests::movingAHandleTransfersTheJoinObligationExactlyOnce.
#[test]
fn moving_a_handle_transfers_the_join_obligation_exactly_once() {
    let _serial = serial();
    let scheduler = Arc::new(DeferredScheduler::default());
    let service = service_on(scheduler.clone());

    let source = service.start(no_work_request(), None).unwrap();
    {
        let _destination = source;
        assert_eq!(scheduler.joins.load(Ordering::SeqCst), 0);
    }

    // The destination owed the join; the moved-from binding owes nothing.
    assert_eq!(scheduler.completions.load(Ordering::SeqCst), 1);
    assert_eq!(scheduler.joins.load(Ordering::SeqCst), 1);
}

/// Origin: OptimizationRunServiceTests::moveAssigningOverAnActiveHandleJoinsTheReplacedRun.
/// Rust assignment drops the replaced value, so overwriting an owner slot must
/// join the run it held.
#[test]
fn overwriting_an_active_handle_joins_the_replaced_run() {
    let _serial = serial();
    let scheduler = Arc::new(DeferredScheduler::default());
    let service = service_on(scheduler.clone());
    let mut owner = Some(service.start(no_work_request(), None).unwrap());
    assert!(owner.is_some());
    assert_eq!(scheduler.joins.load(Ordering::SeqCst), 0);

    owner = None;

    assert!(owner.is_none());
    assert_eq!(scheduler.completions.load(Ordering::SeqCst), 1);
    assert_eq!(scheduler.joins.load(Ordering::SeqCst), 1);
}

/// Origin: OptimizationRunServiceTests::aSchedulerThatCannotStartAWorkerCommitsAFailedRun.
#[test]
fn a_scheduler_that_cannot_start_a_worker_commits_a_failed_run() {
    let _serial = serial();
    let service = service_on(Arc::new(ExhaustedScheduler));

    // The run already exists, so it owes a terminal result, not a Start Error.
    let handle = service
        .start(no_work_request(), None)
        .expect("the run started");
    let terminal = handle.wait();

    assert_eq!(terminal.outcome(), RunOutcome::Failed);
    assert_eq!(terminal.final_phase(), RunPhase::Preparing);
    assert_eq!(terminal.phases().len(), 1);
    let only = terminal.phases()[0];
    assert_eq!(
        (only.phase(), only.status()),
        (RunPhase::SafetyCleanup, RunPhaseStatus::Executed)
    );
    assert_eq!(
        terminal.failures()[0].code,
        RunFailureCode::SchedulingFailed
    );
}

/// Origin: OptimizationRunServiceTests::aSecondActiveRunIsRejectedAcrossServices.
#[test]
fn a_second_active_run_is_rejected_across_services() {
    let _serial = serial();
    let first_service = service_on(Arc::new(DeferredScheduler::default()));
    let second_scheduler = Arc::new(CountingInlineScheduler::default());
    let second_service = service_on(second_scheduler.clone());
    let first = first_service.start(no_work_request(), None).unwrap();

    let second = second_service.start(no_work_request(), None);

    assert_eq!(second.unwrap_err(), StartError::ActiveRun);
    assert_eq!(second_scheduler.scheduled.load(Ordering::SeqCst), 0);
    assert!(first.terminal_result().is_none());
}

/// Origin: OptimizationRunServiceTests::destroyingTheServiceCancelsAndJoinsARetainedHandle.
#[test]
fn dropping_the_service_cancels_and_joins_a_retained_handle() {
    let _serial = serial();
    let scheduler = Arc::new(DeferredScheduler::default());
    let service = service_on(scheduler.clone());
    let handle = service.start(no_work_request(), None).unwrap();

    drop(service);

    assert!(handle.terminal_result().is_some());
    let terminal = handle.wait();
    assert_eq!(terminal.outcome(), RunOutcome::Cancelled);
    assert_eq!(
        terminal.phases().last().unwrap().phase(),
        RunPhase::SafetyCleanup
    );
    assert_eq!(scheduler.joins.load(Ordering::SeqCst), 1);
}

/// Origin: OptimizationRunServiceTests::waitingFromTheWorkerIsDiagnosed.
#[test]
fn waiting_from_the_worker_is_diagnosed() {
    let _serial = serial();
    let scheduler = Arc::new(GatedScheduler::default());
    let service = service_on(scheduler.clone());
    let slot: HandleSlot = Arc::new(OnceLock::new());
    let diagnosed = Arc::new(AtomicBool::new(false));
    {
        let slot = Arc::clone(&slot);
        let diagnosed = Arc::clone(&diagnosed);
        *scheduler.before_work.lock().unwrap() = Some(Box::new(move || {
            let waited = catch_unwind(AssertUnwindSafe(|| slot.get().unwrap().wait()));
            diagnosed.store(waited.is_err(), Ordering::SeqCst);
        }));
    }
    slot.set(service.start(no_work_request(), None).unwrap())
        .unwrap();
    scheduler.release();

    assert_eq!(slot.get().unwrap().wait().outcome(), RunOutcome::Succeeded);
    assert!(diagnosed.load(Ordering::SeqCst));
    *scheduler.before_work.lock().unwrap() = None;
    drop(reclaim(slot));
}

/// Origin: OptimizationRunServiceTests::cancellationKeepsTheSlotUntilTerminalCommit.
#[test]
fn cancellation_keeps_the_slot_until_terminal_commit() {
    let _serial = serial();
    let scheduler = Arc::new(GatedScheduler::default());
    let service = service_on(scheduler.clone());
    let next_service = service_on(Arc::new(CountingInlineScheduler::default()));
    let handle = service.start(no_work_request(), None).unwrap();
    handle.request_cancellation();
    handle.request_cancellation();

    let blocked = next_service.start(no_work_request(), None);
    let before_commit = handle.terminal_result();
    scheduler.release();

    assert_eq!(blocked.unwrap_err(), StartError::ActiveRun);
    assert!(before_commit.is_none());
    let terminal = handle.wait();
    assert_eq!(terminal.outcome(), RunOutcome::Cancelled);
    assert_eq!(terminal.phases().len(), 2);
    assert_eq!(
        terminal.phases().last().unwrap().phase(),
        RunPhase::SafetyCleanup
    );
    let next = next_service
        .start(no_work_request(), None)
        .expect("the slot was released at commit");
    assert_eq!(next.wait().outcome(), RunOutcome::Succeeded);
    handle.request_cancellation();
    assert!(Arc::ptr_eq(&handle.terminal_result().unwrap(), &terminal));
}

/// Origin: OptimizationRunServiceTests::terminalCommitReleasesTheSlotBeforeWorkerJoin.
#[test]
fn terminal_commit_releases_the_slot_before_the_worker_is_joined() {
    let _serial = serial();
    let scheduler = Arc::new(GatedScheduler::default());
    let (committed_tx, committed_rx) = mpsc::channel();
    let (finish_tx, finish_rx) = mpsc::channel::<()>();
    let finish_rx = Mutex::new(finish_rx);
    *scheduler.after_work.lock().unwrap() = Some(Box::new(move || {
        committed_tx.send(()).unwrap();
        // Either the test releases the worker or it has gone and dropped the
        // sender; both mean the worker may return.
        let _ = finish_rx.lock().unwrap().recv();
    }));
    let service = service_on(scheduler.clone());
    let first = service.start(no_work_request(), None).unwrap();
    scheduler.release();
    committed_rx.recv().unwrap();

    // The first worker has not returned yet, but its run has committed.
    let next_service = OptimizationRunService::new(Some(test_configuration()), None);
    let next = next_service.start(no_work_request(), None);
    finish_tx.send(()).unwrap();

    assert!(first.terminal_result().is_some());
    assert_eq!(
        next.expect("the slot is free").wait().outcome(),
        RunOutcome::Succeeded
    );
}

/// Origin: OptimizationRunServiceTests::simultaneousStartsAdmitExactlyOneRun.
#[test]
fn simultaneous_starts_admit_exactly_one_run() {
    let _serial = serial();
    let first_scheduler = Arc::new(GatedScheduler::default());
    let second_scheduler = Arc::new(GatedScheduler::default());
    let first_service = service_on(first_scheduler.clone());
    let second_service = service_on(second_scheduler.clone());
    let together = Barrier::new(2);

    let (first, second) = std::thread::scope(|scope| {
        let first = scope.spawn(|| {
            together.wait();
            first_service.start(no_work_request(), None)
        });
        let second = scope.spawn(|| {
            together.wait();
            second_service.start(no_work_request(), None)
        });
        (first.join().unwrap(), second.join().unwrap())
    });
    first_scheduler.release();
    second_scheduler.release();

    assert_ne!(first.is_ok(), second.is_ok());
    let (admitted, rejected) = if first.is_ok() {
        (first, second)
    } else {
        (second, first)
    };
    assert_eq!(rejected.unwrap_err(), StartError::ActiveRun);
    assert_eq!(admitted.unwrap().wait().outcome(), RunOutcome::Succeeded);
}

/// Origin: OptimizationRunServiceTests::destroyingTheHandleRequestsCancellationBeforeJoin.
#[test]
fn dropping_the_handle_requests_cancellation_before_joining() {
    let _serial = serial();
    let scheduler = Arc::new(DeferredScheduler::default());
    let service = service_on(scheduler.clone());
    let events = EventLog::default();

    // The worker only runs inside the handle's join, so whatever it observed
    // proves cancellation reached the Run Executor before the join.
    drop(
        service
            .start(no_work_request(), Some(events.dispatcher()))
            .unwrap(),
    );

    let terminal = events
        .events()
        .into_iter()
        .find_map(|event| match event.payload {
            RunEventPayload::Terminal(result) => Some(result),
            _ => None,
        })
        .expect("the joined run committed a terminal result");
    assert_eq!(terminal.outcome(), RunOutcome::Cancelled);
    assert_eq!(scheduler.completions.load(Ordering::SeqCst), 1);
}

/// Origin: OptimizationRunServiceTests::schedulingFailureReleasesTheActiveSlot.
#[test]
fn a_scheduling_failure_releases_the_active_slot() {
    let _serial = serial();
    let failed_service = service_on(Arc::new(ExhaustedScheduler));
    let failed = failed_service.start(no_work_request(), None).unwrap();
    let terminal = failed
        .terminal_result()
        .expect("a scheduling failure commits at once");

    let next_service = OptimizationRunService::new(Some(test_configuration()), None);
    let next = next_service
        .start(no_work_request(), None)
        .expect("the slot was released");

    assert_eq!(next.wait().outcome(), RunOutcome::Succeeded);
    assert!(Arc::ptr_eq(&failed.terminal_result().unwrap(), &terminal));
    assert_eq!(terminal.outcome(), RunOutcome::Failed);
}

/// Origin: OptimizationRunServiceTests::destructionFromTheWorkerIsDiagnosed. A
/// fatal contract violation aborts the process, so the scenario runs in a child
/// copy of this test binary and asserts its diagnostic.
#[test]
fn destroying_a_run_from_its_own_worker_is_diagnosed() {
    if let Ok(owner) = std::env::var("CAO_CORE_SELF_DESTRUCTION") {
        self_destruct(&owner);
        return;
    }
    for owner in ["handle", "service", "joining-service"] {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "destroying_a_run_from_its_own_worker_is_diagnosed",
                "--exact",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("CAO_CORE_SELF_DESTRUCTION", owner)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "{owner}: the violation must not return normally"
        );
        assert!(
            stderr.contains("An Optimization Run cannot destroy its own worker"),
            "{owner}: {stderr}"
        );
    }
}

/// The child half of the self-destruction scenario; never returns normally.
fn self_destruct(owner: &str) {
    if owner == "joining-service" {
        // The deferred worker runs on the thread that joins it, so only the
        // run's execution scope, not a worker thread ID, identifies it. The
        // dispatcher holds the service's only owner and drops it mid-run.
        let cell: Arc<Mutex<Option<OptimizationRunService>>> = Arc::new(Mutex::new(None));
        let held = Arc::clone(&cell);
        let dispatcher = Box::new(move |_| drop(held.lock().unwrap().take()));
        let service = service_on(Arc::new(DeferredScheduler::default()));
        let handle = service.start(no_work_request(), Some(dispatcher)).unwrap();
        *cell.lock().unwrap() = Some(service);
        drop(handle);
        return;
    }
    let scheduler = Arc::new(GatedScheduler::default());
    let service = Arc::new(Mutex::new(Some(service_on(scheduler.clone()))));
    let handle: Arc<Mutex<Option<RunHandle>>> = Arc::new(Mutex::new(Some(
        service
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .start(no_work_request(), None)
            .unwrap(),
    )));
    let destroy_handle = owner == "handle";
    let (held_service, held_handle) = (Arc::clone(&service), Arc::clone(&handle));
    *scheduler.before_work.lock().unwrap() = Some(Box::new(move || {
        if destroy_handle {
            drop(held_handle.lock().unwrap().take());
        } else {
            drop(held_service.lock().unwrap().take());
        }
    }));
    scheduler.release();
    std::thread::sleep(std::time::Duration::from_secs(10));
}

/// Origin: OptimizationRunServiceTests::inlineEventsOwnAnOrderedRunHistory.
#[test]
fn events_form_one_ordered_history_ending_in_one_terminal_event() {
    let _serial = serial();
    let events = EventLog::default();
    let run_id = {
        let service = service_on(Arc::new(CountingInlineScheduler::default()));
        let handle = service
            .start(no_work_request(), Some(events.dispatcher()))
            .unwrap();
        assert_eq!(handle.wait().outcome(), RunOutcome::Succeeded);
        handle.run_id().to_owned()
    };

    // Copies keep their payloads after the service, handle and worker are gone.
    let events = events.events();
    assert_eq!(events.len(), 8);
    for (index, event) in events.iter().enumerate() {
        assert_eq!(event.sequence, index as u64 + 1);
        assert_eq!(event.run_id, run_id);
    }
    for (index, phase) in RunPhase::SEQUENCE.into_iter().enumerate() {
        let RunEventPayload::Phase(record) = &events[index].payload else {
            panic!("event {index} is a phase");
        };
        assert_eq!(record.phase(), phase);
        if (1..6).contains(&index) {
            assert_eq!(record.skip_reason(), Some(PhaseSkipReason::NoRequestedWork));
        }
    }
    let RunEventPayload::Terminal(terminal) = &events[7].payload else {
        panic!("the last event is the terminal result");
    };
    assert_eq!(terminal.outcome(), RunOutcome::Succeeded);
    assert_eq!(terminal.run_id(), run_id);
}

/// Origin: OptimizationRunServiceTests::failureEventsPrecedeCleanupAndTerminal.
#[test]
fn failure_events_precede_safety_cleanup_and_the_terminal_event() {
    let _serial = serial();
    for scheduling_fails in [false, true] {
        let scheduler: Arc<dyn cao_core::run::RunScheduler> = if scheduling_fails {
            Arc::new(ExhaustedScheduler)
        } else {
            Arc::new(CountingInlineScheduler::default())
        };
        let service = service_on(scheduler);
        let events = EventLog::default();
        let request = RunRequest::new(
            "SkyrimSE",
            ExecutionMode::Apply,
            ModSelection::SingleModRoot(test_mod_root()),
            vec![RequestedWork::NativeTextureOptimization],
        );
        let handle = service.start(request, Some(events.dispatcher())).unwrap();
        let result = handle.wait();
        let events = events.events();

        assert_eq!(result.outcome(), RunOutcome::Failed);
        assert_eq!(events.len(), if scheduling_fails { 3 } else { 4 });
        let RunEventPayload::Failure(failure) = &events[events.len() - 3].payload else {
            panic!("the failure precedes cleanup");
        };
        let expected = if scheduling_fails {
            RunFailureCode::SchedulingFailed
        } else {
            RunFailureCode::RequestedWorkUnavailable
        };
        assert_eq!(failure.code, expected);
        assert_eq!(failure.phase, RunPhase::Preparing);
        assert!(!failure.detail.is_empty());
        assert_eq!(result.failures(), std::slice::from_ref(failure));
        let RunEventPayload::Phase(cleanup) = &events[events.len() - 2].payload else {
            panic!("Safety Cleanup precedes the terminal event");
        };
        assert_eq!(cleanup.phase(), RunPhase::SafetyCleanup);
        assert!(matches!(
            events.last().unwrap().payload,
            RunEventPayload::Terminal(_)
        ));
    }
}

/// Origin: OptimizationRunServiceTests::failureRetainsObservedCancellationAndOneTerminalEvent.
#[test]
fn a_failure_keeps_cancellation_observed_during_it_and_one_terminal_event() {
    let _serial = serial();
    let scheduler = Arc::new(GatedScheduler::default());
    let provider = Arc::new(CallbackConfiguration(Box::new(|_| {
        Err(Error::ConfigurationLoading(
            "Configuration could not be loaded".to_owned(),
        ))
    })));
    let service = OptimizationRunService::with_scheduler(scheduler.clone(), Some(provider), None);
    let slot: HandleSlot = Arc::new(OnceLock::new());
    let events = EventLog::default();
    let dispatcher = {
        let slot = Arc::clone(&slot);
        let log = events.dispatcher();
        Box::new(move |event: cao_core::run::RunEvent| {
            if matches!(event.payload, RunEventPayload::Failure(_)) {
                slot.get().unwrap().request_cancellation();
            }
            log(event);
        })
    };
    slot.set(service.start(no_work_request(), Some(dispatcher)).unwrap())
        .unwrap();
    scheduler.release();
    let result = slot.get().unwrap().wait();

    assert_eq!(result.outcome(), RunOutcome::Failed);
    assert!(result.cancellation_observed());
    assert_eq!(result.failures().len(), 1);
    assert_eq!(
        result.failures()[0].code,
        RunFailureCode::ConfigurationLoadingFailed
    );
    let terminals: Vec<_> = events
        .events()
        .into_iter()
        .filter_map(|event| match event.payload {
            RunEventPayload::Terminal(terminal) => Some(terminal),
            _ => None,
        })
        .collect();
    assert_eq!(terminals.len(), 1);
    assert!(Arc::ptr_eq(&terminals[0], &result));
    drop(reclaim(slot));
}

/// Origin: OptimizationRunServiceTests::lateCancellationDoesNotChangeCommittedEvidence.
#[test]
fn late_cancellation_does_not_change_the_committed_result() {
    let _serial = serial();
    let service = service_on(Arc::new(CountingInlineScheduler::default()));
    let handle = service.start(no_work_request(), None).unwrap();
    let result = handle.wait();
    assert!(!result.cancellation_observed());

    handle.request_cancellation();

    assert!(handle.snapshot().cancellation_requested);
    assert!(Arc::ptr_eq(&handle.wait(), &result));
    assert_eq!(result.outcome(), RunOutcome::Succeeded);
    assert!(!result.cancellation_observed());
}

/// Origin: OptimizationRunServiceTests::inlineCallbacksObservePublishedStateAndCanCancel.
#[test]
fn the_dispatcher_sees_published_state_and_can_cancel() {
    let _serial = serial();
    let scheduler = Arc::new(GatedScheduler::default());
    let service = service_on(scheduler.clone());
    let slot: HandleSlot = Arc::new(OnceLock::new());
    let phases = Arc::new(Mutex::new(Vec::new()));
    let self_wait_diagnosed = Arc::new(AtomicBool::new(false));
    let state_published = Arc::new(AtomicBool::new(true));
    let dispatcher = {
        let (slot, phases) = (Arc::clone(&slot), Arc::clone(&phases));
        let (self_wait_diagnosed, state_published) = (
            Arc::clone(&self_wait_diagnosed),
            Arc::clone(&state_published),
        );
        Box::new(move |event: cao_core::run::RunEvent| {
            let handle = slot.get().unwrap();
            let snapshot = handle.snapshot();
            let mut published = snapshot.run_id == event.run_id;
            match event.payload {
                RunEventPayload::Phase(record) => {
                    phases.lock().unwrap().push(record.phase());
                    published &= snapshot.phase == record.phase() && snapshot.progress.is_none();
                    if record.phase() == RunPhase::Preparing {
                        let waited = catch_unwind(AssertUnwindSafe(|| handle.wait()));
                        self_wait_diagnosed.store(waited.is_err(), Ordering::SeqCst);
                        handle.request_cancellation();
                        published &= handle.snapshot().cancellation_requested;
                    }
                }
                RunEventPayload::Terminal(_) => {
                    published &= handle.terminal_result().is_some()
                        && snapshot.outcome == Some(RunOutcome::Cancelled);
                }
                // A no-work run publishes no failures or diagnostics to check.
                _ => {}
            }
            if !published {
                state_published.store(false, Ordering::SeqCst);
            }
        })
    };
    slot.set(service.start(no_work_request(), Some(dispatcher)).unwrap())
        .unwrap();
    scheduler.release();

    assert_eq!(slot.get().unwrap().wait().outcome(), RunOutcome::Cancelled);
    assert!(self_wait_diagnosed.load(Ordering::SeqCst));
    assert!(state_published.load(Ordering::SeqCst));
    assert_eq!(
        *phases.lock().unwrap(),
        [RunPhase::Preparing, RunPhase::SafetyCleanup]
    );
    drop(reclaim(slot));
}

/// Origin: OptimizationRunServiceTests::concurrentCancellationAndSnapshotsPreserveRunState.
#[test]
fn concurrent_cancellation_and_snapshots_preserve_run_state() {
    let _serial = serial();
    let scheduler = Arc::new(GatedScheduler::default());
    let service = service_on(scheduler.clone());
    let handle = service.start(no_work_request(), None).unwrap();
    let initial = handle.snapshot();
    let invalid = AtomicBool::new(false);

    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| {
                for _ in 0..100 {
                    handle.request_cancellation();
                    let snapshot = handle.snapshot();
                    if !snapshot.cancellation_requested
                        || snapshot.run_id != initial.run_id
                        || snapshot.phase != RunPhase::Preparing
                        || snapshot.outcome.is_some()
                        || snapshot.diagnostic_count != 0
                        || snapshot.failure_count != 0
                        || handle.terminal_result().is_some()
                    {
                        invalid.store(true, Ordering::SeqCst);
                    }
                }
            });
        }
    });
    let cancelled = handle.snapshot();
    scheduler.release();
    let terminal = handle.wait();

    assert!(!invalid.load(Ordering::SeqCst));
    assert!(!initial.cancellation_requested);
    assert!(cancelled.cancellation_requested && cancelled.outcome.is_none());
    assert_eq!(terminal.outcome(), RunOutcome::Cancelled);
    assert_eq!(terminal.phases().len(), 2);
    assert_eq!(handle.snapshot().phase, RunPhase::SafetyCleanup);
    assert_eq!(handle.snapshot().outcome, Some(RunOutcome::Cancelled));
}

/// Origin: OptimizationRunServiceTests::concurrentReadersObservePublishedPhaseTransitions.
#[test]
fn concurrent_readers_observe_each_published_phase_transition() {
    let _serial = serial();
    let scheduler = Arc::new(GatedScheduler::default());
    let service = service_on(scheduler.clone());
    let phase_published = Arc::new(Barrier::new(4));
    let snapshots_read = Arc::new(Barrier::new(4));
    // Seven phase events, then the terminal event, published at Safety Cleanup.
    let mut expected = RunPhase::SEQUENCE.to_vec();
    expected.push(RunPhase::SafetyCleanup);
    let dispatcher = {
        let (published, read) = (Arc::clone(&phase_published), Arc::clone(&snapshots_read));
        // Let every reader inspect this publication before the worker can
        // publish the next one.
        Box::new(move |_| {
            published.wait();
            read.wait();
        })
    };
    let handle = service.start(no_work_request(), Some(dispatcher)).unwrap();
    let initial = handle.snapshot();
    let invalid = AtomicBool::new(false);

    std::thread::scope(|scope| {
        for _ in 0..3 {
            scope.spawn(|| {
                for (index, phase) in expected.iter().enumerate() {
                    phase_published.wait();
                    let snapshot = handle.snapshot();
                    let outcome = (index == 7).then_some(RunOutcome::Succeeded);
                    if snapshot.run_id != initial.run_id
                        || snapshot.phase != *phase
                        || snapshot.outcome != outcome
                        || snapshot.progress.is_some()
                        || snapshot.cancellation_requested
                        || snapshot.failure_count != 0
                        || snapshot.diagnostic_count != 0
                    {
                        invalid.store(true, Ordering::SeqCst);
                    }
                    snapshots_read.wait();
                }
            });
        }
        scheduler.release();
    });
    let terminal = handle.wait();

    assert!(!invalid.load(Ordering::SeqCst));
    assert_eq!(terminal.outcome(), RunOutcome::Succeeded);
    assert_eq!(initial.phase, RunPhase::Preparing);
    assert!(initial.outcome.is_none());
}

/// Origin: OptimizationRunServiceTests::cancellationFromASkippedPhaseStopsFurtherTraversal.
#[test]
fn cancelling_from_a_skipped_phase_stops_further_traversal() {
    let _serial = serial();
    let scheduler = Arc::new(GatedScheduler::default());
    let service = service_on(scheduler.clone());
    let slot: HandleSlot = Arc::new(OnceLock::new());
    let events = EventLog::default();
    let dispatcher = {
        let (slot, log) = (Arc::clone(&slot), events.dispatcher());
        Box::new(move |event: cao_core::run::RunEvent| {
            if let RunEventPayload::Phase(record) = &event.payload
                && record.phase() == RunPhase::DiscoveringArchives
            {
                slot.get().unwrap().request_cancellation();
            }
            log(event);
        })
    };
    slot.set(service.start(no_work_request(), Some(dispatcher)).unwrap())
        .unwrap();
    scheduler.release();
    let terminal = slot.get().unwrap().wait();

    assert_eq!(terminal.outcome(), RunOutcome::Cancelled);
    assert_eq!(terminal.final_phase(), RunPhase::DiscoveringArchives);
    assert_eq!(
        events.phases(),
        [
            RunPhase::Preparing,
            RunPhase::DiscoveringArchives,
            RunPhase::SafetyCleanup
        ]
    );
    drop(reclaim(slot));
}

/// Origin: OptimizationRunServiceTests::presentationFailuresDisableOnlyTheFailingObserver.
/// With one dispatcher, a panicking dispatcher is disabled and diagnosed while
/// the run's outcome is untouched.
#[test]
fn a_panicking_dispatcher_is_disabled_and_diagnosed_without_changing_the_outcome() {
    let _serial = serial();
    let service = service_on(Arc::new(CountingInlineScheduler::default()));
    let attempts = Arc::new(AtomicUsize::new(0));
    let dispatcher = {
        let attempts = Arc::clone(&attempts);
        Box::new(move |_| {
            attempts.fetch_add(1, Ordering::SeqCst);
            panic!("view unavailable");
        })
    };

    let handle = service.start(no_work_request(), Some(dispatcher)).unwrap();

    assert_eq!(handle.wait().outcome(), RunOutcome::Succeeded);
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        1,
        "a failed dispatcher receives nothing more"
    );
    let diagnostics = handle.diagnostics();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code, RunDiagnosticCode::DispatcherFailed);
    assert!(diagnostics[0].detail.contains("view unavailable"));
}
