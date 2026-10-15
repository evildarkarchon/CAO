//! Starting an Optimization Run from the window when it cannot start (#510):
//! each refusal is shown in a message box and leaves the window unlocked.
//!
//! These tests need no event loop: no run reaches its worker's events. The run
//! lock and closing during a run are in `run_lock.rs` and `run_close.rs`, which
//! need one.

mod common;

use cao_gui::MessageLevel;
use cao_optimizers::application_log::ApplicationLog;
use common::*;

/// The messages shown so far, as (level, title, text).
fn messages(desktop: &FakeDesktop) -> Vec<(MessageLevel, String, String)> {
    desktop
        .shown
        .borrow()
        .iter()
        .filter_map(|shown| match shown {
            Shown::Message { level, title, text } => Some((*level, title.clone(), text.clone())),
            _ => None,
        })
        .collect()
}

/// Asserts that no run holds the window: Run reads Run, and the profile
/// combo, New and the tabs are enabled.
fn assert_unlocked(ui: &cao_gui::ui::MainWindow) {
    assert!(!ui.get_running());
    assert!(has_control(ui, "Run") && !has_control(ui, "Cancel"));
    for label in ["Profile", "New", "BSA", "Textures"] {
        assert!(enabled(ui, label), "{label} is disabled");
    }
}

/// An app directory on the SSE profile with tutorials off, and a mod folder.
fn sse_app(name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let app_dir = app_dir(name);
    edit_ini(&app_dir, "common.ini", "profile", "SSE");
    edit_ini(&app_dir, "common.ini", "showTutorial", "false");
    let mods = app_dir.join("mod");
    std::fs::create_dir_all(&mods).unwrap();
    (app_dir, mods)
}

#[test]
fn a_log_file_that_cannot_be_opened_shows_an_error_and_starts_no_run() {
    let _serial = serial();
    headless();
    let (app_dir, mods) = sse_app("run_log_unopenable");
    let (app, desktop) = open(&app_dir);
    let log = ApplicationLog::open(&app_dir.join("startup.html"), false).unwrap();
    app.use_application_log(Box::leak(Box::new(log)));
    // A file where the `logs` folder belongs: the run's
    // `logs/SSE/<stamp>.html` cannot be created.
    std::fs::write(app_dir.join("logs"), "").unwrap();
    let ui = app.window();
    ui.set_user_path(mods.display().to_string().into());
    ui.set_dry_run(true);

    click(ui, "Run");

    let shown = messages(&desktop);
    assert_eq!(shown.len(), 1, "{shown:?}");
    let (level, title, text) = &shown[0];
    assert_eq!((*level, title.as_str()), (MessageLevel::Error, "Error"));
    assert!(
        text.starts_with("The log file for this run could not be opened: "),
        "{text}"
    );
    assert_unlocked(ui);
    assert_eq!(ui.get_status_text(), "", "no run was presented");
}

#[test]
fn invalid_options_show_the_setup_error_and_start_no_run() {
    let _serial = serial();
    headless();
    let (app_dir, _mods) = sse_app("run_relative_folder");
    let (app, desktop) = open(&app_dir);
    let ui = app.window();
    ui.set_user_path("relative\\mod".into());

    click(ui, "Run");

    assert_eq!(
        messages(&desktop),
        [(
            MessageLevel::Error,
            "Start Error".to_owned(),
            // The window cleans the folder to forward slashes, as Qt's did.
            "The selected folder `relative/mod` is not an absolute path.".to_owned()
        )]
    );
    assert_unlocked(ui);
}

#[test]
fn a_missing_mod_folder_is_a_start_error() {
    let _serial = serial();
    headless();
    let (app_dir, _mods) = sse_app("run_no_folder");
    let (app, desktop) = open(&app_dir);
    let ui = app.window();
    ui.set_user_path("".into());

    click(ui, "Run");

    assert_eq!(
        messages(&desktop),
        [(
            MessageLevel::Error,
            "Start Error".to_owned(),
            "The run could not start: no mod folder is selected.".to_owned()
        )]
    );
    assert_unlocked(ui);
}
