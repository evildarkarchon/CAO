//! The window's UI rules (#509), driven headlessly through the real `.slint`
//! components (spec seam 4). Each rule is pinned twice: when the user changes
//! the control that triggers it, and when the settings that trigger it load
//! from the INI files (deviation 3).

mod common;

use std::path::Path;

use cao_archive::Game;
use cao_gui::App;
use cao_gui::ui::MainWindow;
use cao_optimizers::profile::keeps_textures_separate;
use cao_profiles::{Options, Profiles};
use common::*;

/// The three base archive options the Dry Run rule clears and disables.
const BASE_ARCHIVE_OPTIONS: [&str; 3] = ["Extract BSA", "Create BSA", "Delete backups"];

/// An app directory whose `common.ini` starts on `profile`, without tutorial boxes.
fn app_on(name: &str, profile: &str) -> std::path::PathBuf {
    let app_dir = app_dir(name);
    edit_ini(&app_dir, "common.ini", "profile", profile);
    edit_ini(&app_dir, "common.ini", "showTutorial", "false");
    app_dir
}

/// Saves the window and reads back the options `profile` now holds on disk.
fn saved_options(app: &App, app_dir: &Path, profile: &str) -> Options {
    app.save();
    Profiles::new(app_dir)
        .open(profile)
        .load_options(&Options::default())
        .unwrap()
}

/// Asserts each of `labels` is checked (or not) and enabled (or not).
fn assert_controls(ui: &MainWindow, labels: &[&str], is_checked: bool, is_enabled: bool) {
    for label in labels {
        assert_eq!(checked(ui, label), is_checked, "{label} checked");
        assert_eq!(enabled(ui, label), is_enabled, "{label} enabled");
    }
}

// ---- Dry Run

#[test]
fn clicking_dry_run_clears_and_disables_the_base_archive_options() {
    headless();
    let app_dir = app_on("dry_run_click", "SSE");
    for key in ["bBsaExtract", "bBsaCreate", "bBsaDeleteBackup"] {
        edit_ini(&app_dir, "SSE/settings.ini", key, "true");
    }
    let (app, _desktop) = open(&app_dir);
    let ui = app.window();
    select_tab(ui, "BSA");
    assert_controls(ui, &BASE_ARCHIVE_OPTIONS, true, true);

    click(ui, "Dry run");
    assert_controls(ui, &BASE_ARCHIVE_OPTIONS, false, false);

    // Turning it off gives the options back, still cleared.
    click(ui, "Dry run");
    assert_controls(ui, &BASE_ARCHIVE_OPTIONS, false, true);
    let options = saved_options(&app, &app_dir, "SSE");
    assert!(!options.dry_run);
    assert!(!options.bsa_extract && !options.bsa_create && !options.bsa_delete_backup);
}

#[test]
fn a_dry_run_loaded_from_the_ini_clears_and_disables_the_base_archive_options() {
    headless();
    // A profile C++ could save: Dry Run on, with the base options it would
    // have cleared on a click.
    let app_dir = app_on("dry_run_load", "SSE");
    for key in ["bDryRun", "bBsaExtract", "bBsaCreate", "bBsaDeleteBackup"] {
        edit_ini(&app_dir, "SSE/settings.ini", key, "true");
    }
    let (app, _desktop) = open(&app_dir);
    let ui = app.window();
    select_tab(ui, "BSA");

    assert!(checked(ui, "Dry run"));
    assert_controls(ui, &BASE_ARCHIVE_OPTIONS, false, false);
    let options = saved_options(&app, &app_dir, "SSE");
    assert!(options.dry_run);
    assert!(!options.bsa_extract && !options.bsa_create && !options.bsa_delete_backup);

    // Turning it off gives the options back, cleared.
    click(ui, "Dry run");
    assert_controls(ui, &BASE_ARCHIVE_OPTIONS, false, true);
}

// ---- Several Mods

/// The mesh levels above "Necessary", which Several Mods disables.
const HEAVY_MESH_LEVELS: [&str; 2] = ["Medium optimization", "Full optimization"];

