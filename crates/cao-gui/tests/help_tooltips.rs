//! The help tooltips and the help cursor (#512), driven headlessly through the
//! real `.slint` widgets (spec seam 4).

mod common;

use std::time::Duration;

use cao_gui::ui::HelpTip;
use common::*;
use i_slint_backend_testing::{AccessibleRole, ElementHandle};
use slint::platform::WindowEvent;
use slint::{ComponentHandle, LogicalPosition, LogicalSize};

/// The main window's design size (`C:MainWindow.ui`).
const WINDOW: LogicalSize = LogicalSize {
    width: 676.0,
    height: 700.0,
};

/// QToolTip-like delay before a tip shows, plus some slack.
const HOVER_DELAY: Duration = Duration::from_millis(800);

const EXTRACT_HELP: &str = "Extracts any BSAs present in the mod folder.";

/// Opens SSE, which enables every tab, at the design size.
fn open_sse(name: &str) -> (std::rc::Rc<cao_gui::App>, std::rc::Rc<FakeDesktop>) {
    let app_dir = app_dir(name);
    edit_ini(&app_dir, "common.ini", "profile", "SSE");
    edit_ini(&app_dir, "common.ini", "showTutorial", "false");
    let (app, desktop) = open(&app_dir);
    app.window().window().set_size(WINDOW);
    next_turn();
    (app, desktop)
}

/// Moves the pointer to `at` (window coordinates) and rests it there for the
/// tooltip delay.
fn hover_at(ui: &impl ComponentHandle, at: LogicalPosition) {
    ui.window()
        .dispatch_event(WindowEvent::PointerMoved { position: at });
    next_turn();
    i_slint_backend_testing::mock_elapsed_time(HOVER_DELAY);
    next_turn();
}

/// Rests the pointer on the middle of `element`.
fn hover(ui: &impl ComponentHandle, element: &ElementHandle) {
    let at = element.absolute_position();
    let size = element.size();
    hover_at(
        ui,
        LogicalPosition::new(at.x + size.width / 2.0, at.y + size.height / 2.0),
    );
}

/// Whether a tooltip showing `text` is on screen.
fn tip_shows(ui: &impl ComponentHandle, text: &str) -> bool {
    ElementHandle::find_by_accessible_label(ui, text)
        .any(|element| element.accessible_role() == Some(AccessibleRole::Text))
}

#[test]
fn resting_on_an_option_shows_its_help_after_a_delay() {
    headless();
    let (app, _desktop) = open_sse("tip_delay");
    let ui = app.window();
    assert!(!tip_shows(ui, EXTRACT_HELP));

    hover(ui, &control(ui, "Extract BSA"));

    assert!(tip_shows(ui, EXTRACT_HELP));
}

#[test]
fn a_disabled_option_still_shows_its_help() {
    headless();
    let (app, _desktop) = open_sse("tip_disabled");
    let ui = app.window();
    // Dry Run disables the BSA base group.
    click(ui, "Dry run");
    assert!(!enabled(ui, "Extract BSA"));

    hover(ui, &control(ui, "Extract BSA"));

    assert!(tip_shows(ui, EXTRACT_HELP));
}

#[test]
fn a_disabled_mesh_level_still_shows_its_help() {
    headless();
    let (app, _desktop) = open_sse("tip_disabled_radio");
    let ui = app.window();
    select_tab(ui, "Meshes");
    // An unchecked group disables its level radios.
    if checked(ui, "Process meshes") {
        click(ui, "Process meshes");
    }
    assert!(!enabled(ui, "Medium optimization"));

    hover(ui, &control(ui, "Medium optimization"));

    assert!(tip_shows(
        ui,
        "Perform necessary optimization, and also lightly optimizes other meshes. This may fix some visual issues, but may also lower quality."
    ));
}

// ---- Placement: one layer draws every tip, clamped inside the window.

/// A shown tip's box, in window coordinates.
#[derive(Debug, Clone, Copy)]
struct TipBox {
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
}

