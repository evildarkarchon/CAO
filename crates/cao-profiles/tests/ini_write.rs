//! Loading and saving through the filesystem, and round trips of the shipped
//! `profiles/`.

mod common;

use std::path::Path;

use cao_profiles::{IniError, IniFile, Value};
use common::{read, read_as_crlf, scratch_dir, shipped_profiles, show};

const SHIPPED: [&str; 7] = [
    "common.ini",
    "FO4/profile.ini",
    "FO4/settings.ini",
    "SSE/profile.ini",
    "SSE/settings.ini",
    "TES5/profile.ini",
    "TES5/settings.ini",
];

#[test]
fn shipped_profiles_load_without_format_errors_and_rewrite_unchanged() {
    for name in SHIPPED {
        let original = read_as_crlf(&shipped_profiles().join(name));
        let ini = IniFile::parse(&original);

        assert_eq!(ini.format_error(), None, "{name}");
        assert!(
            ini.to_bytes() == original,
            "{name} rewrote as:\n{}",
            show(&ini.to_bytes())
        );
    }
}

#[test]
fn the_tes5_profile_reads_as_tes5() {
    let ini = IniFile::load(&shipped_profiles().join("TES5/profile.ini")).unwrap();

    assert_eq!(ini.value("BSA/bsaGame").to_i32(), 3);
    assert!(ini.value("BSA/bsaEnabled").to_bool());
    assert_eq!(
        ini.value("BSA/maxBsaUncompressedSize").to_f64(),
        2104533975.04
    );
    assert_eq!(ini.value("Meshes/meshesFileVersion").to_i32(), 335675399);
    assert_eq!(ini.value("Meshes/meshesStream").to_u32(), 83);
    assert_eq!(ini.value("Meshes/meshesUser").to_u32(), 12);
    assert!(ini.value("Meshes/meshesEnabled").to_bool());
    assert!(!ini.value("Animations/animationsEnabled").to_bool());
    assert_eq!(ini.value("Textures/texturesFormat").to_i32(), 77);
    assert_eq!(
        ini.value("Textures/texturesUnwantedFormats").to_int_list(),
        [98, 99]
    );
    assert!(!ini.value("Textures/texturesConvertTga").to_bool());
}

#[test]
fn the_fo4_unwanted_formats_keep_their_order() {
    let ini = IniFile::load(&shipped_profiles().join("FO4/profile.ini")).unwrap();

    assert_eq!(
        ini.value("Textures/texturesUnwantedFormats").to_int_list(),
        [86, 85, 115]
    );
}

#[test]
fn a_one_element_unwanted_formats_list_survives_save_and_load() {
    let dir = scratch_dir("one_element_list");
    let path = dir.join("profile.ini");
    std::fs::write(&path, read(&shipped_profiles().join("TES5/profile.ini"))).unwrap();

    let mut ini = IniFile::load(&path).unwrap();
    ini.set("Textures/texturesUnwantedFormats", Value::int_list(&[85]));
    ini.save(&path).unwrap();

    let reloaded = IniFile::load(&path).unwrap();
    assert_eq!(
        reloaded
            .value("Textures/texturesUnwantedFormats")
            .to_int_list(),
        [85]
    );
    assert!(String::from_utf8(read(&path)).unwrap().contains(
        "texturesUnwantedFormats=@Variant(\\0\\0\\0\\t\\0\\0\\0\\x1\\0\\0\\0\\x2\\0\\0\\0U)\r\n"
    ));

    // And an emptied list stays empty rather than becoming one empty element.
    let mut ini = reloaded;
    ini.set("Textures/texturesUnwantedFormats", Value::int_list(&[]));
    ini.save(&path).unwrap();
    let reloaded = IniFile::load(&path).unwrap();
    assert!(
        reloaded
            .value("Textures/texturesUnwantedFormats")
            .to_int_list()
            .is_empty()
    );
}

