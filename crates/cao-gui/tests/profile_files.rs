//! What the window reads from and writes to `profiles/`: a new install with no
//! `common.ini` (WS-8), a remembered profile that is gone and a profile
//! deleted while selected (GUI-8), saves that write only what changed and
//! report every failure at once (GUI-10), and options that never leak from
//! one profile into the next (GUI-11).

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use cao_gui::MessageLevel;
use common::*;
use slint::ComponentHandle;
use slint::platform::WindowEvent;

/// Every file under `dir`, by path, with its bytes.
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                files.insert(path.clone(), std::fs::read(&path).unwrap());
            }
        }
    }
    files
}

/// The messages shown so far, as (level, title, text).
fn messages(desktop: &FakeDesktop) -> Vec<(MessageLevel, String, String)> {
    desktop
        .shown
        .borrow()
        .iter()
        .filter_map(|shown| match shown {
            Shown::Message { level, title, text } => Some((*level, title.clone(), text.clone())),
            _ => None,
        })
        .collect()
}

/// The profile the combo shows.
fn shown_profile(ui: &cao_gui::ui::MainWindow) -> String {
    use slint::Model;
    let index = usize::try_from(ui.get_profile_index()).unwrap();
    ui.get_profiles().row_data(index).unwrap().to_string()
}

/// An app directory with tutorials off, two editable copies of SSE named
/// `names`, and the first of them selected.
fn custom_app(test: &str, names: &[&str]) -> PathBuf {
    let app_dir = app_dir(test);
    edit_ini(&app_dir, "common.ini", "showTutorial", "false");
    let profiles = cao_profiles::Profiles::new(&app_dir);
    for name in names {
        profiles.create(name, "SSE").unwrap();
    }
    edit_ini(&app_dir, "common.ini", "profile", names[0]);
    app_dir
}

/// Sets or clears the read-only attribute of `path`, so saving into it fails.
fn set_readonly(path: &Path, readonly: bool) {
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_readonly(readonly);
    std::fs::set_permissions(path, permissions).unwrap();
}

/// CAO ships no `common.ini`, so a new install opens on the default profile
/// with the welcome message, dark, and with the advanced settings hidden; the
/// first start is then remembered (WS-8).
#[test]
fn a_new_install_opens_on_sse_with_the_welcome_message() {
    headless();
    let app_dir = new_install("new_install");
    let common_ini = app_dir.join("profiles").join("common.ini");
    assert!(!common_ini.exists());

    let (app, desktop) = open(&app_dir);
    let ui = app.window();

    assert_eq!(shown_profile(ui), "SSE");
    assert!(ui.get_dark());
    assert!(!ui.get_show_advanced());
    assert!(
        desktop
            .message_titles()
            .iter()
            .any(|title| title.starts_with("Welcome to Cathedral Assets Optimizer")),
        "{:?}",
        desktop.message_titles()
    );
    let common = cao_profiles::Profiles::new(&app_dir).load_common().unwrap();
    assert_eq!(common.profile, "SSE");
    assert!(common.not_first_start);

    // A second start is no first start.
    drop(app);
    let (_app, desktop) = open(&app_dir);
    assert!(desktop.message_titles().is_empty());
}

/// A remembered profile that is gone, in an install without SSE, opens the
/// first profile listed: the one the combo shows is the one loaded, and no
/// save recreates SSE (GUI-8).
#[test]
fn a_stale_profile_without_sse_opens_on_the_first_listed() {
    headless();
    let app_dir = app_dir("stale_without_sse");
    edit_ini(&app_dir, "common.ini", "profile", "Gone");
    edit_ini(&app_dir, "common.ini", "showTutorial", "false");
    let profiles = app_dir.join("profiles");
    std::fs::remove_dir_all(profiles.join("SSE")).unwrap();

    let (app, desktop) = open(&app_dir);
    let ui = app.window();

    assert_eq!(shown_profile(ui), "FO4");
    assert!(messages(&desktop).is_empty(), "{:?}", messages(&desktop));
    assert!(app.save());
    assert!(!profiles.join("SSE").exists());
    assert!(!profiles.join("Gone").exists());
    let common = cao_profiles::Profiles::new(&app_dir).load_common().unwrap();
    assert_eq!(common.profile, "FO4");
}

