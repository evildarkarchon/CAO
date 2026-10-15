//! The Log tab over the process's Application Log (#511), driven headlessly
//! through the real window (spec seam 4).
//!
//! The Log tab's rows arrive through `slint::invoke_from_event_loop`, which
//! needs the testing backend's event loop, and the Application Log is the
//! process's `log` sink. Both can be set up once per process, so this file is a
//! single test that walks through the scenarios in order.

mod common;

use std::path::Path;
use std::rc::Rc;

use cao_gui::App;
use cao_gui::ui::{LogRow, LogSeverity, LogTab};
use common::*;
use i_slint_backend_testing::{AccessibleRole, ElementHandle};
use slint::{ComponentHandle, Model};

/// Runs the event loop until everything queued so far, such as the Log tab's
/// drain, has run.
fn pump() {
    // The testing backend's queue is first in, first out, so the quit request
    // is handled after the work queued before it.
    slint::quit_event_loop().unwrap();
    slint::run_event_loop().unwrap();
}

/// The Log tab's rows, oldest first.
fn rows(app: &App) -> Vec<LogRow> {
    app.window().global::<LogTab>().get_rows().iter().collect()
}

/// The texts of the rows of `severity`.
fn texts(app: &App, severity: LogSeverity) -> Vec<String> {
    rows(app)
        .into_iter()
        .filter(|row| row.severity == severity)
        .map(|row| row.text.to_string())
        .collect()
}

fn read(path: &Path) -> String {
    String::from_utf8_lossy(&std::fs::read(path).unwrap()).into_owned()
}

#[test]
fn the_log_tab_mirrors_the_application_log() {
    i_slint_backend_testing::init_integration_test_with_mock_time();
    let app_dir = app_dir("log_tab");
    // The startup profile logs at the verbose level.
    edit_ini(&app_dir, "TES5/settings.ini", "bDebugLog", "true");

    let log = cao_gui::bootstrap_log(&app_dir).unwrap();
    log::warn!("logged before the window existed");
    let (app, desktop) = open(&app_dir);
    app.attach_log(log);
    pump();

    startup_records_show_before_any_run(&app);
    records_from_other_threads_show_in_order(&app);
    open_log_file_opens_the_current_file(&app, &desktop, log);
    toggling_debug_logging_keeps_logging(&app, log);
    selecting_a_profile_stamps_a_new_log(&app, &app_dir, log);
}

fn startup_records_show_before_any_run(app: &App) {
    let verbose = texts(app, LogSeverity::Trace);
    assert!(
        verbose
            .iter()
            // TES4 ships without a `profile.ini`, so it is no profile.
            .any(|text| text.contains("VERB") && text.ends_with("Profiles found: FO4\nSSE\nTES5")),
        "the bootstrap's own record is missing: {verbose:?}"
    );
    let warnings = texts(app, LogSeverity::Warn);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].contains("WARN") && warnings[0].ends_with("logged before the window existed"),
        "{warnings:?}"
    );

    // The page shows them, as plain text without the file's markup.
    select_tab(app.window(), "Log");
    assert!(rows(app).iter().all(|row| !row.text.contains("<font")));
    let shown: Vec<_> =
        ElementHandle::find_by_accessible_label(app.window(), &warnings[0]).collect();
    assert_eq!(
        shown.len(),
        1,
        "the Log page does not show {:?}",
        warnings[0]
    );
    assert_eq!(shown[0].accessible_role(), Some(AccessibleRole::Text));
}

fn records_from_other_threads_show_in_order(app: &App) {
    let before = rows(app).len();
    std::thread::spawn(|| {
        log::error!("first <from> a worker");
        log::info!("second");
        log::debug!("third");
    })
    .join()
    .unwrap();
    pump();

    let added: Vec<(LogSeverity, String)> = rows(app)[before..]
        .iter()
        .map(|row| (row.severity, row.text.to_string()))
        .collect();
    let severities: Vec<LogSeverity> = added.iter().map(|(severity, _)| *severity).collect();
    assert_eq!(
        severities,
        [LogSeverity::Error, LogSeverity::Info, LogSeverity::Debug],
        "{added:?}"
    );
    // Unescaped: the tab shows the message as logged.
    assert!(added[0].1.ends_with("first <from> a worker"), "{added:?}");
}

