//! Volume identity and free space (#463), ported from C++'s `NativeVolume.h`.
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

use crate::mutate::terminated;

/// `UNICODE_STRING` lengths cap native paths at 32,767 characters plus the
/// terminator.
const NATIVE_PATH_LIMIT: usize = 32_768;

/// Returns the mount point of the volume containing `path`, such as `C:\` or a
/// mounted folder, with its trailing separator.
///
/// # Errors
///
/// Returns the `GetVolumePathNameW` error, or [`io::ErrorKind::InvalidInput`]
/// for a path with an interior NUL.
pub fn volume_mount_point(path: &Path) -> io::Result<PathBuf> {
    let path = terminated(path.as_os_str())?;
    let mount = mount_point_with(&path, |buffer| {
        let size =
            u32::try_from(buffer.len()).expect("mount buffer is capped at NATIVE_PATH_LIMIT");
        // SAFETY: `path` is NUL-terminated and `buffer` holds `size` units.
        if unsafe { GetVolumePathNameW(path.as_ptr(), buffer.as_mut_ptr(), size) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    })?;
    Ok(PathBuf::from(OsString::from_wide(&mount)))
}

/// Returns the volume GUID path (`\\?\Volume{GUID}\`) of the volume containing
/// the existing `path`, including volumes mounted in folders.
///
/// # Errors
///
/// Returns the [`volume_mount_point`] or `GetVolumeNameForVolumeMountPointW`
/// error.
pub fn volume_guid_path(path: &Path) -> io::Result<PathBuf> {
    let mount = terminated(volume_mount_point(path)?.as_os_str())?;
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
/// directory `path`, honouring quotas: `std::filesystem::space(path).available`.
///
/// # Errors
///
/// Returns the `GetDiskFreeSpaceExW` error, for example when `path` does not
/// name an existing directory.
pub fn available_space(path: &Path) -> io::Result<u64> {
    let path = terminated(path.as_os_str())?;
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
/// mount point fits, as C++'s `volumeMountPoint` does. `path` is
/// NUL-terminated; the result is not.
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
    use windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED;

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
