//! Native pins that keep an Archive source stable while pathname-based
//! Archive I/O runs, then clean up only that file object.
//!
//! Ported from `src/Run/NativeFilePins.cpp`'s `SourceFilePin` and
//! `backupExtractedArchive`. The archive reader opens its Archive by path, so
//! the pin denies writes, renames and deletes of the source and of every
//! ancestor directory for as long as that reading lasts. Cleanup then
//! reopens the path and acts only if it still names the very file object
//! that was pinned, unchanged.

use std::ffi::OsString;
use std::fs::File;
use std::io;
use std::path::{Component, Path, PathBuf};

use cao_winfs::{Access, FileFacts, Open, RenameMode, Share, delete_by_handle, rename_by_handle};

/// Why a source could not be pinned or cleaned up. Nothing has been mutated
/// when one is returned: a cleanup either happens whole or not at all.
#[derive(Debug, thiserror::Error)]
pub enum SourcePinError {
    /// The source is not strictly inside its Mod Root.
    #[error("A source file is outside its Mod Root.")]
    OutsideModRoot,
    /// A native open, query, rename or delete failed.
    #[error("{action} {}: {source}", .path.display())]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// A directory on the source's path is a link or not a directory.
    #[error("A source parent is no longer an ordinary directory: {}", .0.display())]
    ParentNotDirectory(PathBuf),
    /// The source is a link or a directory rather than an ordinary file.
    #[error("A source file is no longer an ordinary file: {}", .0.display())]
    NotOrdinaryFile(PathBuf),
    /// The path names another file object, or the pinned one was changed.
    #[error("A source file changed before cleanup: {}", .0.display())]
    Changed(PathBuf),
}

impl SourcePinError {
    /// Wraps a native error with what was being attempted on `path`.
    fn io<'p>(action: &'static str, path: &'p Path) -> impl FnOnce(io::Error) -> Self + 'p {
        move |source| Self::Io {
            action,
            path: path.to_path_buf(),
            source,
        }
    }
}

/// Holds an Archive source stable while it is read, then cleans only that file.
///
/// The pin is a read handle that shares only reading, so no other handle can
/// write, rename or delete the source, plus a pin on each ancestor directory
/// from the volume root down, so no parent can be swapped for a junction
/// either. Dropping it releases everything.
pub struct SourceFilePin {
    source: PathBuf,
    /// The ancestor pins, from the volume root to the source's parent. They
    /// stay live through cleanup, so a parent cannot be substituted while the
    /// source's read pin gives way to a delete-capable handle.
    _directories: Vec<File>,
    /// The read pin; `None` once released for cleanup.
    pinned: Option<File>,
    /// The pinned file object's identity and change metadata.
    facts: FileFacts,
}

impl SourceFilePin {
    /// Pins the ordinary file `source` inside `mod_root`, and its directory chain.
    ///
    /// Both paths are made absolute and lexically normal first, as C++ did.
    ///
    /// # Errors
    /// [`SourcePinError::OutsideModRoot`] for a source not strictly inside
    /// the root; [`SourcePinError::ParentNotDirectory`] or
    /// [`SourcePinError::NotOrdinaryFile`] when a link stands in the way; and
    /// [`SourcePinError::Io`] when an open or query fails, including a
    /// sharing violation with a handle that already writes the source.
    pub fn new(source: &Path, mod_root: &Path) -> Result<Self, SourcePinError> {
        let absolute = |path: &Path| {
            std::path::absolute(path).map_err(SourcePinError::io("Could not resolve", path))
        };
        let source = absolute(source)?;
        let root = absolute(mod_root)?;
        let directories = pin_source_directories(&source, &root)?;
        let pinned = Open::new(Access::READ, Share::READ)
            .open(&source)
            .map_err(SourcePinError::io("Could not pin source file", &source))?;
        let facts = inspect_source(&pinned, &source)?;
        Ok(Self {
            source,
            _directories: directories,
            pinned: Some(pinned),
            facts,
        })
    }

    /// The absolute source path the pin holds.
    pub fn path(&self) -> &Path {
        &self.source
    }

    /// Releases the read pin so a delete-capable handle can be opened, as a
    /// replacement test does. The directory pins stay live.
    pub fn release_for_cleanup(&mut self) {
        self.pinned = None;
    }

    /// Deletes the source, provided its path still names the pinned file
    /// object with unchanged content metadata.
    ///
    /// The delete is bound to the reopened handle, so it can never reach a
    /// file put at the path afterwards.
    ///
    /// # Errors
    /// [`SourcePinError::Changed`] for a replaced or modified source, and
    /// [`SourcePinError::Io`] when Windows refuses the delete, for example
    /// because another handle or a memory map still holds the file.
    pub fn remove_if_unchanged(&mut self) -> Result<(), SourcePinError> {
        let candidate = self.open_for_cleanup()?;
        delete_by_handle(&candidate).map_err(SourcePinError::io(
            "A source file could not be removed",
            &self.source,
        ))
    }

