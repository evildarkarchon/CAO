//! The MSVC-canonical helpers against the MSVC STL itself.
//!
//! `tests/fixtures/msvc-probe/expected.txt` is what `canonical_probe.cpp` recorded
//! from MSVC's `std::filesystem::canonical` and `weakly_canonical` (STL 14.51,
//! built with CAO's manifest). These tests rebuild the same tree, run every case
//! through [`msvc_canonical`] or [`msvc_weakly_canonical`], and expect the same
//! native text or the same Win32 error. C++-written `CAO-STAGING` manifests store
//! that text, and recovery compares it byte for byte.
//!
//! In a result, `{root}` stands for the canonical scratch root. Only the root
//! itself is therefore checked through the helper under test; the literal
//! `C:\Windows` cases, and the assertion that the root comes back without a
//! `\\?\` prefix, cover it independently.

mod common;

use std::path::{Path, PathBuf};

use cao_winfs::{msvc_canonical, msvc_weakly_canonical};
use common::{junction, scratch_dir};

/// The probe's `{deep}` chain: six 57-character components.
fn deep() -> String {
    (0..6)
        .map(|i| format!("deep{i}-{}", "x".repeat(50)))
        .collect::<Vec<_>>()
        .join("\\")
}

fn expand(text: &str, root: &str) -> String {
    text.replace("{deep}", &deep()).replace("{root}", root)
}

/// A recorded case: its kind, its input template and the recorded result.
struct Case<'a> {
    kind: &'a str,
    input: &'a str,
    expected: &'a str,
}

/// Builds the recorded tree under `root` and returns the recorded cases.
fn build_tree<'a>(recording: &'a str, root: &Path) -> Vec<Case<'a>> {
    let root_text = root.to_str().unwrap();
    let mut cases = Vec::new();
    for line in recording.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        if let [kind, input, expected] = fields[..] {
            cases.push(Case {
                kind,
                input,
                expected,
            });
            continue;
        }
        let words: Vec<&str> = line.split(' ').collect();
        let path = root.join(expand(words[1], root_text));
        match words[0] {
            "dir" => std::fs::create_dir_all(&path).unwrap(),
            "file" => common::write(&path, b""),
            "junction" => junction(&path, &root.join(expand(words[2], root_text))),
            other => panic!("unknown recording line kind {other:?}"),
        }
    }
    cases
}

/// Formats a helper result the way the probe records it.
fn recorded_form(result: std::io::Result<PathBuf>, canonical_root: &str) -> String {
    match result {
        Ok(path) => {
            let text = path.to_str().unwrap();
            let text = match text.strip_prefix(canonical_root) {
                Some(rest) => format!("{{root}}{rest}"),
                None => text.to_owned(),
            };
            text.replace(&deep(), "{deep}")
        }
        Err(error) => format!("!error {}", error.raw_os_error().unwrap()),
    }
}

#[test]
fn helpers_reproduce_recorded_msvc_output() {
    let recording = include_str!("fixtures/msvc-probe/expected.txt");
    let root = scratch_dir("msvc-canonical");
    let cases = build_tree(recording, &root);
    assert!(cases.len() >= 40, "the recording lost its cases");

    let canonical_root = msvc_canonical(&root).unwrap();
    let canonical_root = canonical_root.to_str().unwrap();
    assert!(
        !canonical_root.starts_with(r"\\?\"),
        "MSVC strips the verbatim prefix from drive-letter results: {canonical_root}"
    );

    let root_text = root.to_str().unwrap();
    let mut mismatches = Vec::new();
    for case in &cases {
        let input = PathBuf::from(expand(case.input, root_text));
        let result = match case.kind {
            "canonical" => msvc_canonical(&input),
            "weakly" => msvc_weakly_canonical(&input),
            other => panic!("unknown case kind {other:?}"),
        };
        let actual = recorded_form(result, canonical_root);
        if actual != case.expected {
            mismatches.push(format!(
                "{}\t{}\n  expected {}\n  actual   {actual}",
                case.kind, case.input, case.expected
            ));
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

/// An empty path canonicalizes to an empty path, as MSVC's `_Canonical`
/// returns success without opening anything.
#[test]
fn empty_path_stays_empty() {
    assert_eq!(msvc_canonical(Path::new("")).unwrap(), PathBuf::new());
    assert_eq!(
        msvc_weakly_canonical(Path::new("")).unwrap(),
        PathBuf::new()
    );
}
