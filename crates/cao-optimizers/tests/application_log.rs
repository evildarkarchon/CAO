//! The Application Log sink, driven through `log::Log` on a sink that is not
//! installed as the process's logger, so each scenario owns its own sink.
//!
//! Golden tests pin plog's bytes and deviation 22; the rest pin rotation, the
//! per-run redirect, deviation 23, the failure scopes and the Log tab feed.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use cao_optimizers::application_log::{
    ApplicationLog, FeedUpdate, LogError, LogFeed, LogLine, MAX_FEED_ROWS, MAX_FILE_SIZE, log_path,
};
use chrono::{NaiveDate, NaiveDateTime};
use log::{Level, Log, Record};

const HEADER: &str = "\u{feff}<style>html{line-height:1.5rem}pre{line-height:1rem}</style>";

/// A fresh, empty directory under the target directory for one scenario.
fn scratch_dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("cao-optimizers-log")
        .join(name);
    // A missing directory is the expected case; anything else surfaces below.
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The fixed instant every record is stamped with: 2026-10-08 14:03:07.042.
fn fixed_clock() -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2026, 10, 8)
        .unwrap()
        .and_hms_milli_opt(14, 3, 7, 42)
        .unwrap()
}

fn open(path: &Path, debug: bool) -> ApplicationLog {
    ApplicationLog::open_with_clock(path, debug, fixed_clock).unwrap()
}

/// Sends one record from `cao_core::run`, line 42, as a `log!` macro there would.
fn emit(sink: &ApplicationLog, level: Level, message: &str) {
    sink.log(
        &Record::builder()
            .level(level)
            .target("cao_core::run")
            .module_path(Some("cao_core::run"))
            .line(Some(42))
            .args(format_args!("{message}"))
            .build(),
    );
}

fn read(path: &Path) -> String {
    String::from_utf8(std::fs::read(path).unwrap()).unwrap()
}

fn texts(lines: &[LogLine]) -> Vec<&str> {
    lines.iter().map(|line| line.text.as_str()).collect()
}

/// A subscriber whose wake callback only counts how often it was signalled.
fn counting_subscriber(sink: &ApplicationLog) -> (LogFeed, Arc<AtomicUsize>) {
    let wakes = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&wakes);
    let feed = sink.subscribe(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    });
    (feed, wakes)
}

#[test]
fn the_info_layout_keeps_plogs_bytes_with_escaped_text_and_a_newline_per_record() {
    let path = scratch_dir("golden-info").join("26.10.08.14.03.html");
    let sink = open(&path, false);

    emit(&sink, Level::Error, "a < b & c > d");
    emit(&sink, Level::Warn, "careful");
    emit(&sink, Level::Info, "Profiles found: SSE");

    let expected = [
        HEADER,
        "<br><font color=Red>2026-10-08 14:03:07 [ERROR] a &lt; b &amp; c &gt; d</font>\n",
        "<br><font color=Orange>2026-10-08 14:03:07 [WARN] careful</font>\n",
        "<br><font color=Green>2026-10-08 14:03:07 [INFO] Profiles found: SSE</font>\n",
    ]
    .concat();
    assert_eq!(read(&path), expected);
}

#[test]
fn the_debug_layout_keeps_plogs_bytes_with_the_module_path_for_the_function() {
    let path = scratch_dir("golden-debug").join("26.10.08.14.03.html");
    let sink = open(&path, true);

    emit(&sink, Level::Error, "failed");
    emit(&sink, Level::Warn, "careful");
    emit(&sink, Level::Info, "started");
    emit(&sink, Level::Debug, "detail");
    emit(&sink, Level::Trace, "x<y>");

    // The severity is padded to five characters, then `{` follows directly,
    // exactly as plog's `setw(5) << left` wrote it.
    let expected = [
        HEADER,
        "<br><font color=Red>2026-10-08 14:03:07.042 ERROR{cao_core::run@42} failed</font>\n",
        "<br><font color=Orange>2026-10-08 14:03:07.042 WARN {cao_core::run@42} careful</font>\n",
        "<br><font color=Green>2026-10-08 14:03:07.042 INFO {cao_core::run@42} started</font>\n",
        "<br><font color=Blue>2026-10-08 14:03:07.042 DEBUG{cao_core::run@42} detail</font>\n",
        "<br><font color=Purple>2026-10-08 14:03:07.042 VERB {cao_core::run@42} x&lt;y&gt;</font>\n",
    ]
    .concat();
    assert_eq!(read(&path), expected);
}

