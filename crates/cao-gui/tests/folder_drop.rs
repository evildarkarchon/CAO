//! Dropping a folder from Explorer onto the main window (#512). The headless
//! backend has no winit window, so the tests hand the window the same winit
//! events the real one receives.

mod common;

use std::path::Path;

use common::*;
use i_slint_backend_testing::{AccessibleRole, ElementHandle};
use slint::winit_030::winit::event::WindowEvent;

const DROP_HINT: &str = "Drop a folder to set the mod path";

/// What the "Mod folder" box shows.
fn mod_folder(ui: &cao_gui::ui::MainWindow) -> String {
    control(ui, "Mod folder")
        .accessible_value()
        .unwrap_or_default()
        .to_string()
}

/// Whether the drop hint covers the window.
fn hint_shown(ui: &cao_gui::ui::MainWindow) -> bool {
    ElementHandle::find_by_accessible_label(ui, DROP_HINT)
        .any(|element| element.accessible_role() == Some(AccessibleRole::Text))
}

/// Drags `paths` over the window and drops them, as Explorer does: one
/// `HoveredFile` per path, then one `DroppedFile` per path.
fn drop_paths(app: &cao_gui::App, paths: &[&Path]) {
    for path in paths {
        app.window_event(&WindowEvent::HoveredFile(path.to_path_buf()));
    }
    for path in paths {
        app.window_event(&WindowEvent::DroppedFile(path.to_path_buf()));
    }
    next_turn();
}

#[test]
fn dropping_a_folder_selects_it_as_the_mod_folder() {
    headless();
    let app_dir = app_dir("drop_folder");
    let mods = scratch_dir("drop_folder_mods");
    let (app, _desktop) = open(&app_dir);

    drop_paths(&app, &[&mods]);

    assert_eq!(mod_folder(app.window()), mods.display().to_string());
}

#[test]
fn dropping_an_existing_file_selects_it_too() {
    headless();
    let app_dir = app_dir("drop_file");
    let file = scratch_dir("drop_file_mods").join("readme.txt");
    std::fs::write(&file, "x").unwrap();
    let (app, _desktop) = open(&app_dir);

    // C++ accepted any existing path (`QDir::exists`), and so does the port.
    drop_paths(&app, &[&file]);

    assert_eq!(mod_folder(app.window()), file.display().to_string());
}

#[test]
fn dropping_a_missing_path_leaves_the_mod_folder_alone() {
    headless();
    let app_dir = app_dir("drop_missing");
    let (app, _desktop) = open(&app_dir);
    let before = mod_folder(app.window());

    drop_paths(&app, &[&app_dir.join("no such folder")]);

    assert_eq!(mod_folder(app.window()), before);
}

#[test]
fn only_the_first_of_several_dropped_paths_counts() {
    headless();
    let app_dir = app_dir("drop_several");
    let first = scratch_dir("drop_several_first");
    let second = scratch_dir("drop_several_second");
    let (app, _desktop) = open(&app_dir);

    drop_paths(&app, &[&first, &second]);

    assert_eq!(mod_folder(app.window()), first.display().to_string());
}

#[test]
fn a_drag_over_the_window_shows_the_drop_hint_until_it_leaves() {
    headless();
    let app_dir = app_dir("drop_hint");
    let mods = scratch_dir("drop_hint_mods");
    let (app, _desktop) = open(&app_dir);
    let ui = app.window();
    assert!(!hint_shown(ui));

    app.window_event(&WindowEvent::HoveredFile(mods.clone()));
    next_turn();
    assert!(hint_shown(ui));

    app.window_event(&WindowEvent::HoveredFileCancelled);
    next_turn();
    assert!(!hint_shown(ui));
}

#[test]
fn a_drop_while_a_dialog_is_open_is_ignored() {
    headless();
    let app_dir = app_dir("drop_modal");
    let mods = scratch_dir("drop_modal_mods");
    let (app, _desktop) = open(&app_dir);
    let ui = app.window();
    let before = mod_folder(ui);
    click_menu(ui, "Help");
    click_menu(ui, "About Slint");
    assert!(app.about_slint_dialog().is_some());

    // As Qt ignores drops on a window blocked by a modal dialog.
    app.window_event(&WindowEvent::HoveredFile(mods.clone()));
    next_turn();
    assert!(!hint_shown(ui));
    drop_paths(&app, &[&mods]);

    assert_eq!(mod_folder(ui), before);
}

#[test]
fn dropping_hides_the_drop_hint() {
    headless();
    let app_dir = app_dir("drop_hint_dropped");
    let mods = scratch_dir("drop_hint_dropped_mods");
    let (app, _desktop) = open(&app_dir);

    drop_paths(&app, &[&mods]);

    assert!(!hint_shown(app.window()));
}
