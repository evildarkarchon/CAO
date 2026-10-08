//! MSVC-compatible `canonical` and `weakly_canonical` (#463).
//!
//! A C++-written `CAO-STAGING` manifest stores the Mod Root as the generic UTF-8
//! text of MSVC `std::filesystem::canonical`, and recovery compares it byte for
//! byte. Neither `std::fs::canonicalize` (which keeps `\\?\` and fails on
//! volumes with no DOS name) nor `dunce::canonicalize` (which keeps `\\?\` past
//! 260 characters and never rewrites UNC) produces that text, so this module
//! ports MSVC STL 14.51's `_Canonical`, `weakly_canonical`, `lexically_normal`
//! and root-name parser line for line. `tests/msvc_canonical.rs` checks the
//! port against output recorded from the STL itself.
//!
//! Paths are handled as UTF-16 units, as the STL does, so text that is not
//! valid Unicode passes through unchanged.

use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::{
    ERROR_BAD_NETPATH, ERROR_BAD_PATHNAME, ERROR_DIRECTORY, ERROR_FILE_NOT_FOUND,
    ERROR_INVALID_NAME, ERROR_NETNAME_DELETED, ERROR_PATH_NOT_FOUND, MAX_PATH,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_BACKUP_SEMANTICS, FILE_NAME_NORMALIZED, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, GETFINALPATHNAMEBYHANDLE_FLAGS, GetFinalPathNameByHandleW,
    VOLUME_NAME_DOS, VOLUME_NAME_NT,
};

const SEPARATOR: u16 = b'\\' as u16;
const ALT_SEPARATOR: u16 = b'/' as u16;
const DOT: &[u16] = &[b'.' as u16];
const DOT_DOT: &[u16] = &[b'.' as u16, b'.' as u16];

/// Resolves `path` to the exact text MSVC `std::filesystem::canonical` returns.
///
/// Opens the path (following links) and asks Windows for its final normalized
/// name. A drive-letter result loses its `\\?\` prefix whatever its length,
/// `\\?\UNC\server\share` becomes `\\server\share`, and a volume with no DOS
/// name comes back as `\\?\GLOBALROOT\Device\...`. An empty path returns an
/// empty path without touching the filesystem.
///
/// # Errors
///
/// Returns the open or `GetFinalPathNameByHandleW` error, with its Win32 code
/// as the raw OS error, when the path does not exist or cannot be opened.
pub fn msvc_canonical(path: &Path) -> io::Result<PathBuf> {
    canonical_units(&units(path)).map(path_from_units)
}

/// Resolves `path` to the exact text MSVC `std::filesystem::weakly_canonical`
/// returns.
///
/// When the whole path exists this is [`msvc_canonical`]. Otherwise the path is
/// lexically normalized, the longest existing prefix is canonicalized, and the
/// rest is appended as normalized text, keeping a trailing separator.
///
/// # Errors
///
/// Returns the error of any canonicalization step that fails for a reason
/// other than a missing path (MSVC's `__std_is_file_not_found` set), such as
/// access denied.
pub fn msvc_weakly_canonical(path: &Path) -> io::Result<PathBuf> {
    let text = units(path);
    match canonical_units(&text) {
        Ok(result) => return Ok(path_from_units(result)),
        Err(error) if !is_file_not_found(&error) => return Err(error),
        // Part of the path is missing: fall through to the lexical walk below.
        Err(_) => {}
    }

    let normalized = lexically_normal(&text);
    let relative = find_relative_path(&normalized);
    let mut result = normalized[..relative].to_vec();
    let mut call_canonical = true;
    // MSVC iterates `relative_path()` as a path of its own, so after a `\\?\`
    // root the `C:` and `\` of `\\?\C:\x` come back as a root name and a root
    // directory, and `/=` replaces the result with them.
    for element in path_elements(&normalized[relative..]) {
        append_path(&mut result, element);
        if call_canonical {
            match canonical_units(&result) {
                Ok(canonical) => result = canonical,
                Err(error) if is_file_not_found(&error) => call_canonical = false,
                Err(error) => return Err(error),
            }
        }
    }
    Ok(path_from_units(result))
}

