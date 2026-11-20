//! The INI reader and writer, without the models: the recorded deviations from
//! Qt 5.15 (spec #476, items 12–14), UTF-8 BOM files, format errors, lookup,
//! saving through the filesystem, and the shipped `profiles/`.
//!
//! Reading existing users' files is the requirement this module serves, and no
//! other implementation checks it any more, so the escape and quoting paths are
//! pinned here: golden reads of each escape form, golden writes, and a
//! write-then-parse round trip over awkward strings (GUI-7).

use std::path::{Path, PathBuf};

use super::{FormatError, FormatErrorKind, IniError, IniFile, Value};

/// Every key of `ini`, in file order, as first spelt.
fn keys(ini: &IniFile) -> Vec<&str> {
    ini.entries.iter().map(|entry| entry.key.as_str()).collect()
}

/// `value` read as a list of integers, the way the closed-set reader reads a
/// list, each entry by `QVariant::toInt()`.
fn ints(value: &Value) -> Vec<i32> {
    value
        .list_entries()
        .expect("a readable list")
        .iter()
        .map(Value::to_i32)
        .collect()
}

/// The repository's shipped `profiles/` directory.
fn shipped_profiles() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../profiles")
}

/// Reads a file, panicking with its path on failure.
fn read(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// Reads a shipped file as QSettings wrote it: Git may check text files out
/// with LF or CRLF, and QSettings always writes CRLF.
fn read_as_crlf(path: &Path) -> Vec<u8> {
    let text = String::from_utf8(read(path)).expect("shipped INI files are ASCII");
    text.replace("\r\n", "\n")
        .replace('\n', "\r\n")
        .into_bytes()
}

/// A fresh, empty directory for one test. Unit tests have no
/// `CARGO_TARGET_TMPDIR`, so it lives under the system's temporary folder.
fn scratch_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("cao-profiles-ini-tests")
        .join(name);
    // A missing directory is the expected case; anything else surfaces in create_dir_all.
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Shows bytes as text with CR and LF visible, for readable assertion failures.
fn show(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| match b {
            b'\r' => "\\r".to_owned(),
            b'\n' => "\\n\n".to_owned(),
            0x20..=0x7E => (b as char).to_string(),
            _ => format!("<{b:02X}>"),
        })
        .collect()
}

/// The names in `dir`, sorted.
fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

// ---- Reading: deviations, BOM, format errors and lookup

#[test]
fn deviation_12_a_scalar_where_a_list_is_expected_reads_as_one_element() {
    // Qt reads a hand-edited `texturesUnwantedFormats=85` as an empty list.
    let ini = IniFile::parse(b"[Textures]\r\ntexturesUnwantedFormats=85\r\n");

    assert_eq!(ints(ini.value("Textures/texturesUnwantedFormats")), [85]);
}

#[test]
fn deviation_12_does_not_turn_an_empty_value_into_a_list() {
    let ini = IniFile::parse(b"[Textures]\r\ntexturesUnwantedFormats=\r\n");

    assert!(ints(ini.value("Textures/texturesUnwantedFormats")).is_empty());
}

#[test]
fn deviation_13_a_hash_line_is_a_comment() {
    // In Qt, `# note` is a line without `=` and sets FormatError, so run setup fails;
    // `#bsaGame=3` is a key named `#bsaGame`.
    let ini = IniFile::parse(
        b"# note: tuned for my setup\r\n[BSA]\r\n  # indented \"quote\r\n#bsaGame=3\r\nbsaGame=4\r\n",
    );

    assert_eq!(ini.format_error(), None);
    assert_eq!(keys(&ini), ["BSA/bsaGame"]);
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
    assert!(ini.get("BSA/bsaEnabled").is_none());
    assert!(!missing.to_bool());
    assert_eq!(missing.to_i32(), 0);
    assert_eq!(missing.to_u32(), 0);
    assert_eq!(missing.to_qstring(), "");
    assert!(ints(missing).is_empty());
}

