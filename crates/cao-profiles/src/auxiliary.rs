//! The auxiliary text files a profile may carry beside its INI files:
//! `customHeadparts.txt`, `FilesToNotPack.txt` and `ignoredMods.txt`.
//!
//! Each is looked up per file in the selected profile, then in `profiles/SSE`
//! ([`crate::DEFAULT_PROFILE`]). `customLandscape.txt` and the profile's
//! `DummyPlugin.esp` are dead data (deviation 10): nothing here reads them.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// `customHeadparts.txt`: extra Headpart Mesh paths.
pub(crate) const CUSTOM_HEADPARTS: &str = "customHeadparts.txt";
/// `FilesToNotPack.txt`: the profile's Packing Exclusion rules.
pub(crate) const FILES_TO_NOT_PACK: &str = "FilesToNotPack.txt";
/// `ignoredMods.txt`: the names of the profile's Mod Exclusions.
pub(crate) const IGNORED_MODS: &str = "ignoredMods.txt";

/// The path a profile reads `name` from: its own file when anything exists at that
/// path, otherwise the fallback profile's. As in C++ (`QFile::exists`), a directory
/// under the file's name counts as existing, so it stops the fallback.
pub(crate) fn locate(directory: &Path, fallback: &Path, name: &str) -> PathBuf {
    let own = directory.join(name);
    if own.exists() {
        own
    } else {
        fallback.join(name)
    }
}

/// Reads the list at `path` with C++'s rules, or `Ok(None)` when nothing exists
/// there.
///
/// Lines split on `\n`. Each is decoded as lossy UTF-8 with a leading BOM dropped
/// (`QString::fromUtf8`), then `simplified()`. Empty lines and lines that start with
/// `#` after that are skipped.
///
/// # Errors
/// The I/O error when something exists at `path` but cannot be read.
pub(crate) fn read_list(path: &Path) -> io::Result<Option<Vec<String>>> {
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(path)?;
    Ok(Some(
        bytes
            .split(|&byte| byte == b'\n')
            .map(|line| {
                let text = String::from_utf8_lossy(line);
                simplified(text.strip_prefix('\u{FEFF}').unwrap_or(&text))
            })
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .collect(),
    ))
}

/// `QString::simplified()`: whitespace trimmed from both ends and each inner run of
/// whitespace replaced by one space. Qt's `QChar::isSpace` and Rust's
/// `char::is_whitespace` accept the same characters.
fn simplified(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
