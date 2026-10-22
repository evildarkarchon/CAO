//! The Mesh Target and game combos (#624): built from the typed Profile model,
//! so each shows the value its Profile holds, and a Profile whose Mesh Target is
//! outside its closed set is reported and never rewritten. Driven headlessly
//! through the real main window.

mod common;

use cao_profiles::{
    BsaGame, MeshFileVersion, MeshStreamVersion, MeshUserVersion, ProfileSettings, Profiles,
};
use common::*;

/// The text the combo labelled `combo` shows.
fn shown(ui: &cao_gui::ui::MainWindow, combo: &str) -> String {
    control(ui, combo)
        .accessible_value()
        .unwrap_or_default()
        .to_string()
}

/// The settings the Profile `name` holds on disk.
fn settings(app_dir: &std::path::Path, name: &str) -> ProfileSettings {
    Profiles::new(app_dir).open(name).load_settings().unwrap()
}

#[test]
fn switching_to_a_profile_with_an_out_of_set_mesh_user_reports_it_and_never_rewrites_it() {
    // Before #624 the User combo kept the previous Profile's value, and the
    // next save wrote it into this Profile's `profile.ini`.
    headless();
    let app_dir = app_dir("mesh_user_out_of_set");
    edit_ini(&app_dir, "common.ini", "profile", "TES5");
    edit_ini(&app_dir, "common.ini", "showTutorial", "false");
    edit_ini(&app_dir, "SSE/profile.ini", "meshesUser", "13");
    let profile_ini = app_dir.join("profiles").join("SSE").join("profile.ini");
    let before = std::fs::read(&profile_ini).unwrap();

    let (app, desktop) = open(&app_dir);
    let ui = app.window();
    assert!(desktop.message_titles().is_empty(), "TES5 is readable");
    choose(ui, "Profile", "SSE");

    let shown_messages = desktop.shown.borrow().clone();
    let [Shown::Message { title, text, .. }] = shown_messages.as_slice() else {
        panic!("expected one message, got {shown_messages:?}");
    };
    assert_eq!(title, "Profile");
    assert_eq!(
        text,
        "The profile `SSE` could not be read: `profile.ini`: `Meshes/meshesUser` is `13`; \
         expected one of 11, 12"
    );

    assert!(app.save());
    choose(ui, "Profile", "TES5");
    assert!(
        std::fs::read(&profile_ini).unwrap() == before,
        "SSE's `profile.ini` changed"
    );
}

#[test]
fn the_mesh_and_game_combos_show_each_shipped_profiles_values() {
    headless();
    let app_dir = app_dir("mesh_combos_shipped");
    edit_ini(&app_dir, "common.ini", "profile", "FO4");
    edit_ini(&app_dir, "common.ini", "showTutorial", "false");
    let (app, desktop) = open(&app_dir);
    let ui = app.window();

    // FO4 disables the Meshes tab, so only its game is visible.
    select_tab(ui, "BSA");
    assert_eq!(shown(ui, "Game"), "FO4");

    // Switch between the two Mesh-enabled Profiles both ways, so a combo left
    // at the previous Profile's value would show.
    for (profile, game, stream) in [
        ("TES5", "Skyrim LE", "Skyrim (83)"),
        ("SSE", "SSE", "Skyrim SE (100)"),
        ("TES5", "Skyrim LE", "Skyrim (83)"),
    ] {
        choose(ui, "Profile", profile);
        select_tab(ui, "BSA");
        assert_eq!(shown(ui, "Game"), game, "{profile}");
        select_tab(ui, "Meshes");
        assert_eq!(
            shown(ui, "Version"),
            "Fallout 3 / Skyrim / Skyrim SE / Fallout 4 (V20_2_0_7)",
            "{profile}"
        );
        assert_eq!(
            shown(ui, "User"),
            "Fallout 3 / Skyrim / Skyrim SE / Fallout 4 (12)",
            "{profile}"
        );
        assert_eq!(shown(ui, "Stream"), stream, "{profile}");
    }
    assert!(desktop.message_titles().is_empty());
}

#[test]
fn every_mesh_target_and_game_the_combos_offer_saves_as_its_typed_value() {
    headless();
    let app_dir = app_dir("mesh_combos_save");
    // A custom profile, so the advanced groups are editable.
    Profiles::new(&app_dir).create("Custom", "SSE").unwrap();
    edit_ini(&app_dir, "common.ini", "profile", "Custom");
    edit_ini(&app_dir, "common.ini", "showTutorial", "false");
    let (app, _desktop) = open(&app_dir);
    let ui = app.window();

    let games = [
        ("Skyrim LE", BsaGame::Tes5),
        ("SSE", BsaGame::Sse),
        ("FO4", BsaGame::Fo4),
    ];
    for (label, game) in games {
        select_tab(ui, "BSA");
        choose(ui, "Game", label);
        assert!(app.save());
        assert_eq!(settings(&app_dir, "Custom").bsa_game, game, "{label}");
    }

    let versions = [
        ("Oblivion (V20_0_0_5)", MeshFileVersion::V20_0_0_5),
        (
            "Fallout 3 / Skyrim / Skyrim SE / Fallout 4 (V20_2_0_7)",
            MeshFileVersion::V20_2_0_7,
        ),
    ];
    for (label, version) in versions {
        select_tab(ui, "Meshes");
        choose(ui, "Version", label);
        assert!(app.save());
        assert_eq!(
            settings(&app_dir, "Custom").meshes_file_version,
            version,
            "{label}"
        );
    }

    let users = [
        ("Oblivion (11)", MeshUserVersion::V11),
        (
            "Fallout 3 / Skyrim / Skyrim SE / Fallout 4 (12)",
            MeshUserVersion::V12,
        ),
    ];
    for (label, user) in users {
        select_tab(ui, "Meshes");
        choose(ui, "User", label);
        assert!(app.save());
        assert_eq!(settings(&app_dir, "Custom").meshes_user, user, "{label}");
    }

    let streams = [
        ("Oblivion / Fallout 3 (82)", MeshStreamVersion::V82),
        ("Skyrim (83)", MeshStreamVersion::V83),
        ("Skyrim SE (100)", MeshStreamVersion::V100),
        ("Fallout 4 (130)", MeshStreamVersion::V130),
    ];
    for (label, stream) in streams {
        select_tab(ui, "Meshes");
        choose(ui, "Stream", label);
        assert!(app.save());
        assert_eq!(
            settings(&app_dir, "Custom").meshes_stream,
            stream,
            "{label}"
        );
    }
}
