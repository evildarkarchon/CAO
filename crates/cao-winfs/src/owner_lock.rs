//! The staging `owner.lock` handle (#463).
//!
//! Staging ownership is the share-mode-0 open itself: while one process holds
//! it, every other open of `owner.lock` fails with a sharing violation, and the
//! OS releases it when that process dies. `File::lock` is not a substitute. It
//! is a `LockFileEx` byte-range lock, which neither a C++ run nor this open
//! would see.

use std::fs::File;
use std::io;
use std::path::Path;

use crate::{Access, FileFacts, Open, Share, is_sharing_violation};

/// Why `owner.lock` could not be claimed.
#[derive(Debug, thiserror::Error)]
pub enum OwnerLockError {
    /// Another process holds the lock open: staging is active
    /// (`StagingActive`).
    #[error("another process holds the staging owner lock")]
    Active(#[source] io::Error),
    /// The open failed for another reason, such as a missing file for
    /// recovery or an existing one for bootstrap.
    #[error("the staging owner lock could not be opened")]
    Open(#[source] io::Error),
    /// The entry is a reparse point, a directory, or hard-linked.
    #[error("the staging owner lock is not an ordinary single-link file")]
    NotOrdinary,
}

/// An exclusive handle to a staging `owner.lock`, held until dropped.
#[derive(Debug)]
pub struct OwnerLock {
    _file: File,
}

impl OwnerLock {
    /// Claims a new `owner.lock` at `path` (`CREATE_NEW`), for staging
    /// bootstrap.
    ///
    /// # Errors
    ///
    /// [`OwnerLockError::Open`] with `ERROR_FILE_EXISTS` when the entry
    /// exists, and otherwise as for [`OwnerLock::open_existing`].
    pub fn create(path: &Path) -> Result<Self, OwnerLockError> {
        Self::claim(path, Self::OPEN.create_new())
    }

    /// Claims the existing `owner.lock` at `path` (`OPEN_EXISTING`), for
    /// recovery.
    ///
    /// # Errors
    ///
    /// [`OwnerLockError::Active`] on a sharing or lock violation,
    /// [`OwnerLockError::NotOrdinary`] when the entry is not an ordinary
    /// single-link file, and [`OwnerLockError::Open`] otherwise.
    pub fn open_existing(path: &Path) -> Result<Self, OwnerLockError> {
        Self::claim(path, Self::OPEN)
    }

    /// `GENERIC_READ` with no sharing, never following a link.
    const OPEN: Open = Open::new(Access::READ, Share::NONE);

    fn claim(path: &Path, open: Open) -> Result<Self, OwnerLockError> {
        let file = open.open(path).map_err(|error| {
            if is_sharing_violation(&error) {
                OwnerLockError::Active(error)
            } else {
                OwnerLockError::Open(error)
            }
        })?;
        let facts = FileFacts::of(&file).map_err(OwnerLockError::Open)?;
        if !facts.is_ordinary_file() || facts.link_count() != 1 {
            return Err(OwnerLockError::NotOrdinary);
        }
        Ok(Self { _file: file })
    }
}