#[test]
fn choosing_several_mods_forces_necessary_meshes_and_disables_the_heavier_levels() {
    headless();
    let app_dir = app_on("several_mods_click", "SSE");
    edit_ini(
        &app_dir,
        "SSE/settings.ini",
        "iMeshesOptimizationLevel",
        "3",
    );
    let (app, _desktop) = open(&app_dir);
    let ui = app.window();
    select_tab(ui, "Meshes");
    assert!(checked(ui, "Full optimization"));
    assert!(enabled(ui, "Full optimization") && enabled(ui, "Medium optimization"));

    choose(ui, "Optimization mode", "Several mods");
    assert!(checked(ui, "Necessary optimization"));
    assert_controls(ui, &HEAVY_MESH_LEVELS, false, false);
    assert!(checked(ui, "Process meshes"));
    let options = saved_options(&app, &app_dir, "SSE");
    assert_eq!(options.mode, cao_profiles::OptimizationMode::SeveralMods);
    assert_eq!(options.meshes_optimization_level, 1);

    // Back to one mod: the heavier levels come back, and the level stays.
    choose(ui, "Optimization mode", "One mod");
    assert!(checked(ui, "Necessary optimization"));
    assert_controls(ui, &HEAVY_MESH_LEVELS, false, true);
    click(ui, "Full optimization");
    assert!(checked(ui, "Full optimization") && !checked(ui, "Necessary optimization"));
}

#[test]
fn several_mods_loaded_from_the_ini_forces_necessary_meshes_and_disables_the_heavier_levels() {
    headless();
    // A profile C++ could save: Several Mods with Full mesh optimization.
    let app_dir = app_on("several_mods_load", "SSE");
    edit_ini(&app_dir, "SSE/settings.ini", "mode", "1");
    edit_ini(
        &app_dir,
        "SSE/settings.ini",
        "iMeshesOptimizationLevel",
        "3",
    );
    let (app, _desktop) = open(&app_dir);
    let ui = app.window();
    select_tab(ui, "Meshes");

    assert!(checked(ui, "Process meshes"));
    assert!(checked(ui, "Necessary optimization"));
    assert_controls(ui, &HEAVY_MESH_LEVELS, false, false);
    assert_eq!(
        saved_options(&app, &app_dir, "SSE").meshes_optimization_level,
        1
    );
}

#[test]
fn several_mods_loaded_without_mesh_work_keeps_it_off() {
    headless();
    // Level 0 is the unchecked group, which Several Mods allows.
    let app_dir = app_on("several_mods_no_meshes", "SSE");
    edit_ini(&app_dir, "SSE/settings.ini", "mode", "1");
    edit_ini(
        &app_dir,
        "SSE/settings.ini",
        "iMeshesOptimizationLevel",
        "0",
    );
    let (app, _desktop) = open(&app_dir);
    let ui = app.window();
    select_tab(ui, "Meshes");

    assert!(!checked(ui, "Process meshes"));
    assert_eq!(
        saved_options(&app, &app_dir, "SSE").meshes_optimization_level,
        0
    );

    click(ui, "Process meshes");
    assert!(checked(ui, "Necessary optimization"));
    assert_controls(ui, &HEAVY_MESH_LEVELS, false, false);
    assert_eq!(
        saved_options(&app, &app_dir, "SSE").meshes_optimization_level,
        1
    );
}

#[test]
fn several_mods_chosen_while_mesh_work_is_off_holds_once_it_is_back_on() {
    headless();
    let app_dir = app_on("several_mods_meshes_off", "SSE");
    edit_ini(
        &app_dir,
        "SSE/settings.ini",
        "iMeshesOptimizationLevel",
        "3",
    );
    let (app, _desktop) = open(&app_dir);
    let ui = app.window();
    select_tab(ui, "Meshes");

    // The radios are disabled with their group while it is unchecked, so the
    // rule cannot move the selection then.
    click(ui, "Process meshes");
    choose(ui, "Optimization mode", "Several mods");
    click(ui, "Process meshes");

    assert!(checked(ui, "Necessary optimization"));
    assert_controls(ui, &HEAVY_MESH_LEVELS, false, false);
    assert_eq!(
        saved_options(&app, &app_dir, "SSE").meshes_optimization_level,
        1
    );
}

