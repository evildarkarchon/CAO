//! bethutil's `ArchiveData`: the files planned into one output Archive, and the
//! split and merge rules that partition a Mod Root's sources between Archives.
//!
//! Sources: `src/bsa/archive_data.cpp` and `merge` in `src/bsa/pack.cpp` at
//! bethutil `81f882ed`, and the splitter in C++ CAO's `planFinalization`
//! (`src/Run/ArchiveFinalizationPlanning.cpp`), which replaced bethutil's `split`.

use std::collections::BTreeSet;
use std::os::windows::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};

use crate::error::{ArchiveError, AssetLimit};
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
///
/// A BSA whose game has a [`Settings::size_cap`] also counts the most bytes
/// its written file can take ([`CappedLayout`]), and never plans past the cap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveData {
    size: u64,
    max_size: u64,
    archive_type: ArchiveType,
    version: ArchiveVersion,
    files: Vec<PathBuf>,
    layout: Option<CappedLayout>,
}

impl ArchiveData {
    /// An empty Archive of `archive_type`, in the container `settings` gives that
    /// type, grouping up to [`Settings::archive_limit`] source bytes and, for a
    /// BSA, limited to [`Settings::size_cap`] written bytes.
    pub fn new(settings: &Settings, archive_type: ArchiveType) -> Self {
        let version = settings.version_for(archive_type);
        Self {
            size: 0,
            max_size: settings.archive_limit(),
            archive_type,
            version,
            files: Vec::new(),
            layout: settings
                .size_cap()
                .and_then(|cap| CappedLayout::new(cap, version)),
        }
    }

    /// Adds `path` unless its `size` would take the total past the Archive
    /// Limit (strict `>`: filling to exactly the limit is allowed), or the
    /// written BSA past its game's Archive Size Cap, or the file is too large
    /// for any BSA ([`AssetLimit::BsaFile`]). Returns whether it was added.
    ///
    /// An empty Archive takes any one file the size cap and the container
    /// allow, however far past the Archive Limit: the limit only groups files,
    /// so a file larger than it gets an Archive of its own rather than none.
    pub fn add_file(&mut self, path: PathBuf, size: u64) -> bool {
        // Refused while planning, so split fails the file naming the limit
        // that applies, rather than packing it alone for the writer to fail.
        if self.file_limit().is_some_and(|limit| size >= limit) {
            return false;
        }
        let Some(total) = self.size.checked_add(size) else {
            return false;
        };
        if total > self.max_size && !self.files.is_empty() {
            return false;
        }
        if let Some(layout) = &mut self.layout
            && !layout.add(&path, size)
        {
            return false;
        }
        self.size = total;
        self.files.push(path);
        true
    }

    /// The most bytes the written Archive may take, when its game caps them:
    /// planning keeps it there, and writing refuses an Archive that is not.
    pub fn size_cap(&self) -> Option<u64> {
        self.layout.as_ref().map(|layout| layout.cap)
    }

    /// The bytes from which no file fits the container, whatever its game's
    /// cap: [`BSA_FILE_LIMIT`] for a BSA, `None` for a BA2.
    fn file_limit(&self) -> Option<u64> {
        match self.version {
            ArchiveVersion::Tes5 | ArchiveVersion::Sse => Some(BSA_FILE_LIMIT),
            ArchiveVersion::Fo4 | ArchiveVersion::Fo4Dx => None,
        }
    }

    /// Whether `other` may join this Archive in a merge: their summed source
    /// sizes are strictly under the limit (bethutil's test), and the merged
    /// BSA's layout fits its game's size cap.
    fn can_absorb(&self, other: &Self) -> bool {
        let layout_fits = match (&self.layout, &other.layout) {
            (Some(layout), Some(other)) => layout.merged_bound(other) <= layout.cap,
            _ => true,
        };
        self.size + other.size < self.max_size && layout_fits
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

    /// The Archive Limit it groups files by. It holds more only when one file
    /// past the limit has it to itself.
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
        if let (Some(layout), Some(other)) = (&mut self.layout, &mut other.layout) {
            layout.absorb(other);
        }
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
    /// type's open partition past the Archive Limit closes that partition and
    /// starts a new one. A file larger than the Archive Limit by itself gets
    /// that new partition to itself, and the next file starts another.
    ///
    /// Selecting `sources` (skipping root-level files, staging, links and Packing
    /// Exclusions) is the caller's job.
    ///
    /// # Errors
    ///
    /// [`ArchiveError::AssetTooLarge`] when one file is too large for even a
    /// BSA of its own: past its game's Archive Size Cap, or 1 GiB or more,
    /// which no BSA file record can hold. bethutil's own `split` dropped a file
    /// past the Archive Limit; CAO packs it alone instead, and fails only where
    /// the game could not load the result.
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
                FileType::Unpackable | FileType::Plugin | FileType::Archive => continue,
            };
            if partition.add_file(path.clone(), size) {
                continue;
            }
            let fresh = ArchiveData::new(settings, partition.archive_type);
            split.full.push(std::mem::replace(partition, fresh));
            if !partition.add_file(path.clone(), size) {
                // An empty partition takes any file past the Archive Limit, so
                // only the BSA file limit or its layout's size cap can refuse
                // one; the cap's fallback is never reached.
                let limit = match partition.file_limit() {
                    Some(limit) if size >= limit => AssetLimit::BsaFile(limit),
                    _ => AssetLimit::ArchiveSizeCap(partition.size_cap().unwrap_or(u64::MAX)),
                };
                return Err(ArchiveError::AssetTooLarge { path, size, limit });
            }
        }
        Ok(split)
    }

    /// Merges the open partitions as `merge` asks (bethutil's `merge`), then drops
    /// empty Archives. Returns the full partitions in the order they filled, then
    /// what is left of the open Standard, Incompressible and Textures partitions.
    ///
    /// - Incompressible joins Standard when their sizes sum to strictly less than
    ///   the limit and, for a BSA under a size cap, their merged layout fits it.
    ///   Even an empty Incompressible partition joins, which makes the
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
        if merge.incompressible && standard.can_absorb(&incompressible) {
            standard.absorb(&mut incompressible);
        }
        if merge.textures && standard.can_absorb(&textures) {
            standard.absorb(&mut textures);
        }
        full.extend([standard, incompressible, textures]);
        full.retain(|archive| !archive.is_empty());
        full
    }
}

