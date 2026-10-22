//! The BSA tab's Maximum size spin box shows a Profile's whole-byte maximum
//! Archive size in gigabytes and saves it back as whole bytes (#626), keeping
//! the size as the user wrote it until the spin box changes it. While the Game
//! combo shows a BSA game it stops at the 2 GiB BSA cap. Driven headlessly
//! through the real main window.

mod common;

use cao_profiles::Profiles;
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

#[test]
fn a_hand_edited_size_saves_as_written_until_the_spin_box_changes_it() {
    headless();
    let app_dir = app_dir("archive_size_hand_edited");
    // Copied Profiles, whose advanced groups are editable: one raising FO4's
    // Archives past the Creation Kit's 4 GB with a unit, and one with a byte
    // count, under the Skyrim cap, that the spin box's two decimals cannot hold
    // (2000000000 bytes shows as 1.86 GB, which is 1997159792 bytes).
    let profiles = Profiles::new(&app_dir);
    profiles.create("BigFO4", "FO4").unwrap();
    profiles.create("ExactTES5", "TES5").unwrap();
    edit_ini(
        &app_dir,
        "BigFO4/profile.ini",
        "maxBsaUncompressedSize",
        "8GB",
    );
    edit_ini(
        &app_dir,
        "ExactTES5/profile.ini",
        "maxBsaUncompressedSize",
        "2000000000",
    );
    edit_ini(&app_dir, "common.ini", "profile", "SSE");
    edit_ini(&app_dir, "common.ini", "showTutorial", "false");
    let (app, desktop) = open(&app_dir);
    let ui = app.window();

    for (profile, shown) in [("BigFO4", "8.00"), ("ExactTES5", "1.86")] {
        choose(ui, "Profile", profile);
        select_tab(ui, "BSA");
        assert_eq!(
            control(ui, "Maximum size").accessible_value().as_deref(),
            Some(shown),
            "the {profile} profile"
        );
    }
    choose(ui, "Profile", "SSE");

    // Switching away saved both Profiles without touching their sizes.
    assert_eq!(
        saved_size_line(&app_dir, "BigFO4"),
        "maxBsaUncompressedSize=8GB"
    );
    assert_eq!(
        saved_size_line(&app_dir, "ExactTES5"),
        "maxBsaUncompressedSize=2000000000"
    );

    // A size the user changes is saved in bytes.
    choose(ui, "Profile", "BigFO4");
    select_tab(ui, "BSA");
    enter(ui, "Maximum size", "6");
    assert!(app.save());
    assert_eq!(
        saved_size_line(&app_dir, "BigFO4"),
        "maxBsaUncompressedSize=6442450944"
    );
    assert!(desktop.message_titles().is_empty());
}

#[test]
fn the_maximum_size_stops_at_2_gb_while_the_game_combo_shows_a_bsa_game() {
    headless();
    let app_dir = app_dir("archive_size_bsa_cap");
    // Copied Profiles get no leeway past the BSA cap: a Skyrim one asking for
    // 8 GB, and an FO4 one whose 8 GB is fine until its game becomes a Skyrim.
    let profiles = Profiles::new(&app_dir);
    profiles.create("BigSSE", "SSE").unwrap();
    profiles.create("BigFO4", "FO4").unwrap();
    for profile in ["BigSSE", "BigFO4"] {
        edit_ini(
            &app_dir,
            &format!("{profile}/profile.ini"),
            "maxBsaUncompressedSize",
            "8GB",
        );
    }
    edit_ini(&app_dir, "common.ini", "profile", "SSE");
    edit_ini(&app_dir, "common.ini", "showTutorial", "false");
    let (app, desktop) = open(&app_dir);
    let ui = app.window();
    let shown = || control(ui, "Maximum size").accessible_value();
    let maximum = || control(ui, "Maximum size").accessible_value_maximum();

    // The Skyrim Profile shows, and accepts, no more than 2 GB.
    choose(ui, "Profile", "BigSSE");
    select_tab(ui, "BSA");
    assert_eq!(shown().as_deref(), Some("2.00"));
    assert_eq!(maximum(), Some(2.0));
    // Passing through the spin box without typing changes nothing.
    enter(ui, "Maximum size", "2.00");
    choose(ui, "Profile", "SSE");
    assert_eq!(
        saved_size_line(&app_dir, "BigSSE"),
        "maxBsaUncompressedSize=8GB",
        "the run caps it; the user's line stays until they change it"
    );

    // The FO4 Profile's 8 GB stands, stops at 2 GB while the Game combo shows
    // a Skyrim, and comes back with FO4.
    choose(ui, "Profile", "BigFO4");
    select_tab(ui, "BSA");
    assert_eq!(shown().as_deref(), Some("8.00"));
    choose(ui, "Game", "SSE");
    assert_eq!(shown().as_deref(), Some("2.00"));
    assert_eq!(maximum(), Some(2.0));
    choose(ui, "Game", "FO4");
    assert_eq!(shown().as_deref(), Some("8.00"));
    assert!(app.save());
    assert_eq!(
        saved_size_line(&app_dir, "BigFO4"),
        "maxBsaUncompressedSize=8GB"
    );

    // Typing past the cap under a Skyrim gives the cap.
    choose(ui, "Game", "Skyrim LE");
    enter(ui, "Maximum size", "5");
    assert_eq!(shown().as_deref(), Some("2.00"));
    assert!(app.save());
    assert_eq!(
        saved_size_line(&app_dir, "BigFO4"),
        "maxBsaUncompressedSize=2147483648"
    );
    assert!(desktop.message_titles().is_empty());
}
