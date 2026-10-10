//! Core's Archive seams over the real libraries (#497, #498).
//!
//! [`ArchiveFileReader`] is the archive reader over `cao-archive`,
//! [`GameArchivePacker`] the packer, and [`VolumeProbes`] answers the capacity
//! and volume-identity probes through `cao-winfs`, as C++
//! `availableArchiveCapacity` and `archiveVolumeIdentity` did. `cao-core` owns
//! everything they feed: discovery's preflight, the Capacity Check,
//! extraction with source cleanup, and Archive Finalization.

use std::collections::HashMap;
use std::io::{BufWriter, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use cao_archive::{
    ArchiveData, ArchiveType, FilePath, MergeSettings, PackSource, ReadArchive, Settings,
    SplitArchives,
};
use cao_core::Error;
use cao_core::run::{
    ArchiveEntry, ArchiveMerge, ArchiveName, ArchiveNameKind, ArchiveNamingRules, ArchivePacker,
    ArchiveReader, CapacityProbe, PackedArchiveKind, PackedFile, PlannedArchive,
    VolumeIdentityProbe,
};
use cao_winfs::{Access, Open, Share};

/// The archive reader over `cao-archive`, for TES3, TES4 and FO4 Archives.
///
/// `ba2` memory-maps an Archive, and Windows refuses to delete a mapped file.
/// Listing therefore maps and unmaps its Archive within the call. Extraction
/// keeps one Archive mapped from its first entry until the next Archive or
/// [`ArchiveReader::release`], since C++ read an Archive once per extraction
/// and re-parsing a large manifest per entry would be quadratic. The extractor
/// releases the reader before any source cleanup.
#[derive(Default)]
pub struct ArchiveFileReader {
    /// The Archive extraction is reading, if any.
    held: Mutex<Option<ReadArchive>>,
}

impl ArchiveFileReader {
    /// Locks the held Archive, recovering it from a panicking holder: the
    /// slot only ever holds a whole parsed Archive or nothing.
    fn held(&self) -> MutexGuard<'_, Option<ReadArchive>> {
        self.held
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Opens `archive`, treating an unrecognized magic as unreadable, as C++
/// extraction's "Unrecognized Archive format." did.
fn open(archive: &Path) -> Result<ReadArchive, Error> {
    ReadArchive::open(archive)
        .map_err(|error| Error::Archive(error.to_string()))?
        .ok_or_else(|| Error::Archive("Unrecognized Archive format.".to_owned()))
}

impl ArchiveReader for ArchiveFileReader {
    /// Lists every entry with the size it extracts to, a DX10 texture's
    /// rebuilt DDS header included. The Archive is unmapped before returning.
    fn list_entries(&self, archive: &Path) -> Result<Vec<ArchiveEntry>, Error> {
        let read = open(archive)?;
        let assets = read
            .archived_assets()
            .map_err(|error| Error::Archive(error.to_string()))?;
        Ok(assets
            .into_iter()
            .map(|asset| ArchiveEntry {
                name: asset.name,
                decompressed_size: asset.size,
            })
            .collect())
    }

    /// Writes one entry's decompressed bytes over the staged file at
    /// `destination`, keeping the Archive mapped for the next entry.
    fn extract_entry(&self, archive: &Path, entry: &str, destination: &Path) -> Result<(), Error> {
        let mut held = self.held();
        if held.as_ref().is_none_or(|read| read.path() != archive) {
            // Unmap the previous Archive before mapping the next, so at most
            // one source is ever held.
            *held = None;
            *held = Some(open(archive)?);
        }
        let read = held.as_ref().expect("the Archive was just opened");
        // Staging already created the file, so open it as it is: exclusively,
        // and never through a link that replaced it, as every other staged
        // write does. A fresh staged file is empty, so nothing is truncated.
        let staged = Open::new(Access::WRITE, Share::NONE).open(destination)?;
        let mut out = BufWriter::new(staged);
        read.extract(entry, &mut out)
            .map_err(|error| Error::Archive(error.to_string()))?;
        out.into_inner()
            .map_err(|error| Error::Io(error.into_error()))?;
        Ok(())
    }

    /// Unmaps the Archive extraction held, so it can be deleted or renamed.
    fn release(&self) {
        *self.held() = None;
    }
}

/// The capacity and volume-identity probes over the real volumes.
pub struct VolumeProbes;

impl CapacityProbe for VolumeProbes {
    /// The bytes available to this process at `root`, honouring quotas. A
    /// failed query is unknown capacity, and so is an all-ones answer, which
    /// C++ treated as the library's "unknown" sentinel.
    fn available_bytes(&self, root: &Path) -> Option<u64> {
        cao_winfs::available_space(root)
            .ok()
            .filter(|&available| available != u64::MAX)
    }
}

impl VolumeIdentityProbe for VolumeProbes {
    /// The volume GUID path of the volume holding `root`, mounted folders
    /// included; a failed query leaves the identity unknown, which keeps the
    /// Capacity Check conservative.
    fn volume_identity(&self, root: &Path) -> Option<String> {
        cao_winfs::volume_guid_path(root)
            .ok()
            .and_then(|volume| volume.into_os_string().into_string().ok())
    }
}

/// The Archive packer over `cao-archive`, for one game's rules: its tables,
/// classification, split and merge, `FilePath` naming, and the writer.
pub struct GameArchivePacker {
    settings: Settings,
    rules: ArchiveNamingRules,
}

impl GameArchivePacker {
    /// A packer for `settings`, already adjusted to the profile's maximum
    /// Archive size.
    ///
    /// **Deviation 21:** a game whose Textures need their own container (FO4's
    /// DX10 BA2) keeps them separate, so Archive Finalization never merges DDS
    /// files into the `GNRL` Main BA2, which C++ did when "create texture
    /// archive" was off.
    pub fn new(settings: Settings) -> Self {
        let rules = ArchiveNamingRules {
            extension: settings.extension.to_owned(),
            suffix: settings.suffix.map(str::to_owned),
            texture_suffix: settings.texture_suffix.map(str::to_owned),
            plugin_extensions: settings
                .plugin_extensions
                .iter()
                .map(|extension| (*extension).to_owned())
                .collect(),
            dummy_plugin: settings.dummy_plugin.to_vec(),
            separate_textures: settings.version_for(ArchiveType::Textures)
                != settings.version_for(ArchiveType::Standard),
        };
        Self { settings, rules }
    }
}

/// Core's name for a parsed `cao-archive` name.
fn archive_name(name: FilePath) -> ArchiveName {
    ArchiveName {
        dir: name.dir,
        name: name.name,
        suffix: name.suffix,
        ext: name.ext,
        counter: name.counter,
    }
}

/// The kind core plans an Archive of `archive_type` as.
fn packed_kind(archive_type: ArchiveType) -> PackedArchiveKind {
    match archive_type {
        ArchiveType::Standard => PackedArchiveKind::Standard,
        ArchiveType::Incompressible => PackedArchiveKind::Incompressible,
        ArchiveType::Textures => PackedArchiveKind::Textures,
    }
}

/// The `cao-archive` type an Archive planned as `kind` is written as.
fn archive_type(kind: PackedArchiveKind) -> ArchiveType {
    match kind {
        PackedArchiveKind::Standard => ArchiveType::Standard,
        PackedArchiveKind::Incompressible => ArchiveType::Incompressible,
        PackedArchiveKind::Textures => ArchiveType::Textures,
    }
}

/// An archive failure as core reports it.
fn archive_error(error: impl std::fmt::Display) -> Error {
    Error::Archive(error.to_string())
}

impl ArchivePacker for GameArchivePacker {
    fn rules(&self) -> &ArchiveNamingRules {
        &self.rules
    }

    fn list_names(&self, dir: &Path, kind: ArchiveNameKind) -> Result<Vec<ArchiveName>, Error> {
        let names = match kind {
            ArchiveNameKind::Plugin => cao_archive::list_plugins(dir, &self.settings),
            ArchiveNameKind::Archive => cao_archive::list_archives(dir, &self.settings),
        }
        .map_err(archive_error)?;
        Ok(names.into_iter().map(archive_name).collect())
    }

    fn partition(
        &self,
        root: &Path,
        sources: Vec<PackedFile>,
        merge: ArchiveMerge,
    ) -> Result<Vec<PlannedArchive>, Error> {
        let sizes: HashMap<PathBuf, u64> = sources
            .iter()
            .map(|source| (source.path.clone(), source.size))
            .collect();
        let sources = sources
            .into_iter()
            .map(|source| PackSource {
                path: source.path,
                size: source.size,
            })
            .collect();
        let archives = SplitArchives::split(root, sources, &self.settings)
            .map_err(archive_error)?
            .merge(MergeSettings {
                textures: merge.textures,
                incompressible: merge.incompressible,
            });
        Ok(archives
            .into_iter()
            .map(|archive| PlannedArchive {
                kind: packed_kind(archive.archive_type()),
                files: archive
                    .files()
                    .iter()
                    .map(|path| PackedFile {
                        path: path.clone(),
                        size: sizes[path],
                    })
                    .collect(),
            })
            .collect())
    }

    /// Rebuilds the planned Archive and writes it into the staged file.
    ///
    /// A merged Archive's container is always its Standard partition's, and
    /// the type it was planned as gives that same container, so rebuilding
    /// from the type writes exactly what was planned.
    fn write(
        &self,
        archive: &PlannedArchive,
        compress: bool,
        root: &Path,
        destination: &Path,
    ) -> Result<(), Error> {
        let mut data = ArchiveData::new(&self.settings, archive_type(archive.kind));
        for file in &archive.files {
            if !data.add_file(file.path.clone(), file.size) {
                return Err(Error::Archive(format!(
                    "`{}` no longer fits its planned Archive",
                    file.path.display()
                )));
            }
        }
        // Staging already created the file, so open it as it is: exclusively,
        // and never through a link that replaced it, as every other staged
        // write does.
        let staged = Open::new(Access::WRITE, Share::NONE).open(destination)?;
        let mut out = BufWriter::new(staged);
        cao_archive::write_archive_into(compress, &data, root, &mut out, destination)
            .map_err(archive_error)?;
        out.flush()?;
        // Close the staged file before it is published.
        drop(out);
        Ok(())
    }
}
