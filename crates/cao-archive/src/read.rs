//! Reading an existing Archive: its inventory and the decompressed bytes of each
//! Archived Asset (bethutil's `read_archive` and `File::write(path)`, and C++
//! CAO's `inspectArchiveInventory`).

use std::fs::File as FsFile;
use std::io::Write;
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};

use ba2::prelude::*;
use ba2::{FileFormat, fo4, tes3, tes4};

use crate::error::ArchiveError;
use crate::settings::ArchiveVersion;

/// The bytes a DX10 texture's rebuilt DDS header takes: the magic, the 124-byte
/// header and the 20-byte DX10 extension. C++ CAO's inventory adds them.
const DX10_HEADER_LEN: u64 = 4 + 124 + 20;

/// One Archived Asset, as [`ReadArchive::archived_assets`] lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchivedAsset {
    /// The name as stored. TES4 names join directory and file with a backslash
    /// and map any `/` to `\`; FO4 and TES3 names are kept as stored.
    pub name: String,
    /// The bytes it extracts to, as C++ CAO's inventory counts them: the
    /// decompressed size, plus [`DX10_HEADER_LEN`] for a DX10 texture.
    pub size: u64,
    /// Whether its data is stored compressed (for FO4: every chunk is).
    pub compressed: bool,
    /// How many chunks hold its data: 1 except in FO4 Archives.
    pub chunks: usize,
    /// The mip range each chunk of a DX10 texture holds, in chunk order;
    /// empty for every other entry.
    pub mip_ranges: Vec<RangeInclusive<u16>>,
}

/// An Archive's container family, as its magic names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArchiveFormat {
    /// A Morrowind BSA.
    Tes3,
    /// A BSA from Oblivion to Skyrim SE.
    Tes4,
    /// A BTDX BA2.
    Fo4,
}

/// An FO4 BA2's container kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Fo4Container {
    /// `GNRL`: general files.
    General,
    /// `DX10`: textures, with a DDS header rebuilt on extraction.
    Dx10,
    /// `GNMF`: PlayStation textures, which CAO never writes.
    Gnmf,
}

/// What an Archive's header says, beyond its entries: the facts the parity
/// comparator holds equal between two Archives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArchiveHeader {
    /// The container family, from the magic.
    pub format: ArchiveFormat,
    /// The version field: 103, 104 or 105 for a TES4 BSA, 1 to 8 for a BA2,
    /// and 0 for TES3, which has none.
    pub version: u32,
    /// The TES4 archive flags' bits; 0 for the other formats.
    pub flags: u32,
    /// The TES4 archive types' bits; 0 for the other formats.
    pub types: u32,
    /// The BA2 container kind; `None` for a BSA.
    pub container: Option<Fo4Container>,
    /// Whether the Archive stores its entries' names: a BA2's name table, or
    /// a TES4 BSA's directory and file strings both. TES3 always does.
    pub name_table: bool,
}

/// An Archive opened for reading.
///
/// `ba2` memory-maps the file and every Archived Asset borrows that mapping, so
/// the Archive file cannot be deleted or renamed on Windows until this is dropped.
pub struct ReadArchive {
    path: PathBuf,
    inner: Inner,
}

