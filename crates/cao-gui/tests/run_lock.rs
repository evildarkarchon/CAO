//! The run lock (#510): what a run disables, Run becoming Cancel, Cancel
//! disabling itself once clicked, and everything coming back when the run
//! reports its outcome. Runs go through the real composition root over a
//! scratch mod folder.
//!
//! Run events reach the window through the event loop, so this file starts the
//! event-loop test backend, which a process may do once: it holds one test.

mod common;

use cao_gui::MessageLevel;
use common::*;
use slint::Model;

/// Controls the run lock never touches, as in C++, which disabled only the
/// tab widget, the profile combo and New.
const NEVER_LOCKED: &[&str] = &[
    "Mod folder",
    "Open Directory",
    "Optimization mode",
    "Dry run",
    "Show advanced settings",
];
/// The controls the lock disables outside the tab pages.
const LOCKED: &[&str] = &[
    "Profile",
    "New",
    "BSA",
    "Meshes",
    "Textures",
    "Animations",
    "Log",
];
/// The Textures page's controls with advanced settings hidden, which must AND
/// in the lock by hand: Slint has no enabled cascade.
const TEXTURES_PAGE: &[&str] = &[
    "Process textures",
    "Necessary optimization",
    "Compress textures",
    "Generate mipmaps",
    "Downsizing",
    "By ratio",
    "Height ratio",
    "Width ratio",
    "By fixed size",
    "Fixed height",
    "Fixed width",
];

/// Asserts each control's enabled state.
fn assert_enabled(ui: &cao_gui::ui::MainWindow, labels: &[&str], expected: bool) {
    for label in labels {
        assert_eq!(enabled(ui, label), expected, "{label}");
    }
}

/// The last message box shown, as (level, title, text).
fn last_message(desktop: &FakeDesktop) -> Option<(MessageLevel, String, String)> {
    desktop
        .shown
        .borrow()
        .iter()
        .rev()
        .find_map(|shown| match shown {
            Shown::Message { level, title, text } => Some((*level, title.clone(), text.clone())),
            _ => None,
        })
}

#[test]
fn a_run_locks_the_profile_new_and_tab_pages_until_it_reports_its_outcome() {
    i_slint_backend_testing::init_integration_test_with_mock_time();
    let app_dir = app_dir("run_lock");
    edit_ini(&app_dir, "common.ini", "profile", "SSE");
    edit_ini(&app_dir, "common.ini", "showTutorial", "false");
    let mods = app_dir.join("mod");
    std::fs::create_dir_all(&mods).unwrap();
    let (app, desktop) = open(&app_dir);
    let ui = app.window();
    ui.set_user_path(mods.display().to_string().into());
    ui.set_dry_run(true);
    select_tab(ui, "Textures");
    // Its radios and spin boxes are enabled only inside a checked group.
    if !checked(ui, "Downsizing") {
        click(ui, "Downsizing");
    }
    assert_enabled(ui, LOCKED, true);
    assert_enabled(ui, TEXTURES_PAGE, true);

    // A run left to finish: Run becomes Cancel and the lock holds until the
    // terminal result is delivered.
    click(ui, "Run");
    assert!(ui.get_running());
    assert!(has_control(ui, "Cancel") && !has_control(ui, "Run"));
    assert!(enabled(ui, "Cancel"));
    assert_enabled(ui, LOCKED, false);
    assert_enabled(ui, TEXTURES_PAGE, false);
    assert_enabled(ui, NEVER_LOCKED, true);
    assert_eq!(ui.get_status_text(), "Preparing");
    assert_eq!(ui.get_progress_text(), "Preparing");
    assert!(ui.get_progress_indeterminate());

    run_event_loop_until(|| !ui.get_running());
    assert!(has_control(ui, "Run") && enabled(ui, "Run"));
    assert_enabled(ui, LOCKED, true);
    assert_enabled(ui, TEXTURES_PAGE, true);
    assert_eq!(ui.get_status_text(), "Done");
    assert_eq!(ui.get_progress_text(), "Done");
    assert!(!ui.get_progress_indeterminate());
    assert_eq!(
        last_message(&desktop),
        Some((
            MessageLevel::Info,
            "Optimization Run".to_owned(),
            "Done".to_owned()
        ))
    );
    assert!(
        ui.get_run_details()
            .iter()
            .any(|row| row == "Final Phase: Archive Finalization"),
        "the terminal evidence reaches the detail rows"
    );

    // A cancelled run: Cancel disables itself at once, and the lock holds
    // until the outcome arrives, whichever it turns out to be.
    click(ui, "Run");
    click(ui, "Cancel");
    assert!(ui.get_running() && ui.get_cancelling());
    assert!(
        !enabled(ui, "Cancel"),
        "Cancel disables itself once clicked"
    );
    assert!(
        ui.get_status_text().starts_with("Cancelling - "),
        "{}",
        ui.get_status_text()
    );
    assert_enabled(ui, LOCKED, false);
    assert_enabled(ui, TEXTURES_PAGE, false);

    run_event_loop_until(|| !ui.get_running());
    assert!(!ui.get_cancelling());
    assert!(has_control(ui, "Run") && enabled(ui, "Run"));
    assert_enabled(ui, LOCKED, true);
    assert_enabled(ui, TEXTURES_PAGE, true);
    let status = ui.get_status_text();
    assert!(status == "Done" || status == "Cancelled", "{status}");
}
