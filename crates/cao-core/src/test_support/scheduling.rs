//! Run Scheduler fakes.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use crate::Error;
use crate::run::{RunScheduler, RunWork, ScheduledRunWorker, StandardRunScheduler};

/// Runs each worker inline, counting how many workers it was asked to start.
#[derive(Default)]
pub struct CountingInlineScheduler {
    pub scheduled: AtomicUsize,
}

struct CompletedWorker;

impl ScheduledRunWorker for CompletedWorker {
    // The work ran inline before `schedule` returned; nothing is left to join.
    fn join(&self) {}
    fn is_current_thread(&self) -> bool {
        false
    }
}

impl RunScheduler for CountingInlineScheduler {
    fn schedule(&self, work: RunWork) -> Result<Box<dyn ScheduledRunWorker>, Error> {
        self.scheduled.fetch_add(1, Ordering::SeqCst);
        work();
        Ok(Box::new(CompletedWorker))
    }
}

/// Holds each worker's work until it is joined, and never runs it otherwise.
///
/// A real worker would join itself as a last resort, which would hide a
/// missing join in the Run Handle; this one makes an abandoned run visible as
/// work that never completed.
#[derive(Default)]
pub struct DeferredScheduler {
    pub joins: Arc<AtomicUsize>,
    pub completions: Arc<AtomicUsize>,
}

struct DeferredWorker {
    work: Mutex<Option<RunWork>>,
    joins: Arc<AtomicUsize>,
    completions: Arc<AtomicUsize>,
}

impl ScheduledRunWorker for DeferredWorker {
    fn join(&self) {
        self.joins.fetch_add(1, Ordering::SeqCst);
        let work = self.work.lock().unwrap().take();
        if let Some(work) = work {
            work();
            self.completions.fetch_add(1, Ordering::SeqCst);
        }
    }

    // No execution thread exists until this deterministic worker is joined.
    fn is_current_thread(&self) -> bool {
        false
    }
}

impl RunScheduler for DeferredScheduler {
    fn schedule(&self, work: RunWork) -> Result<Box<dyn ScheduledRunWorker>, Error> {
        Ok(Box::new(DeferredWorker {
            work: Mutex::new(Some(work)),
            joins: Arc::clone(&self.joins),
            completions: Arc::clone(&self.completions),
        }))
    }
}

/// Fails every scheduling attempt, as a thread-backed scheduler out of resources would.
pub struct ExhaustedScheduler;

impl RunScheduler for ExhaustedScheduler {
    fn schedule(&self, _work: RunWork) -> Result<Box<dyn ScheduledRunWorker>, Error> {
        Err(Error::Scheduling(
            "No Run Worker could be started".to_owned(),
        ))
    }
}

/// A hook a gated worker runs on its own thread.
pub type WorkerHook = Arc<Mutex<Option<Box<dyn FnMut() + Send>>>>;

/// Runs the production worker, holding it until `release` so a test can
/// inspect or cancel a run it knows is active.
pub struct GatedScheduler {
    release: Mutex<Option<mpsc::Sender<()>>>,
    gate: Mutex<Option<mpsc::Receiver<()>>>,
    pub before_work: WorkerHook,
    pub after_work: WorkerHook,
}

impl Default for GatedScheduler {
    fn default() -> Self {
        let (release, gate) = mpsc::channel();
        Self {
            release: Mutex::new(Some(release)),
            gate: Mutex::new(Some(gate)),
            before_work: Arc::default(),
            after_work: Arc::default(),
        }
    }
}

impl GatedScheduler {
    /// Lets the held worker run.
    pub fn release(&self) {
        if let Some(release) = self.release.lock().unwrap().take() {
            // The worker may already be gone (a scheduling failure never
            // started it); then there is nothing to release.
            let _ = release.send(());
        }
    }
}

impl RunScheduler for GatedScheduler {
    fn schedule(&self, work: RunWork) -> Result<Box<dyn ScheduledRunWorker>, Error> {
        let gate = self
            .gate
            .lock()
            .unwrap()
            .take()
            .expect("a gated scheduler starts one run");
        let before = Arc::clone(&self.before_work);
        let after = Arc::clone(&self.after_work);
        StandardRunScheduler.schedule(Box::new(move || {
            // A dropped scheduler disconnects the gate, which releases the
            // worker just as `release` would.
            let _ = gate.recv();
            if let Some(hook) = before.lock().unwrap().as_mut() {
                hook();
            }
            work();
            if let Some(hook) = after.lock().unwrap().as_mut() {
                hook();
            }
        }))
    }
}
