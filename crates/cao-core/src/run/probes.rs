//! The archive reader and the capacity and volume-identity probes.
//!
//! These seams are declared here so `cao-core` never depends on `ba2` or on a
//! filesystem-space API (#468). Archive discovery, extraction and the Capacity
//! Check consume them (#496, #497); `cao-optimizers` implements them.

use std::path::Path;

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
/// `AssetRunAdapters`. Implementations must release any memory map of an
/// Archive before returning, so the Archive can be renamed or deleted.
pub trait ArchiveReader: Send + Sync {
    /// Lists every entry without extracting or decompressing it.
    fn list_entries(&self, archive: &Path) -> Result<Vec<ArchiveEntry>, Error>;

    /// Writes one entry's decompressed bytes to `destination`, which the caller
    /// has already registered for Temporary Ownership.
    fn extract_entry(&self, archive: &Path, entry: &str, destination: &Path) -> Result<(), Error>;
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