#[test]
fn lookup_ignores_case_and_writing_keeps_the_original_case_and_place() {
    let mut ini = IniFile::parse(b"[BSA]\r\nbsaGame=4\r\nbsaEnabled=true\r\n");

    assert!(ini.get("bsa/BSAGAME").is_some());
    assert_eq!(ini.value("bsa/BSAGAME").to_i32(), 4);

    ini.set("BSA/BSAGAME", 5);
    ini.set("bsa/bsaNew", 1);

    assert_eq!(keys(&ini), ["BSA/bsaGame", "BSA/bsaEnabled", "bsa/bsaNew"]);
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
        b"[T]\r\nyes=True\r\nno=FALSE\r\nword=no\r\nnum= 12 \r\nneg=-3\r\nbad=12abc\r\nbig=4294967296\r\nlist=1, x, 3\r\n",
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
    assert_eq!(ints(ini.value("T/list")), [1, 0, 3]);
    assert_eq!(ini.value("T/list").to_qstring(), "");
}

// ---- Reading: golden cases for each escape and quoting form (GUI-7)

/// The value of `key` after reading `text` as a whole file.
fn read_value(text: &[u8], key: &str) -> Value {
    let ini = IniFile::parse(text);
    assert_eq!(ini.format_error(), None, "{}", show(text));
    ini.value(key).clone()
}

/// The string `key` holds after reading `text` as a whole file.
fn read_string(text: &[u8], key: &str) -> String {
    read_value(text, key).to_qstring()
}

#[test]
fn each_simple_escape_reads_as_its_character() {
    let text = br#"k=a\ab\bf\fn\nr\rt\tv\vq\"m\?s\'b\\e"#;

    assert_eq!(
        read_string(text, "k"),
        "a\u{7}b\u{8}f\u{c}n\nr\rt\tv\u{b}q\"m?s'b\\e"
    );
}

#[test]
fn hex_and_octal_escapes_are_greedy_16_bit_units() {
    for (text, expected) in [
        (&br"k=\x41"[..], "A"),
        // Every following hex digit joins the unit, as Qt's reader takes it.
        (br"k=\x4142", "\u{4142}"),
        (br"k=\xe9t", "\u{e9}t"),
        // Two units that form a surrogate pair decode as one character.
        (br"k=\xd83d\xde00", "\u{1F600}"),
        (br"k=\101", "A"),
        (br"k=\1012", "\u{20A}"),
        (br"k=a\0b", "a\0b"),
        // `8` is not an octal digit, so it ends the escape.
        (br"k=\18", "\u{1}8"),
    ] {
        assert_eq!(read_string(text, "k"), expected, "{}", show(text));
    }
}

#[test]
fn an_unknown_escape_is_dropped_with_its_backslash() {
    assert_eq!(read_string(br"k=a\qb", "k"), "ab");
    // `\x` with no hex digit after it drops only the escape.
    assert_eq!(read_string(br"k=a\xgb", "k"), "agb");
}

#[test]
fn a_backslash_newline_continues_the_line() {
    for text in [
        &b"k=abc\\\r\ndef\r\nnext=1\r\n"[..],
        b"k=abc\\\ndef\nnext=1\n",
        b"k=abc\\\n\rdef\r\nnext=1\r\n",
    ] {
        assert_eq!(read_string(text, "k"), "abcdef", "{}", show(text));
        assert_eq!(read_string(text, "next"), "1", "{}", show(text));
    }
}

#[test]
fn a_quoted_value_spans_lines_and_keeps_its_line_breaks() {
    let text = b"k=\"line one\r\nline two\"\r\nnext=1\r\n";

    assert_eq!(read_string(text, "k"), "line one\r\nline two");
    assert_eq!(read_string(text, "next"), "1");
}

#[test]
fn spaces_around_a_value_are_dropped_unless_quoted() {
    for (text, expected) in [
        (&b"k=  a b  \r\n"[..], "a b"),
        (b"k=\ta\t\r\n", "a"),
        (b"k=\" a b \"\r\n", " a b "),
        (b"k=  \" a \"  \r\n", " a "),
        // Spaces between quoted and unquoted parts stay.
        (b"k=\"a\" b\r\n", "ab"),
        (b"k=a \"b\"\r\n", "a b"),
        (b"key  =  v\r\n", "v"),
    ] {
        let key = if text.starts_with(b"key") { "key" } else { "k" };
        assert_eq!(read_string(text, key), expected, "{}", show(text));
    }
}

#[test]
fn a_semicolon_ends_an_unquoted_value_and_a_quoted_one_keeps_it() {
    assert_eq!(read_string(b"k=abc ; a comment\r\n", "k"), "abc");
    assert_eq!(read_string(b"k=\"a;b\"\r\n", "k"), "a;b");
    assert_eq!(read_string(b"k=\"a=b\"\r\n", "k"), "a=b");
    // An unquoted `=` after the first is part of the value.
    assert_eq!(read_string(b"k=a=b\r\n", "k"), "a=b");
    assert_eq!(read_string(b"; whole line\r\nk=1\r\n", "k"), "1");
}

