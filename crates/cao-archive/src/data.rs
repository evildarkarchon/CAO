//! bethutil's `ArchiveData`: the files planned into one output Archive, and the
//! split and merge rules that partition a Mod Root's sources between Archives.
//!
//! Sources: `src/bsa/archive_data.cpp` and `merge` in `src/bsa/pack.cpp` at
//! bethutil `81f882ed`, and the splitter in C++ CAO's `planFinalization`
//! (`src/Run/ArchiveFinalizationPlanning.cpp`), which replaced bethutil's `split`.

use std::os::windows::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};

use crate::error::ArchiveError;
use crate::settings::{ArchiveVersion, FileType, Settings, file_type};

/// What an output Archive holds (`btu::bsa::ArchiveType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArchiveType {
    /// Standard files, compressed when compression is on.
    Standard,
    /// Incompressible files, never compressed. A Standard Archive that absorbed
    /// Incompressible files becomes this type, so it is written uncompressed.
    Incompressible,
    /// Textures, in the game's texture container.
    Textures,
}

/// A file offered for packing, with the size split and merge count for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackSource {
    /// The file, inside the Mod Root being packed.
    pub path: PathBuf,
    /// The file's size on disk in bytes (C++ read `fs::file_size`).
    pub size: u64,
}

/// The files planned into one output Archive (`btu::bsa::ArchiveData`).
///
/// bethutil tracks a "compressed" and an "uncompressed" size, but CAO never
/// overrides them, so both are always the summed source sizes; this holds one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveData {
    size: u64,
    max_size: u64,
    archive_type: ArchiveType,
    version: ArchiveVersion,
    files: Vec<PathBuf>,
}

impl ArchiveData {
    /// An empty Archive of `archive_type`, in the container `settings` gives that
    /// type, limited to `settings.max_size` source bytes.
    pub fn new(settings: &Settings, archive_type: ArchiveType) -> Self {
        Self {
            size: 0,
            max_size: settings.max_size,
            archive_type,
            version: settings.version_for(archive_type),
            files: Vec::new(),
        }
    }

    /// Adds `path` unless its `size` would take the total past the limit (strict
    /// `>`: filling to exactly the limit is allowed). Returns whether it was added.
    pub fn add_file(&mut self, path: PathBuf, size: u64) -> bool {
        match self.size.checked_add(size) {
            Some(total) if total <= self.max_size => {
                self.size = total;
                self.files.push(path);
                true
            }
            _ => false,
        }
    }

    /// The Archive's type. A merge can change it; see [`SplitArchives::merge`].
    pub fn archive_type(&self) -> ArchiveType {
        self.archive_type
    }

    /// The container the Archive is written as. A merge never changes it.
    pub fn version(&self) -> ArchiveVersion {
        self.version
    }

    /// The summed source sizes of its files.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// The most source bytes it may hold.
    pub fn max_size(&self) -> u64 {
        self.max_size
    }

    /// Its files, in the order they were added.
    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }

    /// Whether it has no files. A zero-byte file still counts, as bethutil's
    /// `merge` erases by file count.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Moves `other`'s files into this Archive (bethutil's `operator+=`), leaving
    /// `other` empty and Standard as bethutil's `clear` does.
    ///
    /// Absorbing an Incompressible Archive makes this one Incompressible; absorbing
    /// any other different type makes it Standard. The version never changes, so a
    /// merged Archive keeps Standard's container.
    fn absorb(&mut self, other: &mut Self) {
        // bethutil throws here; `merge` only calls this after its own size check.
        debug_assert!(self.size + other.size <= self.max_size);
        self.size += other.size;
        self.files.append(&mut other.files);
        if self.archive_type == ArchiveType::Incompressible
            || other.archive_type == ArchiveType::Incompressible
        {
            self.archive_type = ArchiveType::Incompressible;
        } else if self.archive_type != other.archive_type {
            self.archive_type = ArchiveType::Standard;
        }
        other.size = 0;
        other.archive_type = ArchiveType::Standard;
    }
}

/// Which open partitions [`SplitArchives::merge`] folds into the Standard one
/// (`btu::bsa::MergeSettings`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MergeSettings {
    /// Merge the open Textures partition (`MergeTextures`).
    pub textures: bool,
    /// Merge the open Incompressible partition (`MergeIncompressible`).
    pub incompressible: bool,
}

