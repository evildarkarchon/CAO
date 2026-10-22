//! The BSA tab's Maximum size spin box shows a Profile's whole-byte maximum
//! Archive size in gigabytes and saves it back as whole bytes (#626). Driven
//! headlessly through the real main window.

mod common;

use common::*;

/// The `maxBsaUncompressedSize=` line of `profile`'s `profile.ini` in `app_dir`.
fn saved_size_line(app_dir: &std::path::Path, profile: &str) -> String {
    let path = app_dir.join("profiles").join(profile).join("profile.ini");
    let text = std::fs::read_to_string(&path).unwrap();
    text.lines()
        .find(|line| line.starts_with("maxBsaUncompressedSize="))
        .unwrap_or_else(|| panic!("{profile} saves the size:\n{text}"))
        .to_owned()
}

#[test]
fn each_shipped_profile_shows_its_size_in_gigabytes_and_saves_it_as_whole_bytes() {
    headless();
    let app_dir = app_dir("archive_size_shipped");
    edit_ini(&app_dir, "common.ini", "profile", "SSE");
    edit_ini(&app_dir, "common.ini", "showTutorial", "false");
    let (app, desktop) = open(&app_dir);
    let ui = app.window();

    // The shipped sizes are 1.96 GiB (`2104533975.04`) and 3.9 GiB
    // (`4187593113.6`); switching away saves each Profile, which drops only
    // the fractional byte.
    let expected = [
        ("SSE", "1.96", "maxBsaUncompressedSize=2104533975"),
        ("FO4", "3.90", "maxBsaUncompressedSize=4187593113"),
        ("TES5", "1.96", "maxBsaUncompressedSize=2104533975"),
    ];
    for (profile, shown, _) in expected {
        choose(ui, "Profile", profile);
        select_tab(ui, "BSA");
        assert_eq!(
            control(ui, "Maximum size").accessible_value().as_deref(),
            Some(shown),
            "the {profile} profile"
        );
    }
    assert!(app.save());

    for (profile, _, line) in expected {
        assert_eq!(saved_size_line(&app_dir, profile), line, "{profile}");
    }
    assert!(desktop.message_titles().is_empty());
}
