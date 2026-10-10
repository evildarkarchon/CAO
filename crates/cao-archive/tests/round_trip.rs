//! Writing Archives from a Mod Root and reading them back, for each game.
//!
//! Headers are checked by reading the output with `ba2` directly; contents through
//! [`ReadArchive`], against the source files.

mod common;

use std::path::{Path, PathBuf};

use ba2::prelude::*;
use cao_archive::{
    ArchiveData, ArchiveError, ArchiveFormat, ArchiveHeader, ArchiveType, ArchiveVersion,
    Fo4Container, Game, MergeSettings, PackSource, ReadArchive, Settings, SplitArchives,
    write_archive,
};
use directxtex::{CP_FLAGS_NONE, DDS_FLAGS_NONE, DXGI_FORMAT_BC1_UNORM, ScratchImage};

const MESH: &str = r"meshes\Armor\Cuirass.nif";
const SCRIPT: &str = r"scripts\source\Quest.psc";
const SOUND: &str = r"sound\Hit.wav";
const TEXTURE: &str = r"textures\Armor\Cuirass.dds";

/// A 1024x1024 BC1 texture with a full mip chain: large enough that FO4 splits it
/// into three DX10 chunks at the 512x512 pitch.
fn texture_bytes() -> Vec<u8> {
    let mut scratch = ScratchImage::default();
    scratch
        .initialize_2d(DXGI_FORMAT_BC1_UNORM, 1024, 1024, 1, 11, CP_FLAGS_NONE)
        .unwrap();
    for (index, byte) in scratch.pixels_mut().iter_mut().enumerate() {
        *byte = (index * 7) as u8;
    }
    scratch.save_dds(DDS_FLAGS_NONE).unwrap().buffer().to_vec()
}

/// A Mod Root holding one Standard, one Incompressible and one Texture file.
fn mod_root(name: &str) -> PathBuf {
    let root = common::scratch_dir(name).join("Example");
    // Repetitive bytes, so compression visibly shrinks them.
    common::write(&root, MESH, &b"NiTriShape ".repeat(500));
    common::write(&root, SCRIPT, &b"pex script ".repeat(50));
    common::write(&root, SOUND, &b"RIFF wave ".repeat(300));
    common::write(&root, TEXTURE, &texture_bytes());
    root
}

/// Splits everything under `root` into Archives without merging.
fn plan(root: &Path, settings: &Settings) -> Vec<ArchiveData> {
    let sources = [MESH, SCRIPT, SOUND, TEXTURE]
        .into_iter()
        .map(|relative| {
            let path = root.join(relative);
            let size = std::fs::metadata(&path).unwrap().len();
            PackSource { path, size }
        })
        .collect();
    SplitArchives::split(root, sources, settings)
        .unwrap()
        .merge(MergeSettings::default())
}

/// Writes each planned Archive next to `root` and returns the written paths.
fn write_all(
    root: &Path,
    archives: &[ArchiveData],
    settings: &Settings,
    compress: bool,
) -> Vec<PathBuf> {
    archives
        .iter()
        .enumerate()
        .map(|(index, archive)| {
            let out = root.with_file_name(format!("Example{index}{}", settings.extension));
            write_archive(compress, archive, root, &out).unwrap();
            out
        })
        .collect()
}

/// Asserts every entry of `archive` extracts to the bytes of the matching file
/// under `root`, and returns the entry names sorted (entries come in hash order).
fn assert_round_trips(archive: &Path, root: &Path) -> Vec<String> {
    let read = ReadArchive::open(archive)
        .unwrap()
        .expect("a known archive format");
    let entries = read.archived_assets().unwrap();
    for entry in &entries {
        let mut extracted = Vec::new();
        read.extract(&entry.name, &mut extracted).unwrap();
        let source = std::fs::read(root.join(&entry.name)).unwrap();
        if entry.name.ends_with(".dds") && read.version() == Some(ArchiveVersion::Fo4Dx) {
            // DX10 rebuilds the DDS header, so compare what the texture decodes to.
            assert_same_texture(&extracted, &source);
        } else {
            assert!(
                extracted == source,
                "{} differs after extraction",
                entry.name
            );
        }
    }
    let mut names: Vec<_> = entries.into_iter().map(|entry| entry.name).collect();
    names.sort();
    names
}

