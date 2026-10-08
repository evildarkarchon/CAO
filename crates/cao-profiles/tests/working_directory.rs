//! Deviation 1 for profiles: everything resolves against the app directory, never the
//! working directory. C++ CAO opened `profiles/` relative to the working directory, so
//! a shortcut with another start-in folder lost the user's profiles.
//!
//! The working directory is process-wide, so this file holds a single test and runs
//! as its own test binary.

mod common;

use std::fs;

use cao_profiles::{BsaGame, Profiles};

#[test]
fn profiles_resolve_against_the_app_directory_from_another_working_directory() {
    let app_dir = common::copy_of_shipped("working-directory-app");
    // A decoy install in the working directory, which C++ would have read instead.
    let decoy = common::scratch_dir("working-directory-decoy");
    let decoy_sse = decoy.join("profiles/SSE");
    fs::create_dir_all(&decoy_sse).unwrap();
    fs::write(decoy_sse.join("profile.ini"), "[BSA]\r\nbsaGame=5\r\n").unwrap();
    fs::write(decoy_sse.join("ignoredMods.txt"), "Decoy\r\n").unwrap();
    fs::write(decoy.join("profiles/common.ini"), "profile=Decoy\r\n").unwrap();
    std::env::set_current_dir(&decoy).unwrap();

    let profiles = Profiles::new(&app_dir);
    let sse = profiles.open("SSE");
    let fo4 = profiles.open("FO4");

    assert_eq!(profiles.list(), ["FO4", "SSE", "TES5"]);
    assert_eq!(profiles.load_common().unwrap().profile, "TES5");
    assert_eq!(sse.load_settings().unwrap().bsa_game, BsaGame::Sse);
    assert!(sse.load_options().unwrap().bsa_create_dummies);
    // FO4 has no ignoredMods.txt, so it falls back to the app directory's SSE.
    assert_eq!(
        fo4.ignored_mods().unwrap(),
        ["Nemesis", "FNIS", "Bodyslide"]
    );

    let mine = profiles.create("Mine", "SSE").unwrap();
    mine.save_options(&mine.load_options().unwrap()).unwrap();
    assert!(app_dir.join("profiles/Mine/settings.ini").exists());
    assert!(!decoy.join("profiles/Mine").exists());
}
