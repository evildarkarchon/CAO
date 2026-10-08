//! Profile discovery, creation and the auxiliary text files, resolved against an
//! explicit app directory.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use cao_profiles::{ProfileError, Profiles};
use common::shipped_profiles;

/// The repository root, which holds the shipped `profiles/` as an app directory would.
fn shipped_app_dir() -> PathBuf {
    shipped_profiles().parent().unwrap().to_owned()
}

#[test]
fn discovers_the_shipped_fo4_sse_and_tes5_profiles() {
    // `profiles/TES4` has only `isBase` and no `profile.ini`, so it is not a profile.
    let profiles = Profiles::new(&shipped_app_dir());

    assert_eq!(profiles.list(), ["FO4", "SSE", "TES5"]);
    assert!(profiles.exists("TES5"));
    assert!(!profiles.exists("TES4"));
    assert!(!profiles.exists(""));
}

#[test]
fn profiles_resolve_under_the_app_directory() {
    let profiles = Profiles::new(Path::new("D:/Games/CAO"));

    assert_eq!(profiles.root(), Path::new("D:/Games/CAO/profiles"));
}

#[test]
fn every_shipped_profile_is_a_base_profile() {
    let profiles = Profiles::new(&shipped_app_dir());

    for name in profiles.list() {
        assert!(profiles.open(&name).is_base(), "{name}");
    }
}

#[test]
fn a_new_profile_is_a_copy_of_its_base_that_is_not_a_base_profile() {
    let app_dir = common::copy_of_shipped("create-from-tes5");
    let profiles = Profiles::new(&app_dir);

    let mine = profiles.create("Mine", "TES5").unwrap();

    assert_eq!(mine.name(), "Mine");
    assert_eq!(profiles.list(), ["FO4", "Mine", "SSE", "TES5"]);
    assert!(!mine.is_base());
    assert!(profiles.open("TES5").is_base());
    for file in ["profile.ini", "settings.ini", "DummyPlugin.esp"] {
        assert_eq!(
            common::read(&mine.directory().join(file)),
            common::read(&profiles.root().join("TES5").join(file)),
            "{file}"
        );
    }
    assert_eq!(
        mine.load_settings().unwrap(),
        profiles.open("TES5").load_settings().unwrap()
    );
}

#[test]
fn a_new_profile_copies_subdirectories() {
    let app_dir = common::copy_of_shipped("create-nested");
    let profiles = Profiles::new(&app_dir);
    let nested = profiles.root().join("FO4/extra/deeper");
    fs::create_dir_all(&nested).unwrap();
    fs::write(nested.join("note.txt"), "kept").unwrap();

    let mine = profiles.create("Mine", "FO4").unwrap();

    assert_eq!(
        common::read(&mine.directory().join("extra/deeper/note.txt")),
        b"kept"
    );
}

#[test]
fn a_new_profile_with_an_unknown_base_copies_sse() {
    let app_dir = common::copy_of_shipped("create-unknown-base");
    let profiles = Profiles::new(&app_dir);

    let mine = profiles.create("Mine", "Morrowind").unwrap();

    assert_eq!(
        common::read(&mine.profile_ini()),
        common::read(&profiles.open("SSE").profile_ini())
    );
    assert!(mine.directory().join("ignoredMods.txt").exists());
}

#[test]
fn deviation_10_dead_data_files_are_left_alone() {
    // `customLandscape.txt` (UTF-16LE with a BOM) and the profile `DummyPlugin.esp`
    // are read by nothing. Loading and saving a profile never touches them, and a new
    // profile gets byte-for-byte copies.
    let app_dir = common::copy_of_shipped("deviation-10-files");
    let profiles = Profiles::new(&app_dir);
    let sse = profiles.open("SSE");
    let landscape = common::read(&sse.directory().join("customLandscape.txt"));
    let dummy = common::read(&sse.directory().join("DummyPlugin.esp"));

    sse.save_settings(&sse.load_settings().unwrap()).unwrap();
    sse.save_options(&sse.load_options().unwrap()).unwrap();
    let _ = (
        sse.custom_headparts(),
        sse.files_to_not_pack(),
        sse.ignored_mods(),
    );
    let mine = profiles.create("Mine", "SSE").unwrap();

    for profile in [&sse, &mine] {
        assert_eq!(
            common::read(&profile.directory().join("customLandscape.txt")),
            landscape
        );
        assert_eq!(
            common::read(&profile.directory().join("DummyPlugin.esp")),
            dummy
        );
    }
    assert_eq!(landscape[..2], [0xFF, 0xFE]);
}