#[test]
fn the_info_level_writes_neither_debug_nor_verbose_records() {
    let path = scratch_dir("info-filter").join("log.html");
    let sink = open(&path, false);
    let (feed, _wakes) = counting_subscriber(&sink);

    emit(&sink, Level::Debug, "hidden debug");
    emit(&sink, Level::Trace, "hidden verbose");
    emit(&sink, Level::Info, "shown");

    let file = read(&path);
    assert!(!file.contains("hidden"), "{file}");
    assert!(file.contains("shown"), "{file}");
    assert_eq!(
        texts(&feed.drain().lines),
        ["2026-10-08 14:03:07 [INFO] shown"]
    );
}

#[test]
fn a_file_that_already_has_records_gets_no_second_header() {
    let path = scratch_dir("append").join("log.html");
    emit(&open(&path, false), Level::Info, "first launch");
    emit(&open(&path, false), Level::Info, "second launch");

    let expected = [
        HEADER,
        "<br><font color=Green>2026-10-08 14:03:07 [INFO] first launch</font>\n",
        "<br><font color=Green>2026-10-08 14:03:07 [INFO] second launch</font>\n",
    ]
    .concat();
    assert_eq!(read(&path), expected);
}

#[test]
fn opening_creates_the_profile_log_folder() {
    let app = scratch_dir("creates-folder");
    let path = log_path(&app, "SSE", fixed_clock());
    assert_eq!(
        path,
        app.join("logs").join("SSE").join("26.10.08.14.03.html")
    );

    let sink = open(&path, false);
    emit(&sink, Level::Info, "hello");

    assert!(read(&path).contains("hello"));
}

#[test]
fn the_log_rolls_past_the_size_limit_into_numbered_files() {
    let dir = scratch_dir("rotation");
    let path = dir.join("log.html");
    // Older chunks shift up by one; the oldest kept chunk (999) falls off.
    std::fs::write(dir.join("log.1.html"), "chunk one").unwrap();
    std::fs::write(dir.join("log.998.html"), "chunk 998").unwrap();
    std::fs::write(dir.join("log.999.html"), "chunk 999").unwrap();

    let sink = open(&path, false);
    let big = "x".repeat(10_000);
    // The size is checked before each write, so the record that passes the
    // limit still lands in the old chunk.
    while std::fs::metadata(&path).unwrap().len() <= MAX_FILE_SIZE {
        emit(&sink, Level::Info, &big);
    }
    let full = read(&path);
    emit(&sink, Level::Info, "after the roll");

    assert_eq!(read(&dir.join("log.1.html")), full);
    assert_eq!(read(&dir.join("log.2.html")), "chunk one");
    assert_eq!(read(&dir.join("log.999.html")), "chunk 998");
    assert!(!dir.join("log.998.html").exists());
    assert_eq!(
        read(&path),
        [
            HEADER,
            "<br><font color=Green>2026-10-08 14:03:07 [INFO] after the roll</font>\n"
        ]
        .concat()
    );
}

/// Deviation 23: in C++ the two plog appenders each held the file with
/// `_SH_DENYWR`, so toggling debug logging between runs silently dropped the log.
#[test]
fn toggling_debug_logging_between_runs_keeps_logging_to_the_same_file() {
    let path = scratch_dir("deviation-23").join("log.html");
    let sink = open(&path, false);
    emit(&sink, Level::Info, "first run");

    sink.redirect(&path, true).unwrap();
    emit(&sink, Level::Debug, "second run");

    sink.redirect(&path, false).unwrap();
    emit(&sink, Level::Debug, "third run hidden");
    emit(&sink, Level::Info, "third run");

    let expected = [
        HEADER,
        "<br><font color=Green>2026-10-08 14:03:07 [INFO] first run</font>\n",
        "<br><font color=Blue>2026-10-08 14:03:07.042 DEBUG{cao_core::run@42} second run</font>\n",
        "<br><font color=Green>2026-10-08 14:03:07 [INFO] third run</font>\n",
    ]
    .concat();
    assert_eq!(read(&path), expected);
}

