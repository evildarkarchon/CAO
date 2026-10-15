//! Reordering the main window's tabs by dragging them (#512), driven headlessly
//! through the real `.slint` tab bar (spec seam 4).

mod common;

use common::*;
use i_slint_backend_testing::ElementHandle;
use slint::LogicalPosition;
use slint::platform::PointerEventButton;

/// The tab titles in the order the tab bar shows them, read from each tab's
/// accessible item index.
fn tab_order(ui: &cao_gui::ui::MainWindow) -> Vec<String> {
    let mut tabs: Vec<(usize, String)> = ["BSA", "Meshes", "Textures", "Animations", "Log"]
        .iter()
        .map(|title| {
            let tab = control(ui, title);
            (tab.accessible_item_index().unwrap(), (*title).to_owned())
        })
        .collect();
    tabs.sort();
    tabs.into_iter().map(|(_, title)| title).collect()
}

/// Drags `tab` and releases it `dx` logical pixels left (negative) or right of
/// where it was grabbed.
fn drag_tab(tab: &ElementHandle, dx: f32) {
    let at = tab.absolute_position();
    let size = tab.size();
    let target = LogicalPosition::new(at.x + size.width / 2.0 + dx, at.y + size.height / 2.0);
    tab.mock_drag(target, PointerEventButton::Left);
    next_turn();
}

#[test]
fn dragging_a_tab_to_the_front_moves_it_there_on_release() {
    headless();
    let app_dir = app_dir("tab_to_front");
    edit_ini(&app_dir, "common.ini", "profile", "SSE");
    let (app, _desktop) = open(&app_dir);
    let ui = app.window();
    assert_eq!(
        tab_order(ui),
        ["BSA", "Meshes", "Textures", "Animations", "Log"]
    );

    // From the middle of Textures to the left edge of the bar: two tabs left.
    let textures = control(ui, "Textures");
    drag_tab(&textures, -(2.0 * textures.size().width + 40.0));

    assert_eq!(
        tab_order(ui),
        ["Textures", "BSA", "Meshes", "Animations", "Log"]
    );
}

#[test]
fn dragging_a_tab_right_past_its_neighbour_swaps_them() {
    headless();
    let app_dir = app_dir("tab_right");
    edit_ini(&app_dir, "common.ini", "profile", "SSE");
    let (app, _desktop) = open(&app_dir);
    let ui = app.window();

    // From the middle of BSA to just past the middle of Meshes: the drop slot is
    // between Meshes and Textures.
    let bsa = control(ui, "BSA");
    drag_tab(&bsa, bsa.size().width + 10.0);

    assert_eq!(
        tab_order(ui),
        ["Meshes", "BSA", "Textures", "Animations", "Log"]
    );
}

#[test]
fn a_short_drag_does_not_reorder_the_tabs() {
    headless();
    let app_dir = app_dir("tab_short_drag");
    edit_ini(&app_dir, "common.ini", "profile", "SSE");
    let (app, _desktop) = open(&app_dir);
    let ui = app.window();

    // Under the 8px threshold the press is a plain click on the tab.
    let meshes = control(ui, "Meshes");
    drag_tab(&meshes, -6.0);

    assert_eq!(
        tab_order(ui),
        ["BSA", "Meshes", "Textures", "Animations", "Log"]
    );
    assert_eq!(ui.get_current_tab(), "meshes");
}
