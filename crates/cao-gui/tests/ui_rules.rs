//! The window's rules (`rules.rs`), driven headlessly through the real
//! `.slint` components (spec seam 4). Each rule is pinned twice: when the user
//! changes the control that triggers it, and when the settings that trigger it
//! load from the INI files. A rule forces only what a control shows, so each
//! test also saves and checks that the user's choice is kept.

mod common;

use std::path::Path;

use cao_archive::Game;
use cao_archive::Settings;
use cao_gui::App;
use cao_gui::ui::MainWindow;
use cao_profiles::{Options, Profiles};
use common::*;

/// Whether `game` keeps Textures in an Archive of their own, as the run's
/// Archive Rules say.
fn keeps_textures_separate(game: Game) -> bool {
    Settings::get(game).keeps_textures_separate()
}

/// The three base archive options a Dry Run shows unchecked and disabled.
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

/// Turns on the three base archive options of `profile` in `settings.ini`.
fn base_archive_options_on(app_dir: &Path, profile: &str) {
    for key in ["bBsaExtract", "bBsaCreate", "bBsaDeleteBackup"] {
        edit_ini(app_dir, &format!("{profile}/settings.ini"), key, "true");
    }
}

/// Whether `options` keep all three base archive options on.
fn keeps_base_archive_options(options: &Options) -> bool {
    options.bsa_extract && options.bsa_create && options.bsa_delete_backup
}

#[test]
fn clicking_dry_run_shows_the_base_archive_options_off_then_gives_them_back() {
    headless();
    let app_dir = app_on("dry_run_click", "SSE");
    base_archive_options_on(&app_dir, "SSE");
    let (app, _desktop) = open(&app_dir);
    let ui = app.window();
    select_tab(ui, "BSA");
    assert_controls(ui, &BASE_ARCHIVE_OPTIONS, true, true);

    click(ui, "Dry run");
    assert_controls(ui, &BASE_ARCHIVE_OPTIONS, false, false);
    // The rule only shows what a Dry Run does; the choices are saved as made.
    let options = saved_options(&app, &app_dir, "SSE");
    assert!(options.dry_run);
    assert!(keeps_base_archive_options(&options), "{options:?}");

    // Turning it off gives the choices back.
    click(ui, "Dry run");
    assert_controls(ui, &BASE_ARCHIVE_OPTIONS, true, true);
    let options = saved_options(&app, &app_dir, "SSE");
    assert!(!options.dry_run);
    assert!(keeps_base_archive_options(&options), "{options:?}");
}

#[test]
fn a_dry_run_loaded_from_the_ini_shows_the_base_archive_options_off_and_keeps_them() {
    headless();
    let app_dir = app_on("dry_run_load", "SSE");
    base_archive_options_on(&app_dir, "SSE");
    edit_ini(&app_dir, "SSE/settings.ini", "bDryRun", "true");
    let (app, _desktop) = open(&app_dir);
    let ui = app.window();
    select_tab(ui, "BSA");

    assert!(checked(ui, "Dry run"));
    assert_controls(ui, &BASE_ARCHIVE_OPTIONS, false, false);
    let options = saved_options(&app, &app_dir, "SSE");
    assert!(options.dry_run);
    assert!(keeps_base_archive_options(&options), "{options:?}");

    // Turning it off shows the choices that were loaded.
    click(ui, "Dry run");
    assert_controls(ui, &BASE_ARCHIVE_OPTIONS, true, true);
}

#[test]
fn a_save_under_dry_run_keeps_the_extra_archive_options() {
    headless();
    let app_dir = app_on("dry_run_extra", "SSE");
    // Every Extra choice on, and both Separate boxes cleared (merging), so a
    // save reading any of them as off shows on disk.
    let sse = Profiles::new(&app_dir).open("SSE");
    let mut options = sse.load_options(&Options::default()).unwrap();
    options.bsa_compress = true;
    options.bsa_create_dummies = true;
    options.bsa_delete_source = true;
    options.bsa_merge_textures = true;
    options.bsa_merge_incompressible = true;
    sse.save_options(&options).unwrap();
    let (app, _desktop) = open(&app_dir);
    let ui = app.window();
    select_tab(ui, "BSA");

    click(ui, "Dry run");
    // The Extra group stays the user's under a Dry Run.
    assert_controls(
        ui,
        &[
            "Delete source files",
            "Create dummy plugins",
            "Allow compression",
        ],
        true,
        true,
    );
    assert_controls(
        ui,
        &["Separate textures BSA", "Separate incompressible BSA"],
        false,
        true,
    );

    let saved = saved_options(&app, &app_dir, "SSE");
    assert_eq!(
        saved,
        Options {
            dry_run: true,
            ..options
        }
    );
}

