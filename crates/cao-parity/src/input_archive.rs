//! Input Archives (#529): an `archive` recipe entry written with the pinned
//! `ba2` writer and explicit options, never with `cao-archive`'s
//! `write_archive`, so the Rust port's own packing code never builds the
//! inputs it is tested on. The oracle reads them with rsm-bsa, so any reader
//! disagreement still shows up as Different.
//!
//! - TES5 writes BSA v104 and SSE BSA v105, with directory and file strings,
//!   `COMPRESSED` when compressed, and no archive types.
//! - FO4 writes BTDX v1, `DX10` for `textures` and `GNRL` otherwise, with a
//!   name table, zlib at FO4's level and DX10 mips chunked at 512x512.
//! - **Verbatim names.** `ba2` stores every name lowercase and `\`-separated.
//!   A `raw` entry marked `verbatim` keeps its exact spelling: after writing,
//!   the stored name is overwritten in place, as the C++ tests'
//!   `createRawArchive` does (`tests/ArchiveFirstAssetDiscoveryTests.cpp`).
//!   The overwrite keeps every offset, so only a spelling of the stored
//!   name's own length can be written; the name's hash stays the stored
//!   name's, which is the hash of the spelling once a reader normalises it.

use std::collections::HashMap;

use ba2::prelude::*;
use ba2::{BStr, ByteSlice, CompressionResult, Copied, fo4, tes4};

use crate::recipe::{ArchiveGame, ArchiveKind, ArchiveRecipe};

/// One file to pack: its game path, its bytes, and whether the name is
/// stored exactly as spelled.
pub(crate) struct PackedFile {
    pub name: String,
    pub bytes: Vec<u8>,
    pub verbatim: bool,
}

/// Writes `files` as `archive`'s container and returns its bytes.
///
/// Files are compressed when `archive.compress` is set, except in an
/// `incompressible` Archive, and always in an FO4 `textures` one.
///
/// # Errors
/// A message when a file cannot be packed, such as a non-DDS file in a `DX10`
/// BA2; when two names are stored as one; or when a verbatim name cannot be
/// written over the stored one.
pub(crate) fn archive_bytes(
    archive: &ArchiveRecipe,
    files: &[PackedFile],
) -> Result<Vec<u8>, String> {
    let compressed = match (archive.game, archive.archive_type) {
        (ArchiveGame::Fo4, ArchiveKind::Textures) => true,
        (_, ArchiveKind::Incompressible) => false,
        _ => archive.compress,
    };
    let result = if compressed {
        CompressionResult::Compressed
    } else {
        CompressionResult::Decompressed
    };
    match archive.game {
        ArchiveGame::Tes5 | ArchiveGame::Sse => {
            let version = if archive.game == ArchiveGame::Tes5 {
                tes4::Version::v104
            } else {
                tes4::Version::v105
            };
            tes4_bytes(files, version, result)
        }
        ArchiveGame::Fo4 => {
            let format = if archive.archive_type == ArchiveKind::Textures {
                fo4::Format::DX10
            } else {
                fo4::Format::GNRL
            };
            fo4_bytes(files, format, result)
        }
    }
}

/// A BSA's directory and file name: the parts of `name` before and after its
/// last separator, either `/` or `\`. A root-level file's directory is empty,
/// which `ba2` stores as `.`.
fn split(name: &str) -> (&str, &str) {
    name.rfind(['/', '\\'])
        .map_or(("", name), |at| (&name[..at], &name[at + 1..]))
}

/// The bytes a name ends up holding: `written` itself when `verbatim`, or
/// else `stored`, the name `ba2` stores for it.
///
/// # Errors
/// A message when a verbatim `written` is not `stored`'s length, so it
/// cannot be written over it.
fn spelling(verbatim: bool, written: &str, stored: &BStr) -> Result<Vec<u8>, String> {
    if !verbatim {
        Ok(stored.to_vec())
    } else if written.len() == stored.len() {
        Ok(written.as_bytes().to_vec())
    } else {
        Err(format!(
            "`ba2` stores `{written}` as `{stored}`, a name of another length, so it cannot \
             be written verbatim"
        ))
    }
}

