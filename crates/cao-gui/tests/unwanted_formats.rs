//! The unwanted-formats dialog (#509), driven headlessly through the real
//! `.slint` components (spec seam 4). It edits a scratch copy of the Textures
//! tab's list: OK commits it, and Cancel, Escape or the title bar discard it
//! (deviation 2; C++ committed on any close).

mod common;

use std::path::{Path, PathBuf};

use cao_gui::App;
use cao_gui::modal_depth;
use cao_gui::ui::{FormatsDialog, TexturesTab};
use cao_profiles::{ClosedSet, Profiles};
use common::*;
use slint::platform::{Key, WindowEvent};
use slint::{ComponentHandle, Model};

/// What the shipped SSE profile lists, by name and in `profile.ini` order.
const SSE_UNWANTED: [&str; 3] = ["B5G6R5_UNORM", "B5G5R5A1_UNORM", "B4G4R4A4_UNORM"];
/// The same, as `profile.ini` stores them.
const SSE_UNWANTED_NUMBERS: [u32; 3] = [85, 86, 115];

/// An app directory on a custom profile copied from SSE (so its advanced
/// settings are editable), with advanced settings shown and no tutorials.
fn custom_app(name: &str) -> PathBuf {
    let app_dir = app_dir(name);
    Profiles::new(&app_dir).create("Custom", "SSE").unwrap();
    edit_ini(&app_dir, "common.ini", "profile", "Custom");
    edit_ini(&app_dir, "common.ini", "showTutorial", "false");
    edit_ini(&app_dir, "common.ini", "bShowAdvancedSettings", "true");
    app_dir
}

/// The names the Textures tab lists as unwanted.
fn listed(app: &App) -> Vec<String> {
    app.window()
        .global::<TexturesTab>()
        .get_unwanted()
        .iter()
        .map(|name| name.to_string())
        .collect()
}

/// Saves the window and reads back what `profile.ini` of Custom now lists, as
/// the numbers it stores.
fn saved_unwanted(app: &App, app_dir: &Path) -> Vec<u32> {
    app.save();
    Profiles::new(app_dir)
        .open("Custom")
        .load_settings()
        .unwrap()
        .textures_unwanted_formats
        .iter()
        .map(|format| format.number())
        .collect()
}

/// Clicks the Textures tab's Edit button and returns the dialog it opens.
fn open_dialog(app: &App) -> FormatsDialog {
    select_tab(app.window(), "Textures");
    click(app.window(), "Edit");
    let dialog = app.formats_dialog().expect("the formats dialog is open");
    assert_eq!(modal_depth(), 1);
    dialog
}

/// How the dialog is closed without OK.
#[derive(Debug, Clone, Copy)]
enum Discard {
    Cancel,
    TitleBar,
    Escape,
}

#[test]
fn cancel_escape_and_the_title_bar_discard_the_dialog_s_edits() {
    headless();
    let app_dir = custom_app("formats_cancel");
    let (app, _desktop) = open(&app_dir);
    assert_eq!(listed(&app), SSE_UNWANTED);

    for how in [Discard::Cancel, Discard::TitleBar, Discard::Escape] {
        let dialog = open_dialog(&app);
        enter(&dialog, "Search", "BC7");
        click(&dialog, "BC7_UNORM");
        assert!(checked(&dialog, "BC7_UNORM"));
        enter(&dialog, "Search", "B5G6");
        click(&dialog, "B5G6R5_UNORM");
        assert!(!checked(&dialog, "B5G6R5_UNORM"));

        match how {
            Discard::Cancel => click(&dialog, "Cancel"),
            Discard::TitleBar => dialog.window().dispatch_event(WindowEvent::CloseRequested),
            // From the search box, which the dialog focuses on opening.
            Discard::Escape => press(&dialog, Key::Escape),
        }
        next_turn();
        assert!(app.formats_dialog().is_none(), "{how:?}");
        assert_eq!(modal_depth(), 0);
        assert_eq!(listed(&app), SSE_UNWANTED, "{how:?}");
    }

    // Nothing reached the profile, and the next dialog starts from the list.
    assert_eq!(saved_unwanted(&app, &app_dir), SSE_UNWANTED_NUMBERS);
    let dialog = open_dialog(&app);
    enter(&dialog, "Search", "BC7");
    assert!(!checked(&dialog, "BC7_UNORM"));
    enter(&dialog, "Search", "B5G6");
    assert!(checked(&dialog, "B5G6R5_UNORM"));
}

