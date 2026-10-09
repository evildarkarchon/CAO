//! The Application Log: the `log` facade's sink for both binaries (#470).
//!
//! It keeps plog's file: the per-profile `logs/<profile>/yy.MM.dd.hh.mm.html`
//! path, a UTF-8 BOM and `<style>` header written only into an empty file, one
//! `<br><font color=…>` record per write in the info or debug layout, and
//! rotation into `name.1.html` … `name.999.html` once the live file passes
//! [`MAX_FILE_SIZE`]. Deviations: the debug layout names `{module::path@line}`
//! because `log` has no function name, `log` has no fatal level, the text is
//! HTML-escaped, and each record ends with `\n` (deviation 22). One handle serves
//! both layouts, so toggling debug logging between runs keeps logging
//! (deviation 23).
//!
//! `main` bootstraps the sink with [`ApplicationLog::open`] and
//! [`ApplicationLog::install`]; each run start calls [`ApplicationLog::redirect`]
//! before starting the run, and refuses to start it when that fails.
//!
//! Each record is formatted once: the HTML goes to the file and a plain
//! [`LogLine`] goes to the Log tab through a [`LogFeed`], Slint-agnostic, which
//! holds the session's newest [`MAX_FEED_ROWS`] rows from bootstrap onward
//! (deviation 24).

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::fs::{File, OpenOptions};
use std::io::{self, Write as _};
use std::mem;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use chrono::NaiveDateTime;
use log::{Level, LevelFilter, Log, Metadata, Record};

/// The live file rolls before the first write that finds it larger than this.
pub const MAX_FILE_SIZE: u64 = 250_000;
/// The live file plus `name.1.html` … `name.999.html`.
pub const MAX_FILES: u32 = 1_000;
/// The Log tab rows a [`LogFeed`] replays and queues, newest kept.
pub const MAX_FEED_ROWS: usize = 10_000;

/// plog's UTF-8 BOM and spacing header, written only into an empty file.
const HEADER: &str = "\u{feff}<style>html{line-height:1.5rem}pre{line-height:1rem}</style>";

/// The local wall-clock time a record is stamped with.
pub type Clock = fn() -> NaiveDateTime;

/// The Application Log's path for a profile selected at `at`:
/// `<app_dir>/logs/<profile>/yy.MM.dd.hh.mm.html`. All runs of that profile
/// session append to it; two launches in one minute share it.
pub fn log_path(app_dir: &Path, profile: &str, at: NaiveDateTime) -> PathBuf {
    app_dir
        .join("logs")
        .join(profile)
        .join(format!("{}.html", at.format("%y.%m.%d.%H.%M")))
}

/// [`log_path`] stamped now, for `cao-gui` to call when a profile is selected.
pub fn stamp_log_path(app_dir: &Path, profile: &str) -> PathBuf {
    log_path(app_dir, profile, local_now())
}

fn local_now() -> NaiveDateTime {
    chrono::Local::now().naive_local()
}