/// The one element with id `id` in the shared tooltip layer.
fn layer_element(ui: &impl ComponentHandle, id: &str) -> ElementHandle {
    let found: Vec<ElementHandle> =
        ElementHandle::find_by_element_id(ui, &format!("HelpTipLayer::{id}")).collect();
    assert_eq!(
        found.len(),
        1,
        "expected one shown tooltip, found {}",
        found.len()
    );
    found.into_iter().next().unwrap()
}

/// The box of the tip on screen.
fn tip_box(ui: &impl ComponentHandle) -> TipBox {
    let tip = layer_element(ui, "tip");
    let at = tip.absolute_position();
    let size = tip.size();
    TipBox {
        left: at.x,
        top: at.y,
        right: at.x + size.width,
        bottom: at.y + size.height,
    }
}

/// Moves the pointer off every control, which hides any tip.
fn leave(ui: &impl ComponentHandle) {
    ui.window().dispatch_event(WindowEvent::PointerExited);
    next_turn();
}

/// Asserts that `tip` lies wholly inside the window.
fn assert_inside_window(tip: TipBox) {
    assert!(
        tip.left >= 0.0
            && tip.top >= 0.0
            && tip.right <= WINDOW.width
            && tip.bottom <= WINDOW.height,
        "the tip {tip:?} leaves the {WINDOW:?} window"
    );
}

#[test]
fn a_tip_opens_just_below_and_right_of_the_pointer() {
    headless();
    let (app, _desktop) = open_sse("tip_placement");
    let ui = app.window();
    let extract = control(ui, "Extract BSA");
    let at = extract.absolute_position();
    let pointer = LogicalPosition::new(at.x + 20.0, at.y + extract.size().height / 2.0);

    hover_at(ui, pointer);

    let tip = tip_box(ui);
    assert!(
        tip.left >= pointer.x && tip.left <= pointer.x + 10.0,
        "the tip {tip:?} is not beside the pointer at {pointer:?}"
    );
    assert!(
        tip.top > pointer.y && tip.top <= pointer.y + 30.0,
        "the tip {tip:?} is not just below the pointer at {pointer:?}"
    );
}

#[test]
fn a_tip_at_the_right_edge_is_shifted_back_inside_the_window() {
    headless();
    let (app, _desktop) = open_sse("tip_right_edge");
    let ui = app.window();
    // The rightmost option in the BSA tab; its long tip would run past the edge.
    let backups = control(ui, "Delete backups");
    let at = backups.absolute_position();
    let pointer = LogicalPosition::new(
        at.x + backups.size().width - 3.0,
        at.y + backups.size().height / 2.0,
    );

    hover_at(ui, pointer);

    let tip = tip_box(ui);
    assert_inside_window(tip);
    assert!(tip.left < pointer.x, "the tip {tip:?} was not shifted left");
}

/// Publishes a tip for `text` with the pointer at `pointer`, as a `HelpArea`
/// does when its hover delay elapses.
fn publish_tip(ui: &cao_gui::ui::MainWindow, text: &str, pointer: LogicalPosition) {
    let tip = ui.global::<HelpTip>();
    tip.set_text(text.into());
    tip.set_warning("".into());
    tip.set_px(pointer.x);
    tip.set_py(pointer.y);
    tip.set_visible(true);
    next_turn();
}

#[test]
fn a_tip_at_the_bottom_edge_flips_above_the_pointer() {
    headless();
    let (app, _desktop) = open_sse("tip_bottom_edge");
    let ui = app.window();
    // The tip layer must end at the window's bottom edge, or the flip is measured
    // against a layer that overflows the window. The BSA page fits the design size.
    select_tab(ui, "BSA");
    let layer = ElementHandle::find_by_element_type_name(ui, "HelpTipLayer")
        .next()
        .unwrap();
    assert_eq!(
        layer.absolute_position().y + layer.size().height,
        WINDOW.height
    );
    // No help control reaches the bottom edge: with the backend's font metrics, at
    // every page's minimum window size the lowest one ("Unwanted formats") leaves
    // room for its tip below it. So this publishes the pointer position the way a
    // `HelpArea` does, as if a control sat at the bottom edge, and checks the layer.
    let pointer = LogicalPosition::new(40.0, WINDOW.height - 6.0);

    publish_tip(ui, EXTRACT_HELP, pointer);

    let tip = tip_box(ui);
    assert_inside_window(tip);
    assert!(
        tip.bottom <= pointer.y,
        "the tip {tip:?} did not flip above the pointer at {pointer:?}"
    );
}

