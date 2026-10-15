//! The main window shell (#508), driven headlessly through the real `.slint`
//! components (spec seam 4).

mod common;

use common::*;

/// The controls of the top area, the tab bar and the progress bar.
const ALWAYS_SHOWN: &[&str] = &[
    "Profile",
    "New",
    "Mod folder",
    "Open Directory",
    "Run",
    "Optimization mode",
    "Dry run",
    "Show advanced settings",
    "BSA",
    "Meshes",
    "Textures",
    "Animations",
    "Log",
    "Progress",
];

/// Each tab's controls with advanced settings shown, by tab title.
const TAB_CONTROLS: &[(&str, &[&str])] = &[
    (
        "BSA",
        &[
            "Extract BSA",
            "Create BSA",
            "Delete backups",
            "Delete source files",
            "Create dummy plugins",
            "Allow compression",
            "Separate textures BSA",
            "Separate incompressible BSA",
            "Maximum size",
            "Game",
        ],
    ),
    (
        "Meshes",
        &[
            "Process meshes",
            "Necessary optimization",
            "Medium optimization",
            "Full optimization",
            "Always process headparts",
            "Resave meshes",
            "Version",
            "User",
            "Stream",
        ],
    ),
    (
        "Textures",
        &[
            "Process textures",
            "Necessary optimization",
            "Compress textures",
            "Generate mipmaps",
            "Downsizing",
            "By ratio",
            "Height ratio",
            "Width ratio",
            "By fixed size",
            "Fixed height",
            "Fixed width",
            "Enable TGA conversion",
            "Output format",
            "Compress interface textures",
            "Unwanted formats",
            "Edit",
        ],
    ),
    ("Animations", &["Necessary optimization"]),
];

#[test]
fn every_main_window_control_is_found_by_its_accessible_label() {
    headless();
    // SSE enables every tab.
    let app_dir = app_dir("every_control");
    edit_ini(&app_dir, "common.ini", "profile", "SSE");
    let (app, _desktop) = open(&app_dir);
    let ui = app.window();
    if !checked(ui, "Show advanced settings") {
        click(ui, "Show advanced settings");
    }

    for label in ALWAYS_SHOWN {
        control(ui, label);
    }
    for (tab, labels) in TAB_CONTROLS {
        select_tab(ui, tab);
        for label in *labels {
            control(ui, label);
        }
    }
}

/// Picks `name` in the profile combo, as choosing it from the list would.
fn choose_profile(ui: &cao_gui::ui::MainWindow, name: &str) {
    use slint::Model;
    let index = ui
        .get_profiles()
        .iter()
        .position(|profile| profile == name)
        .unwrap() as i32;
    ui.set_profile_index(index);
    ui.invoke_profile_selected(index);
    next_turn();
}

#[test]
fn choosing_a_profile_enables_its_tabs_and_saves_the_one_left() {
    headless();
    let app_dir = app_dir("choose_profile");
    edit_ini(&app_dir, "common.ini", "profile", "SSE");
    edit_ini(&app_dir, "common.ini", "showTutorial", "false");
    let custom = cao_profiles::Profiles::new(&app_dir)
        .create("Custom", "SSE")
        .unwrap();
    let (app, _desktop) = open(&app_dir);
    let ui = app.window();
    if !checked(ui, "Show advanced settings") {
        click(ui, "Show advanced settings");
    }

    choose_profile(ui, "Custom");
    select_tab(ui, "Meshes");
    click(ui, "Resave meshes");
    // A custom profile's advanced settings are editable.
    assert!(enabled(ui, "Stream"));

    // FO4 has no Meshes or Animations; the selection moves off the disabled tab.
    choose_profile(ui, "FO4");
    assert!(!enabled(ui, "Meshes") && !enabled(ui, "Animations"));
    assert!(enabled(ui, "BSA") && enabled(ui, "Textures"));
    assert!(
        !has_control(ui, "Resave meshes"),
        "the Meshes page is still shown"
    );
    // A base profile's advanced settings are read-only.
    select_tab(ui, "BSA");
    assert!(!enabled(ui, "Maximum size") && !enabled(ui, "Game"));

    // Leaving Custom saved it, as C++'s `setGameMode` does.
    assert!(
        custom
            .load_options(&cao_profiles::Options::default())
            .unwrap()
            .meshes_resave
    );
    assert_eq!(
        cao_profiles::Profiles::new(&app_dir)
            .load_common()
            .unwrap()
            .profile,
        "FO4"
    );
}

#[test]
fn the_native_dialogs_and_links_reach_the_desktop() {
    headless();
    let (app, desktop) = open(&app_dir("desktop"));
    let ui = app.window();

    *desktop.folder.borrow_mut() = Some(std::path::PathBuf::from(r"D:\Mods\Picked"));
    click(ui, "Open Directory");
    assert_eq!(
        control(ui, "Mod folder").accessible_value().as_deref(),
        Some(r"D:\Mods\Picked")
    );

    click_menu(ui, "Help");
    click_menu(ui, "Documentation");
    click_menu(ui, "Help");
    click_menu(ui, "Discord");
    click_menu(ui, "Help");
    click_menu(ui, "About");
    let shown = desktop.shown.borrow();
    assert_eq!(shown[0], Shown::FolderPicker);
    assert_eq!(
        shown[1],
        Shown::Opened("https://www.nexusmods.com/skyrimspecialedition/mods/23316".into())
    );
    assert_eq!(
        shown[2],
        Shown::Opened("https://discordapp.com/invite/B9abN8d".into())
    );
    assert!(
        matches!(&shown[3], Shown::Message { title, text, .. } if title == "About" && text.contains("Made by G'k"))
    );
}