/// The log file could not be opened. At bootstrap `main` reports it and exits;
/// at a run's redirect the run is not started.
#[derive(Debug, thiserror::Error)]
pub enum LogError {
    #[error("Cannot open log file `{}`: {source}", path.display())]
    Open {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// One Log tab row: the file's line without markup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    pub severity: Level,
    pub text: String,
}

/// What a [`LogFeed`] collected since the last drain.
#[derive(Debug, Default)]
pub struct FeedUpdate {
    /// The tab must drop its log rows before appending `lines`: the feed was
    /// just attached, or a redirect moved to a new file.
    pub reset: bool,
    pub lines: Vec<LogLine>,
}

/// The Log tab's end of the sink, made by [`ApplicationLog::subscribe`].
pub struct LogFeed {
    subscriber: Arc<Subscriber>,
}

impl LogFeed {
    /// Takes the queued rows. The next queued row signals the wake callback again.
    pub fn drain(&self) -> FeedUpdate {
        let mut pending = self.subscriber.lock();
        FeedUpdate {
            reset: mem::take(&mut pending.reset),
            lines: mem::take(&mut pending.lines).into(),
        }
    }
}

struct Subscriber {
    pending: Mutex<Pending>,
    wake: Box<dyn Fn() + Send + Sync>,
}

#[derive(Default)]
struct Pending {
    reset: bool,
    lines: VecDeque<LogLine>,
}

impl Subscriber {
    fn lock(&self) -> MutexGuard<'_, Pending> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Applies `change` and reports whether the buffer just went from empty to
    /// non-empty, which is the only time the wake callback is due.
    fn queue(&self, change: impl FnOnce(&mut Pending)) -> bool {
        let mut pending = self.lock();
        let was_idle = !pending.reset && pending.lines.is_empty();
        change(&mut pending);
        was_idle
    }
}

#[derive(Clone, Copy)]
enum Layout {
    Info,
    Debug,
}

/// The process's Application Log sink. See the module docs.
pub struct ApplicationLog {
    state: Mutex<State>,
    clock: Clock,
    /// Whether this is the process's `log` sink, so a redirect also moves the
    /// facade's max level. A sink that isn't installed must not touch it.
    installed: AtomicBool,
}

/// Everything a redirect swaps, under the sink's single lock.
struct State {
    file: RollingFile,
    layout: Layout,
    level: LevelFilter,
    /// The session's rows from bootstrap onward, replayed when a feed attaches.
    history: VecDeque<LogLine>,
    subscriber: Option<Arc<Subscriber>>,
}

impl ApplicationLog {
    /// Opens `path` for appending, creating its folder, at the info level or,
    /// with `debug`, the verbose level and debug layout.
    ///
    /// # Errors
    /// [`LogError::Open`] when the folder or file cannot be created or opened.
    pub fn open(path: &Path, debug: bool) -> Result<Self, LogError> {
        Self::open_with_clock(path, debug, local_now)
    }

    /// [`ApplicationLog::open`] with the clock that stamps each record, so tests
    /// can pin the timestamps.
    pub fn open_with_clock(path: &Path, debug: bool, clock: Clock) -> Result<Self, LogError> {
        Ok(Self {
            state: Mutex::new(State {
                file: RollingFile::open(path)?,
                layout: layout_for(debug),
                level: level_for(debug),
                history: VecDeque::new(),
                subscriber: None,
            }),
            clock,
            installed: AtomicBool::new(false),
        })
    }

    /// Makes this the process's `log` sink for the rest of its life and sets
    /// the facade's max level to match.
    ///
    /// # Errors
    /// [`log::SetLoggerError`] when a sink is already installed; this one is
    /// then leaked unused.
    pub fn install(self) -> Result<&'static Self, log::SetLoggerError> {
        let sink: &'static Self = Box::leak(Box::new(self));
        log::set_logger(sink)?;
        // Under the lock, so a concurrent redirect cannot leave a stale level.
        let state = sink.lock();
        sink.installed.store(true, Ordering::SeqCst);
        log::set_max_level(state.level);
        Ok(sink)
    }

    /// Points the log at a run's file and debug choice, at run start.
    ///
    /// A new path opens that file first; only then are the handle, layout and
    /// level swapped, and the feed cleared. The same path keeps its handle, so
    /// no header repeats.
    ///
    /// # Errors
    /// [`LogError::Open`] when a new path cannot be opened. Nothing is swapped,
    /// and the run must not start.
    pub fn redirect(&self, path: &Path, debug: bool) -> Result<(), LogError> {
        let mut state = self.lock();
        let mut woken = None;
        if state.file.path != path {
            state.file = RollingFile::open(path)?;
            state.history.clear();
            if let Some(subscriber) = &state.subscriber
                && subscriber.queue(|pending| {
                    pending.reset = true;
                    pending.lines.clear();
                })
            {
                woken = Some(Arc::clone(subscriber));
            }
        }
        state.layout = layout_for(debug);
        state.level = level_for(debug);
        if self.installed.load(Ordering::SeqCst) {
            log::set_max_level(state.level);
        }
        drop(state);
        wake(woken);
        Ok(())
    }

