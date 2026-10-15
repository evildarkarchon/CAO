//! Deviation 1 (Rust-only): profiles resolve against the app directory `main`
//! derives from the exe, never against the working directory. C++ CAO opened
//! `profiles/` relative to the working directory, so a shortcut with another
//! start-in folder lost them.
//!
//! The working directory is process-wide, so the test that changes it has a
//! file of its own; the other test here uses absolute paths only.

mod common;

use common::*;
use slint::Model;

#[test]
fn launched_from_another_start_in_folder_the_window_finds_its_own_profiles() {
    headless();
    let install = app_dir("install");
    // The start-in folder has a `profiles/` of its own, which C++ would have
    // used: one profile, named and selected so it cannot be mistaken.
    let start_in = scratch_dir("start_in");
    let decoy = start_in.join("profiles").join("Decoy");
    std::fs::create_dir_all(&decoy).unwrap();
    std::fs::copy(
        install.join("profiles").join("SSE").join("profile.ini"),
        decoy.join("profile.ini"),
    )
    .unwrap();
    std::fs::write(
        start_in.join("profiles").join("common.ini"),
        "[General]\r\nprofile=Decoy\r\n",
    )
    .unwrap();
    let decoy_common = std::fs::read(start_in.join("profiles").join("common.ini")).unwrap();
    std::env::set_current_dir(&start_in).unwrap();

    // What `main` derives: the exe's directory, not the start-in folder.
    let exe = std::env::current_exe().unwrap();
    let derived = cao_gui::app_directory().unwrap();
    assert_eq!(derived, exe.parent().unwrap());
    assert_ne!(derived, start_in);

    // The window is handed the install's directory, as `main` hands it the
    // derived one (the test exe's directory holds no `profiles/`).
    let (app, _desktop) = open(&install);
    let ui = app.window();

    // The shipped `common.ini` selects TES5, and the list is the install's.
    assert_eq!(
        control(ui, "Profile").accessible_value().as_deref(),
        Some("TES5")
    );
    let listed: Vec<String> = ui.get_profiles().iter().map(Into::into).collect();
    assert_eq!(listed, ["FO4", "SSE", "TES5"]);

    // Saving writes the install's files and leaves the start-in folder alone.
    app.save();
    assert_eq!(
        std::fs::read(start_in.join("profiles").join("common.ini")).unwrap(),
        decoy_common
    );
    assert!(!start_in.join("profiles").join("TES5").exists());
}

#[test]
fn an_install_without_profiles_does_not_start() {
    headless();
    let empty = scratch_dir("no_profiles");
    let desktop = std::rc::Rc::new(FakeDesktop::default());
    let error = match cao_gui::App::new(&empty, desktop) {
        Ok(_) => panic!("the window started without profiles"),
        Err(error) => error,
    };
    assert!(
        matches!(error, cao_gui::StartupError::NoProfiles { ref path } if path == &empty.join("profiles")),
        "{error}"
    );
}
