//! A Profile that does not support some work keeps the options for it, and a
//! run from the window under that Profile holds the work back rather than
//! failing (#634): FO4 supports no Mesh work. Driven headlessly through the
//! real main window and composition root over a scratch mod folder.
//!
//! Run events reach the window through the event loop, so this file starts the
//! event-loop test backend, which a process may do once: it holds one test.

mod common;

use cao_gui::MessageLevel;
use cao_profiles::{Options, Profiles};
use common::*;

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
fn mesh_options_stay_saved_under_fo4_and_its_run_succeeds() {
    i_slint_backend_testing::init_integration_test_with_mock_time();
    let app_dir = app_dir("unsupported_mesh_work");
    edit_ini(&app_dir, "common.ini", "profile", "FO4");
    edit_ini(&app_dir, "common.ini", "showTutorial", "false");
    // Mesh options a Profile supporting Meshes would run: full optimization,
    // resaving and headparts.
    for (key, value) in [
        ("iMeshesOptimizationLevel", "3"),
        ("bMeshesResave", "true"),
        ("bMeshesHeadparts", "true"),
    ] {
        edit_ini(&app_dir, "FO4/settings.ini", key, value);
    }
    let mods = app_dir.join("mod");
    std::fs::create_dir_all(&mods).unwrap();
    let (app, desktop) = open(&app_dir);
    let ui = app.window();
    assert!(!enabled(ui, "Meshes"), "FO4 supports no Mesh work");
    ui.set_user_path(mods.display().to_string().into());

    // Starting the run saves the window first.
    click(ui, "Run");
    let saved = Profiles::new(&app_dir)
        .open("FO4")
        .load_options(&Options::default())
        .unwrap();
    assert_eq!(saved.meshes_optimization_level, 3);
    assert!(saved.meshes_resave && saved.meshes_headparts);

    // The run's request holds the Mesh work back, so Preparing finds no
    // Policy Conflict and the run completes.
    assert!(ui.get_running());
    run_event_loop_until(|| !ui.get_running());
    // An Apply run shows its attempt counts after its outcome.
    let status = ui.get_status_text();
    assert!(status.starts_with("Done"), "{status}");
    let (level, title, text) = last_message(&desktop).expect("the outcome is shown");
    assert_eq!(
        (level, title.as_str()),
        (MessageLevel::Info, "Optimization Run")
    );
    assert!(text.starts_with("Done"), "{text}");
}
