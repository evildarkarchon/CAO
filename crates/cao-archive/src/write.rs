//! Writing one planned Archive (bethutil's `write` and `write_archive`) with
//! `ba2`, setting explicitly every option whose `ba2` default differs from
//! `rsm-bsa`'s (#461).
//!
//! Sources: `src/bsa/pack.cpp` and `src/bsa/archive.cpp` at bethutil `81f882ed`.

use std::fs::File as FsFile;
use std::io::{BufWriter, Write};
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
///   Archive, and always in an FO4 DX10 Textures BA2. In a BSA, a file is only
///   compressed where that stores it in fewer bytes; see [`smaller_stored`].
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
/// - [`ArchiveError::ArchiveOverSizeCap`] when a BSA would pass its game's size
///   cap, which planning never lets happen; see [`check_size_cap`]. Nothing is
///   written.
pub fn write_archive(
    compress: bool,
    data: &ArchiveData,
    root: &Path,
    out_path: &Path,
) -> Result<(), ArchiveError> {
    if data.is_empty() {
        return Ok(());
    }
    let prepared = prepare(compress, data, root)?;
    write_new_file(out_path, |out| prepared.write(out, out_path))
}

/// Writes `data`'s files as [`write_archive`] does, but into `out`, an output
/// its caller already created and owns, such as a file CAO staged under
/// Temporary Ownership. `out_path` only names the output in errors.
///
/// Every file is read and compressed before the first byte reaches `out`, and
/// every source mapping is released before this returns. Nothing is written
/// for an Archive with no files. On an error, `out` may hold a partial
/// Archive; removing it is the caller's.
///
/// # Errors
///
/// As [`write_archive`], except that `out` is never created here, and a write
/// to it fails as [`ArchiveError::Tes4`] or [`ArchiveError::Fo4`].
pub fn write_archive_into<W: Write>(
    compress: bool,
    data: &ArchiveData,
    root: &Path,
    out: &mut W,
    out_path: &Path,
) -> Result<(), ArchiveError> {
    if data.is_empty() {
        return Ok(());
    }
    prepare(compress, data, root)?.write(out, out_path)
}

/// An Archive read and compressed in memory, with the options it is written with.
/// A BSA also carries its game's size cap, if any.
enum PreparedArchive {
    Tes4(tes4::Archive<'static>, tes4::ArchiveOptions, Option<u64>),
    Fo4(fo4::Archive<'static>, fo4::ArchiveOptions),
}

impl PreparedArchive {
    /// Writes the Archive to `out`; `out_path` names it in errors.
    fn write<W: Write>(&self, out: &mut W, out_path: &Path) -> Result<(), ArchiveError> {
        match self {
            Self::Tes4(archive, options, cap) => {
                if let Some(cap) = *cap {
                    check_size_cap(archive, options, cap, out_path)?;
                }
                tes4_write(archive, options, out, out_path)
            }
            // BA2 data offsets are 64-bit, so a BA2 past 4 GiB is valid (C++
            // wrote one too) and deviation 8 does not apply; any `ba2` error is
            // reported as it is.
            Self::Fo4(archive, options) => {
                archive
                    .write(out, options)
                    .map_err(|source| ArchiveError::Fo4 {
                        path: out_path.to_path_buf(),
                        source,
                    })
            }
        }
    }
}

/// Reads and compresses `data`'s files, which live under `root`, in parallel,
/// and builds the Archive with every option set explicitly.
fn prepare(
    compress: bool,
    data: &ArchiveData,
    root: &Path,
) -> Result<PreparedArchive, ArchiveError> {
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
            // Read as they are; `smaller_stored` then compresses each file only
            // where that shrinks it.
            let read_options = tes4::FileReadOptions::builder()
                .version(version)
                .compression_result(CompressionResult::Decompressed)
                .build();
            let compression = tes4::FileCompressionOptions::builder()
                .version(version)
                .build();
            let files = sources
                .into_par_iter()
                .map(|(key, path)| {
                    let tes4_error = |source| ArchiveError::Tes4 {
                        path: path.to_path_buf(),
                        source,
                    };
                    let file = tes4::File::read(path, &read_options).map_err(tes4_error)?;
                    let file = if compressed {
                        smaller_stored(file, &compression).map_err(tes4_error)?
                    } else {
                        file
                    };
                    Ok((key, file))
                })
                .collect::<Result<Vec<_>, ArchiveError>>()?;
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
            Ok(PreparedArchive::Tes4(
                tes4_archive(files),
                options,
                data.size_cap(),
            ))
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
            Ok(PreparedArchive::Fo4(archive, options))
        }
    }
}