/// How many formats the dialog's list shows.
fn shown(dialog: &FormatsDialog) -> usize {
    control(dialog, "Formats").accessible_item_count().unwrap()
}

#[test]
fn the_search_filters_the_list_without_losing_checked_state() {
    headless();
    let (app, _desktop) = open(&custom_app("formats_search"));
    let dialog = open_dialog(&app);
    assert_eq!(shown(&dialog), 75);

    // Case-insensitive, matching anywhere in the name, as Qt's `MatchContains`.
    enter(&dialog, "Search", "bc7");
    assert_eq!(shown(&dialog), 2);
    click(&dialog, "BC7_UNORM_SRGB");

    enter(&dialog, "Search", "g5r5");
    assert_eq!(shown(&dialog), 1);
    assert!(checked(&dialog, "B5G5R5A1_UNORM"));
    click(&dialog, "B5G5R5A1_UNORM");

    enter(&dialog, "Search", "");
    assert_eq!(shown(&dialog), 75);
    enter(&dialog, "Search", "BC");
    assert_eq!(shown(&dialog), 14);
    assert!(checked(&dialog, "BC7_UNORM_SRGB"));
    assert!(!checked(&dialog, "BC7_UNORM") && !checked(&dialog, "BC1_UNORM_SRGB"));

    // OK takes every checked row, hidden ones too, in the dialog's order
    // (`texturesformats.h`).
    click(&dialog, "OK");
    assert_eq!(
        listed(&app),
        ["B5G6R5_UNORM", "BC7_UNORM_SRGB", "B4G4R4A4_UNORM"]
    );
}

#[test]
fn ok_commits_the_dialog_and_a_one_element_list_is_kept() {
    headless();
    let app_dir = custom_app("formats_ok");
    let (app, _desktop) = open(&app_dir);
    let dialog = open_dialog(&app);

    for name in SSE_UNWANTED {
        enter(&dialog, "Search", name);
        click(&dialog, name);
    }
    enter(&dialog, "Search", "BC7_UNORM");
    click(&dialog, "BC7_UNORM");
    click(&dialog, "OK");
    assert!(app.formats_dialog().is_none());
    assert_eq!(modal_depth(), 0);
    assert_eq!(listed(&app), ["BC7_UNORM"]);
    assert_eq!(
        control(app.window(), "Unwanted formats").accessible_item_count(),
        Some(1)
    );

    // Saved as the plain number, and read back as one element.
    assert_eq!(saved_unwanted(&app, &app_dir), [98]);
    let ini = std::fs::read_to_string(app_dir.join("profiles/Custom/profile.ini")).unwrap();
    assert!(
        ini.lines().any(|line| line == "texturesUnwantedFormats=98"),
        "{ini}"
    );
}

/// The dialog, with a title bar, fits the smallest work area CAO is used on:
/// 1920×1080 at 150% with a 48px taskbar leaves 688 logical pixels, less than
/// 1366×768 at 100% does. Its OK and Cancel row lies inside it (GUI-6).
#[test]
fn the_dialog_fits_a_small_screen_with_its_buttons_inside() {
    /// The work area's height in logical pixels: (1080 - 48) / 1.5.
    const WORK_AREA_HEIGHT: f32 = 688.0;
    /// A Windows 11 title bar at 100%, with some slack.
    const TITLE_BAR: f32 = 40.0;
    headless();
    let (app, _desktop) = open(&custom_app("formats_size"));
    let dialog = open_dialog(&app);
    let height = dialog
        .window()
        .size()
        .to_logical(dialog.window().scale_factor())
        .height;

    assert!(height + TITLE_BAR <= WORK_AREA_HEIGHT, "{height}");
    for button in ["OK", "Cancel"] {
        let button = control(&dialog, button);
        assert!(
            button.absolute_position().y + button.size().height <= height,
            "{:?} below {height}",
            button.absolute_position()
        );
    }
}