/// A profile deleted while it is selected is not recreated by a save, which
/// reports it instead (GUI-8).
#[test]
fn a_profile_deleted_while_selected_is_not_recreated_by_a_save() {
    headless();
    let app_dir = custom_app("deleted_while_selected", &["Custom"]);
    let (app, desktop) = open(&app_dir);
    let ui = app.window();
    let custom = app_dir.join("profiles").join("Custom");
    std::fs::remove_dir_all(&custom).unwrap();
    ui.set_dry_run(!ui.get_dry_run());

    app.save();

    assert!(!custom.exists());
    let shown = messages(&desktop);
    assert_eq!(shown.len(), 1, "{shown:?}");
    assert_eq!(shown[0].1, "Save");
    assert!(
        shown[0]
            .2
            .contains("The profile's options could not be saved"),
        "{}",
        shown[0].2
    );
}

/// Opening CAO and closing it changes no file: every model is written only
/// when the window changed it, so a hand-written comment survives, and the
/// shipped base profiles stay as shipped (GUI-10).
#[test]
fn opening_and_closing_leaves_every_file_as_it_was() {
    headless();
    let app_dir = custom_app("open_close_unchanged", &["Custom"]);
    let settings_ini = app_dir.join("profiles/Custom/settings.ini");
    let mut text = std::fs::read(&settings_ini).unwrap();
    text.splice(0..0, b"# tuned for my load order\r\n".iter().copied());
    std::fs::write(&settings_ini, text).unwrap();
    let profiles = app_dir.join("profiles");
    let before = snapshot(&profiles);

    open_and_close(&app_dir, |_| {});
    assert!(written(&before, &snapshot(&profiles)).is_empty());

    // Switching profiles writes only the choice, into `common.ini`: neither
    // the base profile passed through nor the custom one is rewritten.
    open_and_close(&app_dir, |ui| {
        choose(ui, "Profile", "TES5");
        choose(ui, "Profile", "Custom");
    });
    assert_eq!(
        written(&before, &snapshot(&profiles)),
        [profiles.join("common.ini")]
    );
}

/// Opens the window over `app_dir`, does `act` in it, and closes it, which
/// must close without a message.
fn open_and_close(app_dir: &Path, act: impl FnOnce(&cao_gui::ui::MainWindow)) {
    let (app, desktop) = open(app_dir);
    let ui = app.window();
    act(ui);
    ui.window().dispatch_event(WindowEvent::CloseRequested);
    next_turn();
    assert!(!ui.window().is_visible());
    assert!(messages(&desktop).is_empty(), "{:?}", messages(&desktop));
}

/// The files whose bytes differ between two snapshots, or that only one has.
fn written(
    before: &BTreeMap<PathBuf, Vec<u8>>,
    after: &BTreeMap<PathBuf, Vec<u8>>,
) -> Vec<PathBuf> {
    let paths: BTreeSet<&PathBuf> = before
        .keys()
        .chain(after.keys())
        .filter(|path| before.get(*path) != after.get(*path))
        .collect();
    paths.into_iter().cloned().collect()
}