    /// Renames the source to the first unoccupied `<name>.bak`, `<name>.bak.bak`
    /// and so on, provided its path still names the pinned, unchanged file
    /// object. Returns the backup path.
    ///
    /// An existing backup, or even a dangling link at a backup name, is never
    /// replaced: the rename itself refuses an occupied name, so there is no
    /// check-then-rename window.
    ///
    /// # Errors
    /// [`SourcePinError::Changed`] for a replaced or modified source, and
    /// [`SourcePinError::Io`] when a rename fails for any reason but an
    /// occupied name.
    pub fn backup_if_unchanged(&mut self) -> Result<PathBuf, SourcePinError> {
        let candidate = self.open_for_cleanup()?;
        let mut name = OsString::from(self.source.as_os_str());
        loop {
            name.push(".bak");
            let destination = PathBuf::from(&name);
            match rename_by_handle(&candidate, &destination, RenameMode::NoReplace) {
                Ok(()) => return Ok(destination),
                // Occupied, by anything at all: try the next name.
                Err(_) if std::fs::symlink_metadata(&destination).is_ok() => continue,
                Err(error) => {
                    return Err(SourcePinError::Io {
                        action: "Could not back up extracted Archive",
                        path: self.source.clone(),
                        source: error,
                    });
                }
            }
        }
    }

    /// Pins the source again for reading, while the caller verifies that its
    /// bytes are still a recoverable Archive after a failed cleanup.
    ///
    /// # Errors
    /// [`SourcePinError::Changed`] when the path no longer names the pinned,
    /// unchanged source, since a replacement is not recovery material for the
    /// bytes already extracted; [`SourcePinError::Io`] when it cannot be opened.
    pub fn pin_unchanged_for_recovery(&mut self) -> Result<(), SourcePinError> {
        let candidate = Open::new(Access::READ, Share::READ)
            .open(&self.source)
            .map_err(SourcePinError::io(
                "Could not pin retained source file",
                &self.source,
            ))?;
        if !inspect_source(&candidate, &self.source)?.unchanged_since(&self.facts) {
            return Err(SourcePinError::Changed(self.source.clone()));
        }
        self.pinned = Some(candidate);
        Ok(())
    }

    /// Reopens the source with delete access, rejecting a substituted or
    /// modified file object.
    fn open_for_cleanup(&mut self) -> Result<File, SourcePinError> {
        // Keep the original file object open while the read pin, which shares
        // no delete, gives way to a delete-capable handle: until that handle
        // exists, this guardian still stops the path being replaced.
        // Attribute-only access takes no part in sharing checks, so the
        // guardian never blocks the open that follows.
        let _guardian = match self.pinned.take() {
            Some(pin) => {
                let guardian = Open::new(Access::READ_ATTRIBUTES, Share::READ | Share::DELETE)
                    .open(&self.source)
                    .map_err(SourcePinError::io(
                        "Could not retain source identity for cleanup",
                        &self.source,
                    ));
                drop(pin);
                Some(guardian?)
            }
            None => None,
        };
        let candidate = Open::new(Access::DELETE | Access::READ_ATTRIBUTES, Share::READ)
            .open(&self.source)
            .map_err(SourcePinError::io(
                "Could not reopen source for cleanup",
                &self.source,
            ))?;
        if !inspect_source(&candidate, &self.source)?.unchanged_since(&self.facts) {
            return Err(SourcePinError::Changed(self.source.clone()));
        }
        Ok(candidate)
    }
}

/// Reads an open source's facts, rejecting a link or a directory.
fn inspect_source(file: &File, source: &Path) -> Result<FileFacts, SourcePinError> {
    let facts =
        FileFacts::of(file).map_err(SourcePinError::io("Could not inspect source file", source))?;
    if !facts.is_ordinary_file() {
        return Err(SourcePinError::NotOrdinaryFile(source.to_path_buf()));
    }
    Ok(facts)
}

/// Pins each directory from the volume root through the source's parent,
/// rejecting junctions and other reparse points before a pathname-based reader
/// can follow them.
///
/// Each pin is opened before the next child, from a root that cannot be
/// renamed down: checking only the final parent could still follow an earlier
/// junction. Directory read access makes the withheld delete sharing actually
/// stop a parent rename.
fn pin_source_directories(source: &Path, root: &Path) -> Result<Vec<File>, SourcePinError> {
    let contained = source.strip_prefix(root).is_ok_and(|relative| {
        !relative.as_os_str().is_empty()
            && relative
                .components()
                .all(|component| matches!(component, Component::Normal(_)))
    });
    let parent = source.parent().filter(|_| contained);
    let Some(parent) = parent else {
        return Err(SourcePinError::OutsideModRoot);
    };
    let mut ancestors: Vec<&Path> = parent.ancestors().collect();
    ancestors.reverse();
    ancestors
        .into_iter()
        .map(|directory| {
            let pin = Open::new(
                Access::LIST_DIRECTORY | Access::READ_ATTRIBUTES,
                Share::READ | Share::WRITE,
            )
            .directory()
            .open(directory)
            .map_err(SourcePinError::io("Could not pin source parent", directory))?;
            let facts = FileFacts::of(&pin).map_err(SourcePinError::io(
                "Could not inspect source parent",
                directory,
            ))?;
            if !facts.is_directory() || facts.is_reparse_point() {
                return Err(SourcePinError::ParentNotDirectory(directory.to_path_buf()));
            }
            Ok(pin)
        })
        .collect()
}
