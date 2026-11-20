//! Modal Slint dialogs over the main window (#508): every way of closing every
//! dialog leaves the main window enabled and clickable.

mod common;

use cao_gui::modal_depth;
use cao_gui::ui::MainWindow;
use common::*;
use slint::ComponentHandle;
use slint::platform::{PointerEventButton, WindowEvent};

/// How a dialog is closed.
#[derive(Debug, Clone, Copy)]
enum Close {
    Ok,
    Cancel,
    TitleBar,
}

/// Clicks "Dry run" with the pointer and reports whether it toggled, then
/// restores it. A pointer click, not an accessibility action, so input the
/// open dialog blocks is really blocked.
fn dry_run_clickable(ui: &MainWindow) -> bool {
    let before = checked(ui, "Dry run");
    control(ui, "Dry run").mock_single_click(PointerEventButton::Left);
    let toggled = checked(ui, "Dry run") != before;
    if toggled {
        control(ui, "Dry run").mock_single_click(PointerEventButton::Left);
    }
    toggled
}

fn assert_blocked(ui: &MainWindow) {
    assert_eq!(modal_depth(), 1);
    assert!(ui.get_modal_blocked());
    assert!(
        !dry_run_clickable(ui),
        "the main window took a click under a dialog"
    );
}

fn assert_usable(ui: &MainWindow) {
    assert_eq!(modal_depth(), 0);
    assert!(!ui.get_modal_blocked());
    assert!(dry_run_clickable(ui), "the main window ignores clicks");
}

/// Closes `dialog` the way `how` says, then lets its deferred teardown run.
fn close(dialog: &impl ComponentHandle, how: Close) {
    match how {
        Close::Ok => control(dialog, "OK").invoke_accessible_default_action(),
        Close::Cancel => control(dialog, "Cancel").invoke_accessible_default_action(),
        Close::TitleBar => dialog.window().dispatch_event(WindowEvent::CloseRequested),
    }
    next_turn();
}

#[test]
fn about_slint_closes_with_ok_and_the_title_bar() {
    headless();
    let (app, _desktop) = open(&app_dir("about_slint"));
    let ui = app.window();
    assert_usable(ui);

    for how in [Close::Ok, Close::TitleBar] {
        click_menu(ui, "Help");
        click_menu(ui, "About Slint");
        let dialog = app.about_slint_dialog().expect("About Slint is open");
        assert!(dialog.window().is_visible());
        assert_blocked(ui);

        close(&dialog, how);
        assert!(app.about_slint_dialog().is_none(), "{how:?}");
        assert!(!dialog.window().is_visible(), "{how:?}");
        assert_usable(ui);
    }
}

#[test]
fn the_profile_name_dialog_closes_every_way() {
    headless();
    let app_dir = app_dir("name_dialog");
    edit_ini(&app_dir, "common.ini", "showTutorial", "false");
    let (app, _desktop) = open(&app_dir);
    let ui = app.window();

    for how in [Close::Ok, Close::Cancel, Close::TitleBar] {
        click(ui, "New");
        let dialog = app.name_dialog().expect("the name dialog is open");
        assert_blocked(ui);

        // An empty name ends the flow on OK too, as in C++.
        close(&dialog, how);
        assert!(app.name_dialog().is_none(), "{how:?}");
        assert!(app.base_dialog().is_none(), "{how:?}");
        assert_usable(ui);
    }
}

/// A taken name, in any case, is refused in the name dialog itself: it stays
/// open with the reason, the base dialog never opens, and the shipped profile
/// keeps its `isBase` marker. A corrected name then goes on (GUI-1).
#[test]
fn the_name_dialog_refuses_a_taken_name_in_place() {
    headless();
    let app_dir = app_dir("name_dialog_taken");
    edit_ini(&app_dir, "common.ini", "showTutorial", "false");
    let (app, desktop) = open(&app_dir);
    let ui = app.window();

    click(ui, "New");
    let name = app.name_dialog().expect("the name dialog is open");
    assert_eq!(name.get_error(), "");
    for taken in ["SSE", "sse"] {
        name.set_value(taken.into());
        close(&name, Close::Ok);
        assert!(app.name_dialog().is_some(), "{taken}: the dialog closed");
        assert!(
            app.base_dialog().is_none(),
            "{taken}: the base was asked for"
        );
        assert!(
            name.get_error().contains("already exists"),
            "{taken}: {}",
            name.get_error()
        );
        assert_blocked(ui);
    }
    // The reason is shown in the dialog, not in a message box.
    assert!(desktop.message_titles().is_empty());
    let profiles = cao_profiles::Profiles::new(&app_dir);
    assert!(profiles.open("SSE").is_base());

    name.set_value("Mine".into());
    close(&name, Close::Ok);
    let base = app.base_dialog().expect("the base dialog is open");
    assert!(app.name_dialog().is_none());
    close(&base, Close::Cancel);
    assert!(!profiles.exists("Mine"), "Cancel creates nothing");
    assert_usable(ui);
}

#[test]
fn the_base_profile_dialog_closes_every_way() {
    headless();
    let app_dir = app_dir("base_dialog");
    edit_ini(&app_dir, "common.ini", "showTutorial", "false");
    let (app, _desktop) = open(&app_dir);
    let ui = app.window();

    for how in [Close::Ok, Close::Cancel, Close::TitleBar] {
        click(ui, "New");
        let name = app.name_dialog().expect("the name dialog is open");
        name.set_value(format!("Custom {how:?}").into());
        close(&name, Close::Ok);

        // The name dialog closed and the base dialog opened in the same turn;
        // the main window stayed disabled between them.
        let base = app.base_dialog().expect("the base dialog is open");
        assert!(app.name_dialog().is_none());
        assert_blocked(ui);

        close(&base, how);
        assert!(app.base_dialog().is_none(), "{how:?}");
        assert_usable(ui);
    }
}

#[test]
fn new_profile_creates_the_profile_from_its_base_and_selects_it() {
    headless();
    let app_dir = app_dir("new_profile");
    let (app, desktop) = open(&app_dir);
    let ui = app.window();

    click(ui, "New");
    // With tutorials on, the tutorial box comes first.
    assert_eq!(desktop.message_titles(), ["New profile"]);
    let name = app.name_dialog().expect("the name dialog is open");
    name.set_value("My FO4".into());
    close(&name, Close::Ok);
    let base = app.base_dialog().expect("the base dialog is open");
    base.set_current_index(0); // FO4
    close(&base, Close::Ok);

    let profiles = cao_profiles::Profiles::new(&app_dir);
    assert!(profiles.exists("My FO4"));
    assert!(!profiles.open("My FO4").is_base());
    assert_eq!(
        profiles.open("My FO4").load_settings().unwrap(),
        profiles.open("FO4").load_settings().unwrap()
    );
    assert_eq!(
        control(ui, "Profile").accessible_value().as_deref(),
        Some("My FO4")
    );
    assert_eq!(profiles.load_common().unwrap().profile, "My FO4");
    assert_usable(ui);
}