/// The parsed Archive, by format. TES4 and FO4 keep the header options they were
/// read with: extraction needs the TES4 version's codec, the FO4 compression
/// format, and the FO4 container kind (`GNRL` or `DX10`) to rebuild DDS headers.
enum Inner {
    Tes3(tes3::Archive<'static>),
    Tes4(tes4::Archive<'static>, tes4::ArchiveOptions),
    Fo4(fo4::Archive<'static>, fo4::ArchiveOptions),
}

impl ReadArchive {
    /// Opens the Archive at `path`, or returns `None` when its first four bytes
    /// are not a TES3, TES4 or FO4 magic (bethutil's `read_archive` returning
    /// `nullopt`).
    ///
    /// # Errors
    ///
    /// [`ArchiveError::Io`] when the file cannot be opened, and
    /// [`ArchiveError::Tes3`], [`ArchiveError::Tes4`] or [`ArchiveError::Fo4`]
    /// when it has a known magic but does not parse.
    pub fn open(path: &Path) -> Result<Option<Self>, ArchiveError> {
        let mut file = FsFile::open(path).map_err(|source| ArchiveError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let Some(format) = ba2::guess_format(&mut file) else {
            return Ok(None);
        };
        // `ba2` maps the whole file from this handle; the bytes `guess_format`
        // consumed do not matter to a mapping.
        let inner = match format {
            FileFormat::TES3 => tes3::Archive::read(&file)
                .map(Inner::Tes3)
                .map_err(|source| ArchiveError::Tes3 {
                    path: path.to_path_buf(),
                    source,
                })?,
            FileFormat::TES4 => tes4::Archive::read(&file)
                .map(|(archive, options)| Inner::Tes4(archive, options))
                .map_err(|source| ArchiveError::Tes4 {
                    path: path.to_path_buf(),
                    source,
                })?,
            FileFormat::FO4 => fo4::Archive::read(&file)
                .map(|(archive, options)| Inner::Fo4(archive, options))
                .map_err(|source| ArchiveError::Fo4 {
                    path: path.to_path_buf(),
                    source,
                })?,
        };
        Ok(Some(Self {
            path: path.to_path_buf(),
            inner,
        }))
    }

    /// The container this Archive is, or `None` for one CAO never writes (TES3,
    /// BSA v103, or a `GNMF` BA2). Any BTDX version counts as FO4.
    pub fn version(&self) -> Option<ArchiveVersion> {
        match &self.inner {
            Inner::Tes3(_) => None,
            Inner::Tes4(_, options) => match options.version() {
                tes4::Version::v104 => Some(ArchiveVersion::Tes5),
                tes4::Version::v105 => Some(ArchiveVersion::Sse),
                tes4::Version::v103 => None,
            },
            Inner::Fo4(_, options) => match options.format() {
                fo4::Format::GNRL => Some(ArchiveVersion::Fo4),
                fo4::Format::DX10 => Some(ArchiveVersion::Fo4Dx),
                fo4::Format::GNMF => None,
            },
        }
    }

    /// The path this Archive was opened from.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The header facts of this Archive.
    pub fn header(&self) -> ArchiveHeader {
        match &self.inner {
            Inner::Tes3(_) => ArchiveHeader {
                format: ArchiveFormat::Tes3,
                version: 0,
                flags: 0,
                types: 0,
                container: None,
                name_table: true,
            },
            Inner::Tes4(_, options) => {
                let strings =
                    tes4::ArchiveFlags::DIRECTORY_STRINGS | tes4::ArchiveFlags::FILE_STRINGS;
                ArchiveHeader {
                    format: ArchiveFormat::Tes4,
                    version: options.version() as u32,
                    flags: options.flags().bits(),
                    types: u32::from(options.types().bits()),
                    container: None,
                    name_table: options.flags().contains(strings),
                }
            }
            Inner::Fo4(_, options) => ArchiveHeader {
                format: ArchiveFormat::Fo4,
                version: options.version() as u32,
                flags: 0,
                types: 0,
                container: Some(match options.format() {
                    fo4::Format::GNRL => Fo4Container::General,
                    fo4::Format::DX10 => Fo4Container::Dx10,
                    fo4::Format::GNMF => Fo4Container::Gnmf,
                }),
                name_table: options.strings(),
            },
        }
    }

    /// Lists every Archived Asset, in the Archive's own (hash) order.
    ///
    /// # Errors
    ///
    /// [`ArchiveError::InvalidArchivedAssetName`] for a name that is not UTF-8. C++ CAO's
    /// discovery failed such an Archive the same way (`ArchiveEntryInvalid`).
    pub fn archived_assets(&self) -> Result<Vec<ArchivedAsset>, ArchiveError> {
        let mut assets = Vec::new();
        match &self.inner {
            Inner::Tes3(archive) => {
                for (key, file) in archive {
                    assets.push(ArchivedAsset {
                        name: self.decode(key.name())?.to_owned(),
                        size: file.len() as u64,
                        compressed: false,
                        chunks: 1,
                        mip_ranges: Vec::new(),
                    });
                }
            }
            Inner::Tes4(archive, _) => {
                for (directory_key, directory) in archive {
                    let directory_name = self.decode(directory_key.name())?;
                    for (file_key, file) in directory {
                        let file_name = self.decode(file_key.name())?;
                        let name = if directory_name.is_empty() {
                            file_name.to_owned()
                        } else {
                            format!("{directory_name}\\{file_name}")
                        };
                        assets.push(ArchivedAsset {
                            name: name.replace('/', "\\"),
                            size: file.decompressed_len().unwrap_or(file.len()) as u64,
                            compressed: file.is_compressed(),
                            chunks: 1,
                            mip_ranges: Vec::new(),
                        });
                    }
                }
            }
            Inner::Fo4(archive, options) => {
                let header = if options.format() == fo4::Format::DX10 {
                    DX10_HEADER_LEN
                } else {
                    0
                };
                for (key, file) in archive {
                    let data: u64 = file
                        .iter()
                        .map(|chunk| chunk.decompressed_len().unwrap_or(chunk.len()) as u64)
                        .sum();
                    assets.push(ArchivedAsset {
                        name: self.decode(key.name())?.to_owned(),
                        size: data + header,
                        compressed: !file.is_empty()
                            && file.iter().all(|chunk| chunk.is_compressed()),
                        chunks: file.len(),
                        mip_ranges: file.iter().filter_map(|chunk| chunk.mips.clone()).collect(),
                    });
                }
            }
        }
        Ok(assets)
    }

    /// Writes the decompressed bytes of the Archived Asset `name` to `out`. A DX10 texture
    /// gets a rebuilt DDS header; a cubemap's lists all six faces, so DX10-only
    /// formats extract where `rsm-bsa` failed (deviation 7).
    ///
    /// `name` is looked up by its hash, so case and `/` versus `\` do not matter.
    ///
    /// # Errors
    ///
    /// [`ArchiveError::MissingArchivedAsset`] when nothing has that name, and
    /// [`ArchiveError::Tes3`], [`ArchiveError::Tes4`] or [`ArchiveError::Fo4`]
    /// when it cannot be decompressed or written.
    pub fn extract(&self, name: &str, out: &mut dyn Write) -> Result<(), ArchiveError> {
        let missing = || ArchiveError::MissingArchivedAsset {
            archive: self.path.clone(),
            name: name.to_owned(),
        };
        match &self.inner {
            Inner::Tes3(archive) => {
                let file = archive
                    .get(&tes3::ArchiveKey::from(name))
                    .ok_or_else(missing)?;
                file.write(out).map_err(|source| ArchiveError::Tes3 {
                    path: self.path.clone(),
                    source,
                })
            }
            Inner::Tes4(archive, options) => {
                let normalized = name.replace('/', "\\");
                let (directory, file) = normalized.rsplit_once('\\').unwrap_or(("", &normalized));
                let file = archive
                    .get(&tes4::ArchiveKey::from(directory))
                    .and_then(|directory| directory.get(&tes4::DirectoryKey::from(file)))
                    .ok_or_else(missing)?;
                file.write(out, &options.into())
                    .map_err(|source| ArchiveError::Tes4 {
                        path: self.path.clone(),
                        source,
                    })
            }
            Inner::Fo4(archive, options) => {
                let file = archive
                    .get(&fo4::ArchiveKey::from(name))
                    .ok_or_else(missing)?;
                file.write(out, &options.into())
                    .map_err(|source| ArchiveError::Fo4 {
                        path: self.path.clone(),
                        source,
                    })
            }
        }
    }

    /// Decodes a stored name as strict UTF-8.
    fn decode<'name>(&self, name: &'name ba2::BStr) -> Result<&'name str, ArchiveError> {
        std::str::from_utf8(name).map_err(|_| ArchiveError::InvalidArchivedAssetName {
            archive: self.path.clone(),
            name: String::from_utf8_lossy(name).into_owned(),
        })
    }
}