/// Every file one save cannot write is named in a single box (GUI-10).
#[test]
fn every_failure_of_one_save_shows_in_one_box() {
    headless();
    let app_dir = custom_app("one_box", &["Custom"]);
    let (app, desktop) = open(&app_dir);
    let ui = app.window();
    let profiles = app_dir.join("profiles");
    for file in ["common.ini", "Custom/settings.ini", "Custom/profile.ini"] {
        set_readonly(&profiles.join(file), true);
    }
    ui.set_dark(!ui.get_dark());
    ui.set_dry_run(!ui.get_dry_run());
    ui.global::<cao_gui::ui::TexturesTab>()
        .set_tga_conversion(false);

    assert!(!app.save(), "profile.ini differs from the window");

    for file in ["common.ini", "Custom/settings.ini", "Custom/profile.ini"] {
        set_readonly(&profiles.join(file), false);
    }
    let shown = messages(&desktop);
    assert_eq!(shown.len(), 1, "{shown:?}");
    let (level, title, text) = &shown[0];
    assert_eq!((*level, title.as_str()), (MessageLevel::Error, "Save"));
    for sentence in [
        "The common settings could not be saved",
        "The profile's options could not be saved",
        "The profile's settings could not be saved",
    ] {
        assert!(text.contains(sentence), "{sentence}: {text}");
    }

    // What failed is written by the next save.
    assert!(app.save());
    assert_eq!(messages(&desktop).len(), 1);
    let saved = std::fs::read_to_string(profiles.join("Custom/profile.ini")).unwrap();
    assert!(saved.contains("texturesConvertTga=false"), "{saved}");
}

/// A group that loads off shows its sub-choices reset, so ticking it again
/// preselects the defaults rather than the previous profile's choices
/// (GUI-11).
#[test]
fn a_group_that_loads_off_does_not_keep_the_previous_profiles_choices() {
    headless();
    let app_dir = custom_app("group_reset", &["Heavy", "Off"]);
    for (key, value) in [
        ("bTexturesNecessary", "false"),
        ("bTexturesCompress", "true"),
        ("bTexturesMipmaps", "true"),
        ("iMeshesOptimizationLevel", "3"),
    ] {
        edit_ini(&app_dir, "Heavy/settings.ini", key, value);
    }
    for (key, value) in [
        ("bTexturesNecessary", "false"),
        ("iMeshesOptimizationLevel", "0"),
    ] {
        edit_ini(&app_dir, "Off/settings.ini", key, value);
    }
    let (app, _desktop) = open(&app_dir);
    let ui = app.window();
    select_tab(ui, "Textures");
    assert!(checked(ui, "Compress textures") && checked(ui, "Generate mipmaps"));

    choose(ui, "Profile", "Off");
    select_tab(ui, "Textures");
    assert!(!checked(ui, "Process textures"));
    click(ui, "Process textures");
    assert!(checked(ui, "Necessary optimization"));
    assert!(!checked(ui, "Compress textures"));
    assert!(!checked(ui, "Generate mipmaps"));

    select_tab(ui, "Meshes");
    assert!(!checked(ui, "Process meshes"));
    click(ui, "Process meshes");
    assert!(checked(ui, "Necessary optimization"));
    assert!(!checked(ui, "Full optimization"));
}

/// A profile without `settings.ini` takes the defaults and only the mod
/// folder from the profile before it (GUI-11).
#[test]
fn a_profile_without_settings_ini_takes_only_the_mod_folder() {
    headless();
    let app_dir = custom_app("carry_folder", &["First", "Bare"]);
    edit_ini(&app_dir, "First/settings.ini", "bDryRun", "true");
    edit_ini(&app_dir, "First/settings.ini", "mode", "1");
    std::fs::remove_file(app_dir.join("profiles/Bare/settings.ini")).unwrap();
    let mods = scratch_dir("carry_folder_mods");
    let (app, _desktop) = open(&app_dir);
    let ui = app.window();
    assert!(ui.get_dry_run());
    ui.set_user_path(mods.display().to_string().into());

    choose(ui, "Profile", "Bare");

    assert!(!ui.get_dry_run(), "Dry run is the default, not First's");
    assert_eq!(ui.get_mode_index(), 0, "One mod is the default");
    // The window cleans the folder to forward slashes as it reads it back.
    assert_eq!(
        ui.get_user_path(),
        mods.display().to_string().replace('\\', "/")
    );
}
