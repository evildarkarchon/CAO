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
fn app_with_sse_asset_options_on(name: &str) -> PathBuf {
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
    let app_dir = app_with_sse_asset_options_on("unreadable_profile_switch");
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
fn a_malformed_profile_ini_is_reported_on_selection_and_never_rewritten() {
    // Saves rewrite `profile.ini` from its parsed entries, so a lenient read
    // would let the next save silently delete the line QSettings rejects.
    let _serial = serial();
    headless();
    let app_dir = app_with_sse_asset_options_on("malformed_profile_ini");
    let profile_ini = app_dir.join("profiles").join("SSE").join("profile.ini");
    let mut profile_ini_before = std::fs::read(&profile_ini).unwrap();
    profile_ini_before.extend_from_slice(b"stray words\r\n");
    std::fs::write(&profile_ini, &profile_ini_before).unwrap();
    let loaded = sse_options(&app_dir);
    assert!(loaded.bsa_extract && loaded.textures_compress && loaded.animations_optimization);
    let unchanged = |when: &str| {
        assert!(
            std::fs::read(&profile_ini).unwrap() == profile_ini_before,
            "`profile.ini` changed after {when}"
        );
    };

    let (app, desktop) = open(&app_dir);
    let ui = app.window();
    choose(ui, "Profile", "SSE");

    // Selecting the Profile reports the line, as Preparing would.
    let shown = desktop.shown.borrow().clone();
    let [Shown::Message { title, text, .. }] = shown.as_slice() else {
        panic!("expected one message, got {shown:?}");
    };
    assert_eq!(title, "Profile");
    // The shipped file has 21 lines, so the appended one is line 22.
    let path = Profiles::new(&app_dir).open("SSE").profile_ini();
    assert_eq!(
        *text,
        format!(
            "The profile `SSE` could not be read: `{}` cannot be read: \
             line 22 has no `=` and is not a comment",
            path.display()
        )
    );
    for tab in ["BSA", "Meshes", "Textures", "Animations"] {
        assert!(!enabled(ui, tab), "{tab} is enabled");
    }

    assert!(app.save());
    unchanged("a save");

    // A refused Run saves the window first, then reports a Start Error.
    ui.set_user_path("".into());
    click(ui, "Run");
    assert_eq!(desktop.message_titles(), ["Profile", "Start Error"]);
    assert!(!ui.get_running());
    unchanged("a refused Run");

    choose(ui, "Profile", "TES5");
    choose(ui, "Profile", "SSE");
    choose(ui, "Profile", "TES5");
    unchanged("Profile switches");
    assert_eq!(
        desktop.message_titles(),
        ["Profile", "Start Error", "Profile"],
        "reselecting the Profile reports it again"
    );

    // Its `settings.ini` asset options are kept as loaded.
    let expected = Options {
        user_path: String::new(),
        ..loaded
    };
    assert_eq!(sse_options(&app_dir), expected);
}

#[test]
fn a_readable_profile_lacking_a_capability_saves_that_tabs_options_as_off() {
    headless();
    let app_dir = app_with_sse_asset_options_on("profile_lacking_capability");
    edit_ini(&app_dir, "SSE/profile.ini", "texturesEnabled", "false");
    edit_ini(&app_dir, "SSE/profile.ini", "animationsEnabled", "false");
    let loaded = sse_options(&app_dir);
    assert!(loaded.bsa_extract && loaded.bsa_create && loaded.meshes_resave);
    assert_eq!(loaded.meshes_optimization_level, 3);

    let (app, desktop) = open(&app_dir);
    let ui = app.window();
    choose(ui, "Profile", "SSE");
    assert!(desktop.message_titles().is_empty());
    assert!(enabled(ui, "BSA") && enabled(ui, "Meshes"));
    assert!(!enabled(ui, "Textures") && !enabled(ui, "Animations"));
    choose(ui, "Profile", "TES5");

    // The unsupported tabs' options are off...
    let expected = Options {
        textures_necessary: false,
        textures_compress: false,
        textures_mipmaps: false,
        textures_resize_size: false,
        textures_resize_ratio: false,
        animations_optimization: false,
        // ...and the supported tabs' options are as they were loaded.
        ..loaded
    };
    assert_eq!(sse_options(&app_dir), expected);
}
