//! The reader against Qt 5.15 itself.
//!
//! Every `tests/fixtures/qt-read/<name>.ini` has a `<name>.qt.txt` beside it: what
//! `qt-probe/qsettings_probe.cpp dump` printed after reading the file through
//! QSettings. These tests print the same dump from [`IniFile`] and expect it to
//! match, so escapes, quotes, lists, comments, section rules, duplicates and
//! QVariant's lenient conversions are all checked against Qt rather than against a
//! reading of its source.
//!
//! Two of the inputs were written by the Rust writer. Their Qt dumps show the C++
//! build reads Rust-written files correctly, and a test below keeps the writer's
//! output for them unchanged; if it changes, regenerate them with
//! `CAO_PROFILES_BLESS=1` and dump them through the probe again.
//!
//! No input triggers a recorded deviation, which `ini_read.rs` pins instead: there is
//! no `#` line, no BOM, every non-ASCII byte leaves the file invalid UTF-8, and a
//! scalar's list is compared as Qt's empty list (deviation 12).

mod common;

use std::path::{Path, PathBuf};

use cao_profiles::{IniFile, Value};
use common::{fixtures, read};

fn qt_read() -> PathBuf {
    fixtures().join("qt-read")
}

/// Escapes text like the probe: printable ASCII other than `\` as is, anything else
/// as `\u{XXXX}` per UTF-16 unit.
fn escape(text: &str) -> String {
    text.encode_utf16()
        .map(|unit| match unit {
            0x20..=0x7E if unit != u16::from(b'\\') => char::from(unit as u8).to_string(),
            _ => format!("\\u{{{unit:04x}}}"),
        })
        .collect()
}

/// Qt's `QString::number(d, 'g', QLocale::FloatingPointShortest)`. The writer formats
/// doubles that way, so a one-key file reads it back.
fn qt_number(d: f64) -> String {
    let mut ini = IniFile::new();
    ini.set("d", d);
    ini.value("d").to_qstring()
}

fn category(value: &Value) -> &'static str {
    match value {
        Value::Invalid => "invalid",
        Value::String(_) => "string",
        Value::List(_) => "list",
        Value::Encoded(raw) if raw.starts_with("@String(") => "string",
        Value::Encoded(raw) if raw.starts_with("@Variant(") => "list",
        Value::Encoded(_) => "other",
    }
}

/// The probe's dump format, produced from the Rust reader.
fn dump(ini: &IniFile) -> String {
    let status = if ini.format_error().is_some() { 2 } else { 0 };
    let mut keys: Vec<&str> = ini.keys().collect();
    keys.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()).then(a.cmp(b)));

    let mut text = format!("status={status}\n");
    for key in keys {
        let value = ini.value(key);
        // Deviation 12 reads a scalar string as a one-element list; Qt reads it as `[]`.
        let list = match category(value) {
            "string" => Vec::new(),
            _ => value.to_int_list(),
        };
        let list: Vec<String> = list.iter().map(i32::to_string).collect();
        text += &format!(
            "{}\t{}\ts={}\tb={}\ti={}\tu={}\td={}\tl={}\n",
            escape(key),
            category(value),
            escape(&value.to_qstring()),
            u8::from(value.to_bool()),
            value.to_i32(),
            value.to_u32(),
            qt_number(value.to_f64()),
            list.join(",")
        );
    }
    text
}

fn qt_dump(input: &Path) -> String {
    String::from_utf8(read(&input.with_extension("qt.txt"))).unwrap()
}

#[test]
fn every_input_reads_as_qt_reads_it() {
    let mut inputs: Vec<PathBuf> = std::fs::read_dir(qt_read())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "ini"))
        .collect();
    inputs.sort();
    assert!(inputs.len() >= 6, "fixtures missing: {inputs:?}");

    for input in inputs {
        let rust = dump(&IniFile::parse(&read(&input)));
        let qt = qt_dump(&input);
        if rust != qt {
            let rust_lines: Vec<&str> = rust.lines().collect();
            let differences: Vec<String> = qt
                .lines()
                .zip(rust.lines())
                .filter(|(q, r)| q != r)
                .map(|(q, r)| format!("  qt:   {q}\n  rust: {r}"))
                .collect();
            panic!(
                "{}: {} Qt lines, {} Rust lines\n{}",
                input.display(),
                qt.lines().count(),
                rust_lines.len(),
                differences.join("\n")
            );
        }
    }
}

