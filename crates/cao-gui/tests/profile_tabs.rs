//! The asset tabs a Profile enables come from the Profile Capabilities its
//! `profile.ini` declares, the same capabilities Preparing compiles routing
//! from (#621). Driven headlessly through the real main window.

mod common;

use common::*;

/// The asset tabs, in the tab bar's order.
const ASSET_TABS: [&str; 4] = ["BSA", "Meshes", "Textures", "Animations"];

/// The asset tabs `ui` shows enabled, in the tab bar's order.
fn enabled_asset_tabs(ui: &cao_gui::ui::MainWindow) -> Vec<&'static str> {
    ASSET_TABS
        .into_iter()
        .filter(|tab| enabled(ui, tab))
        .collect()
}

#[test]
fn each_shipped_profile_enables_the_tabs_it_always_has() {
    headless();
    let app_dir = app_dir("profile_tabs_shipped");
    edit_ini(&app_dir, "common.ini", "profile", "SSE");
    edit_ini(&app_dir, "common.ini", "showTutorial", "false");
    let (app, desktop) = open(&app_dir);
    let ui = app.window();

    // What each shipped Profile enabled before the tabs were gated from its
    // Profile Capabilities: FO4 has no Meshes or Animations, and TES5 no
    // Animations.
    let expected: [(&str, &[&str]); 3] = [
        ("SSE", &["BSA", "Meshes", "Textures", "Animations"]),
        ("FO4", &["BSA", "Textures"]),
        ("TES5", &["BSA", "Meshes", "Textures"]),
    ];
    for (profile, tabs) in expected {
        choose(ui, "Profile", profile);
        assert_eq!(enabled_asset_tabs(ui), tabs, "the {profile} profile");
        assert!(enabled(ui, "Log"), "the {profile} profile disables the Log");
    }
    assert!(desktop.message_titles().is_empty());
}

#[test]
fn a_profile_supporting_nothing_enables_only_the_log() {
    headless();
    let app_dir = app_dir("profile_tabs_none");
    edit_ini(&app_dir, "common.ini", "profile", "SSE");
    edit_ini(&app_dir, "common.ini", "showTutorial", "false");
    for key in [
        "bsaEnabled",
        "meshesEnabled",
        "texturesEnabled",
        "animationsEnabled",
    ] {
        edit_ini(&app_dir, "TES5/profile.ini", key, "false");
    }
    let (app, desktop) = open(&app_dir);
    let ui = app.window();

    choose(ui, "Profile", "TES5");

    // A readable Profile that supports nothing is not an unreadable one: no
    // error is shown, yet only the Log tab stays enabled.
    assert!(desktop.message_titles().is_empty());
    assert_eq!(enabled_asset_tabs(ui), Vec::<&str>::new());
    assert!(enabled(ui, "Log"));
}