#[test]
fn a_redirect_to_a_new_path_writes_there_and_leaves_the_old_file_alone() {
    let app = scratch_dir("redirect");
    let first = log_path(&app, "SSE", fixed_clock());
    let second = app.join("logs").join("FO4").join("26.10.08.14.05.html");
    let sink = open(&first, false);
    emit(&sink, Level::Info, "startup");

    sink.redirect(&second, false).unwrap();
    emit(&sink, Level::Info, "run");

    assert_eq!(sink.path(), second);
    assert!(!read(&first).contains("run"));
    assert_eq!(
        read(&second),
        [
            HEADER,
            "<br><font color=Green>2026-10-08 14:03:07 [INFO] run</font>\n"
        ]
        .concat()
    );
}

#[test]
fn a_log_file_that_cannot_be_opened_fails_the_bootstrap() {
    let dir = scratch_dir("bootstrap-failure");
    std::fs::write(dir.join("logs"), "a file where the folder should be").unwrap();
    let path = dir.join("logs").join("SSE").join("log.html");

    let error = ApplicationLog::open(&path, false)
        .err()
        .expect("bootstrap must fail");

    let LogError::Open { path: failed, .. } = &error;
    assert_eq!(failed, &path);
    assert!(error.to_string().contains("log.html"), "{error}");
}

#[test]
fn a_redirect_that_cannot_open_its_file_refuses_and_keeps_the_current_log() {
    let dir = scratch_dir("redirect-failure");
    let path = dir.join("log.html");
    std::fs::write(dir.join("blocked"), "a file where the folder should be").unwrap();
    let unopenable = dir.join("blocked").join("log.html");
    let sink = open(&path, false);
    let (feed, _wakes) = counting_subscriber(&sink);
    emit(&sink, Level::Info, "before");

    let error = sink
        .redirect(&unopenable, true)
        .expect_err("redirect must fail");

    assert!(error.to_string().contains("log.html"), "{error}");
    // Nothing was swapped: same file, same info layout, same Log tab rows.
    emit(&sink, Level::Debug, "still info level");
    emit(&sink, Level::Info, "after");
    assert_eq!(sink.path(), path);
    let file = read(&path);
    assert!(file.contains("[INFO] after"), "{file}");
    assert!(!file.contains("still info level"), "{file}");
    let update = feed.drain();
    assert_eq!(
        texts(&update.lines),
        [
            "2026-10-08 14:03:07 [INFO] before",
            "2026-10-08 14:03:07 [INFO] after"
        ]
    );
}

#[test]
fn a_reader_opening_the_file_mid_run_sees_every_record_so_far() {
    let path = scratch_dir("mid-run").join("log.html");
    let sink = open(&path, false);

    for n in 0..20 {
        emit(&sink, Level::Info, &format!("record {n}"));
        // The sink is still alive and holds its handle: nothing is buffered.
        let file = read(&path);
        assert!(file.ends_with(&format!("record {n}</font>\n")), "{file}");
    }
}

#[test]
fn a_record_that_panics_while_formatting_does_not_stop_later_records() {
    struct Panics;
    impl std::fmt::Display for Panics {
        fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            panic!("a message that cannot be formatted")
        }
    }
    let path = scratch_dir("poisoned").join("log.html");
    let sink = open(&path, false);

    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        sink.log(
            &Record::builder()
                .level(Level::Info)
                .args(format_args!("{}", Panics))
                .build(),
        );
    }));
    assert!(panicked.is_err());
    emit(&sink, Level::Info, "after the panic");

    assert!(read(&path).ends_with("[INFO] after the panic</font>\n"));
}

