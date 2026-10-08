//! Archive reading and writing for Cathedral Assets Optimizer.
//!
//! This is the bethutil port over `ba2` (#488): per-game tables, Dummy Plugins,
//! file-type classification, splitting and merging archive data, output naming,
//! and writing and reading Archives. It is a leaf crate with no workspace
//! dependencies; `cao-optimizers` adapts it to core's archive-reader trait.

mod data;
mod error;
mod name;
mod read;
mod settings;
mod write;

pub use data::{ArchiveData, ArchiveType, MergeSettings, PackSource, SplitArchives};
pub use error::ArchiveError;
pub use name::{FilePath, list_archives, list_plugins};
pub use read::{ArchiveEntry, ReadArchive};
pub use settings::{AllowedPath, ArchiveVersion, FileType, Game, Settings, file_type};
pub use write::write_archive;