    /// The file the log is writing to.
    pub fn path(&self) -> PathBuf {
        self.lock().file.path.clone()
    }

    /// Attaches the Log tab's feed, replacing any earlier one, which then stops
    /// receiving rows. The session's rows are queued at once as a reset.
    ///
    /// `wake` is called, from whichever thread logged, each time the feed's
    /// queue goes from empty to non-empty; the GUI schedules a drain on its
    /// event loop there. It runs outside the sink's locks, so it may log.
    pub fn subscribe(&self, wake: impl Fn() + Send + Sync + 'static) -> LogFeed {
        let mut state = self.lock();
        let subscriber = Arc::new(Subscriber {
            pending: Mutex::new(Pending {
                reset: true,
                lines: state.history.clone(),
            }),
            wake: Box::new(wake),
        });
        state.subscriber = Some(Arc::clone(&subscriber));
        drop(state);
        (subscriber.wake)();
        LogFeed { subscriber }
    }

    /// A poisoned lock is recovered: the state is only ever replaced whole, so
    /// a panic mid-record cannot leave it half-written, and `log` must not panic.
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Log for ApplicationLog {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= self.lock().level
    }

    fn log(&self, record: &Record<'_>) {
        let mut state = self.lock();
        if record.level() > state.level {
            return;
        }
        // Formatted under the lock, so a concurrent redirect cannot put one
        // layout's line into the other's file.
        let line = LogLine {
            severity: record.level(),
            text: plain_text(state.layout, record, (self.clock)()),
        };
        // A failed write or roll is swallowed; the Log tab still gets the row.
        state.file.write(html_line(&line).as_bytes());
        push_capped(&mut state.history, line.clone());
        let woken = state
            .subscriber
            .as_ref()
            .filter(|subscriber| subscriber.queue(|pending| push_capped(&mut pending.lines, line)))
            .cloned();
        drop(state);
        wake(woken);
    }

    /// Nothing to do: every record is one unbuffered write.
    fn flush(&self) {}
}

/// Calls a subscriber's wake callback, after the caller has released the
/// sink's locks so a callback that logs cannot deadlock.
fn wake(subscriber: Option<Arc<Subscriber>>) {
    if let Some(subscriber) = subscriber {
        (subscriber.wake)();
    }
}

fn push_capped(rows: &mut VecDeque<LogLine>, line: LogLine) {
    if rows.len() == MAX_FEED_ROWS {
        rows.pop_front();
    }
    rows.push_back(line);
}

fn layout_for(debug: bool) -> Layout {
    if debug { Layout::Debug } else { Layout::Info }
}

/// Verbose (`trace!`) and up with debug logging; info and up without.
fn level_for(debug: bool) -> LevelFilter {
    if debug {
        LevelFilter::Trace
    } else {
        LevelFilter::Info
    }
}

/// plog's severity token. `trace!` stands in for plog's verbose.
fn token(level: Level) -> &'static str {
    match level {
        Level::Error => "ERROR",
        Level::Warn => "WARN",
        Level::Info => "INFO",
        Level::Debug => "DEBUG",
        Level::Trace => "VERB",
    }
}

fn colour(level: Level) -> &'static str {
    match level {
        Level::Error => "Red",
        Level::Warn => "Orange",
        Level::Info => "Green",
        Level::Debug => "Blue",
        Level::Trace => "Purple",
    }
}