#[test]
fn an_unquoted_comma_makes_a_list_and_a_quoted_one_does_not() {
    let list = |text: &[u8]| -> Vec<String> {
        read_value(text, "k")
            .list_entries()
            .unwrap()
            .iter()
            .map(Value::to_qstring)
            .collect()
    };

    assert_eq!(list(b"k=a, b ,c\r\n"), ["a", "b", "c"]);
    assert_eq!(list(b"k=\"a, b\", c\r\n"), ["a, b", "c"]);
    assert_eq!(list(b"k=\" a \",\"b \"\r\n"), [" a ", "b "]);
    assert_eq!(list(b"k=a,\r\n"), ["a", ""]);
    assert_eq!(read_string(b"k=\"a, b\"\r\n", "k"), "a, b");
}

#[test]
fn keys_unescape_percent_forms_and_backslashes() {
    for (text, key) in [
        (&b"%41bc=1\r\n"[..], "Abc"),
        (b"%e9t%E9=1\r\n", "\u{e9}t\u{e9}"),
        (b"%U4E2D=1\r\n", "\u{4e2d}"),
        (b"%U00e9=1\r\n", "\u{e9}"),
        // A malformed escape keeps its `%`.
        (b"a%zz=1\r\n", "a%zz"),
        (b"a%4=1\r\n", "a%4"),
        (b"a%=1\r\n", "a%"),
        (b"a%U12=1\r\n", "a%U12"),
        // A backslash in a key is Qt's group separator.
        (b"[S]\r\nx\\y=1\r\n", "S/x/y"),
        (b"[S%20T]\r\nx=1\r\n", "S T/x"),
        (b"[%General]\r\nx=1\r\n", "General/x"),
        (b"[general]\r\nx=1\r\n", "x"),
    ] {
        let ini = IniFile::parse(text);
        assert_eq!(keys(&ini), [key], "{}", show(text));
    }
}

#[test]
fn a_percent_in_a_value_is_literal() {
    assert_eq!(read_string(b"k=%41%U0041\r\n", "k"), "%41%U0041");
}

#[test]
fn at_forms_read_as_qt_reads_them() {
    assert_eq!(read_value(b"k=@Invalid()\r\n", "k"), Value::Invalid);
    assert_eq!(read_string(b"k=@@x\r\n", "k"), "@x");
    assert_eq!(read_string(b"k=@String(abc)\r\n", "k"), "abc");
    // An unknown `@…(…)` form is plain text.
    assert_eq!(read_string(b"k=@Foo(1)\r\n", "k"), "@Foo(1)");
    assert_eq!(read_string(b"k=@x\r\n", "k"), "@x");
}

#[test]
fn a_one_element_list_qt_wrote_as_a_variant_blob_still_reads() {
    // What Qt's `setValue` wrote for `QVariantList{85}`: a QDataStream blob of
    // one `Int`. CAO no longer writes this shape, but existing files hold it.
    let text = br"texturesUnwantedFormats=@Variant(\0\0\0\t\0\0\0\x1\0\0\0\x2\0\0\0U)";

    assert_eq!(ints(&read_value(text, "texturesUnwantedFormats")), [85]);
    // A blob of anything but integers or strings cannot be read as a list.
    let other = br"k=@Variant(\0\0\0\x7f\0\0\0\x1)";
    assert_eq!(read_value(other, "k").list_entries(), None);
}

// ---- Writing: golden cases (GUI-7)

/// The line `ini` writes for `value` under the root key `k`.
fn written(value: impl Into<Value>) -> String {
    let mut ini = IniFile::new();
    ini.set("k", value);
    String::from_utf8(ini.to_bytes()).expect("the writer writes ASCII")
}