/// A Mod Root's sources split into Archives: the partitions that filled up, plus
/// one open partition per type, which [`SplitArchives::merge`] may combine.
///
/// The open partitions are separate fields, so bethutil's precondition that
/// `merge` sees exactly the last three Archives in Standard, Incompressible,
/// Textures order holds by construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitArchives {
    full: Vec<ArchiveData>,
    standard: ArchiveData,
    incompressible: ArchiveData,
    textures: ArchiveData,
}

impl SplitArchives {
    /// Splits `sources`, files inside the Mod Root `root`, into Archives as C++
    /// CAO's `planFinalization` does.
    ///
    /// Sources are sorted first, by component and then by UTF-16 code unit as
    /// MSVC's `std::filesystem::path` compares, because the order decides which
    /// files share an Archive. Each is classified with [`file_type`]; anything not
    /// Standard, Texture or Incompressible is skipped. A file that would take its
    /// type's open partition past the limit closes that partition and starts a
    /// new one.
    ///
    /// Selecting `sources` (skipping root-level files, staging, links and Packing
    /// Exclusions) is the caller's job.
    ///
    /// # Errors
    ///
    /// [`ArchiveError::AssetTooLarge`] when one file is larger than the limit by
    /// itself. bethutil's own `split` dropped such a file; C++ CAO threw.
    pub fn split(
        root: &Path,
        mut sources: Vec<PackSource>,
        settings: &Settings,
    ) -> Result<Self, ArchiveError> {
        sources.sort_by_cached_key(|source| msvc_sort_key(&source.path));
        let mut split = Self {
            full: Vec::new(),
            standard: ArchiveData::new(settings, ArchiveType::Standard),
            incompressible: ArchiveData::new(settings, ArchiveType::Incompressible),
            textures: ArchiveData::new(settings, ArchiveType::Textures),
        };
        for PackSource { path, size } in sources {
            let partition = match file_type(&path, root, settings) {
                FileType::Standard => &mut split.standard,
                FileType::Incompressible => &mut split.incompressible,
                FileType::Texture => &mut split.textures,
                FileType::Blacklist | FileType::Plugin | FileType::Archive => continue,
            };
            if partition.add_file(path.clone(), size) {
                continue;
            }
            let fresh = ArchiveData::new(settings, partition.archive_type);
            split.full.push(std::mem::replace(partition, fresh));
            if !partition.add_file(path.clone(), size) {
                return Err(ArchiveError::AssetTooLarge {
                    path,
                    size,
                    max_size: settings.max_size,
                });
            }
        }
        Ok(split)
    }

    /// Merges the open partitions as `merge` asks (bethutil's `merge`), then drops
    /// empty Archives. Returns the full partitions in the order they filled, then
    /// what is left of the open Standard, Incompressible and Textures partitions.
    ///
    /// - Incompressible joins Standard when their sizes sum to strictly less than
    ///   the limit. Even an empty Incompressible partition joins, which makes the
    ///   Standard Archive Incompressible, so it is written uncompressed.
    /// - Textures then join Standard on the same strict test. The merged Archive
    ///   keeps Standard's container: under FO4 the DDS files go into the `GNRL`
    ///   BA2. Callers that must not do that do not ask for it.
    pub fn merge(self, merge: MergeSettings) -> Vec<ArchiveData> {
        let Self {
            mut full,
            mut standard,
            mut incompressible,
            mut textures,
        } = self;
        if merge.incompressible && incompressible.size + standard.size < standard.max_size {
            standard.absorb(&mut incompressible);
        }
        if merge.textures && textures.size + standard.size < standard.max_size {
            standard.absorb(&mut textures);
        }
        full.extend([standard, incompressible, textures]);
        full.retain(|archive| !archive.is_empty());
        full
    }
}

/// The sort key that orders paths as MSVC's `std::filesystem::path::compare`:
/// component by component, each compared as UTF-16 code units. That differs from
/// [`Path`]'s own order only for code points above U+FFFF.
fn msvc_sort_key(path: &Path) -> Vec<Vec<u16>> {
    path.components()
        .map(|component| component.as_os_str().encode_wide().collect())
        .collect()
}
