//! The archive reader and packer, and the capacity and volume-identity probes.
//!
//! These seams are declared here so `cao-core` never depends on `ba2` or on a
//! filesystem-space API (#468). Archive discovery, extraction and the Capacity
//! Check consume them (#496, #497), Archive Finalization adds the packer
//! (#498), and `cao-optimizers` implements them all.

use std::path::{Path, PathBuf};

use crate::Error;

/// One raw entry of an Archive's manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveEntry {
    /// The entry's game path exactly as the Archive spells it, unsanitized, so
    /// discovery can reject an Unsafe Game Path before extraction.
    pub name: String,
    /// The entry's decompressed size in bytes, including any header extraction
    /// reconstructs (such as a DX10 texture's DDS header).
    pub decompressed_size: u64,
}

/// Reads Archives without CAO depending on the archive library.
///
/// It replaces the C++ `ArchiveExtractionOperation` and the Archive closures of
/// `AssetRunAdapters`.
///
/// On Windows an open memory map stops its file from being deleted, so when
/// an Archive's handles and maps are released is part of the contract:
/// [`Self::list_entries`] releases everything before it returns, while
/// [`Self::extract_entry`] may keep its Archive open for the next entry, as
/// C++ read an Archive once per extraction, until [`Self::release`]. The
/// extractor calls `release` at the end of every attempt, before the source
/// Archive can be removed or renamed.
pub trait ArchiveReader: Send + Sync {
    /// Lists every entry without extracting or decompressing it, holding
    /// nothing open afterwards.
    fn list_entries(&self, archive: &Path) -> Result<Vec<ArchiveEntry>, Error>;

    /// Writes one entry's decompressed bytes to `destination`, which the caller
    /// has already registered for Temporary Ownership.
    fn extract_entry(&self, archive: &Path, entry: &str, destination: &Path) -> Result<(), Error>;

    /// Drops every Archive this reader still holds open, so each can be
    /// renamed or deleted. The default holds nothing, so does nothing.
    fn release(&self) {}
}

/// Samples the bytes available to the caller at a Mod Root.
pub trait CapacityProbe: Send + Sync {
    /// The available bytes, or `None` when capacity is unknown. Unknown is not
    /// unlimited, but it never fails a Capacity Check.
    fn available_bytes(&self, root: &Path) -> Option<u64>;
}

/// Identifies the volume containing a Mod Root, so Capacity Checks group roots by volume.
pub trait VolumeIdentityProbe: Send + Sync {
    /// An opaque volume identity, or `None` when unknown; an unknown root may
    /// share any other root's volume, which keeps the check conservative.
    fn volume_identity(&self, root: &Path) -> Option<String>;
}

/// What one planned output Archive holds (bethutil's `ArchiveType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PackedArchiveKind {
    /// Standard files, compressed when compression is on.
    Standard,
    /// Incompressible files, never compressed. A Standard Archive that
    /// absorbed Incompressible files becomes this kind.
    Incompressible,
    /// Textures, in the game's texture container and named with its texture
    /// suffix.
    Textures,
}

/// A file offered for packing, with the size splitting counts for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackedFile {
    /// The file, beneath the Mod Root being packed.
    pub path: PathBuf,
    /// Its size on disk in bytes, as planning measured it.
    pub size: u64,
}

/// One output Archive's frozen contents, as the packer partitioned them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedArchive {
    pub kind: PackedArchiveKind,
    /// Its files, in the order the packer placed them.
    pub files: Vec<PackedFile>,
}

/// Which open partitions the packer folds into the Standard Archive
/// (bethutil's `MergeSettings`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ArchiveMerge {
    /// Incompressible files join the Standard Archive, which is then written
    /// uncompressed.
    pub incompressible: bool,
    /// Textures join the Standard Archive.
    pub textures: bool,
}