// ---- Games that keep Textures in their own Archive (FO4)

#[test]
fn an_fo4_profile_loads_with_separate_textures_checked_and_disabled() {
    headless();
    let app_dir = app_on("fo4_load", "FO4");
    // A profile C++ could save: FO4 merging textures into the general archive.
    let fo4 = Profiles::new(&app_dir).open("FO4");
    let mut options = fo4.load_options(&Options::default()).unwrap();
    options.bsa_merge_textures = true;
    fo4.save_options(&options).unwrap();
    let (app, _desktop) = open(&app_dir);
    let ui = app.window();
    select_tab(ui, "BSA");

    assert_controls(ui, &["Separate textures BSA"], true, false);
    assert!(!saved_options(&app, &app_dir, "FO4").bsa_merge_textures);
}

#[test]
fn choosing_the_fo4_game_checks_and_disables_separate_textures() {
    headless();
    // A custom profile, so its Game combo is editable.
    let app_dir = app_on("fo4_click", "Custom");
    Profiles::new(&app_dir).create("Custom", "SSE").unwrap();
    edit_ini(&app_dir, "common.ini", "bShowAdvancedSettings", "true");
    let (app, _desktop) = open(&app_dir);
    let ui = app.window();
    select_tab(ui, "BSA");
    click(ui, "Separate textures BSA");
    assert_controls(ui, &["Separate textures BSA"], false, true);

    choose(ui, "Game", "FO4");
    assert_controls(ui, &["Separate textures BSA"], true, false);
    assert!(!saved_options(&app, &app_dir, "Custom").bsa_merge_textures);

    // Another game gives the choice back, as it was left.
    choose(ui, "Game", "SSE");
    assert_controls(ui, &["Separate textures BSA"], true, true);
    click(ui, "Separate textures BSA");
    assert!(saved_options(&app, &app_dir, "Custom").bsa_merge_textures);
}

/// Each Game combo item, with the game it names.
const GAME_CHOICES: [(&str, Game); 3] = [
    ("Skyrim LE", Game::Tes5),
    ("SSE", Game::Sse),
    ("FO4", Game::Fo4),
];

#[test]
fn separate_textures_is_forced_exactly_while_the_game_keeps_textures_separate() {
    headless();
    // A custom profile, so its Game combo is editable.
    let app_dir = app_on("separate_textures_per_game", "Custom");
    Profiles::new(&app_dir).create("Custom", "SSE").unwrap();
    edit_ini(&app_dir, "common.ini", "bShowAdvancedSettings", "true");
    let (app, _desktop) = open(&app_dir);
    let ui = app.window();
    select_tab(ui, "BSA");

    // Forward then back, so every game is reached both from a game that
    // forces the checkbox and from one that gives it back.
    for (label, game) in GAME_CHOICES.iter().chain(GAME_CHOICES.iter().rev()) {
        choose(ui, "Game", label);
        if keeps_textures_separate(*game) {
            assert_controls(ui, &["Separate textures BSA"], true, false);
        } else {
            assert!(enabled(ui, "Separate textures BSA"), "{label} enabled");
            // The user can clear it, so the next forced game starts unchecked.
            if checked(ui, "Separate textures BSA") {
                click(ui, "Separate textures BSA");
            }
            assert!(!checked(ui, "Separate textures BSA"), "{label} cleared");
        }
    }
    // At least one game (FO4) keeps Textures in their own Archive, so the
    // loop above did force the checkbox rather than only giving it back.
    assert!(
        GAME_CHOICES
            .iter()
            .any(|(_, game)| keeps_textures_separate(*game))
    );
}

#[test]
fn choosing_several_mods_shows_its_tutorial() {
    headless();
    let app_dir = app_on("several_mods_tutorial", "SSE");
    edit_ini(&app_dir, "common.ini", "showTutorial", "true");
    let (app, desktop) = open(&app_dir);

    choose(app.window(), "Optimization mode", "Several mods");
    choose(app.window(), "Optimization mode", "One mod");
    assert_eq!(desktop.message_titles(), ["Several mods option"]);
}