/// Returns MSVC's `generic_u8string()` of `path`: every `\` becomes `/`.
///
/// This is the form C++ CAO writes into `CAO-STAGING` manifests.
///
/// # Errors
///
/// Returns [`io::ErrorKind::InvalidData`] when the path holds an unpaired
/// surrogate, which MSVC's UTF-8 conversion rejects too.
pub fn generic_utf8(path: &Path) -> io::Result<String> {
    let text = path.to_str().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "the path is not valid Unicode, so it has no UTF-8 form",
        )
    })?;
    Ok(text.replace('\\', "/"))
}

fn units(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().collect()
}

fn path_from_units(units: Vec<u16>) -> PathBuf {
    PathBuf::from(OsString::from_wide(&units))
}

fn is_slash(unit: u16) -> bool {
    unit == SEPARATOR || unit == ALT_SEPARATOR
}

/// MSVC's `__std_is_file_not_found`, which decides whether `weakly_canonical`
/// keeps walking. Windows 11 24H2 reports `ERROR_DIRECTORY` for a file used as
/// a directory, which the STL added to the set.
fn is_file_not_found(error: &io::Error) -> bool {
    error.raw_os_error().is_some_and(|code| {
        [
            ERROR_FILE_NOT_FOUND,
            ERROR_PATH_NOT_FOUND,
            ERROR_BAD_NETPATH,
            ERROR_INVALID_NAME,
            ERROR_BAD_PATHNAME,
            ERROR_DIRECTORY,
            ERROR_NETNAME_DELETED,
        ]
        .contains(&(code as u32))
    })
}

/// MSVC's `_Canonical` over UTF-16 units.
fn canonical_units(text: &[u16]) -> io::Result<Vec<u16>> {
    if text.is_empty() {
        return Ok(Vec::new());
    }
    // MSVC's `_Fs_file`: attribute access, every share mode, links followed.
    // std passes a short path through unchanged and gives a long one a `\\?\`
    // prefix after `GetFullPathNameW`, which resolves the same names.
    let file = OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(OsString::from_wide(text))?;
    let (name, volume_name) = final_path_name(&file)?;
    Ok(rewrite_final_path(name, volume_name))
}

/// Calls `GetFinalPathNameByHandleW` with a growing buffer, retrying with the
/// NT name when the volume has no DOS name, exactly as `_Canonical` does.
fn final_path_name(file: &File) -> io::Result<(Vec<u16>, GETFINALPATHNAMEBYHANDLE_FLAGS)> {
    let mut volume_name = VOLUME_NAME_DOS;
    let mut buffer = vec![0u16; MAX_PATH as usize];
    loop {
        let requested = u32::try_from(buffer.len()).expect("final path buffer fits in u32");
        // SAFETY: the handle stays open for the borrow of `file`, and `buffer`
        // holds `requested` writable UTF-16 units.
        let size = unsafe {
            GetFinalPathNameByHandleW(
                file.as_raw_handle(),
                buffer.as_mut_ptr(),
                requested,
                FILE_NAME_NORMALIZED | volume_name,
            )
        };
        if size == 0 {
            let error = io::Error::last_os_error();
            if volume_name == VOLUME_NAME_DOS
                && error.raw_os_error() == Some(ERROR_PATH_NOT_FOUND as i32)
            {
                // Maybe there is no DOS name for this volume; retry with the NT path.
                volume_name = VOLUME_NAME_NT;
                continue;
            }
            return Err(error);
        }
        // On success `size` excludes the terminator; when the buffer is too
        // small it is the required size including it, so the loop retries.
        buffer.resize(size as usize, 0);
        if size < requested {
            return Ok((buffer, volume_name));
        }
    }
}

/// `_Canonical`'s prefix rewrites of a final path name.
fn rewrite_final_path(mut name: Vec<u16>, volume_name: GETFINALPATHNAMEBYHANDLE_FLAGS) -> Vec<u16> {
    const VERBATIM: &[u16] = &[0x5C, 0x5C, 0x3F, 0x5C]; // \\?\
    const VERBATIM_UNC: &[u16] = &[0x5C, 0x5C, 0x3F, 0x5C, 0x55, 0x4E, 0x43, 0x5C]; // \\?\UNC\
    const GLOBALROOT: &str = r"\\?\GLOBALROOT";
    if volume_name == VOLUME_NAME_DOS {
        if name.len() >= 6 && name.starts_with(VERBATIM) && is_drive_prefix(&name[4..]) {
            // A drive letter: strip the \\?\ prefix, whatever the length.
            name.drain(..4);
        } else if name.starts_with(VERBATIM_UNC) {
            // Chop out ?\UNC\, leaving two separators.
            name.drain(2..8);
        }
    } else {
        // The result is in the NT namespace; reach it through GLOBALROOT.
        name.splice(0..0, GLOBALROOT.encode_utf16());
    }
    name
}