/// The most bytes a planned BSA can take once written, kept against its game's
/// size cap so every Archive CAO writes is one the game loads.
///
/// A BSA is its 36-byte header, one record per folder (16 bytes in v104, 24 in
/// v105) with the folder's name as a length-prefixed, null-terminated string,
/// one 16-byte record per file, every file name null-terminated, and then the
/// file data. Writing never stores a file in more bytes than its source (see
/// `write.rs`: a file compression would grow is stored as it is), so the source
/// size bounds each file's data. Folder names are counted as the absolute
/// parent paths, which are longer than the Mod-Root-relative names written, so
/// the bound is safe without the Mod Root; it is the same whether an Archive is
/// planned whole, merged, or rebuilt file by file for writing.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CappedLayout {
    cap: u64,
    directory_record: u64,
    directories: BTreeSet<PathBuf>,
    bound: u64,
}

/// The bytes from which a file cannot go into a BSA: a BSA file record keeps
/// the file's stored size in its low 30 bits, with the compression toggle and
/// a second flag above them, so `ba2` refuses a stored size of 1 GiB or more.
/// Writing never stores a file in more bytes than its source, so a source
/// under this always fits.
const BSA_FILE_LIMIT: u64 = 1 << 30;
/// A BSA header's size.
const BSA_HEADER: u64 = 36;
/// A BSA file record's size: name hash, size and offset.
const BSA_FILE_RECORD: u64 = 16;

impl CappedLayout {
    /// An empty BSA's layout under `cap`, or `None` for a container this does
    /// not model: BA2s have no size cap.
    fn new(cap: u64, version: ArchiveVersion) -> Option<Self> {
        let directory_record = match version {
            ArchiveVersion::Tes5 => 16,
            ArchiveVersion::Sse => 24,
            ArchiveVersion::Fo4 | ArchiveVersion::Fo4Dx => return None,
        };
        Some(Self {
            cap,
            directory_record,
            directories: BTreeSet::new(),
            bound: BSA_HEADER,
        })
    }

    /// Counts `path`'s `size` bytes, its file record and name, and its folder
    /// when the folder is new, unless that would take the bound past the cap.
    /// Returns whether it was counted.
    fn add(&mut self, path: &Path, size: u64) -> bool {
        let parent = path.parent().unwrap_or(Path::new(""));
        let folder = if self.directories.contains(parent) {
            0
        } else {
            self.folder_bytes(parent)
        };
        let file = size
            .saturating_add(BSA_FILE_RECORD)
            .saturating_add(name_bytes(path.file_name().unwrap_or_default().as_ref()) + 1);
        let bound = self.bound.saturating_add(folder).saturating_add(file);
        if bound > self.cap {
            return false;
        }
        self.bound = bound;
        self.directories.insert(parent.to_path_buf());
        true
    }

    /// The bound of this layout and `other` written as one BSA: one header, and
    /// each folder both hold counted once.
    fn merged_bound(&self, other: &Self) -> u64 {
        let shared: u64 = self
            .directories
            .intersection(&other.directories)
            .map(|folder| self.folder_bytes(folder))
            .sum();
        self.bound + other.bound - BSA_HEADER - shared
    }

    /// Takes `other`'s files into this layout, leaving `other` empty.
    fn absorb(&mut self, other: &mut Self) {
        self.bound = self.merged_bound(other);
        self.directories.append(&mut other.directories);
        other.bound = BSA_HEADER;
    }

    /// A folder's record and its name, length-prefixed and null-terminated.
    fn folder_bytes(&self, folder: &Path) -> u64 {
        self.directory_record + 1 + name_bytes(folder) + 1
    }
}

/// The bytes `name` takes as UTF-8, which `ba2` writes no longer.
fn name_bytes(name: &Path) -> u64 {
    name.to_string_lossy().len() as u64
}

/// The sort key that orders paths as MSVC's `std::filesystem::path::compare`:
/// component by component, each compared as UTF-16 code units. That differs from
/// [`Path`]'s own order only for code points above U+FFFF.
fn msvc_sort_key(path: &Path) -> Vec<Vec<u16>> {
    path.components()
        .map(|component| component.as_os_str().encode_wide().collect())
        .collect()
}
