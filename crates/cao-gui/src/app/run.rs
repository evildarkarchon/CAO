//! Optimization Runs from the main window (#510): Run and Cancel, the run
//! lock, progress, outcomes, and closing the window during a run (C++
//! `MainWindow::initProcess`, `endProcess`, `cancelRun`, `renderRun` and
//! `closeEvent`).
//!
//! A run's events are dispatched on its Run Worker. Each is posted to the UI
//! thread with `slint::Weak::upgrade_in_event_loop`; an event posted after the
//! event loop has ended is dropped. On the UI thread, [`RunView`] orders them
//! and the window renders the result.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Weak;

use cao_core::run::{
    OptimizationRunService, RunEvent, RunEventDispatcher, RunHandle, RunId, RunOutcome, StartError,
};
use cao_optimizers::composition::{run_request, run_service};
use slint::{ComponentHandle, SharedString};

use super::App;
use crate::desktop::MessageLevel;
use crate::log_tab;
use crate::run_view::RunView;

/// The window's run state.
pub(super) struct Runs {
    /// The production Optimization Run Service for the CAO install the window
    /// works over, built once at startup and serving every run of the session.
    ///
    /// Declared before `active`, so at shutdown it drops first: dropping it
    /// cancels and joins any run it retains (ADR-0001), and the Run Handle
    /// dropped after it finds its run already joined and neither blocks nor
    /// panics. A handle owns its run's state and join obligation rather than
    /// borrowing the service, so either drop order is safe.
    service: OptimizationRunService,
    /// The log file of the profile selected in this session, stamped when it
    /// was selected (C++ `Profiles::logPath`), even if it could not be opened
    /// then. `None` for the startup profile, whose file `main` opened.
    log_path: RefCell<Option<PathBuf>>,
    /// The run in flight, from its start until its terminal result arrives.
    active: RefCell<Option<RunHandle>>,
    /// What the window shows of the latest run.
    view: RefCell<RunView>,
}

impl Runs {
    /// The run state of a window over the CAO install in `app_dir`, which
    /// every run resolves against. No run is active yet.
    pub(super) fn new(app_dir: &Path) -> Self {
        Self {
            service: run_service(app_dir),
            log_path: RefCell::default(),
            active: RefCell::default(),
            view: RefCell::default(),
        }
    }
}

thread_local! {
    /// The window of each run in flight, by run identity, on the UI thread.
    /// A run's events come from its worker and can carry only `Send` values,
    /// so they name their window by run rather than hold it.
    static RUN_WINDOWS: RefCell<HashMap<RunId, Weak<App>>> = RefCell::default();
}

impl App {
    /// Remembers `path`, the log file just stamped for a newly selected
    /// profile, as the file this profile session's runs log to.
    pub(super) fn remember_run_log(&self, path: PathBuf) {
        *self.runs.log_path.borrow_mut() = Some(path);
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

    /// Saves the window, redirects the log, builds the run's Run Request and
    /// starts it on the window's service, and locks the controls. Each refusal
    /// is shown and leaves the window as it was.
    fn start_run(&self) {
        // Settles the options, the profile and the debug-log choice the run
        // and its log use. The run reads its settings from `profile.ini`, so
        // when they could not be saved it would run on stale ones; the save
        // has already shown why (#545).
        if !self.save() {
            return;
        }
        let (profile, options) = {
            let loaded = self.loaded.borrow();
            (loaded.profile.name().to_owned(), loaded.options.clone())
        };

        // The profile session's file, at the run's debug choice (C++
        // `applyRunLogging`). It is normally the file the log already writes
        // to; it differs only when the profile's stamp could not be opened,
        // and then the run must not start without it.
        if let Some(log) = self.application_log() {
            let path = self.runs.log_path.borrow().clone();
            let path = path.unwrap_or_else(|| log.path());
            if let Err(error) = log.redirect(&path, options.debug_log) {
                self.error(
                    "Error",
                    &format!("The log file for this run could not be opened: {error}"),
                );
                return;
            }
        }

        let request = match run_request(&profile, &options) {
            Ok(request) => request,
            Err(error) => {
                self.error("Start Error", &error.to_string());
                return;
            }
        };
        let handle = match self
            .runs
            .service
            .start(request, Some(self.run_dispatcher()))
        {
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
        *self.runs.active.borrow_mut() = Some(handle);
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
            let Some(handle) = active.as_ref() else {
                return;
            };
            if !view.state().active {
                return;
            }
            handle.request_cancellation();
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
        if let Some(handle) = self.runs.active.borrow_mut().take() {
            RUN_WINDOWS.with(|windows| windows.borrow_mut().remove(handle.run_id()));
            // The worker has dispatched its last event, so this join is brief.
            drop(handle);
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
        log_tab::show_details(ui, &view.state().details);
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
