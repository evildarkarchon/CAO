//! Writing one planned Archive (bethutil's `write` and `write_archive`) with
//! `ba2`, setting explicitly every option whose `ba2` default differs from
//! `rsm-bsa`'s (#461).
//!
//! Sources: `src/bsa/pack.cpp` and `src/bsa/archive.cpp` at bethutil `81f882ed`.

use std::fs::File as FsFile;
use std::io::{BufWriter, Write as _};
use std::path::Path;

use ba2::prelude::*;
use ba2::{CompressionResult, fo4, tes4};
use rayon::prelude::*;

use crate::data::{ArchiveData, ArchiveType};
use crate::error::ArchiveError;
use crate::settings::ArchiveVersion;

/// Writes `data`'s files, which live under the Mod Root `root`, as a new Archive
/// at `out_path` (bethutil's `write(compress, data, root)`).
///
/// Nothing is written for an Archive with no files. Otherwise:
///
/// - Files are compressed when `compress` is set, except in an Incompressible
///   Archive, and always in an FO4 DX10 Textures BA2.
/// - Each file is keyed by its path relative to `root`, with backslashes; `ba2`
///   stores names lowercase.
/// - TES5 and SSE write BSA v104 and v105 with directory and file strings, plus
///   `COMPRESSED` when compressed, and no archive types. FO4 writes BTDX v1 `GNRL`
///   or `DX10` with a name table, zlib at FO4's level, and DX10 mips chunked at
///   512x512 (`ba2`'s defaults would write no name table).
/// - Files are read and compressed in parallel.
///
/// The output is created new, never overwriting an existing file. On any error
/// after creating it, the partial output is removed. Every source mapping is
/// released before this returns, so the sources can be deleted straight away.
///
/// # Errors
///
/// - [`ArchiveError::Io`] when `out_path` exists or cannot be written.
/// - [`ArchiveError::OutsideRoot`] or [`ArchiveError::NonUnicodeName`] when a file
///   has no usable key.
/// - [`ArchiveError::Tes4`] or [`ArchiveError::Fo4`] when a file cannot be read
///   or compressed (for example, a DDS that DirectXTex cannot load), or the
///   Archive cannot be written.
/// - [`ArchiveError::ArchiveTooLarge`] (**deviation 8**) when a BSA would not fit
///   its 32-bit sizes and offsets, which `rsm-bsa` silently wrapped. BA2s have
///   64-bit offsets and no such limit.
pub fn write_archive(
    compress: bool,
    data: &ArchiveData,
    root: &Path,
    out_path: &Path,
) -> Result<(), ArchiveError> {
    if data.is_empty() {
        return Ok(());
    }
    let version = data.version();
    let compressed = (compress && data.archive_type() != ArchiveType::Incompressible)
        || version == ArchiveVersion::Fo4Dx;
    let compression_result = if compressed {
        CompressionResult::Compressed
    } else {
        CompressionResult::Decompressed
    };
    let sources = data
        .files()
        .iter()
        .map(|path| Ok((archive_key(path, root)?, path.as_path())))
        .collect::<Result<Vec<_>, ArchiveError>>()?;

    match version {
        ArchiveVersion::Tes5 | ArchiveVersion::Sse => {
            let version = if version == ArchiveVersion::Tes5 {
                tes4::Version::v104
            } else {
                tes4::Version::v105
            };
            let read_options = tes4::FileReadOptions::builder()
                .version(version)
                .compression_result(compression_result)
                .build();
            let files = sources
                .into_par_iter()
                .map(|(key, path)| {
                    tes4::File::read(path, &read_options)
                        .map(|file| (key, file))
                        .map_err(|source| ArchiveError::Tes4 {
                            path: path.to_path_buf(),
                            source,
                        })
                })
                .collect::<Result<Vec<_>, _>>()?;
            let mut flags =
                tes4::ArchiveFlags::DIRECTORY_STRINGS | tes4::ArchiveFlags::FILE_STRINGS;
            if compressed {
                flags |= tes4::ArchiveFlags::COMPRESSED;
            }
            let options = tes4::ArchiveOptions::builder()
                .version(version)
                .flags(flags)
                .types(tes4::ArchiveTypes::empty())
                .build();
            write_tes4(&tes4_archive(files), &options, out_path)
        }
        ArchiveVersion::Fo4 | ArchiveVersion::Fo4Dx => {
            let format = if version == ArchiveVersion::Fo4Dx {
                fo4::Format::DX10
            } else {
                fo4::Format::GNRL
            };
            let read_options = fo4::FileReadOptions::builder()
                .format(format)
                .compression_format(fo4::CompressionFormat::Zip)
                .compression_level(fo4::CompressionLevel::FO4)
                .compression_result(compression_result)
                .mip_chunk_width(512)
                .mip_chunk_height(512)
                .build();
            let files = sources
                .into_par_iter()
                .map(|(key, path)| {
                    fo4::File::read(path, &read_options)
                        .map(|file| (key, file))
                        .map_err(|source| ArchiveError::Fo4 {
                            path: path.to_path_buf(),
                            source,
                        })
                })
                .collect::<Result<Vec<_>, _>>()?;
            let mut archive = fo4::Archive::new();
            for (key, file) in files {
                let key = fo4::ArchiveKey::from(key);
                // Keep the first file for a name, as the sorted sources give it.
                if archive.get(&key).is_none() {
                    archive.insert(key, file);
                }
            }
            let options = fo4::ArchiveOptions::builder()
                .format(format)
                .version(fo4::Version::v1)
                .strings(true)
                .compression_format(fo4::CompressionFormat::Zip)
                .build();
            // BA2 data offsets are 64-bit, so a BA2 past 4 GiB is valid (C++ wrote
            // one too) and deviation 8 does not apply; any `ba2` error is reported
            // as it is.
            write_new_file(out_path, |out| {
                archive
                    .write(out, &options)
                    .map_err(|source| ArchiveError::Fo4 {
                        path: out_path.to_path_buf(),
                        source,
                    })
            })
        }
    }
}

