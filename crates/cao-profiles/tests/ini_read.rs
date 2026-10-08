//! Reader behaviour the Qt differential cannot pin: the recorded deviations from
//! Qt 5.15 (spec #476, items 12–14), UTF-8 BOM files, format errors and lookup.

use cao_profiles::{FormatError, FormatErrorKind, IniFile, Value};

#[test]
fn deviation_12_a_scalar_where_a_list_is_expected_reads_as_one_element() {
    // Qt reads a hand-edited `texturesUnwantedFormats=85` as an empty list.
    let ini = IniFile::parse(b"[Textures]\r\ntexturesUnwantedFormats=85\r\n");

    assert_eq!(
        ini.value("Textures/texturesUnwantedFormats").to_int_list(),
        [85]
    );
}

#[test]
fn deviation_12_does_not_turn_an_empty_value_into_a_list() {
    let ini = IniFile::parse(b"[Textures]\r\ntexturesUnwantedFormats=\r\n");

    assert!(
        ini.value("Textures/texturesUnwantedFormats")
            .to_int_list()
            .is_empty()
    );
}

#[test]
fn deviation_13_a_hash_line_is_a_comment() {
    // In Qt, `# note` is a line without `=` and sets FormatError, so run setup fails;
    // `#bsaGame=3` is a key named `#bsaGame`.
    let ini = IniFile::parse(
        b"# note: tuned for my setup\r\n[BSA]\r\n  # indented \"quote\r\n#bsaGame=3\r\nbsaGame=4\r\n",
    );

    assert_eq!(ini.format_error(), None);
    assert_eq!(ini.keys().collect::<Vec<_>>(), ["BSA/bsaGame"]);
    assert_eq!(ini.value("BSA/bsaGame").to_i32(), 4);
}

#[test]
fn deviation_13_any_other_line_without_equals_is_still_a_format_error() {
    let ini = IniFile::parse(b"[BSA]\r\nbsaGame=4\r\njust some words\r\nbsaEnabled=true\r\n");

    assert_eq!(
        ini.format_error(),
        Some(&FormatError {
            line: 3,
            kind: FormatErrorKind::MissingEquals
        })
    );
    // As in Qt, the rest of the file is still read.
    assert_eq!(ini.value("BSA/bsaGame").to_i32(), 4);
    assert!(ini.value("BSA/bsaEnabled").to_bool());
}

#[test]
fn deviation_14_utf8_without_a_bom_decodes_as_utf8() {
    // Qt reads these bytes as Latin-1: `userPath` would come out as `CafÃ©`.
    let ini = IniFile::parse("userPath=D:/Café\r\n".as_bytes());

    assert_eq!(ini.value("userPath").to_qstring(), "D:/Café");
}

#[test]
fn deviation_14_invalid_utf8_without_a_bom_decodes_as_latin1() {
    let ini = IniFile::parse(b"userPath=D:/Caf\xe9\r\n");

    assert_eq!(ini.value("userPath").to_qstring(), "D:/Café");
}

#[test]
fn deviation_14_leaves_the_writer_ascii() {
    let mut ini = IniFile::parse("userPath=D:/Café\r\n".as_bytes());
    ini.set("other", "x");

    assert_eq!(
        ini.to_bytes(),
        b"[General]\r\nuserPath=D:/Caf\\xe9\r\nother=x\r\n"
    );
}

#[test]
fn a_utf8_bom_is_skipped_and_selects_utf8() {
    // Qt 5.15 leaves the BOM bytes in its root section: it reports FormatError for
    // this file and reads a header-less first key as `ï»¿profile`.
    let ini = IniFile::parse(
        b"\xef\xbb\xbfprofile=SSE\r\nuserPath=D:/Caf\xc3\xa9\r\n[BSA]\r\nbsaGame=4\r\n",
    );

    assert_eq!(ini.format_error(), None);
    assert_eq!(ini.value("profile").to_qstring(), "SSE");
    assert_eq!(ini.value("userPath").to_qstring(), "D:/Café");
    assert_eq!(ini.value("BSA/bsaGame").to_i32(), 4);
}

