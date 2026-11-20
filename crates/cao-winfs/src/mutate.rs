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

use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_SHARING_VIOLATION};

use crate::long_path::{extended_length_path, win32_path};
use crate::retry::retry_transient;
use crate::unsupported::is_unsupported;
use windows_sys::Win32::Storage::FileSystem::{
    FILE_DISPOSITION_FLAG_DELETE, FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
    FILE_DISPOSITION_INFO, FILE_DISPOSITION_INFO_EX, FILE_INFO_BY_HANDLE_CLASS, FILE_RENAME_INFO,
    FileDispositionInfo, FileDispositionInfoEx, FileRenameInfo, FileRenameInfoEx,
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW, SetFileInformationByHandle,
};

/// `FILE_RENAME_FLAG_REPLACE_IF_EXISTS` of `FileRenameInfoEx`, which
/// windows-sys does not define (`FILE_RENAME_REPLACE_IF_EXISTS` in ntifs.h).
const FILE_RENAME_FLAG_REPLACE_IF_EXISTS: u32 = 0x1;
/// `FILE_RENAME_IGNORE_READONLY_ATTRIBUTE` of ntifs.h: with
/// [`FILE_RENAME_FLAG_REPLACE_IF_EXISTS`], replace a read-only destination.
const FILE_RENAME_FLAG_IGNORE_READONLY_ATTRIBUTE: u32 = 0x40;

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
/// This is `SetFileInformationByHandle(FileRenameInfoEx)` without the POSIX
/// flag, whose replace semantics would replace a target that has open
/// handles: a destination someone holds open still refuses the rename.
/// [`RenameMode::Replace`] also passes the ignore-read-only flag (FFI-4), so a
/// read-only Asset is replaced like any other; the result carries the
/// source's attributes. A file system without `FileRenameInfoEx` gets the
/// plain `FileRenameInfo` instead, which refuses a read-only destination.
/// The rename never copies, so a destination on another volume fails. The
/// handle needs [`crate::Access::DELETE`].
///
/// A destination longer than `MAX_PATH` is given in its
/// [`extended_length_path`] form, so the rename reaches it whatever the
/// host's `LongPathsEnabled` setting.
///
/// # Errors
///
/// Returns [`io::ErrorKind::InvalidInput`] for a relative destination (the
/// rename passes no root directory, so Windows would read a relative name
/// against the source's directory, and CAO always means a full path), one
/// with an interior NUL, or one too long for the rename buffer, and otherwise
/// the Win32 error: [`RenameMode::NoReplace`] onto an occupied name fails
/// with `ERROR_ALREADY_EXISTS`.
pub fn rename_by_handle(file: &File, destination: &Path, mode: RenameMode) -> io::Result<()> {
    if !destination.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a handle rename needs an absolute destination",
        ));
    }
    let name: Vec<u16> = extended_length_path(destination)?
        .as_os_str()
        .encode_wide()
        .collect();
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
    // the header up to `FileName` plus `name.len()` units and a terminator.
    unsafe {
        (*info).FileNameLength = name_length;
        std::ptr::copy_nonoverlapping(
            name.as_ptr(),
            (&raw mut (*info).FileName).cast::<u16>(),
            name.len(),
        );
    }
    let flags = match mode {
        RenameMode::Replace => {
            FILE_RENAME_FLAG_REPLACE_IF_EXISTS | FILE_RENAME_FLAG_IGNORE_READONLY_ATTRIBUTE
        }
        RenameMode::NoReplace => 0,
    };
    // SAFETY: `info` points into `buffer`, which is live and aligned; `Flags`
    // is the union member FileRenameInfoEx reads.
    unsafe { (*info).Anonymous.Flags = flags };
    let renamed = set_information(file, FileRenameInfoEx, info.cast(), buffer_size);
    match renamed {
        Err(error) if is_unsupported(&error) => {
            // SAFETY: as above. The whole union is cleared before the
            // one-byte `ReplaceIfExists` is set, so no flag bit lingers in
            // the bytes FileRenameInfo does not read.
            unsafe {
                (*info).Anonymous.Flags = 0;
                (*info).Anonymous.ReplaceIfExists = mode == RenameMode::Replace;
            }
            set_information(file, FileRenameInfo, info.cast(), buffer_size)
        }
        other => other,
    }
}