#[test]
fn unknown_keys_survive_a_save() {
    let dir = scratch_dir("unknown_keys");
    let path = dir.join("settings.ini");
    std::fs::write(
        &path,
        b"[BSA]\r\nbBsaLeastBSA=false\r\nbBsaExtract=true\r\n\r\n[Mystery]\r\nkeep=\"a, b\"\r\n",
    )
    .unwrap();

    let mut ini = IniFile::load(&path).unwrap();
    ini.set("BSA/bBsaExtract", false);
    ini.set("BSA/bBsaMergeIncomp", true);
    ini.save(&path).unwrap();

    assert_eq!(
        read(&path),
        b"[BSA]\r\nbBsaLeastBSA=false\r\nbBsaExtract=false\r\nbBsaMergeIncomp=true\r\n\r\n[Mystery]\r\nkeep=\"a, b\"\r\n"
    );
}

#[test]
fn saving_rewrites_the_whole_file_with_crlf() {
    let dir = scratch_dir("crlf_rewrite");
    let path = dir.join("common.ini");
    std::fs::write(
        &path,
        b"; a comment Qt drops\n[General]\nprofile=SSE\n\n\nbDarkMode=true\n",
    )
    .unwrap();

    let mut ini = IniFile::load(&path).unwrap();
    ini.set("profile", "FO4");
    ini.save(&path).unwrap();

    assert_eq!(
        read(&path),
        b"[General]\r\nprofile=FO4\r\nbDarkMode=true\r\n"
    );
}

fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

#[test]
fn saving_replaces_the_file_and_leaves_no_temporary_behind() {
    let dir = scratch_dir("atomic_replace");
    let path = dir.join("profile.ini");
    std::fs::write(
        &path,
        b"old contents that are much longer than the new ones\r\n",
    )
    .unwrap();

    let mut ini = IniFile::new();
    ini.set("BSA/bsaGame", 4);
    ini.save(&path).unwrap();

    assert_eq!(read(&path), b"[BSA]\r\nbsaGame=4\r\n");
    assert_eq!(entries(&dir), ["profile.ini"]);
}

#[test]
fn a_failed_save_keeps_the_old_file_and_cleans_up() {
    let dir = scratch_dir("failed_save");
    // A directory where the file should be makes the final rename fail.
    let path = dir.join("profile.ini");
    std::fs::create_dir(&path).unwrap();
    std::fs::write(path.join("inside.txt"), b"untouched").unwrap();

    let mut ini = IniFile::new();
    ini.set("BSA/bsaGame", 4);
    let error = ini.save(&path).unwrap_err();

    assert!(matches!(error, IniError::Write { .. }), "{error:?}");
    assert!(error.to_string().contains("profile.ini"), "{error}");
    assert_eq!(entries(&dir), ["profile.ini"]);
    assert_eq!(read(&path.join("inside.txt")), b"untouched");
}

#[test]
fn saving_creates_missing_parent_directories() {
    // QSettings creates the directory before writing, which a new profile relies on.
    let dir = scratch_dir("create_parents");
    let path = dir.join("NewProfile").join("profile.ini");

    let mut ini = IniFile::new();
    ini.set("BSA/bsaGame", 5);
    ini.save(&path).unwrap();

    assert_eq!(read(&path), b"[BSA]\r\nbsaGame=5\r\n");
}

#[test]
fn loading_a_missing_file_gives_empty_settings() {
    let dir = scratch_dir("missing_file");

    let ini = IniFile::load(&dir.join("absent.ini")).unwrap();

    assert_eq!(ini.keys().count(), 0);
    assert_eq!(ini.format_error(), None);
}

#[test]
fn loading_an_unreadable_path_is_an_error() {
    let dir = scratch_dir("unreadable");

    let error = IniFile::load(&dir).unwrap_err();

    assert!(matches!(error, IniError::Read { .. }), "{error:?}");
}

#[test]
fn an_empty_file_writes_nothing() {
    assert_eq!(IniFile::new().to_bytes(), b"");
}