/// MSVC's `_Is_drive_prefix`: an ASCII letter followed by `:`.
fn is_drive_prefix(text: &[u16]) -> bool {
    text.len() >= 2 && text[1] == u16::from(b':') && (text[0] & !0x20).wrapping_sub(0x41) < 26
}

/// MSVC's `_Find_root_name_end`: `X:`, `\\?\`, `\\.\` and `\??\` (three
/// units), or `\\server`.
fn find_root_name_end(text: &[u16]) -> usize {
    if text.len() < 2 {
        return 0;
    }
    if is_drive_prefix(text) {
        return 2;
    }
    if !is_slash(text[0]) {
        return 0;
    }
    let question = u16::from(b'?');
    let dot = u16::from(b'.');
    if text.len() >= 4
        && is_slash(text[3])
        && (text.len() == 4 || !is_slash(text[4]))
        && ((is_slash(text[1]) && (text[2] == question || text[2] == dot))
            || (text[1] == question && text[2] == question))
    {
        return 3;
    }
    if text.len() >= 3 && is_slash(text[1]) && !is_slash(text[2]) {
        return text[3..]
            .iter()
            .position(|&unit| is_slash(unit))
            .map_or(text.len(), |offset| 3 + offset);
    }
    0
}

/// MSVC's `_Find_relative_path`: the first non-slash after the root name.
fn find_relative_path(text: &[u16]) -> usize {
    let root_name_end = find_root_name_end(text);
    text[root_name_end..]
        .iter()
        .position(|&unit| !is_slash(unit))
        .map_or(text.len(), |offset| root_name_end + offset)
}

/// MSVC's `path::is_absolute`: `X:\`, or any non-drive root name (`\\?\`,
/// `\\server`, ...).
fn is_absolute(text: &[u16]) -> bool {
    if is_drive_prefix(text) {
        return text.len() >= 3 && is_slash(text[2]);
    }
    find_root_name_end(text) != 0
}

/// The elements MSVC's path iterator yields: the root name, the root
/// directory, each filename, then one empty element for trailing separators.
fn path_elements(text: &[u16]) -> Vec<&[u16]> {
    let root_name_end = find_root_name_end(text);
    let relative = find_relative_path(text);
    let mut elements = Vec::new();
    if root_name_end != 0 {
        elements.push(&text[..root_name_end]);
    }
    if relative != root_name_end {
        elements.push(&text[root_name_end..relative]);
    }
    let mut at = relative;
    while at < text.len() {
        let end = text[at..]
            .iter()
            .position(|&unit| is_slash(unit))
            .map_or(text.len(), |offset| at + offset);
        elements.push(&text[at..end]);
        at = end;
        while at < text.len() && is_slash(text[at]) {
            at += 1;
        }
        if at == text.len() && end != text.len() {
            elements.push(&[]);
        }
    }
    elements
}

/// MSVC's `path::operator/=`.
fn append_path(result: &mut Vec<u16>, other: &[u16]) {
    if is_absolute(other) {
        *result = other.to_vec();
        return;
    }
    let my_root_name_end = find_root_name_end(result);
    let other_root_name_end = find_root_name_end(other);
    if other_root_name_end != 0 && result[..my_root_name_end] != other[..other_root_name_end] {
        // A different root name replaces the whole path.
        *result = other.to_vec();
        return;
    }
    if other_root_name_end != other.len() && is_slash(other[other_root_name_end]) {
        // A root directory replaces everything after the root name.
        result.truncate(my_root_name_end);
    } else if my_root_name_end == result.len() {
        // No root directory and no filename: only `\\server` is absolute here.
        if my_root_name_end >= 3 {
            result.push(SEPARATOR);
        }
    } else if !result.last().copied().is_some_and(is_slash) {
        result.push(SEPARATOR);
    }
    result.extend_from_slice(&other[other_root_name_end..]);
}

