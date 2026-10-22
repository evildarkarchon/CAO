//! The Texture Target in the window (#625): the Output format combo and the
//! unwanted-format list are built from the typed Profile model, so each shows
//! the value its Profile holds, and a Profile whose Texture Target is outside
//! its closed set is reported and never rewritten. Driven headlessly through
//! the real main window.

mod common;

use cao_gui::ui::TexturesTab;
use cao_profiles::{ProfileSettings, Profiles, TextureFormat};
use common::*;
use slint::{ComponentHandle, Model};

/// The text the combo labelled `combo` shows.
fn shown(ui: &cao_gui::ui::MainWindow, combo: &str) -> String {
    control(ui, combo)
        .accessible_value()
        .unwrap_or_default()
        .to_string()
}

/// The names the Textures tab lists as unwanted.
fn listed(ui: &cao_gui::ui::MainWindow) -> Vec<String> {
    ui.global::<TexturesTab>()
        .get_unwanted()
        .iter()
        .map(|name| name.to_string())
        .collect()
}

/// The settings the Profile `name` holds on disk.
fn settings(app_dir: &std::path::Path, name: &str) -> ProfileSettings {
    Profiles::new(app_dir).open(name).load_settings().unwrap()
}

#[test]
fn switching_to_a_profile_with_an_out_of_set_texture_value_reports_it_and_never_rewrites_it() {
    headless();
    for (index, (key, value, reason)) in [
        (
            "texturesFormat",
            "99",
            "`profile.ini`: `Textures/texturesFormat` is `99`; expected one of 98 (BC7_UNORM), \
             83 (BC5_UNORM), 77 (BC3_UNORM), 71 (BC1_UNORM), 28 (R8G8B8A8_UNORM)",
        ),
        (
            "texturesUnwantedFormats",
            "85, 1, 115",
            "`profile.ini`: `Textures/texturesUnwantedFormats` is `1`; expected each entry to \
             be one of 2 (R32G32B32A32_FLOAT), ",
        ),
        // 2^32 + 85, which a 32-bit read would wrap to 85 and save back as `85`.
        (
            "texturesUnwantedFormats",
            "85, 4294967381",
            "`profile.ini`: `Textures/texturesUnwantedFormats` is `4294967381`; expected each \
             entry to be one of 2 (R32G32B32A32_FLOAT), ",
        ),
        (
            "texturesUnwantedFormats",
            "abc",
            "`profile.ini`: `Textures/texturesUnwantedFormats` is `abc`; expected each entry to \
             be one of 2 (R32G32B32A32_FLOAT), ",
        ),
        // A one-element list cut short, which would read as empty and be saved
        // back as `@Invalid()`.
        (
            "texturesUnwantedFormats",
            r"@Variant(\0\0\0\t\0\0\0\x1\0\0\0\x2)",
            r"`profile.ini`: `Textures/texturesUnwantedFormats` is `@Variant(\0\0\0\t\0\0\0\x1\0\0\0\x2)`; expected a list whose entries are each one of 2 (R32G32B32A32_FLOAT), ",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let app_dir = app_dir(&format!("texture_out_of_set_{index}"));
        edit_ini(&app_dir, "common.ini", "profile", "TES5");
        edit_ini(&app_dir, "common.ini", "showTutorial", "false");
        edit_ini(&app_dir, "SSE/profile.ini", key, value);
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
        let expected = format!("The profile `SSE` could not be read: {reason}");
        assert!(text.starts_with(&expected), "{key}: {text}");

        assert!(app.save());
        choose(ui, "Profile", "TES5");
        assert!(
            std::fs::read(&profile_ini).unwrap() == before,
            "{key}: SSE's `profile.ini` changed"
        );
    }
}

#[test]
fn the_texture_target_shows_each_shipped_profiles_values() {
    headless();
    let app_dir = app_dir("texture_target_shipped");
    edit_ini(&app_dir, "common.ini", "profile", "FO4");
    edit_ini(&app_dir, "common.ini", "showTutorial", "false");
    let (app, desktop) = open(&app_dir);
    let ui = app.window();

    // Switch both ways between Profiles with different Texture Targets, so a
    // combo or list left at the previous Profile's value would show.
    for (profile, format, unwanted) in [
        (
            "FO4",
            "BC7 (BC7_UNORM)",
            &["B5G5R5A1_UNORM", "B5G6R5_UNORM", "B4G4R4A4_UNORM"][..],
        ),
        ("TES5", "BC3 (BC3_UNORM)", &["BC7_UNORM", "BC7_UNORM_SRGB"]),
        (
            "SSE",
            "BC7 (BC7_UNORM)",
            &["B5G6R5_UNORM", "B5G5R5A1_UNORM", "B4G4R4A4_UNORM"],
        ),
        ("TES5", "BC3 (BC3_UNORM)", &["BC7_UNORM", "BC7_UNORM_SRGB"]),
    ] {
        if profile != "FO4" {
            choose(ui, "Profile", profile);
        }
        select_tab(ui, "Textures");
        assert_eq!(shown(ui, "Output format"), format, "{profile}");
        assert_eq!(listed(ui), unwanted, "{profile}");
    }
    assert!(desktop.message_titles().is_empty());
}

#[test]
fn every_output_format_the_combo_offers_saves_as_its_typed_value() {
    headless();
    let app_dir = app_dir("texture_target_save");
    // A custom profile, so the advanced groups are editable.
    Profiles::new(&app_dir).create("Custom", "SSE").unwrap();
    edit_ini(&app_dir, "common.ini", "profile", "Custom");
    edit_ini(&app_dir, "common.ini", "showTutorial", "false");
    let (app, _desktop) = open(&app_dir);
    let ui = app.window();

    let formats = [
        ("BC7 (BC7_UNORM)", TextureFormat::Bc7Unorm),
        ("BC5 (BC5_UNORM)", TextureFormat::Bc5Unorm),
        ("BC3 (BC3_UNORM)", TextureFormat::Bc3Unorm),
        ("BC1 (BC1_UNORM)", TextureFormat::Bc1Unorm),
        (
            "Uncompressed (R8G8B8A8_UNORM)",
            TextureFormat::R8G8B8A8Unorm,
        ),
    ];
    // Last first, so the first choice changes the combo.
    for (label, format) in formats.into_iter().rev() {
        select_tab(ui, "Textures");
        choose(ui, "Output format", label);
        assert!(app.save());
        assert_eq!(
            settings(&app_dir, "Custom").textures_format,
            format,
            "{label}"
        );
    }
}