// ---- Several Mods

/// The mesh levels above "Necessary", which Several Mods disables.
const HEAVY_MESH_LEVELS: [&str; 2] = ["Medium optimization", "Full optimization"];

/// Asserts the level radios show Necessary, with Medium and Full unchecked
/// and disabled, as Several Mods shows them.
fn assert_necessary_meshes_forced(ui: &MainWindow) {
    assert!(checked(ui, "Necessary optimization"));
    assert_controls(ui, &HEAVY_MESH_LEVELS, false, false);
}

#[test]
fn choosing_several_mods_shows_necessary_meshes_then_one_mod_shows_full_again() {
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
    assert_necessary_meshes_forced(ui);
    assert!(checked(ui, "Process meshes"));
    // The rule only shows the level a Several Mods run uses; the user's
    // level is saved as chosen.
    let options = saved_options(&app, &app_dir, "SSE");
    assert_eq!(options.mode, cao_profiles::OptimizationMode::SeveralMods);
    assert_eq!(options.meshes_optimization_level, 3);

    // Back to one mod: Full shows again, and every level is enabled.
    choose(ui, "Optimization mode", "One mod");
    assert!(checked(ui, "Full optimization") && !checked(ui, "Necessary optimization"));
    assert!(!checked(ui, "Medium optimization"));
    assert!(enabled(ui, "Full optimization") && enabled(ui, "Medium optimization"));
    assert_eq!(
        saved_options(&app, &app_dir, "SSE").meshes_optimization_level,
        3
    );

    // The level is still the user's to change.
    click(ui, "Medium optimization");
    assert!(checked(ui, "Medium optimization") && !checked(ui, "Full optimization"));
    assert_eq!(
        saved_options(&app, &app_dir, "SSE").meshes_optimization_level,
        2
    );
}

#[test]
fn several_mods_loaded_from_the_ini_shows_necessary_meshes_and_keeps_the_level() {
    headless();
    // A profile saved under Several Mods with Full mesh optimization.
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
    assert_necessary_meshes_forced(ui);
    assert_eq!(
        saved_options(&app, &app_dir, "SSE").meshes_optimization_level,
        3
    );

    // A later One Mod run uses the level the profile keeps.
    choose(ui, "Optimization mode", "One mod");
    assert!(checked(ui, "Full optimization"));
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
    assert!(enabled(ui, "Process meshes"));
    assert_eq!(
        saved_options(&app, &app_dir, "SSE").meshes_optimization_level,
        0
    );

    // The radios hold the window's first level, Necessary, since a level 0
    // profile leaves them as they were.
    click(ui, "Process meshes");
    assert_necessary_meshes_forced(ui);
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

    click(ui, "Process meshes");
    choose(ui, "Optimization mode", "Several mods");
    click(ui, "Process meshes");

    assert_necessary_meshes_forced(ui);
    assert_eq!(
        saved_options(&app, &app_dir, "SSE").meshes_optimization_level,
        3
    );
    choose(ui, "Optimization mode", "One mod");
    assert!(checked(ui, "Full optimization"));
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
    // The rule only shows what the run does; the user's choice is saved as it
    // was loaded.
    assert!(saved_options(&app, &app_dir, "FO4").bsa_merge_textures);
}

#[test]
fn previewing_fo4_forces_separate_textures_then_gives_the_users_choice_back() {
    headless();
    // A custom profile, so its Game combo is editable.
    let app_dir = app_on("fo4_click", "Custom");
    Profiles::new(&app_dir).create("Custom", "SSE").unwrap();
    edit_ini(&app_dir, "common.ini", "bShowAdvancedSettings", "true");
    let (app, _desktop) = open(&app_dir);
    let ui = app.window();
    select_tab(ui, "BSA");
    // The user's choice on SSE: merge Textures into the Standard Archive.
    click(ui, "Separate textures BSA");
    assert_controls(ui, &["Separate textures BSA"], false, true);

    choose(ui, "Game", "FO4");
    assert_controls(ui, &["Separate textures BSA"], true, false);
    // A save while FO4 is shown saves the choice, not what FO4 forces.
    assert!(saved_options(&app, &app_dir, "Custom").bsa_merge_textures);

    // Back on SSE the checkbox shows the user's choice again, and a save
    // keeps it.
    choose(ui, "Game", "SSE");
    assert_controls(ui, &["Separate textures BSA"], false, true);
    assert!(saved_options(&app, &app_dir, "Custom").bsa_merge_textures);

    // The choice is still the user's to change.
    click(ui, "Separate textures BSA");
    assert!(!saved_options(&app, &app_dir, "Custom").bsa_merge_textures);
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