/// Writes a BSA of `version`, then spells each name as asked.
fn tes4_bytes(
    files: &[PackedFile],
    version: tes4::Version,
    result: CompressionResult,
) -> Result<Vec<u8>, String> {
    let read_options = tes4::FileReadOptions::builder()
        .version(version)
        .compression_result(result)
        .build();
    let mut archive = tes4::Archive::new();
    // Each stored directory and (directory, file) name, with the spelling
    // its bytes end up holding.
    let mut directories: HashMap<Vec<u8>, Vec<u8>> = HashMap::new();
    let mut names: HashMap<(Vec<u8>, Vec<u8>), Vec<u8>> = HashMap::new();
    for file in files {
        let (directory, name) = split(&file.name);
        let directory_key = tes4::ArchiveKey::from(directory);
        let file_key = tes4::DirectoryKey::from(name);
        let stored_directory = directory_key.name().to_vec();
        let stored_name = file_key.name().to_vec();
        // A root-level file's empty directory is stored as `.`, which no
        // spelling replaces.
        let directory_spelling = spelling(
            file.verbatim && !directory.is_empty(),
            directory,
            directory_key.name(),
        )?;
        let name_spelling = spelling(file.verbatim, name, file_key.name())?;
        // One directory record holds one spelling for all its files.
        match directories.get(&stored_directory) {
            Some(known) if *known != directory_spelling => {
                return Err(format!(
                    "the directory `{}` is spelled both `{}` and `{}`",
                    stored_directory.as_bstr(),
                    known.as_bstr(),
                    directory_spelling.as_bstr()
                ));
            }
            _ => {
                directories.insert(stored_directory.clone(), directory_spelling);
            }
        }
        let key = (stored_directory, stored_name);
        if names.contains_key(&key) {
            return Err(format!(
                "`{}` is stored as `{}\\{}`, as another file is",
                file.name,
                key.0.as_bstr(),
                key.1.as_bstr()
            ));
        }
        names.insert(key, name_spelling);

        let data = tes4::File::read(Copied(&file.bytes), &read_options)
            .map_err(|error| format!("`{}`: {error}", file.name))?;
        match archive.get_mut(&directory_key) {
            Some(directory) => {
                directory.insert(file_key, data);
            }
            None => {
                let directory: tes4::Directory = [(file_key, data)].into_iter().collect();
                archive.insert(directory_key, directory);
            }
        }
    }
    let mut flags = tes4::ArchiveFlags::DIRECTORY_STRINGS | tes4::ArchiveFlags::FILE_STRINGS;
    if result == CompressionResult::Compressed {
        flags |= tes4::ArchiveFlags::COMPRESSED;
    }
    let options = tes4::ArchiveOptions::builder()
        .version(version)
        .flags(flags)
        .types(tes4::ArchiveTypes::empty())
        .build();
    let mut bytes = Vec::new();
    archive
        .write(&mut bytes, &options)
        .map_err(|error| error.to_string())?;
    respell_tes4(&mut bytes, &directories, &names)?;
    Ok(bytes)
}

/// The little-endian `u32` at `at`, or an error naming a short Archive.
fn u32_at(bytes: &[u8], at: usize) -> Result<usize, String> {
    bytes
        .get(at..at + 4)
        .map(|word| u32::from_le_bytes(word.try_into().expect("four bytes")) as usize)
        .ok_or_else(|| "the written Archive is shorter than its own layout".into())
}

/// Overwrites `bytes[at..]`, which holds `stored`, with `spelling`, which is
/// as long.
fn overwrite(bytes: &mut [u8], at: usize, stored: &[u8], spelling: &[u8]) -> Result<(), String> {
    let slot = bytes
        .get_mut(at..at + stored.len())
        .filter(|slot| *slot == stored)
        .ok_or_else(|| format!("`{}` is not where `ba2` lays it out", stored.as_bstr()))?;
    slot.copy_from_slice(spelling);
    Ok(())
}

