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