/// `file`, compressed when that stores it in fewer bytes, and otherwise as it
/// is.
///
/// A compressed BSA file is its compressed bytes plus a 4-byte original size,
/// and zlib or LZ4 can grow data that is already dense. A file stored as it is
/// carries the per-file toggle bit, which the game reads as "not compressed" in
/// a compressed Archive. Keeping the smaller form means no file ever takes more
/// bytes than its source, which is what lets planning bound a BSA by its
/// sources' sizes and keep it within its game's size cap.
fn smaller_stored(
    file: tes4::File<'static>,
    options: &tes4::FileCompressionOptions,
) -> Result<tes4::File<'static>, tes4::Error> {
    let packed = file.compress(options)?;
    // The original-size prefix every compressed file carries.
    let prefix = std::mem::size_of::<u32>();
    Ok(if packed.len() + prefix < file.len() {
        packed
    } else {
        file
    })
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

/// Writes a TES4 archive to `out`; `out_path` names it in errors.
///
/// **Deviation 8:** `ba2` keeps every TES4 offset in a `u32` and reports
/// `IntegralOverflow` past 4 GiB (or `IntegralTruncation` for a file of 1 GiB or
/// more, whose size bits would collide with the entry flags), where `rsm-bsa`
/// silently wrapped and wrote a corrupt BSA. Both become
/// [`ArchiveError::ArchiveTooLarge`]. `ba2` writes every file entry before any
/// file data, so the overflow is found before any data is written.
fn tes4_write<W: Write>(
    archive: &tes4::Archive<'_>,
    options: &tes4::ArchiveOptions,
    out: &mut W,
    out_path: &Path,
) -> Result<(), ArchiveError> {
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
}

/// Refuses a BSA that would take more than `cap` bytes, before any byte reaches
/// its output.
///
/// Planning keeps every BSA within its game's size cap, so this never fires
/// for an Archive CAO planned, unless that plan's bound is wrong. It is the
/// guarantee that no BSA past the cap is ever written: the Archive is measured
/// by writing it into a counter, which copies no data, so it costs a fraction
/// of reading and compressing the files.
fn check_size_cap(
    archive: &tes4::Archive<'_>,
    options: &tes4::ArchiveOptions,
    cap: u64,
    out_path: &Path,
) -> Result<(), ArchiveError> {
    let mut counter = ByteCounter(0);
    tes4_write(archive, options, &mut counter, out_path)?;
    if counter.0 > cap {
        return Err(ArchiveError::ArchiveOverSizeCap {
            path: out_path.to_path_buf(),
            size: counter.0,
            cap,
        });
    }
    Ok(())
}

/// A [`Write`] that only counts the bytes written to it.
struct ByteCounter(u64);

impl Write for ByteCounter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0 += buf.len() as u64;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Writes a TES4 archive to a new file at `out_path`, as [`write_archive`]
/// does after reading its files.
#[cfg(test)]
fn write_tes4(
    archive: &tes4::Archive<'_>,
    options: &tes4::ArchiveOptions,
    out_path: &Path,
) -> Result<(), ArchiveError> {
    write_new_file(out_path, |out| tes4_write(archive, options, out, out_path))
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

    /// The size-cap check measures exactly the bytes the BSA is written as, and
    /// refuses it one byte under that.
    #[test]
    fn the_size_cap_check_refuses_a_bsa_one_byte_past_its_cap() {
        let block = vec![7u8; 100];
        let (archive, options) = borrowed_bsa(&block, 3);
        let out = scratch_output("size-cap.bsa");
        write_tes4(&archive, &options, &out).unwrap();
        let written = std::fs::metadata(&out).unwrap().len();
        // A file removed by the test's end is the expected case.
        let _ = std::fs::remove_file(&out);

        check_size_cap(&archive, &options, written, &out).unwrap();
        let error = check_size_cap(&archive, &options, written - 1, &out).unwrap_err();
        assert!(
            matches!(
                &error,
                ArchiveError::ArchiveOverSizeCap { path, size, cap }
                    if *path == out && *size == written && *cap == written - 1
            ),
            "{error:?}"
        );
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
