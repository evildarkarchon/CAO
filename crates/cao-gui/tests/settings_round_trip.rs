//! Settings saved from the window reload identically (#508): through
//! `Profile::save_options`/`load_options` (`settings.ini`) and
//! `save_settings`/`load_settings` (`profile.ini`), and `common.ini`.

mod common;

use cao_gui::ui::{MeshesTab, TexturesTab};
use cao_profiles::ArchiveSize;
use cao_profiles::{
    BsaGame, MeshStreamVersion, OptimizationMode, Options, Profiles, TextureFormat, UnwantedFormat,
};
use common::*;
use slint::ComponentHandle;
use slint::platform::WindowEvent;

/// The bytes of the three INI files the window saves.
fn saved_files(app_dir: &std::path::Path) -> [Vec<u8>; 3] {
    let profiles = app_dir.join("profiles");
    [
        std::fs::read(profiles.join("common.ini")).unwrap(),
        std::fs::read(profiles.join("Custom").join("settings.ini")).unwrap(),
        std::fs::read(profiles.join("Custom").join("profile.ini")).unwrap(),
    ]
}

/// Closes the window from its title bar, which saves it as C++'s `closeEvent` did.
fn close_window(app: &cao_gui::App) {
    app.window()
        .window()
        .dispatch_event(WindowEvent::CloseRequested);
}

#[test]
fn settings_saved_from_the_window_reload_identically() {
    headless();
    let app_dir = app_dir("round_trip");
    // A custom profile, so the advanced groups are editable.
    Profiles::new(&app_dir).create("Custom", "SSE").unwrap();
    edit_ini(&app_dir, "common.ini", "profile", "Custom");
    edit_ini(&app_dir, "common.ini", "showTutorial", "false");

    // ---- Change every kind of control away from the profile's values.
    let (app, _desktop) = open(&app_dir);
    let ui = app.window();
    enter(ui, "Mod folder", r"C:\Mods\Some Mod\");
    // Several Mods brings its UI rule (#509): "Necessary" meshes below.
    choose(ui, "Optimization mode", "Several mods");
    // The shipped `common.ini` shows them; turn them off and back on.
    click(ui, "Show advanced settings");
    assert!(!checked(ui, "Show advanced settings"));
    click(ui, "Show advanced settings");

    select_tab(ui, "BSA");
    click(ui, "Extract BSA");
    click(ui, "Create BSA");
    click(ui, "Separate textures BSA");
    enter(ui, "Maximum size", "3.5");
    // Not FO4, whose UI rule (#509) would force "Separate textures BSA" back on.
    choose(ui, "Game", "Skyrim LE");

    select_tab(ui, "Meshes");
    // SSE ships `iMeshesOptimizationLevel=0`, which unchecks the group.
    assert!(!checked(ui, "Process meshes"));
    click(ui, "Process meshes");
    click(ui, "Resave meshes");
    ui.global::<MeshesTab>().set_stream_index(3); // Fallout 4 (130)

    select_tab(ui, "Textures");
    click(ui, "Compress textures");
    click(ui, "Generate mipmaps");
    // SSE resizes neither way, which unchecks the group.
    assert!(!checked(ui, "Downsizing"));
    click(ui, "Downsizing");
    click(ui, "By fixed size");
    enter(ui, "Fixed width", "1024");
    click(ui, "Enable TGA conversion");
    ui.global::<TexturesTab>().set_output_format_index(2); // BC3

    select_tab(ui, "Animations");
    click(ui, "Necessary optimization");

    close_window(&app);
    drop(app);
    let first = saved_files(&app_dir);

    // ---- What reached the files.
    let profiles = Profiles::new(&app_dir);
    let profile = profiles.open("Custom");
    let options = profile.load_options(&Options::default()).unwrap();
    assert_eq!(options.user_path, "C:/Mods/Some Mod", "QDir::cleanPath");
    assert_eq!(options.mode, OptimizationMode::SeveralMods);
    assert!(options.bsa_extract && options.bsa_create);
    assert!(options.bsa_merge_textures);
    assert_eq!(options.meshes_optimization_level, 1);
    assert!(options.meshes_resave);
    assert!(options.textures_compress && options.textures_mipmaps);
    assert!(options.textures_resize_size && !options.textures_resize_ratio);
    assert_eq!(options.textures_target_width, 1024);
    assert!(options.animations_optimization);
    let settings = profile.load_settings().unwrap();
    assert_eq!(settings.bsa_game, BsaGame::Tes5);
    // 3.5 GiB, in whole bytes.
    assert_eq!(
        settings.max_bsa_uncompressed_size,
        ArchiveSize::from_bytes(3_758_096_384)
    );
    assert_eq!(settings.meshes_stream, MeshStreamVersion::V130);
    assert_eq!(settings.textures_format, TextureFormat::Bc3Unorm);
    assert!(!settings.textures_convert_tga);
    assert_eq!(
        settings.textures_unwanted_formats,
        [
            UnwantedFormat::B5G6R5Unorm,
            UnwantedFormat::B5G5R5A1Unorm,
            UnwantedFormat::B4G4R4A4Unorm,
        ]
    );
    assert!(profiles.load_common().unwrap().show_advanced_settings);

    // ---- A new window shows them all, and saving it changes no byte.
    let (app, _desktop) = open(&app_dir);
    let ui = app.window();
    assert_eq!(
        control(ui, "Mod folder").accessible_value().as_deref(),
        Some("C:/Mods/Some Mod")
    );
    assert_eq!(
        control(ui, "Optimization mode")
            .accessible_value()
            .as_deref(),
        Some("Several mods")
    );
    select_tab(ui, "BSA");
    assert!(checked(ui, "Extract BSA") && checked(ui, "Create BSA"));
    assert!(!checked(ui, "Separate textures BSA"));
    assert_eq!(
        control(ui, "Maximum size").accessible_value().as_deref(),
        Some("3.50")
    );
    assert_eq!(
        control(ui, "Game").accessible_value().as_deref(),
        Some("Skyrim LE")
    );
    select_tab(ui, "Meshes");
    assert!(checked(ui, "Necessary optimization") && !checked(ui, "Full optimization"));
    assert!(checked(ui, "Resave meshes"));
    select_tab(ui, "Textures");
    assert!(checked(ui, "By fixed size") && !checked(ui, "By ratio"));
    assert_eq!(
        control(ui, "Fixed width").accessible_value().as_deref(),
        Some("1024")
    );
    assert_eq!(
        control(ui, "Unwanted formats").accessible_item_count(),
        Some(3)
    );
    select_tab(ui, "Animations");
    assert!(checked(ui, "Necessary optimization"));

    close_window(&app);
    assert_eq!(saved_files(&app_dir), first);
}

#[test]
fn a_shipped_profile_saved_unchanged_reloads_identically() {
    headless();
    let app_dir = app_dir("shipped_round_trip");
    let profiles = Profiles::new(&app_dir);
    let before_options = profiles
        .open("SSE")
        .load_options(&Options::default())
        .unwrap();
    let before_settings = profiles.open("SSE").load_settings().unwrap();
    edit_ini(&app_dir, "common.ini", "profile", "SSE");

    let (app, _desktop) = open(&app_dir);
    close_window(&app);

    assert_eq!(
        profiles
            .open("SSE")
            .load_options(&Options::default())
            .unwrap(),
        before_options
    );
    assert_eq!(
        profiles.open("SSE").load_settings().unwrap(),
        before_settings
    );
}
