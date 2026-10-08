//! The writer against bytes Qt 5.15 wrote.
//!
//! `tests/fixtures/qt-written/` holds files written by `qt-probe/qsettings_probe.cpp`,
//! built against the Qt 5.15 the C++ CAO build uses. The probe makes the same
//! `QSettings::setValue` calls as `Profiles::saveToIni` and `OptionsCAO::saveToIni`.
//! Each test replays those calls through [`IniFile`] and expects Qt's bytes exactly,
//! which pins key order, unknown keys, escaping, quoting, doubles, `@Variant(…)`,
//! `@Invalid()` and CRLF in one comparison.

mod common;

use cao_profiles::{IniFile, Value};
use common::{fixtures, read, read_as_crlf, shipped_profiles, show};

fn qt_written(name: &str) -> Vec<u8> {
    read(&fixtures().join("qt-written").join(name))
}

fn assert_bytes(actual: &[u8], expected: &[u8]) {
    assert!(
        actual == expected,
        "Rust wrote:\n{}\nQt wrote:\n{}",
        show(actual),
        show(expected)
    );
}

/// `Profiles::saveToIni` with the shipped TES5 values and the given unwanted formats.
fn save_tes5_profile(ini: &mut IniFile, unwanted: &[i32]) {
    ini.set("BSA/bsaEnabled", true);
    ini.set("BSA/maxBsaUncompressedSize", 2104533975.04);
    ini.set("BSA/bsaGame", 3);
    ini.set("Meshes/meshesEnabled", true);
    ini.set("Meshes/meshesFileVersion", 335675399u32);
    ini.set("Meshes/meshesStream", 83u32);
    ini.set("Meshes/meshesUser", 12u32);
    ini.set("Animations/animationsEnabled", false);
    ini.set("Textures/texturesEnabled", true);
    ini.set("Textures/texturesFormat", 77);
    ini.set("Textures/texturesConvertTga", false);
    ini.set(
        "Textures/texturesUnwantedFormats",
        Value::int_list(unwanted),
    );
    ini.set("Textures/texturesCompressInterface", true);
}

#[test]
fn resaving_the_tes5_profile_with_one_unwanted_format_matches_qt() {
    let shipped = read_as_crlf(&shipped_profiles().join("TES5/profile.ini"));
    let mut ini = IniFile::parse(&shipped);
    save_tes5_profile(&mut ini, &[98]);

    // Qt keeps the shipped key order and the dead `animationFormat` key, and writes the
    // one-element list as a QDataStream blob whose last byte, 98, is the hex digit `b`.
    assert_bytes(
        &ini.to_bytes(),
        &qt_written("tes5-profile-one-unwanted.ini"),
    );
}

#[test]
fn a_new_profile_with_no_unwanted_formats_matches_qt() {
    let mut ini = IniFile::new();
    save_tes5_profile(&mut ini, &[]);

    assert_bytes(&ini.to_bytes(), &qt_written("new-profile-no-unwanted.ini"));
}

#[test]
fn resaving_the_tes5_settings_matches_qt() {
    let shipped = read_as_crlf(&shipped_profiles().join("TES5/settings.ini"));
    let mut ini = IniFile::parse(&shipped);

    // `OptionsCAO::saveToIni`. The shipped file lacks the two merge keys, which Qt
    // appends to [BSA], and carries `bBsaLeastBSA`, which Qt keeps.
    ini.set("bDryRun", false);
    ini.set("bDebugLog", false);
    ini.set("mode", 0);
    ini.set("userPath", "C:/Mods/Café, Stuff");
    for key in [
        "bBsaExtract",
        "bBsaCreate",
        "bBsaDeleteBackup",
        "bBsaMergeIncomp",
        "bBsaMergeTexture",
        "bBsaProcessContent",
    ] {
        ini.set(&format!("BSA/{key}"), false);
    }
    for key in ["bBsaCreateDummies", "bBsaCompress", "bBsaDeleteSource"] {
        ini.set(&format!("BSA/{key}"), true);
    }
    ini.set("Textures/bTexturesNecessary", true);
    ini.set("Textures/bTexturesCompress", false);
    ini.set("Textures/bTexturesMipmaps", false);
    ini.set("Textures/bTexturesResizeSize", false);
    ini.set("Textures/iTexturesTargetWidth", 2048u32);
    ini.set("Textures/iTexturesTargetHeight", 2048u32);
    ini.set("Textures/bTexturesResizeRatio", false);
    ini.set("Textures/iTexturesTargetHeightRatio", 2u32);
    ini.set("Textures/iTexturesTargetWidthRatio", 2u32);
    ini.set("Meshes/iMeshesOptimizationLevel", 0);
    ini.set("Meshes/bMeshesHeadparts", true);
    ini.set("Meshes/bMeshesResave", false);
    ini.set("Animations/bAnimationsOptimization", false);

    assert_bytes(&ini.to_bytes(), &qt_written("tes5-settings-resaved.ini"));
}

