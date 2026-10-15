//! Optimization Runs from the main window (#510): Run and Cancel, the run
//! lock, progress, outcomes, and closing the window during a run (C++
//! `MainWindow::initProcess`, `endProcess`, `cancelRun`, `renderRun` and
//! `closeEvent`).
//!
//! A run's events are dispatched on its Run Worker. Each is posted to the UI
//! thread with `slint::Weak::upgrade_in_event_loop`; an event posted after the
//! event loop has ended is dropped. On the UI thread, [`RunView`] orders them
//! and the window renders the result.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::{Rc, Weak};

use cao_core::run::{RunEvent, RunEventDispatcher, RunHandle, RunId, RunOutcome, StartError};
use cao_optimizers::application_log::{ApplicationLog, stamp_log_path};
use cao_optimizers::composition::ApplicationRun;
use slint::{ComponentHandle, Model, SharedString, VecModel};

use super::App;
use crate::desktop::MessageLevel;
use crate::run_view::RunView;

/// The window's run state.
#[derive(Default)]
pub(super) struct Runs {
    /// The Application Log each run start redirects; `None` until `main`
    /// hands it over (tests may run without one).
    log: Cell<Option<&'static ApplicationLog>>,
    /// The selected profile's log file, stamped when the profile was selected
    /// (C++ `Profiles::logPath`). Each run start redirects the log to it.
    log_path: RefCell<PathBuf>,
    /// The run in flight, from its start until its terminal result arrives.
    active: RefCell<Option<ActiveRun>>,
    /// What the window shows of the latest run.
    view: RefCell<RunView>,
    /// The latest run's detail rows, for the Log tab (#511 shows them).
    pub(super) details: Rc<VecModel<SharedString>>,
}

/// A started run and the composition root that wired it.
struct ActiveRun {
    // Declared first, so it drops first: dropping the handle cancels the run
    // and joins its worker before the service that owns the wiring goes.
    handle: RunHandle,
    _setup: ApplicationRun,
}

thread_local! {
    /// The window of each run in flight, by run identity, on the UI thread.
    /// A run's events come from its worker and can carry only `Send` values,
    /// so they name their window by run rather than hold it.
    static RUN_WINDOWS: RefCell<HashMap<RunId, Weak<App>>> = RefCell::default();
}

impl App {
    /// Hands the window the process's Application Log, which each run start
    /// then points at the selected profile's log file and debug choice.
    pub fn use_application_log(&self, log: &'static ApplicationLog) {
        self.runs.log.set(Some(log));
    }

    /// Stamps the log file of profile `name`, selected now:
    /// `logs/<name>/yy.MM.dd.hh.mm.html` in the app directory. Runs of this
    /// profile session log to it.
    pub(super) fn stamp_run_log(&self, name: &str) {
        *self.runs.log_path.borrow_mut() = stamp_log_path(&self.app_dir, name);
    }

    /// Run, or Cancel while a run is active (C++ `initProcess`).
    pub(super) fn run_clicked(&self) {
        let (active, can_start) = {
            let view = self.runs.view.borrow();
            (view.state().active, view.can_start())
        };
        if active {
            self.cancel_run();
        } else if can_start {
            self.start_run();
        }
    }

    /// Saves the window, redirects the log, sets up and starts the run, and
    /// locks the controls. Each refusal is shown and leaves the window as it
    /// was.
    fn start_run(&self) {
        // Settles the options, the profile and the debug-log choice the run
        // and its log use.
        self.save();
        let (profile, options) = {
            let loaded = self.loaded.borrow();
            (loaded.profile.name().to_owned(), loaded.options.clone())
        };

        if let Some(log) = self.runs.log.get() {
            let path = self.runs.log_path.borrow().clone();
            if let Err(error) = log.redirect(&path, options.debug_log) {
                self.error(
                    "Error",
                    &format!("The log file for this run could not be opened: {error}"),
                );
                return;
            }
        }

        let setup = match ApplicationRun::new(&self.app_dir, &profile, &options) {
            Ok(setup) => setup,
            Err(error) => {
                self.error("Start Error", &error.to_string());
                return;
            }
        };
        let handle = match setup.start(Some(self.run_dispatcher())) {
            Ok(handle) => handle,
            Err(error) => {
                self.error("Start Error", &start_error_message(error));
                return;
            }
        };

        // Events are only ever queued to this thread, so the run is
        // registered before its first event can be delivered.
        let run_id = handle.run_id().to_owned();
        RUN_WINDOWS.with(|windows| windows.borrow_mut().insert(run_id.clone(), self.weak()));
        self.runs.view.borrow_mut().begin(run_id);
        self.runs.details.set_vec(Vec::new());
        *self.runs.active.borrow_mut() = Some(ActiveRun {
            handle,
            _setup: setup,
        });
        self.locked.set(true);
        self.window.set_cancelling(false);
        self.window.set_running(true);
        self.render_run();
    }