/// The menus of `C:MainWindow.ui`, with About Slint where About Qt was.
const MENUS: &[(&str, &[&str])] = &[
    (
        "Tools",
        &[
            "Enable dark theme",
            "Enable debug log",
            "Show tutorials",
            "Open log file",
        ],
    ),
    (
        "Help",
        &["Documentation", "Discord", "About", "About Slint"],
    ),
];

#[test]
fn every_menu_item_is_found_by_its_text() {
    headless();
    let (app, _desktop) = open(&app_dir("every_menu_item"));
    let ui = app.window();

    for (menu, items) in MENUS {
        click_menu(ui, menu);
        for item in *items {
            menu_entry(ui, item);
        }
        // Clicking the title again closes the menu.
        click_menu(ui, menu);
    }
}

/// Starts the headless backend on this test thread with a real software
/// rasterizer, so the window can be rendered and its pixels read. Text is then
/// measured with the system's fonts rather than the backend's fixed metrics.
fn headless_with_pixels() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            mock_time: true,
            threading: false,
            renderer_name: Some("software".into()),
        },
    )))
    .unwrap();
}

/// What the rendered window shows where the theme is visible: the menu bar
/// beside its entries, the window background below it, and the lightest and
/// darkest pixels of the "Tools" title (its text and the bar behind it).
#[derive(Debug)]
struct ThemeSample {
    menu_bar: u8,
    background: u8,
    title_lightest: u8,
    title_darkest: u8,
}

/// Renders the window and samples it, as luma (0 black, 255 white).
fn sample_theme(ui: &cao_gui::ui::MainWindow) -> ThemeSample {
    use slint::ComponentHandle;
    let shot = ui.window().take_snapshot().unwrap();
    let width = shot.width() as usize;
    let height = shot.height() as usize;
    let luma = |x: usize, y: usize| {
        let pixel = shot.as_slice()[y * width + x];
        ((u32::from(pixel.r) * 299 + u32::from(pixel.g) * 587 + u32::from(pixel.b) * 114) / 1000)
            as u8
    };
    let title = menu_entry(ui, "Tools");
    let at = title.absolute_position();
    let size = title.size();
    let title_y = (at.y + size.height / 2.0) as usize;
    let title_pixels: Vec<u8> = (at.y as usize..(at.y + size.height) as usize)
        .flat_map(|y| (at.x as usize..(at.x + size.width) as usize).map(move |x| (x, y)))
        .map(|(x, y)| luma(x, y))
        .collect();
    ThemeSample {
        // The right end of the bar holds no entry.
        menu_bar: luma(width - 4, title_y),
        background: luma(width - 4, height - 4),
        title_lightest: *title_pixels.iter().max().unwrap(),
        title_darkest: *title_pixels.iter().min().unwrap(),
    }
}

/// Asserts the rendered window, menu bar included, is in the dark (or light)
/// scheme: dark surfaces with light text, or the reverse.
fn assert_rendered_scheme(ui: &cao_gui::ui::MainWindow, dark: bool) {
    let sample = sample_theme(ui);
    let scheme = if dark { "dark" } else { "light" };
    let surfaces_match = |luma: u8| if dark { luma < 80 } else { luma > 175 };
    assert!(
        surfaces_match(sample.menu_bar) && surfaces_match(sample.background),
        "the window is not drawn {scheme}: {sample:?}"
    );
    let title_text_matches = if dark {
        sample.title_lightest > 175
    } else {
        sample.title_darkest < 80
    };
    assert!(
        title_text_matches,
        "the menu title is not drawn {scheme}: {sample:?}"
    );
}

#[test]
fn the_dark_theme_menu_item_restyles_the_whole_window() {
    headless_with_pixels();
    let app_dir = app_dir("theme");
    edit_ini(&app_dir, "common.ini", "bDarkMode", "true");
    let (app, _desktop) = open(&app_dir);
    let ui = app.window();
    assert!(ui.get_palette_dark(), "bDarkMode=true starts dark");
    assert_rendered_scheme(ui, true);

    // The menu bar is drawn by Slint from the same palette as the rest of the
    // window (`SLINT_NO_MUDA` in the app; always under the test backend), so
    // flipping the palette restyles it too.
    click_menu(ui, "Tools");
    click_menu(ui, "Enable dark theme");
    assert!(!ui.get_palette_dark());
    assert_rendered_scheme(ui, false);

    click_menu(ui, "Tools");
    click_menu(ui, "Enable dark theme");
    assert!(ui.get_palette_dark());
    assert_rendered_scheme(ui, true);
}

#[test]
fn the_theme_choice_is_saved_to_common_ini() {
    headless();
    let app_dir = app_dir("theme_saved");
    edit_ini(&app_dir, "common.ini", "bDarkMode", "false");
    let (app, _desktop) = open(&app_dir);
    assert!(!app.window().get_palette_dark());

    click_menu(app.window(), "Tools");
    click_menu(app.window(), "Enable dark theme");
    app.save();

    let common = cao_profiles::Profiles::new(&app_dir).load_common().unwrap();
    assert!(common.dark_mode);
}

#[test]
fn toggling_debug_logging_saves_the_window_at_once() {
    headless();
    let app_dir = app_dir("debug_log");
    let (app, _desktop) = open(&app_dir);
    let options = || {
        cao_profiles::Profiles::new(&app_dir)
            .open("TES5")
            .load_options(&cao_profiles::Options::default())
            .unwrap()
    };
    assert!(!options().debug_log);

    // C++ saves the whole window when Tools → Enable debug log is triggered.
    click_menu(app.window(), "Tools");
    click_menu(app.window(), "Enable debug log");
    assert!(options().debug_log);
}
