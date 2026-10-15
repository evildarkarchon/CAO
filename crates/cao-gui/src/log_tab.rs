//! The Log tab over the process's Application Log (#511).
//!
//! The sink's [`LogFeed`] queues plain rows from whichever thread logged and
//! wakes the tab once each time its queue goes from empty to non-empty; the
//! wake schedules one drain on the event loop, which moves the rows into the
//! `LogTab.rows` model. Attaching replays the session's rows from bootstrap
//! onward, so the tab shows startup records before any run and is never cut to
//! the current rotation chunk (deviation 24).
//!
//! The model holds the log's rows (at most [`MAX_FEED_ROWS`], newest kept),
//! then the run-detail rows (#510), which are GUI-only and never dropped.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, Weak};

use cao_gui_ui::{LogRow, LogSeverity, LogTab, MainWindow};
use cao_optimizers::application_log::{
    self, ApplicationLog, FeedUpdate, LogError, LogFeed, LogLine, MAX_FEED_ROWS,
};
use slint::{ComponentHandle, Model, ModelRc, VecModel};

/// The window's end of the Application Log: the sink, the install it stamps
/// paths under, and the feed the Log tab drains.
pub(crate) struct AttachedLog {
    sink: &'static ApplicationLog,
    app_dir: PathBuf,
    /// The feed, which the sink's wake callback reaches weakly: the sink keeps
    /// that callback for good, and a strong reference there would keep the feed
    /// (and through it the callback) alive in a cycle after the window is gone.
    /// A `OnceLock` because `subscribe` calls the wake before it returns the
    /// feed; the drain it schedules runs later, on the event loop, by when the
    /// feed is set.
    #[expect(
        dead_code,
        reason = "held only to keep the feed alive; the wake callback reads it through its weak reference"
    )]
    feed: Arc<OnceLock<LogFeed>>,
}

impl AttachedLog {
    /// Subscribes the Log tab of `window` to `sink`, which replays the session's
    /// rows on the next event-loop turn. `app_dir` is the install the stamped
    /// log paths go under.
    pub(crate) fn attach(
        window: &MainWindow,
        sink: &'static ApplicationLog,
        app_dir: &Path,
    ) -> Self {
        let feed = Arc::new(OnceLock::new());
        let wake = {
            let window = window.as_weak();
            let feed = Arc::downgrade(&feed);
            move || {
                let (window, feed) = (window.clone(), feed.clone());
                // Fails only once the event loop is gone (or, headless, was never
                // there); nothing could show the rows then.
                let _ = slint::invoke_from_event_loop(move || drain(&window, &feed));
            }
        };
        // Set at once: nothing can drain before this turn returns to the loop.
        let _ = feed.set(sink.subscribe(wake));
        Self {
            sink,
            app_dir: app_dir.to_owned(),
            feed,
        }
    }

    /// The sink, for a run start's redirect (#510).
    pub(crate) fn sink(&self) -> &'static ApplicationLog {
        self.sink
    }

    /// Stamps `logs/<profile>/yy.MM.dd.hh.mm.html` for a profile selected now
    /// (C++ `Profiles::loadProfile`). [`AttachedLog::stamp`] moves the log
    /// there; runs of the profile session log to it too (#510).
    pub(crate) fn stamp_path(&self, profile: &str) -> PathBuf {
        application_log::stamp_log_path(&self.app_dir, profile)
    }

    /// Moves the log to `path`, stamped by [`AttachedLog::stamp_path`] for a
    /// newly selected profile, at that profile's debug choice. The tab then
    /// clears and follows the new file.
    ///
    /// # Errors
    /// [`LogError::Open`] when the file cannot be opened; the log keeps writing
    /// to the old one.
    pub(crate) fn stamp(&self, path: &Path, debug: bool) -> Result<(), LogError> {
        self.sink.redirect(path, debug)
    }

    /// Switches debug logging on or off in the current file: the level and
    /// layout change, the file and the tab's rows stay (deviation 23).
    ///
    /// # Errors
    /// None in practice: the file is not reopened.
    pub(crate) fn set_debug(&self, debug: bool) -> Result<(), LogError> {
        self.sink.redirect(&self.sink.path(), debug)
    }

    /// The file the log is writing to now, for "Open log file".
    pub(crate) fn path(&self) -> PathBuf {
        self.sink.path()
    }
}