fn assert_same_texture(actual: &[u8], expected: &[u8]) {
    let actual = ScratchImage::load_dds(actual, DDS_FLAGS_NONE, None, None).unwrap();
    let expected = ScratchImage::load_dds(expected, DDS_FLAGS_NONE, None, None).unwrap();
    let (a, e) = (actual.metadata(), expected.metadata());
    assert_eq!(
        (a.width, a.height, a.mip_levels, a.format),
        (e.width, e.height, e.mip_levels, e.format)
    );
    assert!(
        actual.pixels() == expected.pixels(),
        "texture pixels differ"
    );
}

/// The version and flags of a TES4 archive, read with `ba2` itself.
fn tes4_header(path: &Path) -> (ba2::tes4::Version, u32) {
    let (_, options) = ba2::tes4::Archive::read(path).unwrap();
    (options.version(), options.flags().bits())
}

#[test]
fn tes5_archives_are_v104_bsas_that_round_trip() {
    let root = mod_root("round_trip_tes5");
    let settings = Settings::get(Game::Tes5);
    let archives = plan(&root, &settings);
    let types: Vec<_> = archives.iter().map(ArchiveData::archive_type).collect();
    assert_eq!(
        types,
        [
            ArchiveType::Standard,
            ArchiveType::Incompressible,
            ArchiveType::Textures
        ]
    );
    let written = write_all(&root, &archives, &settings, true);

    // Directory and file strings (0x3), plus COMPRESSED (0x4) except for the
    // Incompressible Archive, which is never compressed.
    assert_eq!(tes4_header(&written[0]), (ba2::tes4::Version::v104, 0x7));
    assert_eq!(tes4_header(&written[1]), (ba2::tes4::Version::v104, 0x3));
    assert_eq!(tes4_header(&written[2]), (ba2::tes4::Version::v104, 0x7));

    // Names are stored lowercase with backslashes.
    assert_eq!(
        assert_round_trips(&written[0], &root),
        [r"meshes\armor\cuirass.nif", r"scripts\source\quest.psc"]
    );
    assert_eq!(assert_round_trips(&written[1], &root), [r"sound\hit.wav"]);
    assert_eq!(
        assert_round_trips(&written[2], &root),
        [r"textures\armor\cuirass.dds"]
    );
}

#[test]
fn sse_archives_are_v105_bsas_that_round_trip() {
    let root = mod_root("round_trip_sse");
    let settings = Settings::get(Game::Sse);
    let written = write_all(&root, &plan(&root, &settings), &settings, true);
    assert_eq!(written.len(), 3);
    assert_eq!(tes4_header(&written[0]), (ba2::tes4::Version::v105, 0x7));
    assert_eq!(tes4_header(&written[1]), (ba2::tes4::Version::v105, 0x3));
    assert_eq!(tes4_header(&written[2]), (ba2::tes4::Version::v105, 0x7));
    for archive in &written {
        assert_round_trips(archive, &root);
    }

    let read = ReadArchive::open(&written[0]).unwrap().unwrap();
    assert_eq!(read.version(), Some(ArchiveVersion::Sse));
    assert_eq!(
        read.header(),
        ArchiveHeader {
            format: ArchiveFormat::Tes4,
            version: 105,
            flags: 0x7,
            types: 0,
            container: None,
            name_table: true,
        }
    );
    let entries = read.archived_assets().unwrap();
    assert!(entries.iter().all(|entry| entry.compressed));
    // Inventory sizes are the decompressed sizes.
    let mesh = entries
        .iter()
        .find(|entry| entry.name == r"meshes\armor\cuirass.nif")
        .unwrap();
    assert_eq!(mesh.size, 11 * 500);
}

#[test]
fn uncompressed_bsas_leave_the_compressed_flag_off() {
    let root = mod_root("round_trip_sse_uncompressed");
    let settings = Settings::get(Game::Sse);
    let written = write_all(&root, &plan(&root, &settings), &settings, false);
    for archive in &written {
        assert_eq!(tes4_header(archive), (ba2::tes4::Version::v105, 0x3));
        let read = ReadArchive::open(archive).unwrap().unwrap();
        assert!(
            read.archived_assets()
                .unwrap()
                .iter()
                .all(|entry| !entry.compressed)
        );
        assert_round_trips(archive, &root);
    }
}

