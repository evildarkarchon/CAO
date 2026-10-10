//! The comparator's Archive rule (#467, #497): equal header facts, equal entry
//! names, equal per-entry compression and chunking, and decompressed content
//! under each entry's own Asset Kind rule; never compressed bytes, offsets or
//! entry order.
//!
//! Archives are built with `ba2` directly, so each scenario can change one
//! fact at a time.

mod common;

use std::path::{Path, PathBuf};

use ba2::prelude::*;
use ba2::{fo4, tes4};
use cao_parity::archives::{ArchiveRule, compare_archives};
use cao_parity::rules::ParityRules;
use cao_parity::tree::{RuleOutcome, TreeRules};
use common::TempDir;
use directxtex::{
    CP_FLAGS_NONE, DDS_FLAGS_NONE, DXGI_FORMAT, ScratchImage, TEX_COMPRESS_BC7_QUICK,
    TEX_COMPRESS_DEFAULT, TEX_COMPRESS_FLAGS, TEX_FILTER_FORCE_NON_WIC, TEX_THRESHOLD_DEFAULT,
};

const BC7: DXGI_FORMAT = DXGI_FORMAT::DXGI_FORMAT_BC7_UNORM;
const BC1: DXGI_FORMAT = DXGI_FORMAT::DXGI_FORMAT_BC1_UNORM;
const RGBA8: DXGI_FORMAT = DXGI_FORMAT::DXGI_FORMAT_R8G8B8A8_UNORM;

/// One TES4 entry: its directory, file name, bytes, and whether it is stored
/// compressed.
type Tes4Entry<'a> = (&'a str, &'a str, &'a [u8], bool);

