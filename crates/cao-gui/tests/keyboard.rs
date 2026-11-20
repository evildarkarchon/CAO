//! Keyboard access to the hand-drawn controls (GUI-5): the tab bar, the
//! Downsizing radios and the Maximum size spin box answer the keys their native
//! counterparts do, show a focus ring, and the help each option carries reaches
//! keyboard and screen-reader users too. Driven headlessly through the real
//! main window, by Tab and key presses only.

mod common;

use std::time::Duration;

use cao_gui::ui::{HelpTip, MainWindow};
use cao_profiles::Profiles;
use common::*;
use i_slint_backend_testing::ElementHandle;
use slint::ComponentHandle;
use slint::platform::{Key, PointerEventButton};

/// QToolTip-like delay before a tip shows, plus some slack.
const HOVER_DELAY: Duration = Duration::from_millis(800);

const DRY_RUN_HELP: &str = "Will list all files that would be modified, without actually processing them. Will ignore files in BSAs.";

const BY_RATIO_HELP: &str = "Resizes textures, dividing the current width and height by the values given. Must result in texture dimensions expressed in powers of two, no less than 4x4.";

/// Opens the shipped profile `profile` without tutorial boxes.
fn open_on(name: &str, profile: &str) -> (std::rc::Rc<cao_gui::App>, std::rc::Rc<FakeDesktop>) {
    let app_dir = app_dir(name);
    edit_ini(&app_dir, "common.ini", "profile", profile);
    edit_ini(&app_dir, "common.ini", "showTutorial", "false");
    open(&app_dir)
}

/// Presses Tab until `reached` holds, as a keyboard user moves focus through
/// the window, and fails when sixty presses never get there.
fn tab_until(ui: &MainWindow, mut reached: impl FnMut() -> bool) {
    for _ in 0..60 {
        if reached() {
            return;
        }
        press(ui, Key::Tab);
    }
    panic!("Tab never reached the expected control");
}

/// The focus rings with element id `id` shown in `ui`.
fn rings(ui: &MainWindow, id: &str) -> Vec<ElementHandle> {
    ElementHandle::find_by_element_id(ui, id).collect()
}

/// Whether the tab titled `title` is the selected one.
fn selected(ui: &MainWindow, title: &str) -> bool {
    control(ui, title).accessible_item_selected() == Some(true)
}

#[test]
fn the_tab_bar_is_one_tab_stop_whose_arrows_select_the_enabled_tabs() {
    headless();
    // TES5 has no Animations, so its tab is disabled and the arrows skip it.
    let (app, _desktop) = open_on("keys_tab_bar", "TES5");
    let ui = app.window();
    select_tab(ui, "BSA");
    assert!(!enabled(ui, "Animations"));
    assert!(rings(ui, "CaoTabBar::focus-ring").is_empty());

    tab_until(ui, || !rings(ui, "CaoTabBar::focus-ring").is_empty());

    for (key, expected) in [
        (Key::RightArrow, "Meshes"),
        (Key::RightArrow, "Textures"),
        (Key::RightArrow, "Log"),
        // The last tab stops the arrow, as in a native tab control.
        (Key::RightArrow, "Log"),
        (Key::Home, "BSA"),
        (Key::LeftArrow, "BSA"),
        (Key::End, "Log"),
        (Key::LeftArrow, "Textures"),
    ] {
        press(ui, key);
        assert!(selected(ui, expected), "{key:?} should select {expected}");
        assert_eq!(
            rings(ui, "CaoTabBar::focus-ring").len(),
            1,
            "the ring stays on the selected tab"
        );
    }

    // One Tab stop: the next press leaves the bar for the page.
    press(ui, Key::Tab);
    assert!(rings(ui, "CaoTabBar::focus-ring").is_empty());
}