/// A profile and settings written from scratch through the Rust API, with every
/// encoding the writer can produce: a one-element list, an empty list, a userPath
/// that needs quoting and `\x` escapes, doubles in both notations, root keys and a
/// real `General` group.
fn rust_written_profile() -> IniFile {
    let mut ini = IniFile::new();
    ini.set("profile", "SSE");
    ini.set("userPath", "D:/Mods/Café; Stuff, \"Ünïcödé\" 漢 😀 ");
    ini.set("General/real", true);
    ini.set("BSA/bsaGame", 4);
    ini.set("BSA/maxBsaUncompressedSize", 2147483648.0);
    ini.set("BSA/tiny", 1.5e-7);
    ini.set("BSA/huge", 3e12);
    ini.set("Meshes/meshesStream", 100u32);
    ini.set("Textures/texturesUnwantedFormats", Value::int_list(&[85]));
    ini.set("Textures/one98", Value::int_list(&[98]));
    ini.set("Textures/oneNegative", Value::int_list(&[-1]));
    ini.set("Textures/oneZero", Value::int_list(&[0]));
    ini.set("Textures/none", Value::int_list(&[]));
    ini.set("Textures/three", Value::int_list(&[86, 85, 115]));
    ini
}

/// The hand-edited input after one Rust read and write.
fn rust_rewrite_of_hand_edited() -> IniFile {
    IniFile::parse(&read(&qt_read().join("hand-edited.ini")))
}

#[test]
fn rust_written_inputs_are_what_the_writer_still_writes() {
    let cases = [
        ("rust-written-profile.ini", rust_written_profile()),
        (
            "rust-rewrite-of-hand-edited.ini",
            rust_rewrite_of_hand_edited(),
        ),
    ];
    for (name, ini) in cases {
        let path = qt_read().join(name);
        if std::env::var_os("CAO_PROFILES_BLESS").is_some() {
            std::fs::write(&path, ini.to_bytes()).unwrap();
        }
        assert!(
            ini.to_bytes() == read(&path),
            "{name}: the writer changed; regenerate it and its Qt dump"
        );
    }
}

#[test]
fn qt_reads_a_rust_rewrite_to_the_values_it_read_from_the_original() {
    // Qt read the hand-edited file and the Rust rewrite of it to identical values, so
    // a Rust save loses nothing Qt can see, including keys CAO does not know. Key case
    // is compared loosely: Qt writes a `[%general]` group back as `[%General]`, and
    // lookup ignores case anyway.
    let lowercase_keys = |dump: String| -> Vec<String> {
        dump.lines()
            .map(|line| match line.split_once('\t') {
                Some((key, rest)) => format!("{}\t{rest}", key.to_lowercase()),
                None => line.to_owned(),
            })
            .collect()
    };
    assert_eq!(
        lowercase_keys(qt_dump(&qt_read().join("rust-rewrite-of-hand-edited.ini"))),
        lowercase_keys(qt_dump(&qt_read().join("hand-edited.ini")))
    );
}

#[test]
fn qt_reads_the_rust_written_profile_to_the_values_rust_set() {
    let qt = qt_dump(&qt_read().join("rust-written-profile.ini"));
    for line in [
        "status=0",
        "Textures/texturesUnwantedFormats\tlist\ts=\tb=0\ti=0\tu=0\td=0\tl=85\n",
        "Textures/oneNegative\tlist\ts=\tb=0\ti=0\tu=0\td=0\tl=-1\n",
        "Textures/none\tinvalid\ts=\tb=0\ti=0\tu=0\td=0\tl=\n",
        "Textures/three\tlist\ts=\tb=0\ti=0\tu=0\td=0\tl=86,85,115\n",
        "userPath\tstring\ts=D:/Mods/Caf\\u{00e9}; Stuff, \"\\u{00dc}n\\u{00ef}c\\u{00f6}d\\u{00e9}\" \\u{6f22} \\u{d83d}\\u{de00} \t",
        "BSA/maxBsaUncompressedSize\tstring\ts=2147483648\tb=1\ti=-2147483648\tu=2147483648\td=2147483648\t",
        "General/real\tstring\ts=true\t",
    ] {
        assert!(qt.contains(line), "Qt's dump lacks {line:?}:\n{qt}");
    }
}