/// Writes a TES4 BSA of `version` with `flags` and `types` at `path`.
fn bsa(
    path: &Path,
    version: tes4::Version,
    flags: tes4::ArchiveFlags,
    types: tes4::ArchiveTypes,
    entries: &[Tes4Entry<'_>],
) {
    let compression = tes4::FileCompressionOptions::builder()
        .version(version)
        .build();
    let mut archive = tes4::Archive::new();
    for &(directory, name, bytes, compressed) in entries {
        let mut file = tes4::File::from_decompressed(bytes);
        if compressed {
            file = file.compress(&compression).unwrap();
        }
        let key = tes4::ArchiveKey::from(directory);
        match archive.get_mut(&key) {
            Some(existing) => {
                existing.insert(tes4::DirectoryKey::from(name), file);
            }
            None => {
                let entries: tes4::Directory = [(tes4::DirectoryKey::from(name), file)]
                    .into_iter()
                    .collect();
                archive.insert(key, entries);
            }
        }
    }
    let options = tes4::ArchiveOptions::builder()
        .version(version)
        .flags(flags)
        .types(types)
        .build();
    archive
        .write(&mut std::fs::File::create(path).unwrap(), &options)
        .unwrap();
}

/// The flags CAO writes for a compressed SSE BSA: names plus `COMPRESSED`.
fn compressed_flags() -> tes4::ArchiveFlags {
    tes4::ArchiveFlags::DIRECTORY_STRINGS
        | tes4::ArchiveFlags::FILE_STRINGS
        | tes4::ArchiveFlags::COMPRESSED
}

/// An SSE BSA with every entry compressed, as CAO writes one.
fn sse_bsa(path: &Path, entries: &[(&str, &str, &[u8])]) {
    let entries: Vec<Tes4Entry<'_>> = entries
        .iter()
        .map(|&(directory, name, bytes)| (directory, name, bytes, true))
        .collect();
    bsa(
        path,
        tes4::Version::v105,
        compressed_flags(),
        tes4::ArchiveTypes::empty(),
        &entries,
    );
}

/// How an FO4 BA2 is built.
#[derive(Clone, Copy)]
struct Ba2Options {
    format: fo4::Format,
    strings: bool,
    compressed: bool,
    level: fo4::CompressionLevel,
    /// DX10 chunk width and height.
    chunk: usize,
}

impl Default for Ba2Options {
    /// A compressed `GNRL` BA2 with a name table, as CAO writes one.
    fn default() -> Self {
        Self {
            format: fo4::Format::GNRL,
            strings: true,
            compressed: true,
            level: fo4::CompressionLevel::FO4,
            chunk: 512,
        }
    }
}

/// Writes an FO4 BA2 at `path` holding each `(name, source file)`.
fn ba2(path: &Path, options: Ba2Options, entries: &[(&str, &Path)]) {
    let read_options = fo4::FileReadOptions::builder()
        .format(options.format)
        .compression_format(fo4::CompressionFormat::Zip)
        .compression_level(options.level)
        .compression_result(if options.compressed {
            ba2::CompressionResult::Compressed
        } else {
            ba2::CompressionResult::Decompressed
        })
        .mip_chunk_width(options.chunk)
        .mip_chunk_height(options.chunk)
        .build();
    let archive: fo4::Archive = entries
        .iter()
        .map(|(name, source)| {
            (
                fo4::ArchiveKey::from(*name),
                fo4::File::read(*source, &read_options).unwrap(),
            )
        })
        .collect();
    let write_options = fo4::ArchiveOptions::builder()
        .format(options.format)
        .strings(options.strings)
        .compression_format(fo4::CompressionFormat::Zip)
        .build();
    archive
        .write(&mut std::fs::File::create(path).unwrap(), &write_options)
        .unwrap();
}

/// A `size`×`size` texture with a full mip chain. With `flags`, a gradient
/// along one axis, whose 4×4 blocks BC7 fits closely, compressed to `format`;
/// without, seeded raw `format` blocks.
///
/// The mip chain is generated without WIC: test threads never join COM.
fn texture(size: usize, format: DXGI_FORMAT, flags: Option<TEX_COMPRESS_FLAGS>) -> Vec<u8> {
    let mut scratch = ScratchImage::default();
    let Some(flags) = flags else {
        scratch
            .initialize_2d(format, size, size, 1, 0, CP_FLAGS_NONE)
            .unwrap();
        for (index, byte) in scratch.pixels_mut().iter_mut().enumerate() {
            *byte = (index * 7) as u8;
        }
        return scratch.save_dds(DDS_FLAGS_NONE).unwrap().buffer().to_vec();
    };
    scratch
        .initialize_2d(RGBA8, size, size, 1, 1, CP_FLAGS_NONE)
        .unwrap();
    let row_pitch = scratch.images()[0].row_pitch;
    for row in scratch.pixels_mut().chunks_exact_mut(row_pitch) {
        for (x, pixel) in row.as_chunks_mut::<4>().0.iter_mut().take(size).enumerate() {
            let x = x as u8;
            pixel.copy_from_slice(&[x * 4, x * 2, 255 - x * 4, 128 + x * 2]);
        }
    }
    scratch
        .generate_mip_maps(TEX_FILTER_FORCE_NON_WIC, 0)
        .unwrap()
        .compress(format, flags, TEX_THRESHOLD_DEFAULT)
        .unwrap()
        .save_dds(DDS_FLAGS_NONE)
        .unwrap()
        .buffer()
        .to_vec()
}

/// Writes `bytes` to `dir/name` and returns the path.
fn source(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

fn equivalent(oracle: &Path, rust: &Path) {
    assert_ne!(
        std::fs::read(oracle).unwrap(),
        std::fs::read(rust).unwrap(),
        "the Archives must differ in bytes for this test"
    );
    assert_eq!(
        compare_archives(oracle, rust).unwrap(),
        RuleOutcome::Equivalent
    );
}

fn different(oracle: &Path, rust: &Path) -> String {
    match compare_archives(oracle, rust).unwrap() {
        RuleOutcome::Different(detail) => detail,
        RuleOutcome::Equivalent => panic!("expected Different, got Equivalent"),
    }
}

/// The rule's reason to exist: the same entries compressed differently are
/// Equivalent, since compressed bytes are never compared.
#[test]
fn the_same_entries_compressed_differently_are_equivalent() {
    let dir = TempDir::new("archive-rule-compression");
    let script = source(dir.path(), "quest.pex", &b"compiled script ".repeat(200));
    let oracle = dir.path().join("oracle.ba2");
    let rust = dir.path().join("rust.ba2");
    ba2(
        &oracle,
        Ba2Options::default(),
        &[(r"scripts\quest.pex", &script)],
    );
    ba2(
        &rust,
        Ba2Options {
            level: fo4::CompressionLevel::FO4Xbox,
            ..Ba2Options::default()
        },
        &[(r"scripts\quest.pex", &script)],
    );

    equivalent(&oracle, &rust);
}

/// A `.dds` entry is held to the Texture rule: two BC7 encodes of one source
/// pass the PSNR floor, while garbled pixels fail and name their entry.
#[test]
fn dds_entries_follow_the_texture_rule() {
    let dir = TempDir::new("archive-rule-textures");
    let thorough = texture(32, BC7, Some(TEX_COMPRESS_DEFAULT));
    let quick = texture(32, BC7, Some(TEX_COMPRESS_BC7_QUICK));
    assert_ne!(thorough, quick, "the encoders must disagree for this test");
    let mut garbled = thorough.clone();
    let header = garbled.len() - 1024;
    for byte in &mut garbled[header..] {
        *byte ^= 0xA5;
    }
    let oracle = dir.path().join("oracle.bsa");
    let rust = dir.path().join("rust.bsa");
    let broken = dir.path().join("broken.bsa");
    sse_bsa(&oracle, &[("textures", "sky.dds", &thorough)]);
    sse_bsa(&rust, &[("textures", "sky.dds", &quick)]);
    sse_bsa(&broken, &[("textures", "sky.dds", &garbled)]);

    equivalent(&oracle, &rust);
    let detail = different(&oracle, &broken);
    assert!(detail.contains(r"textures\sky.dds"), "{detail}");
}

/// Every other entry must decompress to the same bytes, however small the
/// difference.
#[test]
fn other_entries_must_decompress_to_the_same_bytes() {
    let dir = TempDir::new("archive-rule-bytes");
    let oracle = dir.path().join("oracle.bsa");
    let rust = dir.path().join("rust.bsa");
    sse_bsa(&oracle, &[("meshes", "a.nif", b"mesh bytes 1")]);
    sse_bsa(&rust, &[("meshes", "a.nif", b"mesh bytes 2")]);

    let detail = different(&oracle, &rust);
    assert!(
        detail.contains("decompresses to different bytes"),
        "{detail}"
    );
}

/// Format, version, flags, archive types, container kind and name-table
/// presence are each compared.
#[test]
fn every_header_fact_is_compared() {
    let dir = TempDir::new("archive-rule-header");
    let entry: &[Tes4Entry<'_>] = &[("meshes", "a.nif", b"mesh", false)];
    let names = tes4::ArchiveFlags::DIRECTORY_STRINGS | tes4::ArchiveFlags::FILE_STRINGS;
    let base = dir.path().join("base.bsa");
    bsa(
        &base,
        tes4::Version::v105,
        names,
        tes4::ArchiveTypes::empty(),
        entry,
    );
    let variants = [
        (
            "version",
            tes4::Version::v104,
            names,
            tes4::ArchiveTypes::empty(),
        ),
        (
            "flags",
            tes4::Version::v105,
            names | tes4::ArchiveFlags::RETAIN_FILE_NAMES,
            tes4::ArchiveTypes::empty(),
        ),
        (
            "types",
            tes4::Version::v105,
            names,
            tes4::ArchiveTypes::MESHES,
        ),
        (
            "name table",
            tes4::Version::v105,
            tes4::ArchiveFlags::DIRECTORY_STRINGS,
            tes4::ArchiveTypes::empty(),
        ),
    ];
    for (fact, version, flags, types) in variants {
        let variant = dir.path().join(format!("{fact}.bsa"));
        bsa(&variant, version, flags, types, entry);
        let detail = different(&base, &variant);
        assert!(detail.contains("header"), "{fact}: {detail}");
    }

    // A BSA against a BA2: the format differs.
    let mesh = source(dir.path(), "a.nif", b"mesh");
    let general = dir.path().join("general.ba2");
    ba2(&general, Ba2Options::default(), &[(r"meshes\a.nif", &mesh)]);
    assert!(different(&base, &general).contains("header"));

    // A BA2's container kind and name table.
    let dds = source(dir.path(), "a.dds", &texture(64, BC1, None));
    let gnrl = dir.path().join("gnrl.ba2");
    let dx10 = dir.path().join("dx10.ba2");
    let nameless = dir.path().join("nameless.ba2");
    ba2(&gnrl, Ba2Options::default(), &[(r"textures\a.dds", &dds)]);
    let dx10_options = Ba2Options {
        format: fo4::Format::DX10,
        ..Ba2Options::default()
    };
    ba2(&dx10, dx10_options, &[(r"textures\a.dds", &dds)]);
    ba2(
        &nameless,
        Ba2Options {
            strings: false,
            ..Ba2Options::default()
        },
        &[(r"textures\a.dds", &dds)],
    );
    assert!(different(&gnrl, &dx10).contains("header"));
    assert!(different(&gnrl, &nameless).contains("header"));
}

/// Entry names are compared as a set: an entry on one side only, or under
/// another name with the same content, is a difference.
#[test]
fn entry_names_must_match() {
    let dir = TempDir::new("archive-rule-names");
    let a = source(dir.path(), "a.pex", b"a");
    let b = source(dir.path(), "b.pex", b"b");
    let oracle = dir.path().join("oracle.ba2");
    let extra = dir.path().join("extra.ba2");
    let renamed = dir.path().join("renamed.ba2");
    ba2(&oracle, Ba2Options::default(), &[(r"scripts\a.pex", &a)]);
    ba2(
        &extra,
        Ba2Options::default(),
        &[(r"scripts\a.pex", &a), (r"scripts\b.pex", &b)],
    );
    ba2(&renamed, Ba2Options::default(), &[(r"scripts\c.pex", &a)]);

    let detail = different(&oracle, &extra);
    assert!(
        detail.contains(r"entry `scripts\b.pex` is only on the Rust side"),
        "{detail}"
    );
    let detail = different(&oracle, &renamed);
    assert!(detail.contains("only on the oracle side"), "{detail}");
    assert!(detail.contains("only on the Rust side"), "{detail}");
}

/// An entry stored on one side and compressed on the other differs, even
/// though both decompress to the same bytes.
#[test]
fn each_entry_compression_state_must_match() {
    let dir = TempDir::new("archive-rule-compressed");
    let script = source(dir.path(), "quest.pex", &b"compiled script ".repeat(200));
    let oracle = dir.path().join("oracle.ba2");
    let rust = dir.path().join("rust.ba2");
    ba2(
        &oracle,
        Ba2Options::default(),
        &[(r"scripts\quest.pex", &script)],
    );
    ba2(
        &rust,
        Ba2Options {
            compressed: false,
            ..Ba2Options::default()
        },
        &[(r"scripts\quest.pex", &script)],
    );

    let detail = different(&oracle, &rust);
    assert!(
        detail.contains("compressed on the oracle side and stored"),
        "{detail}"
    );
}

/// A DX10 texture chunked at another pitch rebuilds the same DDS, but its
/// chunk count and mip ranges differ.
#[test]
fn dx10_chunks_and_mip_ranges_must_match() {
    let dir = TempDir::new("archive-rule-chunks");
    let dds = source(dir.path(), "a.dds", &texture(1024, BC1, None));
    let oracle = dir.path().join("oracle.ba2");
    let rust = dir.path().join("rust.ba2");
    let dx10 = Ba2Options {
        format: fo4::Format::DX10,
        ..Ba2Options::default()
    };
    ba2(&oracle, dx10, &[(r"textures\a.dds", &dds)]);
    ba2(
        &rust,
        Ba2Options { chunk: 256, ..dx10 },
        &[(r"textures\a.dds", &dds)],
    );

    let detail = different(&oracle, &rust);
    assert!(detail.contains("chunks with mips"), "{detail}");
    assert!(!detail.contains("decompresses"), "{detail}");
}

/// An Archive that does not parse is a difference on its side, never a
/// harness error.
#[test]
fn an_unreadable_archive_is_different() {
    let dir = TempDir::new("archive-rule-unreadable");
    let oracle = dir.path().join("oracle.bsa");
    sse_bsa(&oracle, &[("meshes", "a.nif", b"mesh")]);
    let rust = source(dir.path(), "rust.bsa", b"not an archive");

    let detail = different(&oracle, &rust);
    assert!(detail.starts_with("the Rust Archive"), "{detail}");
}

/// The comparator sends Archives, by extension and ignoring case, to this
/// rule, but never a `.bak` backup, which must stay byte-identical.
#[test]
fn archives_but_not_their_backups_follow_the_archive_rule() {
    let rule = |path: &str| ParityRules.asset_rule(path).map(|rule| rule.name());
    let archive = Some(cao_parity::tree::ArtifactRule::name(&ArchiveRule));
    assert_eq!(rule("mods/Mod/Mod.bsa"), archive);
    assert_eq!(rule("mods/Mod/Mod - Textures.BA2"), archive);
    assert_eq!(rule("mods/Mod/Mod.bsa.bak"), None);
}
