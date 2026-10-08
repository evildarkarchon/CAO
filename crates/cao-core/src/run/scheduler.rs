//! The Run Scheduler seam and its production and inline schedulers.
//!
//! Ported from `src/Run/RunScheduler.h`. The scheduler starts the single Run
//! Worker of one run; the run's lifetime owns that worker and joins it before
//! the run could be abandoned (ADR-0001).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{JoinHandle, ThreadId};

use crate::Error;

/// The work a scheduler starts: one run, to its terminal result.
pub type RunWork = Box<dyn FnOnce() + Send + 'static>;

/// The single worker one Run Scheduler started for one Optimization Run.
pub trait ScheduledRunWorker: Send + Sync {
    /// Blocks until the scheduled work has finished. Idempotent; the run
    /// serializes callers. Joining from the worker itself is a contract
    /// violation.
    fn join(&self);

    /// Reports whether the caller is running on this worker. It must not wait
    /// for the worker, because the run checks it to diagnose self-waits without
    /// deadlocking.
    fn is_current_thread(&self) -> bool;
}

/// Starts the single Run Worker of one Optimization Run.
///
/// The injectable scheduling seam: it keeps scheduling out of presentation code
/// and lets tests make scheduling deterministic.
pub trait RunScheduler: Send + Sync {
    /// Starts `work` and returns the joinable worker running it.
    ///
    /// An implementation must either return a worker or fail before invoking
    /// `work`; the service turns that failure into the started run's Failed
    /// result, never a Start Error.
    fn schedule(&self, work: RunWork) -> Result<Box<dyn ScheduledRunWorker>, Error>;
}

/// Runs each Optimization Run on one owned `std::thread`; there is no async runtime.
#[derive(Debug, Default, Clone, Copy)]
pub struct StandardRunScheduler;

/// A production Run Worker: one thread, joined defensively when released.
struct StandardRunWorker {
    thread: std::sync::Mutex<Option<JoinHandle<()>>>,
    thread_id: ThreadId,
    finished: Arc<AtomicBool>,
}

impl ScheduledRunWorker for StandardRunWorker {
    fn join(&self) {
        if self.is_current_thread() {
            // Logged, and also written straight to stderr, because a log sink
            // may never flush before the abort.
            log::error!("An Optimization Run cannot join or destroy its own worker");
            eprintln!("An Optimization Run cannot join or destroy its own worker");
            std::process::abort();
        }
        let handle = self
            .thread
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(handle) = handle {
            // The worker converts every run panic into a terminal result; a
            // panic escaping it is already reported by the panic hook.
            let _ = handle.join();
        }
    }

    fn is_current_thread(&self) -> bool {
        // An exited thread's ID may be reused, so a finished worker is never current.
        !self.finished.load(Ordering::Acquire) && self.thread_id == std::thread::current().id()
    }
}

impl Drop for StandardRunWorker {
    /// Joins defensively, even when something outside the run releases the worker.
    fn drop(&mut self) {
        self.join();
    }
}

impl RunScheduler for StandardRunScheduler {
    fn schedule(&self, work: RunWork) -> Result<Box<dyn ScheduledRunWorker>, Error> {
        let finished = Arc::new(AtomicBool::new(false));
        let thread_finished = Arc::clone(&finished);
        let handle = std::thread::Builder::new()
            .name("cao-run-worker".to_owned())
            .spawn(move || {
                // Marks completion even if `work` unwinds, so a reused thread ID
                // can never be mistaken for this worker.
                struct Finished(Arc<AtomicBool>);
                impl Drop for Finished {
                    fn drop(&mut self) {
                        self.0.store(true, Ordering::Release);
                    }
                }
                let _finished = Finished(thread_finished);
                work();
            })
            .map_err(|error| {
                Error::Scheduling(format!("No Run Worker could be started: {error}"))
            })?;
        let thread_id = handle.thread().id();
        Ok(Box::new(StandardRunWorker {
            thread: std::sync::Mutex::new(Some(handle)),
            thread_id,
            finished,
        }))
    }
}

/// Runs each worker inline before scheduling returns, so the run is already
/// terminal when its Run Handle exists. Useful for deterministic callers.
#[derive(Debug, Default, Clone, Copy)]
pub struct InlineRunScheduler;

/// A worker whose work already ran, so there is nothing left to join.
struct CompletedRunWorker;

impl ScheduledRunWorker for CompletedRunWorker {
    // The inline scheduler finished the work before returning this worker.
    fn join(&self) {}

    fn is_current_thread(&self) -> bool {
        false
    }
}

impl RunScheduler for InlineRunScheduler {
    fn schedule(&self, work: RunWork) -> Result<Box<dyn ScheduledRunWorker>, Error> {
        work();
        Ok(Box::new(CompletedRunWorker))
    }
}