#[test]
fn the_sse_profile_reads_its_auxiliary_lists() {
    let sse = Profiles::new(&shipped_app_dir()).open("SSE");

    assert_eq!(
        sse.ignored_mods().unwrap(),
        ["Nemesis", "FNIS", "Bodyslide"]
    );
    assert_eq!(
        sse.files_to_not_pack(),
        [
            "meshes/actors/character/animations",
            "meshes/actors/character/behaviors",
            "meshes/actors/character/_1stperson/animations",
            "meshes/actors/character/_1stperson/behaviors",
            "meshes/animationdatasinglefile.txt",
            "meshes/animationsetdatasinglefile.txt",
            "readme.txt",
            "CalienteTools/",
            "dialogueviews/",
        ]
    );
    let headparts = sse.custom_headparts();
    assert_eq!(headparts.len(), 474);
    assert_eq!(
        headparts.first().unwrap(),
        "meshes/actors/character/character assets/beards/humanbeardlong01.nif"
    );
    assert_eq!(
        headparts.last().unwrap(),
        "meshes/actors/manekin/manekinhead.nif"
    );
}

#[test]
fn a_profile_without_auxiliary_files_falls_back_to_sse() {
    // FO4 ships none of the three files.
    let profiles = Profiles::new(&shipped_app_dir());
    let fo4 = profiles.open("FO4");
    let sse = profiles.open("SSE");

    assert_eq!(fo4.ignored_mods().unwrap(), sse.ignored_mods().unwrap());
    assert_eq!(fo4.files_to_not_pack(), sse.files_to_not_pack());
    assert_eq!(fo4.custom_headparts(), sse.custom_headparts());
}

#[test]
fn the_fallback_to_sse_is_per_file() {
    let app_dir = common::copy_of_shipped("auxiliary-per-file");
    let profiles = Profiles::new(&app_dir);
    fs::write(
        profiles.root().join("TES5/FilesToNotPack.txt"),
        "textures/keep\r\n",
    )
    .unwrap();
    let tes5 = profiles.open("TES5");

    assert_eq!(tes5.files_to_not_pack(), ["textures/keep"]);
    assert_eq!(
        tes5.custom_headparts(),
        profiles.open("SSE").custom_headparts()
    );
}

#[test]
fn auxiliary_lines_are_decoded_simplified_and_filtered() {
    // Each line is read as lossy UTF-8 with a BOM dropped, then `simplified()`:
    // trimmed, with each run of inner whitespace collapsed to one space. Empty lines
    // and lines starting with `#` after that are skipped.
    let app_dir = common::scratch_dir("auxiliary-read-rules");
    let directory = app_dir.join("profiles/Mine");
    fs::create_dir_all(&directory).unwrap();
    let mut bytes = b"\xEF\xBB\xBFfirst mod\r\n".to_vec();
    bytes.extend_from_slice(b"   # an indented comment\r\n");
    bytes.extend_from_slice(b"\t\r\n");
    bytes.extend_from_slice("  spaced \t  out\u{A0}name  \r\n".as_bytes());
    bytes.extend_from_slice("Caf\u{e9}\n".as_bytes());
    bytes.extend_from_slice(b"bad \xFF byte\r\n");
    bytes.extend_from_slice(b"#comment\r\n");
    bytes.extend_from_slice(b"last without newline");
    fs::write(directory.join("ignoredMods.txt"), &bytes).unwrap();

    let ignored = Profiles::new(&app_dir).open("Mine").ignored_mods().unwrap();

    assert_eq!(
        ignored,
        [
            "first mod",
            "spaced out name",
            "Caf\u{e9}",
            "bad \u{FFFD} byte",
            "last without newline",
        ]
    );
}

#[test]
fn missing_auxiliary_files_read_as_empty_lists() {
    let app_dir = common::scratch_dir("auxiliary-missing");
    let profile = Profiles::new(&app_dir).open("Mine");

    assert!(profile.ignored_mods().unwrap().is_empty());
    assert!(profile.files_to_not_pack().is_empty());
    assert!(profile.custom_headparts().is_empty());
}

#[test]
fn an_unreadable_ignored_mods_file_is_an_error_but_other_lists_read_as_empty() {
    // A directory exists under the file's name but cannot be opened as a file. As in
    // C++, an existing path stops the fallback to SSE.
    let app_dir = common::copy_of_shipped("auxiliary-unreadable");
    let profiles = Profiles::new(&app_dir);
    for name in [
        "ignoredMods.txt",
        "FilesToNotPack.txt",
        "customHeadparts.txt",
    ] {
        fs::create_dir(profiles.root().join("TES5").join(name)).unwrap();
    }
    let tes5 = profiles.open("TES5");

    assert!(matches!(
        tes5.ignored_mods(),
        Err(ProfileError::IgnoredMods { path, .. }) if path.ends_with("TES5/ignoredMods.txt")
    ));
    assert!(tes5.files_to_not_pack().is_empty());
    assert!(tes5.custom_headparts().is_empty());
}
