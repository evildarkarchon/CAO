//! Volume identity and free space (#463).
//!
//! Archive capacity grouping and the same-volume staging check both need the
//! volume that contains a path. CAO is `longPathAware`, so a mounted folder can
//! sit deeper than `MAX_PATH`, which is why the mount-point buffer grows.

use std::ffi::OsString;
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::{ERROR_FILENAME_EXCED_RANGE, MAX_PATH};
use windows_sys::Win32::Storage::FileSystem::{
    GetDiskFreeSpaceExW, GetVolumeNameForVolumeMountPointW, GetVolumePathNameW,
};

use crate::long_path::{plain_units, win32_path};

/// `UNICODE_STRING` lengths cap native paths at 32,767 characters plus the
/// terminator.
const NATIVE_PATH_LIMIT: usize = 32_768;

/// Returns the mount point of the volume containing `path`, such as `C:\` or a
/// mounted folder, with its trailing separator.
///
/// A `path` longer than `MAX_PATH` is queried in its
/// [`crate::extended_length_path`] form, so the query works whatever the
/// host's `LongPathsEnabled` setting. Win32 answers that form with a
/// `\\?\` mount point, which is returned without the prefix the conversion
/// added: the same `C:\` a short path on that volume gives.
///
/// # Errors
///
/// Returns the `GetVolumePathNameW` error, or [`io::ErrorKind::InvalidInput`]
/// for a path with an interior NUL.
pub fn volume_mount_point(path: &Path) -> io::Result<PathBuf> {
    let given_verbatim = is_extended(path);
    let path = win32_path(path)?;
    let converted = !given_verbatim && path.starts_with(&EXTENDED_PREFIX);
    let mount = mount_point_with(&path, |buffer| {
        let size =
            u32::try_from(buffer.len()).expect("mount buffer is capped at NATIVE_PATH_LIMIT");
        // SAFETY: `path` is NUL-terminated and `buffer` holds `size` units.
        if unsafe { GetVolumePathNameW(path.as_ptr(), buffer.as_mut_ptr(), size) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    })?;
    // A caller that passed `\\?\` itself gets Win32's answer as it is.
    let mount = if converted { plain_units(mount) } else { mount };
    Ok(PathBuf::from(OsString::from_wide(&mount)))
}

/// The `\\?\` prefix, as UTF-16.
const EXTENDED_PREFIX: [u16; 4] = [0x5C, 0x5C, 0x3F, 0x5C];

/// Whether `path` already starts with `\\?\`.
fn is_extended(path: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .take(EXTENDED_PREFIX.len())
        .eq(EXTENDED_PREFIX)
}

/// Returns the volume GUID path (`\\?\Volume{GUID}\`) of the volume containing
/// the existing `path`, including volumes mounted in folders.
///
/// # Errors
///
/// Returns the [`volume_mount_point`] or `GetVolumeNameForVolumeMountPointW`
/// error.
pub fn volume_guid_path(path: &Path) -> io::Result<PathBuf> {
    // A mount point deeper than MAX_PATH needs the extended form again.
    let mount = win32_path(&volume_mount_point(path)?)?;
    // Volume GUID paths have a fixed 49-character form however long the
    // mounted folder path is, so MAX_PATH exceeds the documented 50 units.
    let mut volume = [0u16; MAX_PATH as usize];
    // SAFETY: `mount` is NUL-terminated and `volume` holds MAX_PATH units.
    if unsafe { GetVolumeNameForVolumeMountPointW(mount.as_ptr(), volume.as_mut_ptr(), MAX_PATH) }
        == 0
    {
        return Err(io::Error::last_os_error());
    }
    let length = volume
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(volume.len());
    Ok(PathBuf::from(OsString::from_wide(&volume[..length])))
}

/// Returns the bytes available to this process on the volume containing the
/// directory `path`, honouring quotas. A deep `path` is queried in its
/// [`crate::extended_length_path`] form.
///
/// A network Mod Root is supported (FFI-3). `GetDiskFreeSpaceExW` documents
/// that a UNC name must end in a backslash, so when a UNC `path` is refused
/// it is asked again with one, and then at its [`volume_mount_point`]
/// (`\\server\share\`), before the first error is returned. Any other path
/// is asked once, so a missing local directory is still an error.
///
/// # Errors
///
/// Returns the `GetDiskFreeSpaceExW` error for `path` itself, for example
/// when it does not name an existing directory.
pub fn available_space(path: &Path) -> io::Result<u64> {
    available_space_with(path, free_bytes_available, volume_mount_point)
}

/// [`available_space`] over a `GetDiskFreeSpaceExW`-shaped `query` and a
/// `mount_point` lookup, so a test can play a share that answers only some
/// forms of its path.
fn available_space_with(
    path: &Path,
    mut query: impl FnMut(&Path) -> io::Result<u64>,
    mount_point: impl FnOnce(&Path) -> io::Result<PathBuf>,
) -> io::Result<u64> {
    let error = match query(path) {
        Ok(available) => return Ok(available),
        Err(error) => error,
    };
    if !is_unc(path) {
        return Err(error);
    }
    let text = path.as_os_str();
    let separated = text
        .to_str()
        .is_some_and(|text| text.ends_with(['\\', '/']));
    if !separated && !text.is_empty() {
        let mut with_separator = text.to_os_string();
        with_separator.push("\\");
        if let Ok(available) = query(Path::new(&with_separator)) {
            return Ok(available);
        }
    }
    // The fallbacks only widen what is accepted; their own errors would hide
    // why `path` itself was refused, so the first error is the one returned.
    if let Ok(available) = mount_point(path).and_then(|mount| query(&mount)) {
        return Ok(available);
    }
    Err(error)
}

/// Whether `path` names a share: `\\server\share`, in either separator, or
/// `\\?\UNC\server\share`. Device paths (`\\?\`, `\\.\`) are not.
fn is_unc(path: &Path) -> bool {
    // Only the ASCII prefix matters, which a lossy conversion keeps intact.
    let text = path.as_os_str().to_string_lossy();
    if text
        .get(..8)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(r"\\?\UNC\"))
    {
        return true;
    }
    let mut start = text.chars();
    let slash = |character: Option<char>| matches!(character, Some('\\' | '/'));
    slash(start.next())
        && slash(start.next())
        && start
            .next()
            .is_some_and(|third| !matches!(third, '?' | '.' | '\\' | '/'))
}

/// `GetDiskFreeSpaceExW`'s bytes available to the caller for `path`.
fn free_bytes_available(path: &Path) -> io::Result<u64> {
    let path = win32_path(path)?;
    let mut available = 0u64;
    // SAFETY: `path` is NUL-terminated, `available` is writable, and the two
    // optional outputs are null.
    let read = unsafe {
        GetDiskFreeSpaceExW(
            path.as_ptr(),
            &mut available,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if read == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(available)
}

/// Runs a `GetVolumePathNameW`-shaped `query` with buffers that grow until the
/// mount point fits. `path` is NUL-terminated; the result is not.
fn mount_point_with(
    path: &[u16],
    mut query: impl FnMut(&mut [u16]) -> io::Result<()>,
) -> io::Result<Vec<u16>> {
    // The mount point is normally a prefix of the path plus a separator, but a
    // traversed junction can resolve to another volume's mounted folder, so
    // this is only a first guess.
    let length = path.len() - 1;
    let mut size = (length + 2).max(MAX_PATH as usize).min(NATIVE_PATH_LIMIT);
    loop {
        let mut mount = vec![0u16; size];
        if let Err(error) = query(&mut mount) {
            if error.raw_os_error() != Some(ERROR_FILENAME_EXCED_RANGE as i32)
                || size == NATIVE_PATH_LIMIT
            {
                return Err(error);
            }
            size = (size * 2).min(NATIVE_PATH_LIMIT);
            continue;
        }
        let written = mount.iter().position(|&unit| unit == 0).unwrap_or(size);
        mount.truncate(written);
        // One character short, the query succeeds but drops the trailing
        // separator (`C:` for `C:\`), which GetVolumeNameForVolumeMountPointW
        // then rejects.
        if written + 1 < size
            || mount.last() == Some(&u16::from(b'\\'))
            || size == NATIVE_PATH_LIMIT
        {
            return Ok(mount);
        }
        size = (size * 2).min(NATIVE_PATH_LIMIT);
    }
}

#[cfg(test)]
mod tests {
    use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_INVALID_NAME};

    use super::*;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    /// Imitates GetVolumePathNameW for one mount point, including the
    /// documented quirk: with exactly one unit too few it succeeds without the
    /// trailing separator.
    fn fake_query(mount: &str, sizes: &mut Vec<usize>) -> impl FnMut(&mut [u16]) -> io::Result<()> {
        let mount = wide(mount);
        move |buffer: &mut [u16]| {
            sizes.push(buffer.len());
            if buffer.len() > mount.len() {
                buffer[..mount.len()].copy_from_slice(&mount);
                Ok(())
            } else if buffer.len() == mount.len() {
                buffer[..mount.len() - 1].copy_from_slice(&mount[..mount.len() - 1]);
                Ok(())
            } else {
                Err(io::Error::from_raw_os_error(
                    ERROR_FILENAME_EXCED_RANGE as i32,
                ))
            }
        }
    }

    fn terminated_wide(text: &str) -> Vec<u16> {
        let mut units = wide(text);
        units.push(0);
        units
    }

    #[test]
    fn short_mount_points_fit_the_first_buffer() {
        let mut sizes = Vec::new();
        let mount = mount_point_with(
            &terminated_wide(r"C:\Mods\a"),
            fake_query(r"C:\", &mut sizes),
        );
        assert_eq!(mount.unwrap(), wide(r"C:\"));
        assert_eq!(sizes, [MAX_PATH as usize]);
    }

    #[test]
    fn long_mount_points_grow_the_buffer() {
        let deep = format!(r"C:\{}\", "m".repeat(400));
        let path = format!("{deep}file");
        let mut sizes = Vec::new();
        let mount = mount_point_with(&terminated_wide(&path), fake_query(&deep, &mut sizes));
        assert_eq!(mount.unwrap(), wide(&deep));
        // The first guess is the path length plus two, which already fits.
        assert_eq!(sizes, [path.len() + 2]);
    }

    #[test]
    fn a_result_one_unit_short_is_retried_for_its_separator() {
        // A junction resolved to a mounted folder longer than the guess.
        let mount = format!(r"C:\{}\", "m".repeat(MAX_PATH as usize - 4));
        assert_eq!(mount.len(), MAX_PATH as usize);
        let mut sizes = Vec::new();
        let result = mount_point_with(&terminated_wide(r"C:\x"), fake_query(&mount, &mut sizes));
        assert_eq!(result.unwrap(), wide(&mount));
        assert_eq!(sizes, [MAX_PATH as usize, 2 * MAX_PATH as usize]);
    }

    const SHARE_ROOT: &str = r"\\nas\mods\";
    const SHARE_DIRECTORY: &str = r"\\nas\mods\Skyrim";

    /// Plays a share for `available_space_with`: only the forms of a path in
    /// `accepted` answer, and every query is recorded in `asked`.
    fn fake_share<'a>(
        accepted: &'a [&'a str],
        asked: &'a mut Vec<String>,
    ) -> impl FnMut(&Path) -> io::Result<u64> + 'a {
        move |path: &Path| {
            let text = path.to_str().unwrap().to_owned();
            let answer = if accepted.contains(&text.as_str()) {
                Ok(42)
            } else {
                Err(io::Error::from_raw_os_error(ERROR_INVALID_NAME as i32))
            };
            asked.push(text);
            answer
        }
    }

    fn share_mount(_: &Path) -> io::Result<PathBuf> {
        Ok(PathBuf::from(SHARE_ROOT))
    }

    #[test]
    fn a_path_the_query_accepts_is_asked_once() {
        let mut asked = Vec::new();
        let available = available_space_with(
            Path::new(SHARE_DIRECTORY),
            fake_share(&[SHARE_DIRECTORY], &mut asked),
            share_mount,
        );
        assert_eq!(available.unwrap(), 42);
        assert_eq!(asked, [SHARE_DIRECTORY]);
    }

    /// GetDiskFreeSpaceExW documents a trailing backslash for UNC names.
    #[test]
    fn a_unc_directory_is_asked_again_with_a_trailing_separator() {
        let separated = format!("{SHARE_DIRECTORY}\\");
        let mut asked = Vec::new();
        let available = available_space_with(
            Path::new(SHARE_DIRECTORY),
            fake_share(&[separated.as_str()], &mut asked),
            share_mount,
        );
        assert_eq!(available.unwrap(), 42);
        assert_eq!(asked, [SHARE_DIRECTORY, separated.as_str()]);
    }

    #[test]
    fn a_share_that_answers_only_at_its_root_is_asked_there() {
        let mut asked = Vec::new();
        let available = available_space_with(
            Path::new(SHARE_DIRECTORY),
            fake_share(&[SHARE_ROOT], &mut asked),
            share_mount,
        );
        assert_eq!(available.unwrap(), 42);
        assert_eq!(
            asked,
            [SHARE_DIRECTORY, &format!("{SHARE_DIRECTORY}\\"), SHARE_ROOT]
        );
    }

    /// When nothing answers, the error is the one for the path asked about,
    /// and a failed mount-point lookup is not an error of its own.
    #[test]
    fn a_refused_path_returns_its_own_error() {
        let mut asked = Vec::new();
        let error = available_space_with(
            Path::new(SHARE_DIRECTORY),
            fake_share(&[], &mut asked),
            |_| Err(io::Error::from_raw_os_error(ERROR_ACCESS_DENIED as i32)),
        )
        .unwrap_err();
        assert_eq!(error.raw_os_error(), Some(ERROR_INVALID_NAME as i32));
        assert_eq!(asked.len(), 2);
    }

    /// A local path is asked once: a missing directory stays an error rather
    /// than borrowing its volume's answer.
    #[test]
    fn a_refused_local_path_is_asked_once() {
        let mut asked = Vec::new();
        let error = available_space_with(
            Path::new(r"C:\Mods\missing"),
            fake_share(&[r"C:\Mods\missing\", r"C:\"], &mut asked),
            |_| Ok(PathBuf::from(r"C:\")),
        )
        .unwrap_err();
        assert_eq!(error.raw_os_error(), Some(ERROR_INVALID_NAME as i32));
        assert_eq!(asked, [r"C:\Mods\missing"]);
    }

    #[test]
    fn unc_paths_are_recognised_in_every_spelling() {
        for unc in [
            r"\\nas\mods",
            "//nas/mods",
            r"\\?\UNC\nas\mods",
            r"\\?\unc\nas\mods",
        ] {
            assert!(is_unc(Path::new(unc)), "{unc}");
        }
        for other in [
            r"C:\Mods",
            r"\\?\C:\Mods",
            r"\\.\pipe\x",
            r"\Mods",
            r"\\\x",
            "",
        ] {
            assert!(!is_unc(Path::new(other)), "{other}");
        }
    }

    #[test]
    fn other_errors_stop_at_once() {
        let mut calls = 0;
        let result = mount_point_with(&terminated_wide(r"C:\x"), |_| {
            calls += 1;
            Err(io::Error::from_raw_os_error(ERROR_ACCESS_DENIED as i32))
        });
        assert_eq!(
            result.unwrap_err().raw_os_error(),
            Some(ERROR_ACCESS_DENIED as i32)
        );
        assert_eq!(calls, 1);
    }
}
