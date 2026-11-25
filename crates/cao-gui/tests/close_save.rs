//! Closing the window when its settings cannot be saved (GUI-4): the window
//! stays open and asks whether to close anyway, naming what failed, and every
//! error the window shows is also written to the log. Driven headlessly
//! through the real main window.

mod common;

use std::path::Path;
use std::sync::{Mutex, Once};

use common::*;
use slint::ComponentHandle;
use slint::platform::WindowEvent;

/// The records logged in this test binary, as level and message.
static RECORDS: Mutex<Vec<(log::Level, String)>> = Mutex::new(Vec::new());

/// Keeps every record, standing in for the Application Log.
struct Recorder;

impl log::Log for Recorder {
    fn enabled(&self, _metadata: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        RECORDS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push((record.level(), record.args().to_string()));
    }

    fn flush(&self) {}
}

/// The error records logged so far that mention `text`. Installs the
/// recorder on first use; the logger is process-wide, so tests filter by text
/// unique to them.
fn errors_mentioning(text: &str) -> Vec<String> {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        log::set_logger(&Recorder).unwrap();
        log::set_max_level(log::LevelFilter::Trace);
    });
    RECORDS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .iter()
        .filter(|(level, message)| *level == log::Level::Error && message.contains(text))
        .map(|(_, message)| message.clone())
        .collect()
}

/// Makes `profiles/common.ini` unwritable: a directory takes its name, as a
/// sync client or a permissions change could leave it.
fn block_common_ini(app_dir: &Path) {
    let common = app_dir.join("profiles").join("common.ini");
    std::fs::remove_file(&common).unwrap();
    std::fs::create_dir(&common).unwrap();
}

/// An app directory without tutorial boxes.
fn quiet_app(name: &str) -> std::path::PathBuf {
    let app_dir = app_dir(name);
    edit_ini(&app_dir, "common.ini", "showTutorial", "false");
    app_dir
}

#[test]
fn a_close_whose_save_fails_stays_open_until_the_user_closes_anyway() {
    errors_mentioning("");
    headless();
    let app_dir = quiet_app("close_save_fails");
    let (app, desktop) = open(&app_dir);
    let ui = app.window();
    block_common_ini(&app_dir);
    // Only a changed file is written, so the theme changes.
    ui.set_dark(!ui.get_dark());

    // Keep open: the window stays, and the failure is in the question itself
    // rather than in an error box the closing window could drop.
    desktop.answer.set(false);
    ui.window().dispatch_event(WindowEvent::CloseRequested);
    next_turn();
    assert!(ui.window().is_visible(), "the window closed on Keep open");
    let questions = desktop.questions();
    assert_eq!(questions.len(), 1);
    assert!(
        questions[0].contains("The common settings could not be saved")
            && questions[0].contains("close_save_fails"),
        "{}",
        questions[0]
    );
    // The buttons say what they do; the one that loses nothing is `no`, the
    // default.
    assert!(matches!(
        &desktop.shown.borrow()[0],
        Shown::Question { yes, no, .. } if yes == "Close anyway" && no == "Keep open"
    ));
    assert!(desktop.message_titles().is_empty());
    // The log keeps the failure whatever the user answers.
    assert_eq!(errors_mentioning("close_save_fails").len(), 1);
    // The window is still usable after Keep open.
    click(ui, "Dry run");

    // Close anyway: the window closes, still without having saved.
    desktop.answer.set(true);
    ui.window().dispatch_event(WindowEvent::CloseRequested);
    next_turn();
    assert!(
        !ui.window().is_visible(),
        "the window stayed open on Close anyway"
    );
    assert_eq!(desktop.questions().len(), 2);
}

#[test]
fn a_close_whose_save_succeeds_closes_without_asking() {
    headless();
    let app_dir = quiet_app("close_save_succeeds");
    let (app, desktop) = open(&app_dir);
    let ui = app.window();
    click(ui, "Dry run");

    ui.window().dispatch_event(WindowEvent::CloseRequested);
    next_turn();

    assert!(!ui.window().is_visible());
    assert!(desktop.questions().is_empty());
    assert!(desktop.message_titles().is_empty());
    let options = cao_profiles::Profiles::new(&app_dir)
        .open(
            &cao_profiles::Profiles::new(&app_dir)
                .load_common()
                .unwrap()
                .profile,
        )
        .load_options("")
        .unwrap();
    assert!(options.dry_run, "the close saved the window");
}

#[test]
fn an_error_box_is_also_written_to_the_log() {
    headless();
    let app_dir = quiet_app("error_box_logged");
    let (app, desktop) = open(&app_dir);
    block_common_ini(&app_dir);
    // Only a changed file is written, so the theme changes.
    app.window().set_dark(!app.window().get_dark());

    app.save();

    assert_eq!(desktop.message_titles(), ["Save"]);
    let logged = errors_mentioning("error_box_logged");
    assert_eq!(logged.len(), 1, "{logged:?}");
    assert!(
        logged[0].starts_with("Save: The common settings could not be saved"),
        "{}",
        logged[0]
    );
}
