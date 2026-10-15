//! Deviation 24 (#511): the Log tab is fed by the Application Log sink, so it is
//! not cut to the current rotation chunk, as C++'s tab, which re-read the live
//! HTML file, was; it keeps the newest 10,000 rows instead (#470 decision 6).
//!
//! Like `log_tab.rs`, this installs the process's `log` sink and runs the
//! testing backend's event loop, both once per process, hence one test.

mod common;

use std::path::Path;

use cao_gui::App;
use cao_gui::ui::{LogRow, LogTab};
use cao_optimizers::application_log::{MAX_FEED_ROWS, MAX_FILE_SIZE};
use common::*;
use slint::{ComponentHandle, Model};

/// Runs the event loop until everything queued so far has run.
fn pump() {
    // First in, first out: the quit request is handled after the queued work.
    slint::quit_event_loop().unwrap();
    slint::run_event_loop().unwrap();
}

fn rows(app: &App) -> Vec<LogRow> {
    app.window().global::<LogTab>().get_rows().iter().collect()
}

fn read(path: &Path) -> String {
    String::from_utf8_lossy(&std::fs::read(path).unwrap()).into_owned()
}

#[test]
fn the_log_tab_keeps_the_rows_from_before_the_log_file_rotated() {
    i_slint_backend_testing::init_integration_test_with_mock_time();
    let app_dir = app_dir("log_tab_rotation");
    let log = cao_gui::bootstrap_log(&app_dir).unwrap();
    log::warn!("a startup record");
    let (app, _desktop) = open(&app_dir);
    app.attach_log(log);
    pump();

    // Well past one file's worth, so the live file rolls at least once.
    let filler = "x".repeat(1_000);
    let records = 2 * MAX_FILE_SIZE as usize / filler.len();
    for n in 0..records {
        log::info!("record {n} {filler}");
    }
    pump();

    let live = log.path();
    let stem = live.file_stem().unwrap().to_str().unwrap();
    assert!(live.with_file_name(format!("{stem}.1.html")).exists());
    assert!(!read(&live).contains("a startup record"));

    let shown = rows(&app);
    assert_eq!(shown.len(), 1 + records);
    assert!(
        shown[0].text.ends_with("a startup record"),
        "{:?}",
        shown[0]
    );
    assert!(
        shown[records]
            .text
            .contains(&format!("record {} ", records - 1)),
        "{:?}",
        shown[records]
    );

    // Past 10,000 rows over several drains, the oldest go first.
    for batch in 0..2 {
        for n in 0..MAX_FEED_ROWS / 2 {
            log::info!("batch {batch} row {n}");
        }
        pump();
    }
    let shown = rows(&app);
    assert_eq!(shown.len(), MAX_FEED_ROWS);
    assert!(shown[0].text.ends_with("batch 0 row 0"), "{:?}", shown[0]);
    assert!(
        shown[MAX_FEED_ROWS - 1]
            .text
            .ends_with(&format!("batch 1 row {}", MAX_FEED_ROWS / 2 - 1)),
        "{:?}",
        shown[MAX_FEED_ROWS - 1]
    );
}