#[test]
fn values_are_escaped_and_quoted_as_qt_writes_them() {
    for (value, line) in [
        ("plain", "k=plain"),
        ("", "k="),
        (" lead", "k=\" lead\""),
        ("trail ", "k=\"trail \""),
        ("a;b", "k=\"a;b\""),
        ("a,b", "k=\"a,b\""),
        ("a=b", "k=\"a=b\""),
        ("q\"b\\s", "k=q\\\"b\\\\s"),
        ("t\tn\nr\r", "k=t\\tn\\nr\\r"),
        ("\u{7}\u{8}\u{c}\u{b}", "k=\\a\\b\\f\\v"),
        ("\u{1}x", "k=\\x1x"),
        ("\u{7f}", "k=\\x7f"),
        ("é", "k=\\xe9"),
        // A hex digit after `\x` or `\0` is escaped too, so the greedy reader
        // stops where the writer meant.
        ("éa", "k=\\xe9\\x61"),
        ("é1g", "k=\\xe9\\x31g"),
        ("\u{4e2d}", "k=\\x4e2d"),
        ("\u{1F600}", "k=\\xd83d\\xde00"),
        ("@x", "k=@@x"),
        ("a\0b", "k=@String(a\\0\\x62)"),
        ("%41", "k=%41"),
    ] {
        assert_eq!(
            written(value),
            format!("[General]\r\n{line}\r\n"),
            "{value:?}"
        );
    }
}

#[test]
fn keys_are_escaped_as_qt_writes_them() {
    for (key, written) in [
        ("plain_key-1.x", "[General]\r\nplain_key-1.x=1\r\n"),
        ("a b", "[General]\r\na%20b=1\r\n"),
        ("a=b", "[General]\r\na%3Db=1\r\n"),
        ("é", "[General]\r\n%E9=1\r\n"),
        ("\u{4e2d}", "[General]\r\n%U4E2D=1\r\n"),
        ("S/x/y", "[S]\r\nx\\y=1\r\n"),
        ("S T/x", "[S%20T]\r\nx=1\r\n"),
        ("General/x", "[%General]\r\nx=1\r\n"),
    ] {
        let mut ini = IniFile::new();
        ini.set(key, 1);
        assert_eq!(
            String::from_utf8(ini.to_bytes()).unwrap(),
            written,
            "{key:?}"
        );
    }
}

#[test]
fn integer_lists_write_plainly_and_an_empty_one_as_invalid() {
    assert_eq!(
        written(Value::int_list(&[])),
        "[General]\r\nk=@Invalid()\r\n"
    );
    // One element is the plain number, which reads back as a one-element list.
    assert_eq!(written(Value::int_list(&[85])), "[General]\r\nk=85\r\n");
    assert_eq!(
        written(Value::int_list(&[86, 85, 115])),
        "[General]\r\nk=86, 85, 115\r\n"
    );
    for list in [&[][..], &[85], &[86, 85, 115], &[-1, 0]] {
        let ini = IniFile::parse(written(Value::int_list(list)).as_bytes());
        assert_eq!(ints(ini.value("k")), list, "{list:?}");
    }
}

// ---- Round trip: what is written reads back unchanged (GUI-7)

/// Strings that exercise every escape and quoting path of the writer and reader.
const AWKWARD: &[&str] = &[
    "",
    " ",
    "  both ends  ",
    " lead",
    "trail ",
    "\tindent",
    "a;b",
    "a,b",
    "a=b",
    "; starts a comment",
    "# not a comment here",
    "=leading equals",
    "semi;colon,comma=equals",
    "\"quoted\"",
    "\"",
    "back\\slash",
    "trailing backslash\\",
    "\\x41 is not an escape",
    "multi\nline\r\nvalue\r",
    "\u{7}\u{8}\u{c}\u{b}\t",
    "\0",
    "nul\0then digits 0123",
    "\u{1}9",
    "\u{7f}",
    "@",
    "@at",
    "@@double",
    "@Invalid()",
    "@String(x)",
    "@Variant(x)",
    "%41 %U0041 %",
    "café",
    "éa éA é9",
    "\u{4e2d}\u{6587}",
    "\u{1F600} emoji",
    "D:/Mods/Café; v2",
    "C:\\Program Files (x86)\\Steam",
    "\\\\server\\share",
];

#[test]
fn awkward_strings_round_trip_through_write_and_parse() {
    for value in AWKWARD {
        let mut ini = IniFile::new();
        ini.set("Section/key", *value);
        ini.set("after", "x");
        let bytes = ini.to_bytes();
        assert!(bytes.is_ascii(), "{value:?}: {}", show(&bytes));

        let read = IniFile::parse(&bytes);
        assert_eq!(read.format_error(), None, "{value:?}: {}", show(&bytes));
        assert_eq!(
            read.value("Section/key").to_qstring(),
            *value,
            "{value:?}: {}",
            show(&bytes)
        );
        assert_eq!(read.value("after").to_qstring(), "x", "{value:?}");
        // Read back, it writes the same bytes again.
        assert_eq!(read.to_bytes(), bytes, "{value:?}");
    }
}

