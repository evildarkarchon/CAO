//! Why an Archive could not be planned, written or read.

use std::io;
use std::path::PathBuf;

/// Why an Archive could not be planned, written or read.
#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    /// One file is larger than the Archive size limit by itself, so no Archive can
    /// hold it (C++: "An Asset exceeds the output Archive size limit.").
    #[error(
        "`{}` is {size} bytes, more than the {max_size}-byte output Archive size limit",
        path.display()
    )]
    AssetTooLarge {
        path: PathBuf,
        size: u64,
        max_size: u64,
    },
    /// A file or directory could not be read, created or removed.
    #[error("cannot access `{}`: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// A name that must become UTF-8 is not valid Unicode. C++ threw from
    /// `path::u8string()` in the same places.
    #[error("`{}` is not a valid Unicode name", path.display())]
    NonUnicodeName { path: PathBuf },
    /// A file to pack is not inside the Mod Root it is packed from, so it has no
    /// name in the Archive.
    #[error("`{}` is not inside `{}`", path.display(), root.display())]
    OutsideRoot { path: PathBuf, root: PathBuf },
    /// **Deviation 8:** the Archive would not fit its format's 32-bit sizes and
    /// offsets (a BSA past 4 GiB). C++ wrote it corrupt; nothing is left behind.
    #[error(
        "`{}` would be too large for its format: a BSA cannot exceed 4 GiB",
        path.display()
    )]
    ArchiveTooLarge { path: PathBuf },
    /// An entry name in an Archive is not UTF-8. `name` is its lossy decoding.
    #[error("`{}` has an entry name that is not UTF-8: `{name}`", archive.display())]
    InvalidEntryName { archive: PathBuf, name: String },
    /// No entry of the Archive has the requested name.
    #[error("`{}` has no entry `{name}`", archive.display())]
    MissingEntry { archive: PathBuf, name: String },
    /// `ba2` failed on a Morrowind BSA.
    #[error("`{}`: {source}", path.display())]
    Tes3 {
        path: PathBuf,
        #[source]
        source: ba2::tes3::Error,
    },
    /// `ba2` failed reading, compressing or writing a BSA or a file for one.
    #[error("`{}`: {source}", path.display())]
    Tes4 {
        path: PathBuf,
        #[source]
        source: ba2::tes4::Error,
    },
    /// `ba2` failed reading, compressing or writing a BA2 or a file for one.
    #[error("`{}`: {source}", path.display())]
    Fo4 {
        path: PathBuf,
        #[source]
        source: ba2::fo4::Error,
    },
}
