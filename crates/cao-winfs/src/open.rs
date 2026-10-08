//! Opening pins and other file-safety handles through `std` (#463).
//!
//! Every `CreateFileW` call in the C++ staging and Archive code maps onto
//! `std::fs::OpenOptions` plus `OpenOptionsExt`, which also adds `\\?\` to long
//! paths and owns the handle. [`Open`] fixes the two traps found in std's
//! source: the share mode must always be explicit, because std's default
//! includes `FILE_SHARE_DELETE` and so silently allows renames, and
//! `create_new` needs `write(true)` even when an access mode is given.

use std::fs::{File, OpenOptions};
use std::io;
use std::ops::BitOr;
use std::os::windows::fs::OpenOptionsExt;
use std::path::Path;

use windows_sys::Win32::Foundation::{
    ERROR_LOCK_VIOLATION, ERROR_SHARING_VIOLATION, GENERIC_READ, GENERIC_WRITE,
};
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_FLAG_WRITE_THROUGH,
    FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_READ_DATA, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE,
};

/// Access rights a handle requests (`dwDesiredAccess`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Access(u32);

impl Access {
    /// `FILE_READ_ATTRIBUTES`. On its own it does not take part in sharing
    /// checks, so a pin also needs read access to deny renames.
    pub const READ_ATTRIBUTES: Self = Self(FILE_READ_ATTRIBUTES);
    /// `FILE_LIST_DIRECTORY`: the read access of a directory pin.
    pub const LIST_DIRECTORY: Self = Self(FILE_LIST_DIRECTORY);
    /// `FILE_READ_DATA`.
    pub const READ_DATA: Self = Self(FILE_READ_DATA);
    /// `GENERIC_READ`.
    pub const READ: Self = Self(GENERIC_READ);
    /// `GENERIC_WRITE`, which `File::sync_all` (`FlushFileBuffers`) needs.
    pub const WRITE: Self = Self(GENERIC_WRITE);
    /// `DELETE`, which handle renames and identity-bound deletes need.
    pub const DELETE: Self = Self(DELETE);
}

impl BitOr for Access {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// Sharing granted to other opens of the same file object (`dwShareMode`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Share(u32);

impl Share {
    /// Exclusive: every other open fails with a sharing violation.
    pub const NONE: Self = Self(0);
    /// `FILE_SHARE_READ`.
    pub const READ: Self = Self(FILE_SHARE_READ);
    /// `FILE_SHARE_WRITE`.
    pub const WRITE: Self = Self(FILE_SHARE_WRITE);
    /// `FILE_SHARE_DELETE`. Without it, no other handle may rename or delete
    /// the entry while this one is open.
    pub const DELETE: Self = Self(FILE_SHARE_DELETE);
}

impl BitOr for Share {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// One file-safety `CreateFileW` request.
///
/// It never follows links: `FILE_FLAG_OPEN_REPARSE_POINT` is always set, so a
/// symlink or junction opens as itself and [`crate::is_reparse_point`] can
/// reject it. An existing entry is never truncated.
#[derive(Clone, Copy, Debug)]
#[must_use]
pub struct Open {
    access: Access,
    share: Share,
    directory: bool,
    write_through: bool,
    create_new: bool,
}

impl Open {
    /// An `OPEN_EXISTING` request with exactly this access and sharing.
    pub const fn new(access: Access, share: Share) -> Self {
        Self {
            access,
            share,
            directory: false,
            write_through: false,
            create_new: false,
        }
    }

    /// Adds `FILE_FLAG_BACKUP_SEMANTICS`, which opening a directory needs.
    pub const fn directory(self) -> Self {
        Self {
            directory: true,
            ..self
        }
    }

    /// Adds `FILE_FLAG_WRITE_THROUGH`.
    pub const fn write_through(self) -> Self {
        Self {
            write_through: true,
            ..self
        }
    }

    /// Creates the file with `CREATE_NEW`: an existing entry, even a dangling
    /// link, fails the open with `ERROR_FILE_EXISTS`.
    pub const fn create_new(self) -> Self {
        Self {
            create_new: true,
            ..self
        }
    }

    /// Opens `path`.
    ///
    /// # Errors
    ///
    /// Returns the `CreateFileW` error. A conflicting open shows as raw OS
    /// error 32 or 33; see [`crate::is_sharing_violation`].
    pub fn open(&self, path: &Path) -> io::Result<File> {
        let mut flags = FILE_FLAG_OPEN_REPARSE_POINT;
        if self.directory {
            flags |= FILE_FLAG_BACKUP_SEMANTICS;
        }
        if self.write_through {
            flags |= FILE_FLAG_WRITE_THROUGH;
        }
        let mut options = OpenOptions::new();
        options
            .access_mode(self.access.0)
            .share_mode(self.share.0)
            .custom_flags(flags);
        if self.create_new {
            // std rejects `create_new` unless `write` or `append` is set, even
            // with an access mode; `access_mode` still decides the rights.
            options.write(true).create_new(true);
        }
        options.open(path)
    }
}

/// Whether an open failed because another handle's share mode or byte-range
/// lock excludes it (`ERROR_SHARING_VIOLATION` or `ERROR_LOCK_VIOLATION`).
pub fn is_sharing_violation(error: &io::Error) -> bool {
    error.raw_os_error().is_some_and(|code| {
        code == ERROR_SHARING_VIOLATION as i32 || code == ERROR_LOCK_VIOLATION as i32
    })
}