#[test]
fn the_downsizing_radios_take_focus_and_answer_space_and_the_arrows() {
    headless();
    let (app, _desktop) = open_on("keys_radios", "SSE");
    let ui = app.window();
    select_tab(ui, "Textures");
    if !checked(ui, "Downsizing") {
        click(ui, "Downsizing");
    }
    let by_ratio = control(ui, "By ratio");
    let by_size = control(ui, "By fixed size");
    assert_eq!(
        by_ratio.accessible_description().as_deref(),
        Some(BY_RATIO_HELP)
    );
    // The ring shown, if one is, and where it is.
    let ring_at = || {
        let found = rings(ui, "HelpRadio::focus-ring");
        assert!(found.len() <= 1, "two radios show a focus ring");
        found.first().map(ElementHandle::absolute_position)
    };

    tab_until(ui, || ring_at().is_some());
    assert_eq!(ring_at(), Some(by_ratio.absolute_position()));

    press(ui, Key::Space);
    assert!(checked(ui, "By ratio") && !checked(ui, "By fixed size"));
    press(ui, Key::DownArrow);
    assert!(checked(ui, "By fixed size") && !checked(ui, "By ratio"));
    assert_eq!(ring_at(), Some(by_size.absolute_position()));
    // Two radios: either arrow moves to the other, as a native group wraps.
    press(ui, Key::DownArrow);
    assert!(checked(ui, "By ratio"));
    assert_eq!(ring_at(), Some(by_ratio.absolute_position()));
    press(ui, Key::LeftArrow);
    assert!(checked(ui, "By fixed size"));
}

#[test]
fn an_options_help_shows_while_it_has_keyboard_focus_and_is_its_description() {
    headless();
    let (app, _desktop) = open_on("keys_help", "SSE");
    let ui = app.window();
    let tip_shows = || {
        let tip = ui.global::<HelpTip>();
        tip.get_visible() && tip.get_text() == DRY_RUN_HELP
    };
    assert_eq!(
        control(ui, "Dry run").accessible_description().as_deref(),
        Some(DRY_RUN_HELP)
    );
    // An option with a warning describes itself with both.
    let extract = control(ui, "Extract BSA")
        .accessible_description()
        .unwrap_or_default();
    assert!(
        extract.starts_with("Extracts any BSAs") && extract.contains("considerably slowed down"),
        "{extract}"
    );

    tab_until(ui, || {
        i_slint_backend_testing::mock_elapsed_time(HOVER_DELAY);
        next_turn();
        tip_shows()
    });

    // The tip goes once the focus moves on, with the pointer nowhere near.
    press(ui, Key::Tab);
    assert!(!tip_shows());
}

#[test]
fn up_and_down_step_the_maximum_size() {
    headless();
    let app_dir = app_dir("keys_spin_box");
    // A custom profile, so the advanced group is editable; FO4's Archives
    // have no 2 GB cap to stop the step.
    Profiles::new(&app_dir).create("Custom", "FO4").unwrap();
    edit_ini(&app_dir, "common.ini", "profile", "Custom");
    edit_ini(&app_dir, "common.ini", "showTutorial", "false");
    edit_ini(&app_dir, "common.ini", "bShowAdvancedSettings", "true");
    let (app, _desktop) = open(&app_dir);
    let ui = app.window();
    select_tab(ui, "BSA");
    let spin = control(ui, "Maximum size");
    let shown = || spin.accessible_value().unwrap_or_default().to_string();
    // The box shows two decimals, so the steps are compared as it shows them.
    let start: f32 = shown().parse().unwrap();
    let stepped = |by: f32| format!("{:.2}", start + by);

    // A click in the text puts the keyboard focus there.
    spin.mock_single_click(PointerEventButton::Left);
    next_turn();
    press(ui, Key::UpArrow);
    assert_eq!(shown(), stepped(1.0));
    press(ui, Key::DownArrow);
    press(ui, Key::DownArrow);
    assert_eq!(shown(), stepped(-1.0));
}
