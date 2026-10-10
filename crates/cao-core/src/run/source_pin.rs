//! Native pins that keep an Archive source stable while pathname-based
//! Archive I/O runs, then clean up only that file object.
//!
//! Ported from `src/Run/NativeFilePins.cpp`'s `SourceFilePin` and
//! `backupExtractedArchive`. The archive reader opens its Archive by path, so
//! the pin denies writes, renames and deletes of the source and of every
//! ancestor directory for as long as that reading lasts. Cleanup then
//! reopens the path and acts only if it still names the very file object
//! that was pinned, unchanged.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::File;
use std::io;
use std::path::{Component, Path, PathBuf};

use cao_winfs::{Access, FileFacts, Open, RenameMode, Share, delete_by_handle, rename_by_handle};

/// Why a source could not be pinned or cleaned up. Nothing has been mutated
/// when one is returned: a cleanup either happens whole or not at all.
#[derive(Debug, thiserror::Error)]
pub enum SourcePinError {
    /// The pinned source or plugin is not strictly inside its Mod Root.
    #[error("A pinned file is outside its Mod Root.")]
    OutsideModRoot,
    /// A native open, query, rename or delete failed.
    #[error("{action} {}: {source}", .path.display())]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// A directory on the pinned path is a link or not a directory.
    #[error("A pinned parent is no longer an ordinary directory: {}", .0.display())]
    ParentNotDirectory(PathBuf),
    /// The source is a link or a directory rather than an ordinary file.
    #[error("A source file is no longer an ordinary file: {}", .0.display())]
    NotOrdinaryFile(PathBuf),
    /// The path names another file object, or the pinned one was changed.
    #[error("A source file changed before cleanup: {}", .0.display())]
    Changed(PathBuf),
    /// A Loading Plugin about to justify source deletion is a link, a
    /// directory or empty.
    #[error("The loading plugin is not a nonempty ordinary file: {}", .0.display())]
    NotLoadingPlugin(PathBuf),
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
        let mut directories = SourceDirectoryPins::default();
        let mut pin = Self::with_shared_directories(source, mod_root, &mut directories)?;
        pin._directories = directories.handles.into_values().collect();
        Ok(pin)
    }

    /// Pins `source` as [`Self::new`] does, but keeps its directory chain in
    /// `directories`, reusing any directory already pinned there, as C++
    /// `SourceFilePin::sharedDirectoryPins` let one output's sources share
    /// ancestor handles.
    ///
    /// The returned pin owns no directory handle: keep `directories` alive
    /// until its cleanup ends.
    ///
    /// # Errors
    /// As [`Self::new`].
    pub fn with_shared_directories(
        source: &Path,
        mod_root: &Path,
        directories: &mut SourceDirectoryPins,
    ) -> Result<Self, SourcePinError> {
        let source = absolute_normal(source)?;
        let root = absolute_normal(mod_root)?;
        pin_directories(
            &source,
            &root,
            Share::READ | Share::WRITE,
            &mut directories.handles,
        )?;
        let pinned = Open::new(Access::READ, Share::READ)
            .open(&source)
            .map_err(SourcePinError::io("Could not pin source file", &source))?;
        let facts = inspect_source(&pinned, &source)?;
        Ok(Self {
            source,
            _directories: Vec::new(),
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

/// Directory pins shared by the sources of one output Archive; see
/// [`SourceFilePin::with_shared_directories`].
///
/// Dropping it releases every directory it pinned.
#[derive(Default)]
pub struct SourceDirectoryPins {
    handles: BTreeMap<PathBuf, File>,
}

/// Keeps one ordinary Loading Plugin entry usable until its Archive's source
/// cleanup ends (C++ `LoadingPluginPin`).
///
/// Packed sources are deleted only because the new Archive loads through this
/// plugin, so the plugin must not disappear, or become a link a later
/// retarget could unload, before the last source is gone. Its directory chain
/// shares only reading, and its entry is held without write or delete sharing.
pub struct LoadingPluginPin {
    _directories: BTreeMap<PathBuf, File>,
    _entry: File,
}

impl LoadingPluginPin {
    /// Pins `plugin`, inside `mod_root`, and its directory chain.
    ///
    /// # Errors
    /// [`SourcePinError::NotLoadingPlugin`] for a link, a directory or an
    /// empty file: an attribute-only write can retarget a symlink despite
    /// sharing locks, while a nonempty ordinary file cannot become one without
    /// a write this pin excludes. Otherwise as [`SourceFilePin::new`].
    pub fn new(plugin: &Path, mod_root: &Path) -> Result<Self, SourcePinError> {
        let plugin = absolute_normal(plugin)?;
        let root = absolute_normal(mod_root)?;
        let mut directories = BTreeMap::new();
        pin_directories(&plugin, &root, Share::READ, &mut directories)?;
        let entry = Open::new(Access::READ, Share::READ)
            .open(&plugin)
            .map_err(SourcePinError::io("Could not pin loading plugin", &plugin))?;
        let facts = FileFacts::of(&entry).map_err(SourcePinError::io(
            "Could not inspect loading plugin",
            &plugin,
        ))?;
        if !facts.is_ordinary_file() || facts.size() == 0 {
            return Err(SourcePinError::NotLoadingPlugin(plugin));
        }
        Ok(Self {
            _directories: directories,
            _entry: entry,
        })
    }
}

/// `path` made absolute and lexically normal, as C++ `absolute(...)
/// .lexically_normal()` did; Windows' full-path resolution does both.
pub(crate) fn absolute_normal(path: &Path) -> Result<PathBuf, SourcePinError> {
    std::path::absolute(path).map_err(SourcePinError::io("Could not resolve", path))
}

/// Pins each directory from the volume root through the parent of `path`,
/// which must be strictly inside `root`, into `pins`, rejecting junctions and
/// other reparse points before a pathname-based reader can follow them.
/// Directories already in `pins` are reused.
///
/// Each pin is opened before the next child, from a root that cannot be
/// renamed down: checking only the final parent could still follow an earlier
/// junction. Directory read access makes the withheld delete sharing actually
/// stop a parent rename. `share` is what the pins let other handles do:
/// sources share reading and writing; a Loading Plugin and a Dummy Plugin
/// being removed share only reading, as C++ did.
pub(crate) fn pin_directories(
    path: &Path,
    root: &Path,
    share: Share,
    pins: &mut BTreeMap<PathBuf, File>,
) -> Result<(), SourcePinError> {
    let contained = path.strip_prefix(root).is_ok_and(|relative| {
        !relative.as_os_str().is_empty()
            && relative
                .components()
                .all(|component| matches!(component, Component::Normal(_)))
    });
    let parent = path.parent().filter(|_| contained);
    let Some(parent) = parent else {
        return Err(SourcePinError::OutsideModRoot);
    };
    let mut ancestors: Vec<&Path> = parent.ancestors().collect();
    ancestors.reverse();
    for directory in ancestors {
        if pins.contains_key(directory) {
            continue;
        }
        let pin = Open::new(Access::LIST_DIRECTORY | Access::READ_ATTRIBUTES, share)
            .directory()
            .open(directory)
            .map_err(SourcePinError::io(
                "Could not pin parent directory",
                directory,
            ))?;
        let facts = FileFacts::of(&pin).map_err(SourcePinError::io(
            "Could not inspect parent directory",
            directory,
        ))?;
        if !facts.is_directory() || facts.is_reparse_point() {
            return Err(SourcePinError::ParentNotDirectory(directory.to_path_buf()));
        }
        pins.insert(directory.to_path_buf(), pin);
    }
    Ok(())
}