/// MSVC's `path::lexically_normal` (N4950 [fs.path.generic]/6).
fn lexically_normal(text: &[u16]) -> Vec<u16> {
    if text.is_empty() {
        return Vec::new();
    }

    // 2. Replace each slash in the root name with a preferred separator.
    let root_name_end = find_root_name_end(text);
    let mut normalized: Vec<u16> = text[..root_name_end]
        .iter()
        .map(|&unit| {
            if unit == ALT_SEPARATOR {
                SEPARATOR
            } else {
                unit
            }
        })
        .collect();

    // 3. Collapse each directory separator. An empty element is a separator,
    // and the vector alternates filename, separator, filename, ...
    let mut parts: Vec<&[u16]> = Vec::new();
    let mut has_root_directory = false;
    let mut at = root_name_end;
    if at < text.len() && is_slash(text[at]) {
        has_root_directory = true;
        normalized.push(SEPARATOR);
        while at < text.len() && is_slash(text[at]) {
            at += 1;
        }
    }
    while at < text.len() {
        if is_slash(text[at]) {
            if parts.last().is_none_or(|part| !part.is_empty()) {
                parts.push(&[]);
            }
            at += 1;
        } else {
            let end = text[at + 1..]
                .iter()
                .position(|&unit| is_slash(unit))
                .map_or(text.len(), |offset| at + 1 + offset);
            parts.push(&text[at..end]);
            at = end;
        }
    }

    // 4-6. Drop dot filenames, fold `name\..`, and drop `..` directly under a
    // root directory. As in MSVC, `new_end` is always even when a filename is
    // written, so the odd slots it skips over are still separators.
    let mut new_end = 0;
    let mut position = 0;
    while position < parts.len() {
        let part = parts[position];
        position += 1;
        if part == DOT {
            if position == parts.len() {
                break;
            }
        } else if part != DOT_DOT {
            parts[new_end] = part;
            new_end += 1;
            if position == parts.len() {
                break;
            }
            new_end += 1;
        } else if new_end != 0 && parts[new_end - 2] != DOT_DOT {
            new_end -= 2;
            if position == parts.len() {
                break;
            }
        } else if !has_root_directory {
            parts[new_end] = DOT_DOT;
            new_end += 1;
            if position == parts.len() {
                break;
            }
            new_end += 1;
        } else if position == parts.len() {
            break;
        }
        position += 1;
    }
    parts.truncate(new_end);

    // 7. A trailing `..` loses its trailing separator.
    if parts.len() >= 2 && parts[parts.len() - 1].is_empty() && parts[parts.len() - 2] == DOT_DOT {
        parts.pop();
    }

    for part in parts {
        if part.is_empty() {
            normalized.push(SEPARATOR);
        } else {
            normalized.extend_from_slice(part);
        }
    }

    // 8. An empty result becomes a dot.
    if normalized.is_empty() {
        normalized.extend_from_slice(DOT);
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    fn normal(text: &str) -> String {
        String::from_utf16(&lexically_normal(&wide(text))).unwrap()
    }

    fn rewritten(text: &str, volume_name: GETFINALPATHNAMEBYHANDLE_FLAGS) -> String {
        String::from_utf16(&rewrite_final_path(wide(text), volume_name)).unwrap()
    }

    /// The live fixture only reaches drive-letter results; UNC and NT results
    /// need a share or a volume with no DOS name, so their rewrites are pinned
    /// here from `_Canonical`'s source.
    #[test]
    fn final_path_rewrites_follow_msvc() {
        assert_eq!(rewritten(r"\\?\C:\Windows", VOLUME_NAME_DOS), r"C:\Windows");
        assert_eq!(rewritten(r"\\?\z:", VOLUME_NAME_DOS), r"z:");
        assert_eq!(
            rewritten(r"\\?\UNC\server\share\dir", VOLUME_NAME_DOS),
            r"\\server\share\dir"
        );
        // Not a drive letter and not UNC: left as Windows returned it.
        assert_eq!(
            rewritten(r"\\?\Volume{1234}\dir", VOLUME_NAME_DOS),
            r"\\?\Volume{1234}\dir"
        );
        assert_eq!(
            rewritten(r"\Device\HarddiskVolume4\", VOLUME_NAME_NT),
            r"\\?\GLOBALROOT\Device\HarddiskVolume4\"
        );
    }

    /// Cases from N4950 [fs.path.generic] and MSVC's own root-name rules; the
    /// live fixture covers the absolute drive-letter forms.
    #[test]
    fn lexically_normal_follows_msvc() {
        assert_eq!(normal(""), "");
        assert_eq!(normal("foo/./bar/.."), r"foo\");
        assert_eq!(normal("foo/.///"), r"foo\");
        assert_eq!(normal("a/.."), ".");
        assert_eq!(normal("../a/../.."), r"..\..");
        assert_eq!(normal("../../"), r"..\..");
        assert_eq!(normal(r"C:\..\x"), r"C:\x");
        assert_eq!(normal("C:a/../../b"), r"C:..\b");
        assert_eq!(normal("//server/share/../x/"), r"\\server\x\");
        assert_eq!(normal(r"\\?\C:\a\.\b"), r"\\?\C:\a\b");
        assert_eq!(normal("/"), r"\");
    }

    #[test]
    fn root_names_follow_msvc() {
        let end = |text: &str| find_root_name_end(&wide(text));
        assert_eq!(end("C:"), 2);
        assert_eq!(end(r"c:\x"), 2);
        assert_eq!(end(r"1:\x"), 0);
        assert_eq!(end(r"\\?\C:\x"), 3);
        assert_eq!(end(r"\\.\pipe"), 3);
        assert_eq!(end(r"\??\C:"), 3);
        assert_eq!(end(r"\\server\share"), 8);
        assert_eq!(end(r"\\server"), 8);
        assert_eq!(end(r"\x"), 0);
        assert_eq!(end(r"\\\x"), 0);
    }

    /// Includes the examples in the comment on MSVC's `operator/=`.
    #[test]
    fn append_path_follows_operator_slash() {
        let appended = |base: &str, other: &str| {
            let mut result = wide(base);
            append_path(&mut result, &wide(other));
            String::from_utf16(&result).unwrap()
        };
        assert_eq!(appended("cat", "c:/dog"), "c:/dog");
        assert_eq!(appended("cat", "c:"), "c:");
        assert_eq!(appended("c:", ""), "c:");
        assert_eq!(appended("c:cat", "/dog"), "c:/dog");
        assert_eq!(appended("c:cat", "c:dog"), r"c:cat\dog");
        assert_eq!(appended("c:cat", "d:dog"), "d:dog");
        assert_eq!(appended("", "a"), "a");
        assert_eq!(appended(r"C:\", "a"), r"C:\a");
        assert_eq!(appended(r"C:\a", ""), r"C:\a\");
        assert_eq!(appended(r"C:\a\", ""), r"C:\a\");
        assert_eq!(appended(r"\\server", "share"), r"\\server\share");
        assert_eq!(appended(r"\\?\", "C:"), "C:");
        assert_eq!(appended(r"C:\Users", r"\"), r"C:\");
    }

    #[test]
    fn path_elements_follow_msvc_iterator() {
        let elements = |text: &str| -> Vec<String> {
            let text = wide(text);
            path_elements(&text)
                .into_iter()
                .map(|element| String::from_utf16(element).unwrap())
                .collect()
        };
        assert_eq!(elements(""), Vec::<String>::new());
        assert_eq!(elements(r"C:\Windows\x"), ["C:", r"\", "Windows", "x"]);
        assert_eq!(elements(r"a\b\\"), ["a", "b", ""]);
        assert_eq!(elements(r"\x"), [r"\", "x"]);
        assert_eq!(elements("C:x"), ["C:", "x"]);
        assert_eq!(elements(r"\\server\share"), [r"\\server", r"\", "share"]);
    }

    #[test]
    fn generic_utf8_uses_forward_slashes() {
        assert_eq!(
            generic_utf8(Path::new(r"C:\Mods\Straße\a")).unwrap(),
            "C:/Mods/Straße/a"
        );
        assert_eq!(
            generic_utf8(Path::new(r"\\?\GLOBALROOT\Device\X\")).unwrap(),
            "//?/GLOBALROOT/Device/X/"
        );
        let unpaired = PathBuf::from(OsString::from_wide(&[0x43, 0x3A, 0x5C, 0xD800]));
        assert_eq!(
            generic_utf8(&unpaired).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
}