#[test]
fn a_long_tip_wraps_within_its_width_cap() {
    headless();
    let (app, _desktop) = open_sse("tip_wraps");
    let ui = app.window();
    // A one-line tip gives the line height.
    hover(ui, &control(ui, "Extract BSA"));
    let line = layer_element(ui, "body").size().height;
    leave(ui);

    hover(ui, &control(ui, "Create dummy plugins"));

    let tip = tip_box(ui);
    assert!(
        tip.right - tip.left <= 380.0,
        "the tip {tip:?} is wider than 380px"
    );
    assert!(
        layer_element(ui, "body").size().height >= 2.0 * line,
        "the long tip did not wrap onto several lines"
    );
}

// ---- The help-cursor overlay forwards clicks to the control under it.

/// Clicks the middle of the control labelled `label` with the mouse, through
/// whatever overlay is on top of it.
fn mouse_click(ui: &impl ComponentHandle, label: &str) {
    control(ui, label).mock_single_click(slint::platform::PointerEventButton::Left);
    next_turn();
}

#[test]
fn clicking_an_option_under_the_help_cursor_toggles_it() {
    headless();
    let (app, _desktop) = open_sse("overlay_click");
    let ui = app.window();
    let before = checked(ui, "Delete source files");

    mouse_click(ui, "Delete source files");

    assert_eq!(checked(ui, "Delete source files"), !before);
}

#[test]
fn clicking_a_disabled_option_under_the_help_cursor_leaves_it() {
    headless();
    let (app, _desktop) = open_sse("overlay_click_disabled");
    let ui = app.window();
    click(ui, "Dry run");
    let before = checked(ui, "Extract BSA");

    mouse_click(ui, "Extract BSA");

    assert_eq!(checked(ui, "Extract BSA"), before);
}

#[test]
fn clicking_a_mesh_level_under_the_help_cursor_selects_it() {
    headless();
    let (app, _desktop) = open_sse("overlay_click_radio");
    let ui = app.window();
    select_tab(ui, "Meshes");
    if !checked(ui, "Process meshes") {
        click(ui, "Process meshes");
    }
    assert!(!checked(ui, "Full optimization"));

    mouse_click(ui, "Full optimization");

    assert!(checked(ui, "Full optimization"));
    assert!(!checked(ui, "Necessary optimization") && !checked(ui, "Medium optimization"));
}

#[test]
fn clicking_a_mesh_level_several_mods_disables_under_the_help_cursor_leaves_it() {
    headless();
    let (app, _desktop) = open_sse("overlay_click_several_mods");
    let ui = app.window();
    choose(ui, "Optimization mode", "Several mods");
    select_tab(ui, "Meshes");
    if !checked(ui, "Process meshes") {
        click(ui, "Process meshes");
    }
    // The group is enabled; only the Several Mods rule disables these two.
    assert!(checked(ui, "Necessary optimization"));
    assert!(!enabled(ui, "Medium optimization") && !enabled(ui, "Full optimization"));

    mouse_click(ui, "Medium optimization");
    mouse_click(ui, "Full optimization");

    assert!(checked(ui, "Necessary optimization"));
    assert!(!checked(ui, "Medium optimization") && !checked(ui, "Full optimization"));
}

#[test]
fn pressing_an_option_hides_its_help() {
    headless();
    let (app, _desktop) = open_sse("tip_hidden_on_press");
    let ui = app.window();
    hover(ui, &control(ui, "Extract BSA"));
    assert!(tip_shows(ui, EXTRACT_HELP));

    mouse_click(ui, "Extract BSA");

    assert!(!tip_shows(ui, EXTRACT_HELP));
}
