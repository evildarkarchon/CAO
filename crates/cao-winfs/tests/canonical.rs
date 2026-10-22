//! `canonical` and `weakly_canonical` against a real directory tree.
//!
//! Recovery compares a manifest's recorded Mod Root with the scope's
//! canonical text byte for byte, so these tests pin the text itself, not just
//! the location: every result is compared as a string, because `Path`
//! equality ignores trailing separators and would hide a missing one.
//!
//! The unit tests in `src/canonical.rs` cover the lexical rules; these cover
//! what needs the filesystem: case, links, long paths and missing entries.

mod common;

use std::io;
use std::path::{Path, PathBuf};

use cao_winfs::{canonical, weakly_canonical};
use common::{junction, scratch_dir, write};

/// `ERROR_FILE_NOT_FOUND`: the last component is missing.
const FILE_NOT_FOUND: i32 = 2;
/// `ERROR_PATH_NOT_FOUND`: a parent component is missing or is a file.
const PATH_NOT_FOUND: i32 = 3;
/// `ERROR_DIRECTORY`: a file was named with a trailing separator.
const NOT_A_DIRECTORY: i32 = 267;

/// Six 57-character components, so any path through them is longer than
/// `MAX_PATH` whatever the scratch root.
fn deep() -> String {
    (0..6)
        .map(|i| format!("deep{i}-{}", "x".repeat(50)))
        .collect::<Vec<_>>()
        .join("\\")
}

/// A fresh tree under the canonical text of a scratch directory:
///
/// ```text
/// Mixed\Case\file.txt
/// Straße\
/// link\            junction to Mixed
/// <deep>\file.txt
/// ```
///
/// Returns the root's canonical text, which every expectation builds on.
fn tree(name: &str) -> String {
    let root = canonical(&scratch_dir(&format!("canonical/{name}"))).unwrap();
    write(&root.join(r"Mixed\Case\file.txt"), b"");
    std::fs::create_dir(root.join("Straße")).unwrap();
    junction(&root.join("link"), &root.join("Mixed"));
    write(&root.join(deep()).join("file.txt"), b"");
    root.into_os_string().into_string().unwrap()
}

/// The result as text, or the Win32 code it failed with.
fn text(result: io::Result<PathBuf>) -> Result<String, i32> {
    result
        .map(|path| path.into_os_string().into_string().unwrap())
        .map_err(|error| error.raw_os_error().expect("a Win32 error code"))
}

fn canon(input: &str) -> Result<String, i32> {
    text(canonical(Path::new(input)))
}

fn weakly(input: &str) -> Result<String, i32> {
    text(weakly_canonical(Path::new(input)))
}

