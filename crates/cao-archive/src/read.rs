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
    /// The name as stored, decoded for display and as a Windows path. TES4
    /// names join directory and file with a backslash and map any `/` to
    /// `\`; FO4 and TES3 names are kept as stored.
    ///
    /// Stored names are bytes. Each name (for TES4, its directory and its
    /// file name separately) is decoded as UTF-8 when it is valid UTF-8 and
    /// as Windows-1252 otherwise, which an author's accented names usually
    /// are. Every byte decodes, so no name makes the Archive unreadable;
    /// [`ReadArchive::extract`] finds the entry again by its stored bytes.
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

/// What an Archive's header says, beyond its entries.
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

    /// Lists every Archived Asset, in the Archive's own (hash) order, with
    /// each name decoded as [`ArchivedAsset::name`] describes.
    ///
    /// # Errors
    ///
    /// None at present; the `Result` keeps room for a listing that can fail.
    pub fn archived_assets(&self) -> Result<Vec<ArchivedAsset>, ArchiveError> {
        let mut assets = Vec::new();
        match &self.inner {
            Inner::Tes3(archive) => {
                for (key, file) in archive {
                    assets.push(ArchivedAsset {
                        name: decode(key.name()),
                        size: file.len() as u64,
                        compressed: false,
                        chunks: 1,
                        mip_ranges: Vec::new(),
                    });
                }
            }
            Inner::Tes4(archive, _) => {
                for (directory_key, directory) in archive {
                    let directory_name = decode(directory_key.name());
                    for (file_key, file) in directory {
                        let file_name = decode(file_key.name());
                        let name = if directory_name.is_empty() {
                            file_name
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
                        name: decode(key.name()),
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
    /// A name [`Self::archived_assets`] decoded from Windows-1252 is found by
    /// its stored bytes: each name (for TES4, its directory and its file name)
    /// is tried as UTF-8 first and then encoded back to Windows-1252.
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
                let file = stored_names(name)
                    .find_map(|stored| archive.get(&tes3::ArchiveKey::from(stored)))
                    .ok_or_else(missing)?;
                file.write(out).map_err(|source| ArchiveError::Tes3 {
                    path: self.path.clone(),
                    source,
                })
            }
            Inner::Tes4(archive, options) => {
                let normalized = name.replace('/', "\\");
                let (directory, file) = normalized.rsplit_once('\\').unwrap_or(("", &normalized));
                let file = stored_names(directory)
                    .find_map(|stored| archive.get(&tes4::ArchiveKey::from(stored)))
                    .and_then(|directory| {
                        stored_names(file)
                            .find_map(|stored| directory.get(&tes4::DirectoryKey::from(stored)))
                    })
                    .ok_or_else(missing)?;
                file.write(out, &options.into())
                    .map_err(|source| ArchiveError::Tes4 {
                        path: self.path.clone(),
                        source,
                    })
            }
            Inner::Fo4(archive, options) => {
                let file = stored_names(name)
                    .find_map(|stored| archive.get(&fo4::ArchiveKey::from(stored)))
                    .ok_or_else(missing)?;
                file.write(out, &options.into())
                    .map_err(|source| ArchiveError::Fo4 {
                        path: self.path.clone(),
                        source,
                    })
            }
        }
    }
}

/// The characters Windows-1252 assigns to bytes 0x80 to 0x9F, as the WHATWG
/// Encoding Standard decodes them. The five bytes the code page leaves
/// undefined decode to the C1 control of the same value, so every byte has
/// exactly one character and the decoding can be reversed.
const WINDOWS_1252_HIGH: [char; 32] = [
    '\u{20AC}', '\u{0081}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{008D}', '\u{017D}', '\u{008F}',
    '\u{0090}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}', '\u{0153}', '\u{009D}', '\u{017E}', '\u{0178}',
];

/// Decodes a stored name: as UTF-8 when it is valid UTF-8, and otherwise as
/// Windows-1252, which never fails.
///
/// A name that is neither, such as one written in the Cyrillic code page,
/// still decodes, to the Windows-1252 reading of its bytes. Whether that is
/// a safe Windows path is for the caller's path rules to decide.
fn decode(name: &ba2::BStr) -> String {
    match std::str::from_utf8(name) {
        Ok(text) => text.to_owned(),
        Err(_) => name
            .iter()
            .map(|&byte| match byte {
                0x80..=0x9F => WINDOWS_1252_HIGH[usize::from(byte - 0x80)],
                _ => char::from(byte),
            })
            .collect(),
    }
}

/// The Windows-1252 bytes of `name`, or `None` when a character has none.
/// The inverse of [`decode`]'s fallback.
fn encode_windows_1252(name: &str) -> Option<Vec<u8>> {
    name.chars()
        .map(|character| match u32::from(character) {
            code @ (0..=0x7F | 0xA0..=0xFF) => u8::try_from(code).ok(),
            _ => WINDOWS_1252_HIGH
                .iter()
                .position(|&high| high == character)
                .and_then(|index| u8::try_from(index + 0x80).ok()),
        })
        .collect()
}

/// The stored bytes `name` may have come from, in the order to try them:
/// its UTF-8 bytes, then, for a name that is not plain ASCII, its
/// Windows-1252 bytes when it has them.
fn stored_names(name: &str) -> impl Iterator<Item = Vec<u8>> {
    let utf8 = name.as_bytes().to_vec();
    let windows_1252 = if name.is_ascii() {
        None
    } else {
        encode_windows_1252(name)
    };
    std::iter::once(utf8).chain(windows_1252)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every byte decodes to one character and encodes back to itself, so a
    /// Windows-1252 name is always found again by its stored bytes.
    #[test]
    fn windows_1252_round_trips_every_byte() {
        let bytes: Vec<u8> = (0x80..=0xFF).collect();
        let decoded = decode(ba2::BStr::new(&bytes));
        assert_eq!(decoded.chars().count(), bytes.len());
        assert_eq!(encode_windows_1252(&decoded), Some(bytes));
        assert_eq!(decode(ba2::BStr::new(b"caf\xE9.dds")), "café.dds");
        assert_eq!(decode(ba2::BStr::new(b"\x80")), "€");
    }

    /// Valid UTF-8 is read as UTF-8, and its UTF-8 bytes are tried first.
    #[test]
    fn utf8_names_stay_utf8() {
        assert_eq!(decode(ba2::BStr::new("café.dds".as_bytes())), "café.dds");
        let tried: Vec<Vec<u8>> = stored_names("café.dds").collect();
        assert_eq!(
            tried,
            [b"caf\xC3\xA9.dds".to_vec(), b"caf\xE9.dds".to_vec()]
        );
        assert_eq!(stored_names("plain.dds").count(), 1);
        // A character Windows-1252 cannot spell is only tried as UTF-8.
        assert_eq!(stored_names("кот.dds").count(), 1);
    }
}
