//! Core's Archive seams over the real libraries (#497).
//!
//! [`ArchiveFileReader`] is the archive reader over `cao-archive`, and
//! [`VolumeProbes`] answers the capacity and volume-identity probes through
//! `cao-winfs`, as C++ `availableArchiveCapacity` and `archiveVolumeIdentity`
//! did. `cao-core` owns everything they feed: discovery's preflight, the
//! Capacity Check, and extraction with source cleanup.

use std::io::BufWriter;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use cao_archive::ReadArchive;
use cao_core::Error;
use cao_core::run::{ArchiveEntry, ArchiveReader, CapacityProbe, VolumeIdentityProbe};
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