/// Moves the feed's queued rows into the window's Log tab, on the UI thread.
/// Does nothing once the window or the feed is gone.
fn drain(window: &slint::Weak<MainWindow>, feed: &Weak<OnceLock<LogFeed>>) {
    let (Some(window), Some(feed)) = (window.upgrade(), feed.upgrade()) else {
        return;
    };
    let Some(feed) = feed.get() else {
        return;
    };
    let update = feed.drain();
    with_rows(&window, |rows| apply(rows, update));
}

/// Runs `action` over the Log tab's model, first installing a [`VecModel`] in
/// place of the empty default.
fn with_rows<R>(window: &MainWindow, action: impl FnOnce(&VecModel<LogRow>) -> R) -> R {
    let tab = window.global::<LogTab>();
    let mut model = tab.get_rows();
    if model.as_any().downcast_ref::<VecModel<LogRow>>().is_none() {
        model = ModelRc::new(VecModel::<LogRow>::default());
        tab.set_rows(model.clone());
    }
    let rows = model
        .as_any()
        .downcast_ref::<VecModel<LogRow>>()
        .expect("the Log tab's model was just made a VecModel");
    action(rows)
}

/// Applies one drain: a reset drops the log's rows first; the new rows go after
/// the log's rows and before the run-detail rows; then the oldest log rows go
/// beyond [`MAX_FEED_ROWS`].
fn apply(rows: &VecModel<LogRow>, update: FeedUpdate) {
    let total = rows.row_count();
    let logged = total - trailing_details(rows);
    let new: Vec<LogRow> = update.lines.into_iter().map(row).collect();

    if !update.reset && logged + new.len() <= MAX_FEED_ROWS {
        for (offset, line) in new.into_iter().enumerate() {
            rows.insert(logged + offset, line);
        }
        return;
    }

    // A reset, or past the cap: rebuild once rather than shift up to 10,000
    // rows out of the front of the model one at a time.
    let new = &new[new.len().saturating_sub(MAX_FEED_ROWS)..];
    let kept = if update.reset {
        0
    } else {
        logged.min(MAX_FEED_ROWS - new.len())
    };
    let mut all: Vec<LogRow> = (logged - kept..logged)
        .filter_map(|row| rows.row_data(row))
        .collect();
    all.extend_from_slice(new);
    all.extend((logged..total).filter_map(|row| rows.row_data(row)));
    rows.set_vec(all);
}

/// Shows the latest run's detail rows pinned below the Log tab's log rows,
/// replacing the previous run's. Rows only grow during a run, so only the new
/// ones are appended; a run that has just begun has none, which clears the
/// previous run's.
pub(crate) fn show_details(window: &MainWindow, details: &[String]) {
    with_rows(window, |rows| {
        let total = rows.row_count();
        let mut shown = trailing_details(rows);
        if shown > details.len() {
            for row in (total - shown..total).rev() {
                rows.remove(row);
            }
            shown = 0;
        }
        for detail in &details[shown..] {
            rows.push(LogRow {
                text: detail.as_str().into(),
                severity: LogSeverity::Detail,
            });
        }
    });
}

/// How many run-detail rows end the model: they always come after every log
/// row.
fn trailing_details(rows: &VecModel<LogRow>) -> usize {
    (0..rows.row_count())
        .rev()
        .take_while(|&row| {
            rows.row_data(row)
                .is_some_and(|row| row.severity == LogSeverity::Detail)
        })
        .count()
}

/// A sink row as the Log tab shows it.
fn row(line: LogLine) -> LogRow {
    LogRow {
        text: line.text.into(),
        severity: match line.severity {
            log::Level::Error => LogSeverity::Error,
            log::Level::Warn => LogSeverity::Warn,
            log::Level::Info => LogSeverity::Info,
            log::Level::Debug => LogSeverity::Debug,
            log::Level::Trace => LogSeverity::Trace,
        },
    }
}
