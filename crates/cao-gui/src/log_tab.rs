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
//! then the run-detail rows (#510), which are never dropped. `LogTab.detail-count`
//! says how many rows end the model, so neither side scans it to find the
//! boundary (GUI-13). At a run's end its detail rows are written to the
//! Application Log ([`AttachedLog::write_details`]) and come back as log rows.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, Weak};

use cao_gui_ui::{LogRow, LogSeverity, LogTab, MainWindow};
use cao_optimizers::application_log::{
    self, ApplicationLog, FeedUpdate, LogError, LogFeed, LogLine, MAX_FEED_ROWS,
};
use log::Log as _;
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

    /// Writes an ended run's detail rows to the Application Log, one record
    /// each, at the level [`detail_level`](crate::run_view::detail_level)
    /// gives the row, so they can be copied from the file and are still there
    /// after the next run starts (GUI-12). The feed then brings them back to the
    /// Log tab as log rows.
    ///
    /// The rows go straight to the attached sink, which in the app is also the
    /// `log` facade's: a sink attached without being installed (as in tests)
    /// still receives them, so the tab never loses them.
    pub(crate) fn write_details(&self, details: &[String]) {
        for detail in details {
            self.sink.log(
                &log::Record::builder()
                    .args(format_args!("{detail}"))
                    .level(crate::run_view::detail_level(detail))
                    .target(module_path!())
                    .build(),
            );
        }
        self.sink.flush();
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
    with_rows(&window, |rows, details| apply(rows, update, details));
}

/// Runs `action` over the Log tab's model and the number of run-detail rows
/// that end it, first installing a [`VecModel`] in place of the empty default.
fn with_rows<R>(window: &MainWindow, action: impl FnOnce(&VecModel<LogRow>, usize) -> R) -> R {
    let tab = window.global::<LogTab>();
    let mut model = tab.get_rows();
    if model.as_any().downcast_ref::<VecModel<LogRow>>().is_none() {
        model = ModelRc::new(VecModel::<LogRow>::default());
        tab.set_rows(model.clone());
        // The new model holds no rows at all, detail rows included.
        tab.set_detail_count(0);
    }
    let rows = model
        .as_any()
        .downcast_ref::<VecModel<LogRow>>()
        .expect("the Log tab's model was just made a VecModel");
    // Clamped, so a count that ever ran ahead of the model cannot underflow.
    let details = usize::try_from(tab.get_detail_count())
        .unwrap_or_default()
        .min(rows.row_count());
    action(rows, details)
}

/// Applies one drain to `rows`, which end in `details` run-detail rows: a
/// reset drops the log's rows first; the new rows go after the log's rows and
/// before the run-detail rows; then the oldest log rows go beyond
/// [`MAX_FEED_ROWS`].
fn apply(rows: &VecModel<LogRow>, update: FeedUpdate, details: usize) {
    let total = rows.row_count();
    let logged = total - details;
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
/// ones are appended, all in one model change, which matters for the many rows
/// a terminal result brings at once; a run that has just begun has none, which
/// clears the previous run's.
pub(crate) fn show_details(window: &MainWindow, details: &[String]) {
    with_rows(window, |rows, mut shown| {
        if shown > details.len() {
            drop_details(rows, shown);
            shown = 0;
        }
        rows.extend(details[shown..].iter().map(|detail| LogRow {
            text: detail.as_str().into(),
            severity: LogSeverity::Detail,
        }));
        set_detail_count(window, details.len());
    });
}

/// Removes the pinned run-detail rows, once they have been written to the
/// Application Log and come back as log rows (GUI-12).
pub(crate) fn clear_details(window: &MainWindow) {
    with_rows(window, drop_details);
    set_detail_count(window, 0);
}

/// Removes the last `count` rows of `rows`, the run-detail rows.
fn drop_details(rows: &VecModel<LogRow>, count: usize) {
    let total = rows.row_count();
    match count {
        0 => {}
        // One removal is a single model change; more are one rebuild rather
        // than a change per row.
        1 => {
            rows.remove(total - 1);
        }
        _ => {
            let kept: Vec<LogRow> = (0..total - count)
                .filter_map(|row| rows.row_data(row))
                .collect();
            rows.set_vec(kept);
        }
    }
}

/// Records how many run-detail rows end the Log tab's model.
fn set_detail_count(window: &MainWindow, count: usize) {
    // The model never holds anywhere near `i32::MAX` rows.
    let count = i32::try_from(count).unwrap_or(i32::MAX);
    window.global::<LogTab>().set_detail_count(count);
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