/// The name a source is stored under: its path relative to `root`, with
/// backslashes, as C++ took `lexically_relative(root).u8string()`.
fn archive_key(path: &Path, root: &Path) -> Result<String, ArchiveError> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| ArchiveError::OutsideRoot {
            path: path.to_path_buf(),
            root: root.to_path_buf(),
        })?;
    let relative = relative
        .to_str()
        .ok_or_else(|| ArchiveError::NonUnicodeName {
            path: path.to_path_buf(),
        })?;
    Ok(relative.replace('/', "\\"))
}

/// Groups keyed files into a TES4 archive: the directory is everything before the
/// last backslash (empty for a root-level file), the file name the rest. The first
/// file for a name wins.
fn tes4_archive(files: Vec<(String, tes4::File<'static>)>) -> tes4::Archive<'static> {
    let mut archive = tes4::Archive::new();
    for (key, file) in files {
        let (directory, name) = key.rsplit_once('\\').unwrap_or(("", &key));
        let directory_key = tes4::ArchiveKey::from(directory);
        let file_key = tes4::DirectoryKey::from(name);
        match archive.get_mut(&directory_key) {
            Some(directory) => {
                if directory.get(&file_key).is_none() {
                    directory.insert(file_key, file);
                }
            }
            None => {
                let directory: tes4::Directory = [(file_key, file)].into_iter().collect();
                archive.insert(directory_key, directory);
            }
        }
    }
    archive
}

/// Writes a TES4 archive to a new file at `out_path`.
///
/// **Deviation 8:** `ba2` keeps every TES4 offset in a `u32` and reports
/// `IntegralOverflow` past 4 GiB (or `IntegralTruncation` for a file of 1 GiB or
/// more, whose size bits would collide with the entry flags), where `rsm-bsa`
/// silently wrapped and wrote a corrupt BSA. Both become
/// [`ArchiveError::ArchiveTooLarge`]. `ba2` writes every file entry before any
/// file data, so the overflow is found before any data is written.
fn write_tes4(
    archive: &tes4::Archive<'_>,
    options: &tes4::ArchiveOptions,
    out_path: &Path,
) -> Result<(), ArchiveError> {
    write_new_file(out_path, |out| {
        archive.write(out, options).map_err(|source| match source {
            tes4::Error::IntegralOverflow | tes4::Error::IntegralTruncation => {
                ArchiveError::ArchiveTooLarge {
                    path: out_path.to_path_buf(),
                }
            }
            source => ArchiveError::Tes4 {
                path: out_path.to_path_buf(),
                source,
            },
        })
    })
}

/// Creates `out_path`, failing if it exists, and fills it with `write`. On any
/// error the partial file is removed: this call created it, so it is ours.
fn write_new_file(
    out_path: &Path,
    write: impl FnOnce(&mut BufWriter<FsFile>) -> Result<(), ArchiveError>,
) -> Result<(), ArchiveError> {
    let io_error = |source| ArchiveError::Io {
        path: out_path.to_path_buf(),
        source,
    };
    let mut out = BufWriter::new(FsFile::create_new(out_path).map_err(io_error)?);
    let result = write(&mut out).and_then(|()| out.flush().map_err(io_error));
    // Close the handle before any removal below.
    drop(out);
    if result.is_err() {
        // The write error is the one worth reporting; a failed removal leaves a
        // partial file the caller's staging cleanup still owns.
        let _ = std::fs::remove_file(out_path);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A fresh output path in the temp directory, unique to this test process.
    fn scratch_output(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("cao-archive-{}-{name}", std::process::id()));
        // A missing file is the expected case.
        let _ = std::fs::remove_file(&path);
        path
    }

    /// 64 MiB: 65 of these pass 4 GiB, 63 do not.
    const BLOCK: usize = 64 << 20;

    /// A v105 BSA of `count` uncompressed files that all borrow `block`, so a
    /// multi-GiB layout costs one buffer. `vec!` zeroes lazily, so even a large
    /// `block` is never touched while `ba2` lays the archive out.
    fn borrowed_bsa(block: &[u8], count: usize) -> (tes4::Archive<'_>, tes4::ArchiveOptions) {
        let directory: tes4::Directory = (0..count)
            .map(|index| {
                (
                    tes4::DirectoryKey::from(format!("{index}.nif")),
                    tes4::File::from_decompressed(block),
                )
            })
            .collect();
        let archive = [(tes4::ArchiveKey::from("meshes"), directory)]
            .into_iter()
            .collect();
        let options = tes4::ArchiveOptions::builder()
            .version(tes4::Version::v105)
            .flags(tes4::ArchiveFlags::DIRECTORY_STRINGS | tes4::ArchiveFlags::FILE_STRINGS)
            .build();
        (archive, options)
    }

    /// Asserts writing `archive` to a new file fails as too large and leaves no file.
    fn assert_too_large(archive: &tes4::Archive<'_>, options: &tes4::ArchiveOptions, name: &str) {
        let out = scratch_output(name);
        let error = write_tes4(archive, options, &out).unwrap_err();
        assert!(
            matches!(&error, ArchiveError::ArchiveTooLarge { path } if *path == out),
            "{error:?}"
        );
        assert!(!out.exists());
    }

    /// **Deviation 8:** a BSA that would pass 4 GiB fails with an error and leaves
    /// no file, where C++ wrote a corrupt one. Driven with borrowed in-memory files,
    /// since `ba2` fails before writing any data: 65 views of one 64 MiB buffer
    /// make 4160 MiB.
    #[test]
    fn deviation_8_a_bsa_over_4_gib_is_an_error_and_leaves_no_file() {
        let block = vec![0u8; BLOCK];
        let (archive, options) = borrowed_bsa(&block, 65);
        assert_too_large(&archive, &options, "deviation-8.bsa");
    }

    /// The same files, two fewer, fit: the error is the 4 GiB limit, not the files.
    #[test]
    fn a_bsa_just_under_4_gib_lays_out_without_error() {
        let block = vec![0u8; BLOCK];
        let (archive, options) = borrowed_bsa(&block, 63);
        // 4032 MiB to a sink: nothing reaches the disk.
        archive.write(&mut std::io::sink(), &options).unwrap();
    }

    /// **Deviation 8:** one file of 1 GiB would set the size's flag bits, which
    /// `rsm-bsa` also wrapped silently.
    #[test]
    fn deviation_8_a_bsa_file_of_1_gib_is_an_error() {
        let block = vec![0u8; 1 << 30];
        let (archive, options) = borrowed_bsa(&block, 1);
        assert_too_large(&archive, &options, "deviation-8-file.bsa");
    }
}
