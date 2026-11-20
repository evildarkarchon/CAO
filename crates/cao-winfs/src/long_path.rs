//! The extended-length (`\\?\`) form of a path, which Win32 accepts past
//! `MAX_PATH` on any host (WS-1 of the 2026-11-13 audit).
//!
//! A plain path longer than `MAX_PATH` only reaches Win32 when the process is
//! `longPathAware` *and* the machine-wide `LongPathsEnabled` registry value is
//! 1, which a default Windows install leaves off. An extended-length path is
//! not subject to either. std converts every path it hands to Win32 this way,
//! so [`crate::Open`] reaches deep files on any host; CAO's own raw calls
//! (renames, the manifest move, the volume queries) convert here so that they
//! agree with std's opens instead of failing part-way through publication.
//!
//! The conversion copies std's rule (`get_long_path` in std's Windows path
//! module): a short rooted path is passed through unchanged, and anything else
//! is made absolute with `GetFullPathNameW` and prefixed only when the result
//! is long. `\\?\` turns off Win32's own path normalization, so the
//! `GetFullPathNameW` step is what keeps `/`, `.`, `..` and trailing dots
//! meaning what they mean in the plain form.

use std::ffi::OsString;
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use crate::mutate::terminated;

/// std's threshold: `MAX_PATH` is 260 units with the terminator, but
/// `CreateDirectoryW` stops at 248, so std prefixes from there.
const LEGACY_MAX_PATH: usize = 248;

const SEPARATOR: u16 = b'\\' as u16;
const ALT_SEPARATOR: u16 = b'/' as u16;
const COLON: u16 = b':' as u16;
const QUERY: u16 = b'?' as u16;
const DOT: u16 = b'.' as u16;

/// `\\?\`
const VERBATIM: &[u16] = &[SEPARATOR, SEPARATOR, QUERY, SEPARATOR];
/// `\??\`, the NT object-manager prefix, which Win32 also passes through.
const NT: &[u16] = &[SEPARATOR, QUERY, QUERY, SEPARATOR];
/// `\\?\UNC\`
const VERBATIM_UNC: &[u16] = &[
    SEPARATOR,
    SEPARATOR,
    QUERY,
    SEPARATOR,
    b'U' as u16,
    b'N' as u16,
    b'C' as u16,
    SEPARATOR,
];

/// Returns `path` in the form Win32 accepts whatever its length and whatever
/// the host's `LongPathsEnabled` setting.
///
/// A rooted path shorter than 248 UTF-16 units, or one that is already
/// `\\?\` or `\??\`, comes back unchanged. Anything else is made absolute
/// with `GetFullPathNameW` (so a relative path resolves against the current
/// directory now, as Win32 would have resolved it during the call), and a
/// result of 248 units or more gets the `\\?\` prefix: `C:\x` becomes
/// `\\?\C:\x`, `\\server\share\x` becomes `\\?\UNC\server\share\x`, and a
/// `\\.\` device path becomes `\\?\`.
///
/// # Errors
///
/// Returns [`io::ErrorKind::InvalidInput`] for a path with an interior NUL or
/// an empty path that needs resolving, and otherwise the `GetFullPathNameW`
/// error.
pub fn extended_length_path(path: &Path) -> io::Result<PathBuf> {
    let units: Vec<u16> = path.as_os_str().encode_wide().collect();
    if units.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a path passed to Win32 cannot contain NUL",
        ));
    }
    if units.starts_with(VERBATIM) || units.starts_with(NT) {
        return Ok(path.to_path_buf());
    }
    if units.len() < LEGACY_MAX_PATH && is_rooted(&units) {
        return Ok(path.to_path_buf());
    }
    let absolute: Vec<u16> = std::path::absolute(path)?
        .as_os_str()
        .encode_wide()
        .collect();
    // std compares the length with the terminator it is about to add.
    if absolute.len() + 1 < LEGACY_MAX_PATH {
        return Ok(from_units(&absolute));
    }
    Ok(from_units(&prefixed(&absolute)))
}

/// [`extended_length_path`] as NUL-terminated UTF-16, for a Win32 path
/// parameter or buffer.
pub(crate) fn win32_path(path: &Path) -> io::Result<Vec<u16>> {
    terminated(extended_length_path(path)?.as_os_str())
}

/// Undoes the prefix [`extended_length_path`] adds to a drive-letter or UNC
/// path: `\\?\C:\x` becomes `C:\x` and `\\?\UNC\server\x` becomes
/// `\\server\x`. Anything else, such as a `\\?\Volume{GUID}\` path, is
/// returned unchanged.
///
/// Win32 answers an extended-length query in the same form (for example
/// `GetVolumePathNameW` returns `\\?\C:\` for `\\?\C:\x`), so a caller
/// that converted only to get past `MAX_PATH` uses this to hand back the form
/// it was given.
pub(crate) fn plain_units(mut units: Vec<u16>) -> Vec<u16> {
    if units.starts_with(VERBATIM_UNC) {
        // Keep two separators of the eight-unit prefix: \\?\UNC\ -> \\.
        units.drain(2..VERBATIM_UNC.len());
    } else if units.starts_with(VERBATIM) && is_drive_rooted(&units[VERBATIM.len()..]) {
        units.drain(..VERBATIM.len());
    }
    units
}