/// Deletes the file object `file` holds open, not whatever its path names now.
///
/// This sets Win32 (not POSIX) delete disposition: the name goes away when the
/// last handle to the object closes. It uses `FileDispositionInfoEx` with the
/// ignore-read-only flag (FFI-4), so a read-only converted texture or
/// extracted Archive is removed like any other. A file system without
/// `FileDispositionInfoEx` gets the plain `FileDispositionInfo`, which
/// refuses a read-only file. The handle needs [`crate::Access::DELETE`].
///
/// # Errors
///
/// Returns the `SetFileInformationByHandle` error, for example for a
/// nonempty directory.
pub fn delete_by_handle(file: &File) -> io::Result<()> {
    let extended = FILE_DISPOSITION_INFO_EX {
        Flags: FILE_DISPOSITION_FLAG_DELETE | FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
    };
    let deleted = set_information(
        file,
        FileDispositionInfoEx,
        (&raw const extended).cast(),
        size_of::<FILE_DISPOSITION_INFO_EX>() as u32,
    );
    match deleted {
        Err(error) if is_unsupported(&error) => {
            let plain = FILE_DISPOSITION_INFO { DeleteFile: true };
            set_information(
                file,
                FileDispositionInfo,
                (&raw const plain).cast(),
                size_of::<FILE_DISPOSITION_INFO>() as u32,
            )
        }
        other => other,
    }
}

/// `SetFileInformationByHandle` on the handle `file` holds, with
/// `GetLastError` as the error.
///
/// `info` must point to `size` readable bytes laid out as `class` expects;
/// every caller passes a live local or buffer of exactly that structure.
fn set_information(
    file: &File,
    class: FILE_INFO_BY_HANDLE_CLASS,
    info: *const std::ffi::c_void,
    size: u32,
) -> io::Result<()> {
    // SAFETY: the handle stays open for the borrow of `file`, and the caller
    // passes `size` readable bytes of the structure `class` names.
    if unsafe { SetFileInformationByHandle(file.as_raw_handle(), class, info, size) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Moves `from` to `to` on the same volume, replacing `to`, with
/// `MoveFileExW(MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH)`.
///
/// Staging replaces `ownership.manifest` this way. Without
/// `MOVEFILE_COPY_ALLOWED` the move is a rename and never copies, so a `to`
/// on another volume fails. The bytes' durability is the caller's: staging
/// flushes the scratch file before it moves it. `MOVEFILE_WRITE_THROUGH` is
/// kept as a request that the rename itself be committed before the call
/// returns. Both paths go to Win32 in their [`extended_length_path`] form,
/// so a deep Mod Root works whatever the host's `LongPathsEnabled` setting.
///
/// A move refused with Access denied or a sharing violation is retried with
/// backoff for about a second before that error is returned. A real-time
/// scanner briefly opens a file a run has just written, and replacing a
/// destination someone holds without delete sharing is refused, though the
/// same move a few milliseconds later succeeds; failing at once would fail
/// the run over that scan. The wait blocks the calling thread, and a lasting
/// refusal, such as a read-only manifest, costs it that second before it
/// fails.
///
/// # Errors
///
/// Returns [`io::ErrorKind::InvalidInput`] for a path with an interior NUL,
/// the [`extended_length_path`] error for a path that cannot be resolved,
/// and otherwise the `MoveFileExW` error, of the last attempt when retried.
/// The source is left in place on failure.
pub fn move_file_write_through(from: &Path, to: &Path) -> io::Result<()> {
    let from = win32_path(from)?;
    let to = win32_path(to)?;
    let attempt = || {
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
    };
    // A held destination answers Access denied rather than a sharing
    // violation when the holder denies delete sharing, so both are waited out.
    retry_transient(attempt, |error| {
        matches!(
            error.raw_os_error().map(|code| code as u32),
            Some(ERROR_ACCESS_DENIED | ERROR_SHARING_VIOLATION)
        )
    })
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
