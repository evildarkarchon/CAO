//! Renames, deletes and moves bound to verified file objects (#463).
//!
//! std has no handle-based rename or delete: `std::fs::rename` and
//! `remove_file` act on pathnames, which another process could replace after
//! CAO checked them, and `rename` drops `MOVEFILE_WRITE_THROUGH`.

use std::ffi::OsStr;
use std::fs::File;
use std::io;
use std::mem::offset_of;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::AsRawHandle;
use std::path::Path;

use windows_sys::Win32::Storage::FileSystem::{
    FILE_DISPOSITION_INFO, FILE_RENAME_INFO, FileDispositionInfo, FileRenameInfo,
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW, SetFileInformationByHandle,
};

/// Whether a handle rename may replace an existing destination.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenameMode {
    /// Replace an existing destination file.
    Replace,
    /// Fail when the destination name is occupied, even by a dangling link.
    NoReplace,
}

/// Renames the file object `file` holds open to the absolute `destination`.
///
/// This is `SetFileInformationByHandle(FileRenameInfo)`, not the POSIX
/// `FileRenameInfoEx` form, whose replace semantics would replace a target
/// that has open handles. The rename never copies, so a destination on
/// another volume fails. The handle needs [`crate::Access::DELETE`].
///
/// # Errors
///
/// Returns [`io::ErrorKind::InvalidInput`] for a relative destination (C++
/// found `RootDirectory`-relative names rejected, so names are always
/// absolute) or one too long for the rename buffer, and otherwise the Win32
/// error: [`RenameMode::NoReplace`] onto an occupied name fails with
/// `ERROR_ALREADY_EXISTS`.
pub fn rename_by_handle(file: &File, destination: &Path, mode: RenameMode) -> io::Result<()> {
    if !destination.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a handle rename needs an absolute destination",
        ));
    }
    let name: Vec<u16> = destination.as_os_str().encode_wide().collect();
    let name_bytes = name.len() * size_of::<u16>();
    // The header, the name, and a terminator the API does not need but std's
    // own implementation also leaves room for.
    let size = offset_of!(FILE_RENAME_INFO, FileName) + name_bytes + size_of::<u16>();
    let (Ok(name_length), Ok(buffer_size)) = (u32::try_from(name_bytes), u32::try_from(size))
    else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the rename destination is too long",
        ));
    };
    // u64 elements give the buffer FILE_RENAME_INFO's 8-byte alignment, and
    // zeroing sets RootDirectory to null and terminates the name.
    let mut buffer = vec![0u64; size.div_ceil(size_of::<u64>())];
    let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    // SAFETY: `buffer` is aligned for FILE_RENAME_INFO and holds `size` bytes:
    // the header up to `FileName` plus `name.len()` units and a terminator. The
    // handle stays open for the borrow of `file`.
    let renamed = unsafe {
        (*info).Anonymous.ReplaceIfExists = mode == RenameMode::Replace;
        (*info).FileNameLength = name_length;
        std::ptr::copy_nonoverlapping(
            name.as_ptr(),
            (&raw mut (*info).FileName).cast::<u16>(),
            name.len(),
        );
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileRenameInfo,
            info.cast(),
            buffer_size,
        )
    };
    if renamed == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Deletes the file object `file` holds open, not whatever its path names now.
///
/// This sets Win32 (not POSIX) delete disposition: the name goes away when the
/// last handle to the object closes. The handle needs [`crate::Access::DELETE`].
///
/// # Errors
///
/// Returns the `SetFileInformationByHandle(FileDispositionInfo)` error, for
/// example for a nonempty directory or a read-only file.
pub fn delete_by_handle(file: &File) -> io::Result<()> {
    let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
    // SAFETY: the handle stays open for the borrow of `file`, and
    // `disposition` is a FILE_DISPOSITION_INFO of the size passed.
    let deleted = unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileDispositionInfo,
            (&raw const disposition).cast(),
            size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    };
    if deleted == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Moves `from` to `to`, replacing `to`, with
/// `MoveFileExW(MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH)`.
///
/// Staging replaces `ownership.manifest` this way. `WRITE_THROUGH` mainly
/// matters for copy-and-delete moves across volumes; it is kept for parity
/// with C++. Paths go to Win32 as they are, so long ones rely on the
/// `longPathAware` manifest.
///
/// # Errors
///
/// Returns [`io::ErrorKind::InvalidInput`] for a path with an interior NUL,
/// and otherwise the `MoveFileExW` error.
pub fn move_file_write_through(from: &Path, to: &Path) -> io::Result<()> {
    let from = terminated(from.as_os_str())?;
    let to = terminated(to.as_os_str())?;
    // SAFETY: both buffers are NUL-terminated UTF-16 and outlive the call.
    let moved = unsafe {
        MoveFileExW(
            from.as_ptr(),
            to.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Encodes `text` as NUL-terminated UTF-16 for a Win32 path parameter.
pub(crate) fn terminated(text: &OsStr) -> io::Result<Vec<u16>> {
    let mut units: Vec<u16> = text.encode_wide().collect();
    if units.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a path passed to Win32 cannot contain NUL",
        ));
    }
    units.push(0);
    Ok(units)
}