/// Whether `units` begins with a drive (`C:` alone, or `C:` and a separator)
/// or with two separators, the forms std passes through without resolving.
fn is_rooted(units: &[u16]) -> bool {
    match units {
        [drive, COLON] | [drive, COLON, SEPARATOR | ALT_SEPARATOR, ..] => !is_separator(*drive),
        [first, second, ..] => is_separator(*first) && is_separator(*second),
        _ => false,
    }
}

/// Whether `units` begins with a drive letter, a colon and a separator.
fn is_drive_rooted(units: &[u16]) -> bool {
    matches!(units, [drive, COLON, SEPARATOR, ..] if !is_separator(*drive))
}

fn is_separator(unit: u16) -> bool {
    unit == SEPARATOR || unit == ALT_SEPARATOR
}

/// std's prefix choice for an absolute path `GetFullPathNameW` returned.
fn prefixed(absolute: &[u16]) -> Vec<u16> {
    let (prefix, rest): (&[u16], &[u16]) = match absolute {
        // C:\ -> \\?\C:\
        [_, COLON, SEPARATOR, ..] => (VERBATIM, absolute),
        // \\.\ -> \\?\
        [SEPARATOR, SEPARATOR, DOT, SEPARATOR, ..] => (VERBATIM, &absolute[4..]),
        // Already in a namespace Win32 passes through.
        [SEPARATOR, SEPARATOR, QUERY, SEPARATOR, ..] | [SEPARATOR, QUERY, QUERY, SEPARATOR, ..] => {
            (&[], absolute)
        }
        // \\server -> \\?\UNC\server
        [SEPARATOR, SEPARATOR, ..] => (VERBATIM_UNC, &absolute[2..]),
        // Anything else is left as GetFullPathNameW wrote it.
        _ => (&[], absolute),
    };
    [prefix, rest].concat()
}

fn from_units(units: &[u16]) -> PathBuf {
    PathBuf::from(OsString::from_wide(units))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(path: &Path) -> String {
        path.to_str().unwrap().to_owned()
    }

    /// A drive path of `length` characters: `C:\` and then `a`s.
    fn drive_path(length: usize) -> String {
        format!(r"C:\{}", "a".repeat(length - 3))
    }

    #[test]
    fn short_rooted_paths_pass_through_unchanged() {
        for plain in [
            r"C:\Mods\Mod\textures\a.dds",
            "C:/Mods/Mod/../a.dds",
            r"\\server\share\a.dds",
            "C:",
        ] {
            assert_eq!(
                text(&extended_length_path(Path::new(plain)).unwrap()),
                plain
            );
        }
        let longest_short = drive_path(LEGACY_MAX_PATH - 1);
        assert_eq!(
            text(&extended_length_path(Path::new(&longest_short)).unwrap()),
            longest_short
        );
    }

    #[test]
    fn long_drive_paths_get_the_verbatim_prefix() {
        let plain = drive_path(LEGACY_MAX_PATH);
        assert_eq!(
            text(&extended_length_path(Path::new(&plain)).unwrap()),
            format!(r"\\?\{plain}")
        );
    }

    /// The prefix turns off Win32's normalization, so it is applied to the
    /// `GetFullPathNameW` result, never to the raw text.
    #[test]
    fn long_paths_are_normalized_before_the_prefix() {
        let folder = "f".repeat(150);
        let messy = format!("C:/{folder}/x/../{folder}/./name.dds");
        let expected = format!(r"\\?\C:\{folder}\{folder}\name.dds");
        assert_eq!(
            text(&extended_length_path(Path::new(&messy)).unwrap()),
            expected
        );
    }

    #[test]
    fn long_unc_paths_get_the_verbatim_unc_prefix() {
        let plain = format!(r"\\server\share\{}", "u".repeat(LEGACY_MAX_PATH));
        let extended = text(&extended_length_path(Path::new(&plain)).unwrap());
        assert_eq!(extended, format!(r"\\?\UNC\{}", &plain[2..]));
    }

    #[test]
    fn verbatim_paths_pass_through_unchanged_whatever_their_length() {
        let long = format!(r"\\?\{}", drive_path(LEGACY_MAX_PATH * 2));
        for verbatim in [r"\\?\C:\a.dds", r"\??\C:\a.dds", long.as_str()] {
            assert_eq!(
                text(&extended_length_path(Path::new(verbatim)).unwrap()),
                verbatim
            );
        }
    }

    /// A relative path is resolved now, as Win32 would have resolved it, and
    /// keeps its plain form while the result stays short.
    #[test]
    fn relative_paths_resolve_against_the_current_directory() {
        let resolved = extended_length_path(Path::new("name.dds")).unwrap();
        assert_eq!(resolved, std::env::current_dir().unwrap().join("name.dds"));
    }

    #[test]
    fn nul_is_rejected() {
        let error = extended_length_path(Path::new("C:\\a\0b")).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn plain_units_undo_the_drive_and_unc_prefixes_only() {
        let plain = |text: &str| -> String {
            String::from_utf16(&plain_units(text.encode_utf16().collect())).unwrap()
        };
        assert_eq!(plain(r"\\?\C:\"), r"C:\");
        assert_eq!(plain(r"\\?\C:\deep\mount\"), r"C:\deep\mount\");
        assert_eq!(plain(r"\\?\UNC\server\share\"), r"\\server\share\");
        assert_eq!(plain(r"\\?\Volume{1}\"), r"\\?\Volume{1}\");
        assert_eq!(plain(r"C:\"), r"C:\");
    }
}