#[test]
fn fo4_archives_are_gnrl_and_dx10_ba2s_with_name_tables_that_round_trip() {
    let root = mod_root("round_trip_fo4");
    let settings = Settings::get(Game::Fo4);
    let written = write_all(&root, &plan(&root, &settings), &settings, true);
    assert_eq!(written.len(), 3);

    let formats: Vec<_> = written
        .iter()
        .map(|path| {
            let (_, options) = ba2::fo4::Archive::read(path.as_path()).unwrap();
            assert!(options.strings(), "{} has no name table", path.display());
            assert_eq!(options.version(), ba2::fo4::Version::v1);
            options.format()
        })
        .collect();
    use ba2::fo4::Format::{DX10, GNRL};
    assert_eq!(formats, [GNRL, GNRL, DX10]);

    // `header` reports the same facts `ba2` read.
    let containers: Vec<_> = written
        .iter()
        .map(|path| {
            let header = ReadArchive::open(path).unwrap().unwrap().header();
            assert_eq!(
                (header.format, header.version, header.flags, header.types),
                (ArchiveFormat::Fo4, 1, 0, 0)
            );
            assert!(header.name_table);
            header.container
        })
        .collect();
    use Fo4Container::{Dx10, General};
    assert_eq!(containers, [Some(General), Some(General), Some(Dx10)]);

    // FO4 names are kept as written, so they come back with backslashes.
    assert_eq!(
        assert_round_trips(&written[0], &root),
        [r"meshes\armor\cuirass.nif", r"scripts\source\quest.psc"]
    );
    assert_eq!(assert_round_trips(&written[1], &root), [r"sound\hit.wav"]);
    assert_eq!(
        assert_round_trips(&written[2], &root),
        [r"textures\armor\cuirass.dds"]
    );
}

#[test]
fn an_fo4_dx10_textures_ba2_is_always_compressed() {
    let root = mod_root("round_trip_fo4_dx10_compressed");
    let settings = Settings::get(Game::Fo4);
    // Compression off: GNRL Archives stay uncompressed, DX10 does not.
    let written = write_all(&root, &plan(&root, &settings), &settings, false);

    let main = ReadArchive::open(&written[0]).unwrap().unwrap();
    assert_eq!(main.version(), Some(ArchiveVersion::Fo4));
    assert!(
        main.archived_assets()
            .unwrap()
            .iter()
            .all(|entry| !entry.compressed)
    );

    let textures = ReadArchive::open(&written[2]).unwrap().unwrap();
    assert_eq!(textures.version(), Some(ArchiveVersion::Fo4Dx));
    let entries = textures.archived_assets().unwrap();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].compressed);
    // 1024x1024 BC1 mips 0 and 1 each fill a 512x512 chunk; the rest share one.
    assert_eq!(entries[0].chunks, 3);
    assert_eq!(entries[0].mip_ranges, [0..=0, 1..=1, 2..=10]);
    assert!(
        main.archived_assets()
            .unwrap()
            .iter()
            .all(|entry| entry.mip_ranges.is_empty()),
        "GNRL chunks carry no mips"
    );
    // Inventory counts the pixel data plus a 148-byte DDS header; the source
    // has a 128-byte legacy header.
    let source_len = std::fs::metadata(root.join(TEXTURE)).unwrap().len();
    assert_eq!(entries[0].size, source_len - 128 + 148);
    assert_round_trips(&written[2], &root);
}

#[test]
fn nothing_is_written_for_an_empty_archive() {
    let dir = common::scratch_dir("round_trip_empty");
    let empty = ArchiveData::new(&Settings::get(Game::Sse), ArchiveType::Standard);
    let out = dir.join("Empty.bsa");
    write_archive(true, &empty, &dir, &out).unwrap();
    assert!(!out.exists());
}

#[test]
fn an_existing_output_is_never_overwritten() {
    let root = mod_root("round_trip_existing_output");
    let settings = Settings::get(Game::Sse);
    let archives = plan(&root, &settings);
    let out = root.with_file_name("Example.bsa");
    std::fs::write(&out, b"keep me").unwrap();
    let error = write_archive(true, &archives[0], &root, &out).unwrap_err();
    assert!(matches!(error, ArchiveError::Io { .. }), "{error:?}");
    assert_eq!(std::fs::read(&out).unwrap(), b"keep me");
}

#[test]
fn unknown_magic_is_not_an_archive() {
    let dir = common::scratch_dir("round_trip_unknown_magic");
    let path = common::write(&dir, "Fake.bsa", b"not an archive at all");
    assert!(ReadArchive::open(&path).unwrap().is_none());
}

#[test]
fn sources_can_be_deleted_once_written() {
    // Uncompressed files are memory-mapped from their sources while writing;
    // every mapping must be gone when `write_archive` returns (Windows refuses to
    // delete a mapped file).
    let root = mod_root("round_trip_sources_released");
    let settings = Settings::get(Game::Sse);
    write_all(&root, &plan(&root, &settings), &settings, false);
    for relative in [MESH, SCRIPT, SOUND, TEXTURE] {
        std::fs::remove_file(root.join(relative)).unwrap();
    }
}