#[test]
fn attaching_replays_startup_records_and_signals_once_until_drained() {
    let path = scratch_dir("feed-replay").join("log.html");
    let sink = open(&path, true);
    emit(&sink, Level::Info, "startup");
    emit(&sink, Level::Error, "startup error");

    let (feed, wakes) = counting_subscriber(&sink);
    assert_eq!(wakes.load(Ordering::SeqCst), 1);

    emit(&sink, Level::Warn, "queued");
    assert_eq!(wakes.load(Ordering::SeqCst), 1, "already signalled");

    let FeedUpdate { reset, lines } = feed.drain();
    assert!(reset, "the replay replaces whatever the tab showed");
    assert_eq!(
        lines
            .iter()
            .map(|line| (line.severity, line.text.as_str()))
            .collect::<Vec<_>>(),
        [
            (
                Level::Info,
                "2026-10-08 14:03:07.042 INFO {cao_core::run@42} startup"
            ),
            (
                Level::Error,
                "2026-10-08 14:03:07.042 ERROR{cao_core::run@42} startup error"
            ),
            (
                Level::Warn,
                "2026-10-08 14:03:07.042 WARN {cao_core::run@42} queued"
            ),
        ]
    );

    emit(&sink, Level::Info, "after drain");
    assert_eq!(wakes.load(Ordering::SeqCst), 2);
    let update = feed.drain();
    assert!(!update.reset);
    assert_eq!(
        texts(&update.lines),
        ["2026-10-08 14:03:07.042 INFO {cao_core::run@42} after drain"]
    );
    // Nothing new: an empty update, and no further signal.
    let empty = feed.drain();
    assert!(!empty.reset && empty.lines.is_empty());
    assert_eq!(wakes.load(Ordering::SeqCst), 2);
}

/// Deviation 24 (sink side): the replay is not truncated to the current
/// rotation chunk, only capped to the newest rows.
#[test]
fn the_feed_keeps_the_newest_rows_across_rotation_up_to_the_cap() {
    let path = scratch_dir("feed-cap").join("log.html");
    let sink = open(&path, false);
    let total = MAX_FEED_ROWS + 5;
    for n in 0..total {
        emit(
            &sink,
            Level::Info,
            &format!("record {n} {}", "y".repeat(40)),
        );
    }
    assert!(path.with_file_name("log.1.html").exists(), "the log rolled");

    let (feed, _wakes) = counting_subscriber(&sink);
    let lines = feed.drain().lines;

    assert_eq!(lines.len(), MAX_FEED_ROWS);
    assert!(lines[0].text.contains("record 5 "), "{}", lines[0].text);
    assert!(
        lines[MAX_FEED_ROWS - 1]
            .text
            .contains(&format!("record {} ", total - 1))
    );
}

#[test]
fn a_redirect_to_a_new_path_clears_the_feed_but_the_same_path_does_not() {
    let dir = scratch_dir("feed-redirect");
    let path = dir.join("log.html");
    let sink = open(&path, false);
    emit(&sink, Level::Info, "startup");
    let (feed, wakes) = counting_subscriber(&sink);
    feed.drain();

    sink.redirect(&path, true).unwrap();
    emit(&sink, Level::Info, "same file");
    let update = feed.drain();
    assert!(!update.reset);
    assert_eq!(update.lines.len(), 1);

    sink.redirect(&dir.join("other.html"), false).unwrap();
    assert_eq!(wakes.load(Ordering::SeqCst), 3, "the clear alone signals");
    emit(&sink, Level::Info, "new file");
    let update = feed.drain();
    assert!(update.reset);
    assert_eq!(
        texts(&update.lines),
        ["2026-10-08 14:03:07 [INFO] new file"]
    );

    // A later subscriber replays only the new file's session.
    let (late, _late_wakes) = counting_subscriber(&sink);
    assert_eq!(
        texts(&late.drain().lines),
        ["2026-10-08 14:03:07 [INFO] new file"]
    );
}