    /// The dispatcher handed to the run: it runs on the Run Worker and posts
    /// each event to the UI thread.
    fn run_dispatcher(&self) -> RunEventDispatcher {
        let window = self.window.as_weak();
        Box::new(move |event: RunEvent| {
            // Fails only once the event loop has ended or the window is gone;
            // nothing is left to show the event then, so it is dropped.
            let _ = window.upgrade_in_event_loop(move |_| deliver(event));
        })
    }

    /// Requests cancellation of the active run, and disables Cancel (C++
    /// `cancelRun`). Idempotent.
    fn cancel_run(&self) {
        {
            let active = self.runs.active.borrow();
            let mut view = self.runs.view.borrow_mut();
            let Some(run) = active.as_ref() else {
                return;
            };
            if !view.state().active {
                return;
            }
            run.handle.request_cancellation();
            view.request_cancellation();
        }
        self.window.set_cancelling(true);
        self.render_run();
    }

    /// Takes in one of the active run's events on the UI thread, and ends the
    /// run when it is the terminal one.
    fn on_run_event(&self, event: &RunEvent) {
        let ended = {
            let mut view = self.runs.view.borrow_mut();
            let was_active = view.state().active;
            if !view.consume(event) {
                return;
            }
            was_active && view.state().outcome.is_some()
        };
        self.render_run();
        if ended {
            self.end_run();
        }
    }

    /// The terminal result has arrived (C++ `endProcess`): Safety Cleanup has
    /// finished. Either closes the window a close was waiting on, or unlocks
    /// the controls and shows the Run Outcome.
    fn end_run(&self) {
        if let Some(run) = self.runs.active.borrow_mut().take() {
            RUN_WINDOWS.with(|windows| windows.borrow_mut().remove(run.handle.run_id()));
            // The worker has dispatched its last event, so this join is brief.
            drop(run);
        }

        if self.runs.view.borrow().state().close_requested {
            // The controls stay locked, so nothing can start another run or
            // change what it depends on before the window is gone.
            self.close_window();
            return;
        }

        self.window.set_running(false);
        self.window.set_cancelling(false);
        self.locked.set(false);
        self.save();
        self.show_outcome();
    }

    /// Shows the latest run's progress and detail rows (C++ `renderRun`): the
    /// same text in the progress bar and the status bar.
    fn render_run(&self) {
        let view = self.runs.view.borrow();
        let ui = &self.window;
        let text = SharedString::from(view.text());
        ui.set_progress_indeterminate(view.indeterminate());
        ui.set_progress(view.fraction());
        ui.set_progress_text(text.clone());
        ui.set_status_text(text);
        // Rows only grow during a run; `start_run` clears them.
        let details = &view.state().details;
        for row in details.iter().skip(self.runs.details.row_count()) {
            self.runs.details.push(row.into());
        }
    }

    /// A message box naming the Run Outcome, over the window.
    fn show_outcome(&self) {
        let (level, text) = {
            let view = self.runs.view.borrow();
            let level = match view.state().outcome {
                Some(RunOutcome::Succeeded) | None => MessageLevel::Info,
                Some(RunOutcome::CompletedWithFailures | RunOutcome::Cancelled) => {
                    MessageLevel::Warning
                }
                Some(RunOutcome::Failed) => MessageLevel::Error,
            };
            (level, view.text())
        };
        self.desktop.message(
            Some(self.window.window()),
            level,
            "Optimization Run",
            &text,
            Box::new(|| {}),
        );
    }

    /// Whether the window may close now (C++ `closeEvent`). During a run it
    /// cancels the run and returns `false`; the window then closes once the
    /// run reports its outcome.
    pub(super) fn may_close(&self) -> bool {
        if self.runs.view.borrow_mut().request_close() {
            return true;
        }
        self.cancel_run();
        false
    }

    /// Closes the window as its title-bar button would once no run holds it.
    fn close_window(&self) {
        self.save();
        // Fails only without a real window or event loop (headless), where
        // hiding and quitting have nothing more to do.
        let _ = self.window.hide();
        let _ = slint::quit_event_loop();
    }
}

/// Hands a run's event to its window on the UI thread. An event of a run no
/// window is waiting for (it has ended, or the window is gone) is dropped.
fn deliver(event: RunEvent) {
    let app =
        RUN_WINDOWS.with(|windows| windows.borrow().get(&event.run_id).and_then(Weak::upgrade));
    if let Some(app) = app {
        app.on_run_event(&event);
    }
}

/// The message for a run the service refused to start.
fn start_error_message(error: StartError) -> String {
    let reason = match error {
        StartError::MissingProfileIdentity => "no profile is selected",
        StartError::MissingModSelectionDirectory => "no mod folder is selected",
        StartError::ActiveRun => "another run is still active",
    };
    format!("The run could not start: {reason}.")
}