/// A drive-letter result never keeps `\\?\`, even past `MAX_PATH`, and a
/// verbatim input resolves to the same text as the plain one.
#[test]
fn drive_letter_results_have_no_verbatim_prefix() {
    let root = tree("verbatim");
    assert!(!root.starts_with(r"\\?\"), "{root}");
    assert!(root.as_bytes()[1] == b':', "{root}");

    let long = format!(r"{root}\{}\file.txt", deep());
    assert!(long.len() > 260);
    assert_eq!(canon(&long), Ok(long.clone()));
    assert_eq!(canon(&format!(r"\\?\{long}")), Ok(long));
    assert_eq!(canon(&format!(r"\\?\{root}")), Ok(root));
}

/// Every spelling of one location resolves to the same text: the property
/// recovery's byte-for-byte comparison relies on.
#[test]
fn every_spelling_of_a_location_is_one_text() {
    let root = tree("spellings");
    let expected = format!(r"{root}\Mixed\Case");
    for spelling in [
        format!(r"{root}\Mixed\Case"),
        format!(r"{root}\MIXED\case"),
        format!(r"{root}\Mixed\Case\"),
        format!(r"{root}\Mixed\Case\\"),
        format!(r"{root}/Mixed//Case/./"),
        format!(r"{root}\Mixed\.\Case\..\Case"),
        format!(r"{root}\link\Case"),
        format!(r"\\?\{root}\Mixed\Case"),
    ] {
        assert_eq!(canon(&spelling), Ok(expected.clone()), "{spelling}");
        assert_eq!(weakly(&spelling), Ok(expected.clone()), "{spelling}");
    }
}

/// Names come back as they are stored on disk, including non-ASCII ones.
#[test]
fn case_is_taken_from_the_disk() {
    let root = tree("case");
    assert_eq!(
        canon(&format!(r"{root}\MIXED\case\FILE.TXT")),
        Ok(format!(r"{root}\Mixed\Case\file.txt"))
    );
    assert_eq!(
        canon(&format!(r"{root}\STRAßE")),
        Ok(format!(r"{root}\Straße"))
    );
}

/// A junction resolves to its target, for a file beneath it too.
#[test]
fn junctions_resolve_to_their_target() {
    let root = tree("junction");
    assert_eq!(
        canon(&format!(r"{root}\link\Case\file.txt")),
        Ok(format!(r"{root}\Mixed\Case\file.txt"))
    );
    assert_eq!(
        weakly(&format!(r"{root}\link\Case\missing\")),
        Ok(format!(r"{root}\Mixed\Case\missing\"))
    );
}

/// `..` in a long path walks back through the real chain.
#[test]
fn long_paths_normalize_like_short_ones() {
    let root = tree("long");
    let deep = deep();
    let back = r"..\".repeat(6);
    assert_eq!(
        canon(&format!(r"{root}\{deep}\{back}{deep}\file.txt")),
        Ok(format!(r"{root}\{deep}\file.txt"))
    );
    assert_eq!(
        canon(&format!(r"{root}\{deep}\")),
        Ok(format!(r"{root}\{deep}"))
    );
}

/// `canonical` needs the whole path to exist; it fails with the Win32 code
/// that says which part is wrong.
#[test]
fn canonical_fails_on_a_missing_path() {
    let root = tree("missing");
    let file = format!(r"{root}\Mixed\Case\file.txt");
    assert_eq!(canon(&format!(r"{root}\missing")), Err(FILE_NOT_FOUND));
    assert_eq!(
        canon(&format!(r"{root}\missing\child")),
        Err(PATH_NOT_FOUND)
    );
    assert_eq!(canon(&format!(r"{file}\child")), Err(PATH_NOT_FOUND));
    assert_eq!(canon(&format!(r"{file}\")), Err(NOT_A_DIRECTORY));
    assert_eq!(
        canon(&format!(r"{root}\{}\missing", deep())),
        Err(FILE_NOT_FOUND)
    );
}

/// `weakly_canonical` resolves the longest existing prefix and appends the
/// rest as normalized text. A trailing separator on the missing part is kept,
/// so a caller can still tell a directory from a file.
#[test]
fn weakly_canonical_keeps_the_missing_tail() {
    let root = tree("weakly");
    let deep = deep();
    let cases = [
        (r"\straße\missing", r"\Straße\missing"),
        (r"\Mixed\missing\", r"\Mixed\missing\"),
        (r"\Mixed\missing\.", r"\Mixed\missing\"),
        (r"\Mixed\missing\..", r"\Mixed"),
        (r"\Mixed\missing\..\", r"\Mixed"),
        (r"\Mixed\.\missing\x\..\y\", r"\Mixed\missing\y\"),
        (r"\Mixed\Case\..\..\missing", r"\missing"),
        (r"/Mixed//missing//child", r"\Mixed\missing\child"),
        (r"\Mixed\Case\file.txt\", r"\Mixed\Case\file.txt\"),
        (r"\Mixed\Case\file.txt\child", r"\Mixed\Case\file.txt\child"),
    ];
    for (input, expected) in cases {
        assert_eq!(
            weakly(&format!("{root}{input}")),
            Ok(format!("{root}{expected}")),
            "{input}"
        );
    }

    let short = deep.rsplit_once('\\').unwrap().0;
    assert_eq!(
        weakly(&format!(r"{root}\{deep}\missing\child\")),
        Ok(format!(r"{root}\{deep}\missing\child\"))
    );
    assert_eq!(
        weakly(&format!(r"{root}\{deep}\.\missing\..\file.txt")),
        Ok(format!(r"{root}\{deep}\file.txt"))
    );
    assert_eq!(
        weakly(&format!(r"{root}\{deep}\missing\..\..\new")),
        Ok(format!(r"{root}\{short}\new"))
    );
}

/// An empty path is returned as is, without touching the filesystem.
#[test]
fn an_empty_path_stays_empty() {
    assert_eq!(canonical(Path::new("")).unwrap(), PathBuf::new());
    assert_eq!(weakly_canonical(Path::new("")).unwrap(), PathBuf::new());
}