/// Spells a written BSA's directory and file names as `directories` and
/// `names` ask, walking `ba2`'s layout: the 36-byte header, one directory
/// record per directory (16 bytes in v104, 24 in v105, each with its file
/// count at byte 8), then per directory its `bzstring` name and 16-byte file
/// records, then every file's NUL-terminated name in the same order.
fn respell_tes4(
    bytes: &mut [u8],
    directories: &HashMap<Vec<u8>, Vec<u8>>,
    names: &HashMap<(Vec<u8>, Vec<u8>), Vec<u8>>,
) -> Result<(), String> {
    let record_size = if u32_at(bytes, 4)? == 105 { 24 } else { 16 };
    let directory_count = u32_at(bytes, 16)?;
    let mut counts = Vec::with_capacity(directory_count);
    for index in 0..directory_count {
        counts.push(u32_at(bytes, 36 + index * record_size + 8)?);
    }
    let mut at = 36 + directory_count * record_size;
    let mut order = Vec::new();
    for &count in &counts {
        let length = usize::from(*bytes.get(at).ok_or("a short directory name")?);
        // The prefix counts the name and its NUL.
        let stored = bytes
            .get(at + 1..at + length)
            .ok_or("a short directory name")?
            .to_vec();
        if let Some(spelling) = directories.get(&stored).filter(|s| **s != stored) {
            overwrite(bytes, at + 1, &stored, spelling)?;
        }
        order.extend(std::iter::repeat_n(stored, count));
        at += 1 + length + 16 * count;
    }
    for directory in order {
        let end = at
            + bytes[at..]
                .iter()
                .position(|&byte| byte == 0)
                .ok_or("a short file name")?;
        let stored = bytes[at..end].to_vec();
        if let Some(spelling) = names
            .get(&(directory, stored.clone()))
            .filter(|s| **s != stored)
        {
            overwrite(bytes, at, &stored, spelling)?;
        }
        at = end + 1;
    }
    Ok(())
}

/// Writes a BA2 of `format`, then spells each name as asked.
fn fo4_bytes(
    files: &[PackedFile],
    format: fo4::Format,
    result: CompressionResult,
) -> Result<Vec<u8>, String> {
    let read_options = fo4::FileReadOptions::builder()
        .format(format)
        .compression_format(fo4::CompressionFormat::Zip)
        .compression_level(fo4::CompressionLevel::FO4)
        .compression_result(result)
        .mip_chunk_width(512)
        .mip_chunk_height(512)
        .build();
    let mut archive = fo4::Archive::new();
    let mut names: HashMap<Vec<u8>, Vec<u8>> = HashMap::new();
    for file in files {
        let key = fo4::ArchiveKey::from(file.name.as_str());
        let stored = key.name().to_vec();
        if names.contains_key(&stored) {
            return Err(format!(
                "`{}` is stored as `{}`, as another file is",
                file.name,
                stored.as_bstr()
            ));
        }
        let spelling = spelling(file.verbatim, &file.name, key.name())?;
        names.insert(stored, spelling);
        let data = fo4::File::read(Copied(&file.bytes), &read_options)
            .map_err(|error| format!("`{}`: {error}", file.name))?;
        archive.insert(key, data);
    }
    let options = fo4::ArchiveOptions::builder()
        .format(format)
        .version(fo4::Version::v1)
        .strings(true)
        .compression_format(fo4::CompressionFormat::Zip)
        .build();
    let mut bytes = Vec::new();
    archive
        .write(&mut bytes, &options)
        .map_err(|error| error.to_string())?;
    respell_fo4(&mut bytes, &names)?;
    Ok(bytes)
}

/// Spells a written BA2's names as `names` asks, walking its name table: the
/// file count at byte 12, the table's offset at byte 16, then one name per
/// file, each a `u16` length and its bytes.
fn respell_fo4(bytes: &mut [u8], names: &HashMap<Vec<u8>, Vec<u8>>) -> Result<(), String> {
    let count = u32_at(bytes, 12)?;
    let offset = bytes
        .get(16..24)
        .map(|word| u64::from_le_bytes(word.try_into().expect("eight bytes")))
        .ok_or("a short BA2 header")?;
    let mut at = usize::try_from(offset).map_err(|error| error.to_string())?;
    for _ in 0..count {
        let length = bytes
            .get(at..at + 2)
            .map(|word| usize::from(u16::from_le_bytes([word[0], word[1]])))
            .ok_or("a short name table")?;
        let stored = bytes
            .get(at + 2..at + 2 + length)
            .ok_or("a short name table")?
            .to_vec();
        if let Some(spelling) = names.get(&stored).filter(|s| **s != stored) {
            overwrite(bytes, at + 2, &stored, spelling)?;
        }
        at += 2 + length;
    }
    Ok(())
}