fn open_log_file_opens_the_current_file(
    app: &App,
    desktop: &Rc<FakeDesktop>,
    log: &'static cao_optimizers::application_log::ApplicationLog,
) {
    log::info!("logged just before the click");
    click_menu(app.window(), "Tools");
    click_menu(app.window(), "Open log file");

    let opened = desktop
        .shown
        .borrow()
        .iter()
        .rev()
        .find_map(|shown| match shown {
            Shown::Opened(target) => Some(target.clone()),
            _ => None,
        })
        .expect("Open log file opened nothing");
    assert_eq!(Path::new(&opened), log.path());
    // Writes are unbuffered, so the file holds the record logged just before.
    assert!(read(&log.path()).contains("logged just before the click"));
}

fn toggling_debug_logging_keeps_logging(
    app: &App,
    log: &'static cao_optimizers::application_log::ApplicationLog,
) {
    let path = log.path();

    // Off: verbose records stop, and info records keep coming (deviation 23).
    click_menu(app.window(), "Tools");
    click_menu(app.window(), "Enable debug log");
    assert!(!app.window().get_debug_log());
    log::trace!("verbose while debug logging is off");
    log::info!("info while debug logging is off");

    // On again: verbose records come back, into the same file.
    click_menu(app.window(), "Tools");
    click_menu(app.window(), "Enable debug log");
    assert!(app.window().get_debug_log());
    log::trace!("verbose after debug logging is back on");
    pump();

    assert_eq!(log.path(), path);
    let file = read(&path);
    assert!(!file.contains("verbose while debug logging is off"));
    assert!(file.contains("info while debug logging is off"));
    assert!(file.contains("verbose after debug logging is back on"));
    let shown: Vec<String> = rows(app)
        .into_iter()
        .map(|row| row.text.to_string())
        .collect();
    assert!(
        shown
            .iter()
            .any(|text| text.ends_with("info while debug logging is off"))
    );
    assert!(
        shown
            .iter()
            .any(|text| text.ends_with("verbose after debug logging is back on"))
    );
    assert!(
        !shown
            .iter()
            .any(|text| text.ends_with("verbose while debug logging is off"))
    );
}

fn selecting_a_profile_stamps_a_new_log(
    app: &App,
    app_dir: &Path,
    log: &'static cao_optimizers::application_log::ApplicationLog,
) {
    let ui = app.window();
    let index = ui
        .get_profiles()
        .iter()
        .position(|name| name == "SSE")
        .unwrap();
    ui.invoke_profile_selected(index as i32);
    pump();

    // `logs/<profile>/yy.MM.dd.hh.mm.html` under the app directory.
    let path = log.path();
    assert_eq!(path.parent().unwrap(), app_dir.join("logs").join("SSE"));
    let name = path.file_name().unwrap().to_str().unwrap();
    let stamp = name.strip_suffix(".html").unwrap();
    assert_eq!(stamp.len(), "yy.MM.dd.hh.mm".len(), "{name}");
    assert!(
        stamp
            .split('.')
            .all(|part| part.len() == 2 && part.bytes().all(|b| b.is_ascii_digit())),
        "{name}"
    );

    // The tab mirrors the new file, which starts empty. SSE logs at the info
    // level, as its `settings.ini` says.
    assert_eq!(rows(app), Vec::<LogRow>::new());
    log::trace!("verbose under SSE");
    log::info!("info under SSE");
    pump();
    let shown: Vec<String> = rows(app)
        .into_iter()
        .map(|row| row.text.to_string())
        .collect();
    assert_eq!(shown.len(), 1, "{shown:?}");
    assert!(shown[0].ends_with("info under SSE"));
    assert!(read(&path).contains("info under SSE"));
}