#[test]
fn awkward_values_match_qt() {
    let mut ini = IniFile::new();
    ini.set("rootString", "plain");
    ini.set("General/inRealGeneral", 1);
    ini.set("Strings/empty", "");
    ini.set("Strings/semicolon", "a;b");
    ini.set("Strings/equals", "x=y");
    ini.set("Strings/leadingSpace", " lead");
    ini.set("Strings/trailingSpace", "trail ");
    ini.set("Strings/innerSpace", "in ner");
    ini.set("Strings/controls", "tab\there\nnl\rcr\x07\x08\x0c\x0b\x01");
    ini.set("Strings/quoteBackslash", "say \"hi\" C:\\dir");
    ini.set("Strings/latin1HexGuard", "é1f");
    ini.set("Strings/nulHexGuard", "a\0b");
    ini.set("Strings/astral", "😀!");
    ini.set("Strings/cjk", "漢");
    ini.set("Strings/at", "@home");
    ini.set("Strings/atAt", "@@twice");
    ini.set("Strings/question", "why?'");
    ini.set("Keys/with space", 1);
    ini.set("Keys/per%cent", 2);
    ini.set("Keys/café", 3);
    ini.set("Keys/漢", 4);
    ini.set("Keys/sub/key", 5);
    ini.set("Keys/Case", 6);
    ini.set("Keys/CASE", 7);
    ini.set("Numbers/negative", -42);
    ini.set("Numbers/bigUInt", 4294967295u32);
    ini.set("Numbers/longLong", -9007199254740993i64);
    ini.set("Doubles/twoE9", 2000000000.0);
    ini.set("Doubles/twoPow31", 2147483648.0);
    ini.set("Doubles/fo4Max", 4187593113.6);
    ini.set("Doubles/tes5Max", 2104533975.04);
    ini.set("Doubles/tenThousandth", 0.0001);
    ini.set("Doubles/hundredThousandth", 0.00001);
    ini.set("Doubles/e21", 1e21);
    ini.set("Doubles/e100", 1e100);
    ini.set("Doubles/eMinus100", 1e-100);
    ini.set("Doubles/negative", -1.5);
    ini.set("Doubles/zero", 0.0);
    ini.set("Doubles/negativeZero", -0.0);
    ini.set("Doubles/pi", std::f64::consts::PI);
    ini.set("Doubles/twelveDigits", 123456789012.0);
    ini.set("Doubles/elevenDigitsE", 12345678901e5);
    ini.set("Doubles/oneThird", 1.0 / 3.0);
    ini.set("Lists/empty", Value::int_list(&[]));
    ini.set("Lists/emptyStrings", Value::List(Vec::new()));
    ini.set("Lists/oneString", Value::List(vec!["solo".into()]));
    ini.set(
        "Lists/strings",
        Value::List(vec!["a".into(), "b, c".into(), " d".into(), "@e".into()]),
    );
    ini.set("Lists/one85", Value::int_list(&[85]));
    ini.set("Lists/one98", Value::int_list(&[98]));
    ini.set("Lists/one61", Value::int_list(&[61]));
    ini.set("Lists/one200", Value::int_list(&[200]));
    ini.set("Lists/one59", Value::int_list(&[59]));
    ini.set("Lists/three", Value::int_list(&[86, 85, 115]));
    ini.set("Bools/yes", true);
    ini.set("Bools/no", false);

    assert_bytes(&ini.to_bytes(), &qt_written("awkward-values.ini"));
}

#[test]
fn qt_written_files_read_back_to_the_values_qt_saved() {
    let profile = IniFile::parse(&qt_written("tes5-profile-one-unwanted.ini"));
    assert_eq!(profile.format_error(), None);
    assert_eq!(
        profile
            .value("Textures/texturesUnwantedFormats")
            .to_int_list(),
        [98]
    );
    assert_eq!(profile.value("BSA/bsaGame").to_i32(), 3);
    assert_eq!(
        profile.value("BSA/maxBsaUncompressedSize").to_f64(),
        2104533975.04
    );
    assert_eq!(
        profile.value("Meshes/meshesFileVersion").to_u32(),
        335675399
    );
    assert!(
        profile
            .value("Textures/texturesCompressInterface")
            .to_bool()
    );
    assert!(!profile.value("Textures/texturesConvertTga").to_bool());
    assert_eq!(profile.value("Animations/animationFormat").to_i32(), 2);

    let empty = IniFile::parse(&qt_written("new-profile-no-unwanted.ini"));
    assert_eq!(
        empty.value("Textures/texturesUnwantedFormats"),
        &Value::Invalid
    );
    assert!(
        empty
            .value("Textures/texturesUnwantedFormats")
            .to_int_list()
            .is_empty()
    );

    let settings = IniFile::parse(&qt_written("tes5-settings-resaved.ini"));
    assert_eq!(
        settings.value("userPath").to_qstring(),
        "C:/Mods/Café, Stuff"
    );

    let awkward = IniFile::parse(&qt_written("awkward-values.ini"));
    for (key, list) in [
        ("Lists/one85", &[85][..]),
        ("Lists/one61", &[61]),
        ("Lists/one200", &[200]),
        ("Lists/one59", &[59]),
        ("Lists/three", &[86, 85, 115]),
        ("Lists/empty", &[]),
    ] {
        assert_eq!(awkward.value(key).to_int_list(), list, "{key}");
    }
    for (key, text) in [
        ("rootString", "plain"),
        ("General/inRealGeneral", "1"),
        ("Strings/controls", "tab\there\nnl\rcr\x07\x08\x0c\x0b\x01"),
        ("Strings/quoteBackslash", "say \"hi\" C:\\dir"),
        ("Strings/latin1HexGuard", "é1f"),
        ("Strings/nulHexGuard", "a\0b"),
        ("Strings/astral", "😀!"),
        ("Strings/cjk", "漢"),
        ("Strings/at", "@home"),
        ("Strings/atAt", "@@twice"),
        ("Strings/leadingSpace", " lead"),
        ("Strings/trailingSpace", "trail "),
        ("Keys/café", "3"),
        ("Keys/漢", "4"),
        ("Keys/sub/key", "5"),
        ("Keys/case", "7"),
        ("Doubles/twoE9", "2e+09"),
    ] {
        assert_eq!(awkward.value(key).to_qstring(), text, "{key}");
    }
    assert_eq!(awkward.value("Doubles/twoE9").to_f64(), 2e9);
    assert_eq!(
        awkward.value("Numbers/longLong").to_i64(),
        -9007199254740993
    );
}
