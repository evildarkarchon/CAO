//! Paths shared by the cao-profiles integration tests.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// The crate's test fixtures directory.
pub fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// The repository's shipped `profiles/` directory.
pub fn shipped_profiles() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../profiles")
}

/// The repository root, which holds the shipped `profiles/` as an app directory would.
pub fn shipped_app_dir() -> PathBuf {
    shipped_profiles().parent().unwrap().to_owned()
}

/// Reads a file, panicking with its path on failure.
pub fn read(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// Reads a file the way Qt wrote it on Windows. Git may check text files out with LF
/// or CRLF; QSettings always writes CRLF, so this converts every line ending to CRLF.
pub fn read_as_crlf(path: &Path) -> Vec<u8> {
    let text = String::from_utf8(read(path)).expect("shipped INI files are ASCII");
    text.replace("\r\n", "\n")
        .replace('\n', "\r\n")
        .into_bytes()
}

/// A fresh, empty directory under the target directory for one test.
pub fn scratch_dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    // A missing directory is the expected case; anything else surfaces in create_dir_all.
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Shows bytes as text with CR and LF visible, for readable assertion failures.
pub fn show(bytes: &[u8]) -> String {
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

/// A fresh app directory for one test holding a copy of the shipped `profiles/`.
/// INI files are copied with CRLF, as Qt wrote them; every other file byte for byte.
pub fn copy_of_shipped(name: &str) -> PathBuf {
    let app_dir = scratch_dir(name);
    copy_tree(&shipped_profiles(), &app_dir.join("profiles"));
    app_dir
}

/// Copies `from` into a new directory `to`, recursively.
fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let path = entry.unwrap().path();
        let target = to.join(path.file_name().unwrap());
        if path.is_dir() {
            copy_tree(&path, &target);
        } else if path.extension().is_some_and(|extension| extension == "ini") {
            std::fs::write(&target, read_as_crlf(&path)).unwrap();
        } else {
            std::fs::copy(&path, &target).unwrap();
        }
    }
}