/// The naming and Loading Plugin rules of the run's game (the parts of
/// bethutil's `Settings` that Archive Finalization reads).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveNamingRules {
    /// The Archive extension with its dot: `.bsa` or `.ba2`.
    pub extension: String,
    /// The name suffix of Standard and Incompressible Archives, such as `Main`.
    pub suffix: Option<String>,
    /// The name suffix of Textures Archives, such as `Textures`.
    pub texture_suffix: Option<String>,
    /// The plugin extensions, with their dots, in the order Loading Plugin
    /// names are tried.
    pub plugin_extensions: Vec<String>,
    /// The canonical Dummy Plugin bytes.
    pub dummy_plugin: Vec<u8>,
    /// Whether Textures always get their own Archive, whatever the merge
    /// choice asks. **Deviation 21:** FO4 sets it, so DDS files never go into
    /// the `GNRL` Main BA2.
    pub separate_textures: bool,
}

/// Which names [`ArchivePacker::list_names`] parses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ArchiveNameKind {
    /// A plugin, matched against the game's plugin extensions.
    Plugin,
    /// An Archive, matched against the game's Archive extension.
    Archive,
}

/// A plugin or Archive name split into its parts, as `cao-archive`'s
/// `FilePath` (bethutil's) parses it: `Foo2 - Textures.bsa` is name `Foo`,
/// counter 2, suffix `Textures` and extension `.bsa`.
///
/// It is plain data so the packer can parse names while Archive
/// Finalization changes the extension, suffix and counter to build the names
/// it needs. The derived ordering compares the fields in declaration order,
/// which is bethutil's; C++ CAO sorted plugins with it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ArchiveName {
    /// The directory holding the file.
    pub dir: PathBuf,
    /// The stem without its counter and suffix.
    pub name: String,
    /// The game's suffix the stem ended with, or empty.
    pub suffix: String,
    /// The extension, with its dot.
    pub ext: String,
    /// The trailing number of the stem, if any.
    pub counter: Option<u32>,
}

impl ArchiveName {
    /// The path this name renders as: name, then counter, then ` - ` and the
    /// suffix when there is one, then the extension, in [`Self::dir`].
    ///
    /// This must render exactly as `cao-archive`'s `FilePath::full_path`; a
    /// test in `cao-optimizers` holds the two equal.
    pub fn full_path(&self) -> PathBuf {
        let mut full = self.name.clone();
        if let Some(counter) = self.counter {
            full.push_str(&counter.to_string());
        }
        if !self.suffix.is_empty() {
            full.push_str(" - ");
            full.push_str(&self.suffix);
        }
        full.push_str(&self.ext);
        self.dir.join(full)
    }
}

/// Packs Archives without CAO depending on the archive library: the game's
/// naming rules, its file classification with the split and merge rules,
/// and the Archive writer.
///
/// Archive Finalization owns everything else: which files are offered, the
/// output names, capacity, publication, Loading Plugins and source cleanup.
pub trait ArchivePacker: Send + Sync {
    /// The run's game rules.
    fn rules(&self) -> &ArchiveNamingRules;

    /// Lists the names of `kind` directly in `dir`, not recursively, skipping
    /// directories, in directory order.
    ///
    /// # Errors
    /// `dir` cannot be listed, or an entry's name is not Unicode.
    fn list_names(&self, dir: &Path, kind: ArchiveNameKind) -> Result<Vec<ArchiveName>, Error>;

    /// Classifies `sources`, files beneath the Mod Root `root`, sorts them and
    /// splits them into Archives by the game's size limit, then merges the
    /// open partitions as `merge` asks. Files that are not packable stay out
    /// of every Archive. Empty Archives are dropped.
    ///
    /// # Errors
    /// One file is larger than the size limit by itself.
    fn partition(
        &self,
        root: &Path,
        sources: Vec<PackedFile>,
        merge: ArchiveMerge,
    ) -> Result<Vec<PlannedArchive>, Error>;

    /// Writes `archive`, whose files live beneath `root`, into the existing
    /// empty file `destination`, which the caller has registered for
    /// Temporary Ownership. Every handle and mapping of the destination and
    /// the sources is released before this returns.
    ///
    /// # Errors
    /// A source cannot be read or compressed, or the Archive cannot be written.
    fn write(
        &self,
        archive: &PlannedArchive,
        compress: bool,
        root: &Path,
        destination: &Path,
    ) -> Result<(), Error>;
}