/// The record's line without markup, in plog's info or debug layout.
fn plain_text(layout: Layout, record: &Record<'_>, at: NaiveDateTime) -> String {
    let mut text = String::new();
    // Writing to a `String` cannot fail.
    let _ = match layout {
        Layout::Info => write!(
            text,
            "{} [{}] {}",
            at.format("%Y-%m-%d %H:%M:%S"),
            token(record.level()),
            record.args()
        ),
        // plog padded the token to five and put `{` straight after it.
        Layout::Debug => write!(
            text,
            "{} {:<5}{{{}@{}}} {}",
            at.format("%Y-%m-%d %H:%M:%S%.3f"),
            token(record.level()),
            record.module_path().unwrap_or(record.target()),
            record.line().unwrap_or(0),
            record.args()
        ),
    };
    text
}

/// The file's bytes for one row. The timestamp, token and module path hold no
/// markup, so escaping the whole line escapes exactly the message.
fn html_line(line: &LogLine) -> String {
    let mut html = format!("<br><font color={}>", colour(line.severity));
    for ch in line.text.chars() {
        match ch {
            '&' => html.push_str("&amp;"),
            '<' => html.push_str("&lt;"),
            '>' => html.push_str("&gt;"),
            _ => html.push(ch),
        }
    }
    html.push_str("</font>\n");
    html
}

/// The live log file and plog's rotation over it.
struct RollingFile {
    path: PathBuf,
    /// `None` after a roll could not reopen the live file; the next record
    /// tries again.
    file: Option<File>,
    size: u64,
}

impl RollingFile {
    fn open(path: &Path) -> Result<Self, LogError> {
        let mut rolling = Self {
            path: path.to_owned(),
            file: None,
            size: 0,
        };
        let opened = match path.parent() {
            Some(folder) => std::fs::create_dir_all(folder),
            None => Ok(()),
        }
        .and_then(|()| rolling.reopen());
        opened.map(|()| rolling).map_err(|source| LogError::Open {
            path: path.to_owned(),
            source,
        })
    }

    /// Opens the live file for appending, writing the header only into an empty one.
    fn reopen(&mut self) -> io::Result<()> {
        let mut file = OpenOptions::new()
            .append(true)
            .create(true)
            .open(&self.path)?;
        let mut size = file.metadata()?.len();
        if size == 0 {
            file.write_all(HEADER.as_bytes())?;
            size = HEADER.len() as u64;
        }
        self.file = Some(file);
        self.size = size;
        Ok(())
    }

    /// Writes one record in one unbuffered write, so a reader sees it at once
    /// and a crash loses nothing. Failures are swallowed.
    fn write(&mut self, bytes: &[u8]) {
        if self.file.is_some() && self.size > MAX_FILE_SIZE {
            self.roll();
        }
        // A failed reopen is retried by the next record.
        if self.file.is_none() && self.reopen().is_err() {
            return;
        }
        if let Some(file) = &mut self.file
            && file.write_all(bytes).is_ok()
        {
            self.size += bytes.len() as u64;
        }
    }

    /// Shifts `name.N.html` to `name.N+1.html`, dropping the oldest, then
    /// starts a fresh live file. Only files that exist are renamed.
    fn roll(&mut self) {
        self.file = None;
        // A missing oldest chunk is the usual case.
        let _ = std::fs::remove_file(self.numbered(MAX_FILES - 1));
        for n in (0..MAX_FILES - 1).rev() {
            let from = self.numbered(n);
            if from.exists() {
                // A failed shift is swallowed, as plog did.
                let _ = std::fs::rename(&from, self.numbered(n + 1));
            }
        }
        // A failure leaves no handle, and the next record retries.
        let _ = self.reopen();
    }

    /// `name.N.html`, or the live file's own path for 0.
    fn numbered(&self, n: u32) -> PathBuf {
        if n == 0 {
            return self.path.clone();
        }
        let mut name = self.path.file_stem().unwrap_or_default().to_owned();
        name.push(format!(".{n}"));
        if let Some(extension) = self.path.extension() {
            name.push(".");
            name.push(extension);
        }
        self.path.with_file_name(name)
    }
}