#[test]
fn awkward_strings_round_trip_as_keys() {
    for name in AWKWARD.iter().filter(|name| !name.is_empty()) {
        // `/` separates groups, so a key cannot hold one of its own.
        if name.contains('/') {
            continue;
        }
        let key = format!("Section/{name}");
        let mut ini = IniFile::new();
        ini.set(&key, 1);
        let bytes = ini.to_bytes();

        let read = IniFile::parse(&bytes);
        assert_eq!(read.format_error(), None, "{key:?}: {}", show(&bytes));
        assert_eq!(keys(&read), [key.as_str()], "{}", show(&bytes));
    }
}

#[test]
fn awkward_strings_round_trip_as_list_entries() {
    let mut ini = IniFile::new();
    let entries: Vec<Value> = AWKWARD
        .iter()
        // A list Qt reads back holds plain strings, and an `@` entry is written
        // as its own encoded form.
        .filter(|entry| !entry.starts_with('@') && !entry.contains('\0'))
        .map(|entry| Value::String((*entry).to_owned()))
        .collect();
    ini.set("k", Value::List(entries.clone()));
    let bytes = ini.to_bytes();

    let read = IniFile::parse(&bytes);
    assert_eq!(read.format_error(), None, "{}", show(&bytes));
    assert_eq!(
        read.value("k").list_entries().unwrap(),
        entries,
        "{}",
        show(&bytes)
    );
}

// ---- The shipped profiles

/// Every shipped INI file. CAO ships no `common.ini` (WS-8).
const SHIPPED: [&str; 6] = [
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
        ini.value("BSA/maxBsaUncompressedSize").to_qstring(),
        "2104533975.04"
    );
    assert_eq!(ini.value("Meshes/meshesFileVersion").to_i32(), 335675399);
    assert_eq!(ini.value("Meshes/meshesStream").to_u32(), 83);
    assert_eq!(ini.value("Meshes/meshesUser").to_u32(), 12);
    assert!(ini.value("Meshes/meshesEnabled").to_bool());
    assert!(!ini.value("Animations/animationsEnabled").to_bool());
    assert_eq!(ini.value("Textures/texturesFormat").to_i32(), 77);
    assert_eq!(
        ints(ini.value("Textures/texturesUnwantedFormats")),
        [98, 99]
    );
    assert!(!ini.value("Textures/texturesConvertTga").to_bool());
}

#[test]
fn the_fo4_unwanted_formats_keep_their_order() {
    let ini = IniFile::load(&shipped_profiles().join("FO4/profile.ini")).unwrap();

    assert_eq!(
        ints(ini.value("Textures/texturesUnwantedFormats")),
        [86, 85, 115]
    );
}

// ---- Loading and saving through the filesystem

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
        ints(reloaded.value("Textures/texturesUnwantedFormats")),
        [85]
    );
    assert!(
        String::from_utf8(read(&path))
            .unwrap()
            .contains("texturesUnwantedFormats=85\r\n")
    );

    // And an emptied list stays empty rather than becoming one empty element.
    let mut ini = reloaded;
    ini.set("Textures/texturesUnwantedFormats", Value::int_list(&[]));
    ini.save(&path).unwrap();
    let reloaded = IniFile::load(&path).unwrap();
    assert!(ints(reloaded.value("Textures/texturesUnwantedFormats")).is_empty());
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
fn saving_into_a_missing_directory_fails_and_creates_nothing() {
    // A profile deleted while CAO runs must not come back as a folder holding
    // only the file saved into it (GUI-8).
    let dir = scratch_dir("missing_parent");
    let path = dir.join("Deleted").join("profile.ini");

    let mut ini = IniFile::new();
    ini.set("BSA/bsaGame", 5);
    let error = ini.save(&path).unwrap_err();

    assert!(matches!(error, IniError::Write { .. }), "{error:?}");
    assert!(entries(&dir).is_empty());
}

#[test]
fn loading_a_missing_file_gives_empty_settings() {
    let dir = scratch_dir("missing_file");

    let ini = IniFile::load(&dir.join("absent.ini")).unwrap();

    assert!(keys(&ini).is_empty());
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