#[test]
fn an_unclosed_section_header_is_a_format_error() {
    let ini = IniFile::parse(b"[BSA]\r\nbsaGame=4\r\n\r\n[Meshes\r\nmeshesStream=83\r\n");

    assert_eq!(
        ini.format_error(),
        Some(&FormatError {
            line: 4,
            kind: FormatErrorKind::UnclosedSection
        })
    );
    // Qt uses the rest of the line as the section name.
    assert_eq!(ini.value("Meshes/meshesStream").to_u32(), 83);
}

#[test]
fn the_first_format_error_is_the_one_reported() {
    let ini = IniFile::parse(b"oops\r\n[BSA\r\n");

    assert_eq!(ini.format_error().map(|error| error.line), Some(1));
    assert_eq!(
        ini.format_error().unwrap().to_string(),
        "line 1 has no `=` and is not a comment"
    );
}

#[test]
fn a_missing_key_reads_as_qts_invalid_variant() {
    let ini = IniFile::parse(b"[BSA]\r\nbsaGame=4\r\n");
    let missing = ini.value("BSA/bsaEnabled");

    assert_eq!(missing, &Value::Invalid);
    assert!(!ini.contains("BSA/bsaEnabled"));
    assert!(!missing.to_bool());
    assert_eq!(missing.to_i32(), 0);
    assert_eq!(missing.to_u32(), 0);
    assert_eq!(missing.to_f64(), 0.0);
    assert_eq!(missing.to_qstring(), "");
    assert!(missing.to_int_list().is_empty());
}

#[test]
fn lookup_ignores_case_and_writing_keeps_the_original_case_and_place() {
    let mut ini = IniFile::parse(b"[BSA]\r\nbsaGame=4\r\nbsaEnabled=true\r\n");

    assert!(ini.contains("bsa/BSAGAME"));
    assert_eq!(ini.value("bsa/BSAGAME").to_i32(), 4);

    ini.set("BSA/BSAGAME", 5);
    ini.set("bsa/bsaNew", 1);

    assert_eq!(
        ini.keys().collect::<Vec<_>>(),
        ["BSA/bsaGame", "BSA/bsaEnabled", "bsa/bsaNew"]
    );
    // QSettings groups sections by their exact spelling, so the new key, set with a
    // lowercase section, gets a section of its own. Qt reads both back as one.
    assert_eq!(
        ini.to_bytes(),
        b"[BSA]\r\nbsaGame=5\r\nbsaEnabled=true\r\n\r\n[bsa]\r\nbsaNew=1\r\n"
    );
}

#[test]
fn the_last_duplicate_wins_but_keeps_the_first_place() {
    let ini = IniFile::parse(b"[BSA]\r\nbsaGame=3\r\nbsaEnabled=true\r\nBSAGAME=5\r\n");

    assert_eq!(ini.value("BSA/bsaGame").to_i32(), 5);
    assert_eq!(ini.to_bytes(), b"[BSA]\r\nbsaGame=5\r\nbsaEnabled=true\r\n");
}

#[test]
fn qvariant_conversions_are_lenient() {
    let ini = IniFile::parse(
        b"[T]\r\nyes=True\r\nno=FALSE\r\nword=no\r\nnum= 12 \r\nneg=-3\r\nbad=12abc\r\nbig=4294967296\r\nd=4187593113.6\r\nlist=1, x, 3\r\n",
    );

    assert!(ini.value("T/yes").to_bool());
    assert!(!ini.value("T/no").to_bool());
    // QVariant's toBool is false only for "", "0" and "false".
    assert!(ini.value("T/word").to_bool());
    assert_eq!(ini.value("T/num").to_i32(), 12);
    assert_eq!(ini.value("T/neg").to_i32(), -3);
    assert_eq!(ini.value("T/neg").to_u32(), 0);
    assert_eq!(ini.value("T/bad").to_i32(), 0);
    // toInt truncates a 64-bit parse the way Qt's int(qlonglong) cast does.
    assert_eq!(ini.value("T/big").to_i32(), 0);
    assert_eq!(ini.value("T/big").to_i64(), 4294967296);
    assert_eq!(ini.value("T/d").to_f64(), 4187593113.6);
    assert_eq!(ini.value("T/list").to_int_list(), [1, 0, 3]);
    assert_eq!(ini.value("T/list").to_qstring(), "");
}
