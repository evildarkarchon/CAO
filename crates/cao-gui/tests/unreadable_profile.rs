//! A profile's capabilities gate which asset options the window saves (#580):
//! an unreadable `profile.ini` leaves the asset options in `settings.ini` as
//! they were loaded, while a readable profile lacking a capability saves that
//! tab's options as off. Driven headlessly through the real main window.

mod common;

use std::path::{Path, PathBuf};

use cao_profiles::{OptimizationMode, Options, Profiles};
use common::*;

/// An app directory starting on the readable TES5 profile, without tutorial
/// boxes, whose SSE profile has every asset tab's options turned on, so that a
/// save clearing them shows on disk.
fn app_with_busy_sse(name: &str) -> PathBuf {
    let app_dir = app_dir(name);
    edit_ini(&app_dir, "common.ini", "profile", "TES5");
    edit_ini(&app_dir, "common.ini", "showTutorial", "false");
    for key in [
        "bBsaExtract",
        "bBsaCreate",
        "bTexturesCompress",
        "bTexturesResizeRatio",
        "bMeshesResave",
        "bAnimationsOptimization",
    ] {
        edit_ini(&app_dir, "SSE/settings.ini", key, "true");
    }
    edit_ini(
        &app_dir,
        "SSE/settings.ini",
        "iMeshesOptimizationLevel",
        "3",
    );
    app_dir
}

/// The options the SSE profile holds on disk.
fn sse_options(app_dir: &Path) -> Options {
    Profiles::new(app_dir)
        .open("SSE")
        .load_options(&Options::default())
        .unwrap()
}

#[test]
fn switching_away_from_an_unreadable_profile_keeps_its_asset_options() {
    headless();
    let app_dir = app_with_busy_sse("unreadable_profile_switch");
    // An unsupported game makes `profile.ini` unreadable (deviation 11).
    edit_ini(&app_dir, "SSE/profile.ini", "bsaGame", "99");
    let profile_ini = app_dir.join("profiles").join("SSE").join("profile.ini");
    let profile_ini_before = std::fs::read(&profile_ini).unwrap();
    let loaded = sse_options(&app_dir);
    assert!(loaded.bsa_extract && loaded.textures_compress && loaded.animations_optimization);

    let (app, desktop) = open(&app_dir);
    let ui = app.window();
    choose(ui, "Profile", "SSE");

    // The error is shown, and every asset tab is disabled.
    assert_eq!(desktop.message_titles(), ["Profile"]);
    for tab in ["BSA", "Meshes", "Textures", "Animations"] {
        assert!(!enabled(ui, tab), "{tab} is enabled");
    }

    // Change every option that does not depend on a capability.
    enter(ui, "Mod folder", "C:/Mods/Broken");
    choose(ui, "Optimization mode", "Several mods");
    click(ui, "Dry run");
    // Toggling the debug log saves the window at once.
    click_menu(ui, "Tools");
    click_menu(ui, "Enable debug log");
    // Switching profile saves the window again.
    choose(ui, "Profile", "TES5");

    let expected = Options {
        user_path: "C:/Mods/Broken".into(),
        mode: OptimizationMode::SeveralMods,
        dry_run: true,
        debug_log: true,
        ..loaded
    };
    assert_eq!(sse_options(&app_dir), expected);
    assert_eq!(std::fs::read(&profile_ini).unwrap(), profile_ini_before);
}

#[test]
fn a_readable_profile_lacking_a_capability_saves_that_tabs_options_as_off() {
    headless();
    let app_dir = app_with_busy_sse("profile_lacking_capability");
    edit_ini(&app_dir, "SSE/profile.ini", "texturesEnabled", "false");
    edit_ini(&app_dir, "SSE/profile.ini", "animationsEnabled", "false");
    let loaded = sse_options(&app_dir);

    let (app, desktop) = open(&app_dir);
    let ui = app.window();
    choose(ui, "Profile", "SSE");
    assert!(desktop.message_titles().is_empty());
    assert!(enabled(ui, "BSA") && enabled(ui, "Meshes"));
    assert!(!enabled(ui, "Textures") && !enabled(ui, "Animations"));
    choose(ui, "Profile", "TES5");

    let saved = sse_options(&app_dir);
    // The unsupported tabs' options are off...
    assert!(!saved.textures_necessary && !saved.textures_compress && !saved.textures_mipmaps);
    assert!(!saved.textures_resize_ratio && !saved.textures_resize_size);
    assert!(!saved.animations_optimization);
    // ...and the supported tabs' options are as they were loaded.
    assert!(saved.bsa_extract && saved.bsa_create);
    assert_eq!(saved.meshes_optimization_level, 3);
    assert!(saved.meshes_resave && saved.meshes_headparts);
    assert_eq!(
        (saved.user_path, saved.mode, saved.dry_run),
        (loaded.user_path, loaded.mode, loaded.dry_run)
    );
}
