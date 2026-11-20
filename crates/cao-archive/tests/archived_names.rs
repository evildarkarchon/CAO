//! Audit BE-4: Archived Asset names are stored as bytes, and an author's
//! accented names are usually Windows-1252, not UTF-8. Such a name no longer
//! makes the whole Archive unreadable: it is listed under its Windows-1252
//! reading and extracted by its stored bytes.

mod common;

use std::path::{Path, PathBuf};

use ba2::prelude::*;
use ba2::{fo4, tes4};
use cao_archive::ReadArchive;

/// "café.dds" as Windows-1252 stores it: 0xE9 alone is not UTF-8.
const WINDOWS_1252_FILE: &[u8] = b"caf\xE9.dds";
/// "Rüstung" as Windows-1252 stores it, for a TES4 directory.
const WINDOWS_1252_DIRECTORY: &[u8] = b"textures\\r\xFCstung";

/// A fresh output path in this test's scratch directory.
fn output(name: &str) -> PathBuf {
    common::scratch_dir(name).join(format!("{name}.archive"))
}

/// Opens `path` as an Archive, which must be one.
fn open(path: &Path) -> ReadArchive {
    ReadArchive::open(path)
        .unwrap()
        .expect("a known Archive magic")
}

/// A BSA whose directory and file names are both Windows-1252, beside one
/// that is plain ASCII, lists both and extracts each by its listed name.
#[test]
fn a_windows_1252_bsa_name_is_listed_and_extracted() {
    let path = output("bsa-windows-1252");
    let accented: tes4::Directory = [(
        tes4::DirectoryKey::from(WINDOWS_1252_FILE),
        tes4::File::from_decompressed(b"accented bytes".as_slice()),
    )]
    .into_iter()
    .collect();
    let plain: tes4::Directory = [(
        tes4::DirectoryKey::from("plain.dds"),
        tes4::File::from_decompressed(b"plain bytes".as_slice()),
    )]
    .into_iter()
    .collect();
    let archive: tes4::Archive = [
        (tes4::ArchiveKey::from(WINDOWS_1252_DIRECTORY), accented),
        (tes4::ArchiveKey::from("textures"), plain),
    ]
    .into_iter()
    .collect();
    let options = tes4::ArchiveOptions::builder()
        .version(tes4::Version::v105)
        .flags(tes4::ArchiveFlags::DIRECTORY_STRINGS | tes4::ArchiveFlags::FILE_STRINGS)
        .build();
    archive
        .write(&mut std::fs::File::create(&path).unwrap(), &options)
        .unwrap();

    let read = open(&path);
    let mut names: Vec<String> = read
        .archived_assets()
        .unwrap()
        .into_iter()
        .map(|asset| asset.name)
        .collect();
    names.sort();

    assert_eq!(names, [r"textures\plain.dds", r"textures\rüstung\café.dds"]);
    for (name, bytes) in [
        (r"textures\rüstung\café.dds", b"accented bytes".as_slice()),
        (r"textures\plain.dds", b"plain bytes".as_slice()),
    ] {
        let mut extracted = Vec::new();
        read.extract(name, &mut extracted).unwrap();
        assert_eq!(extracted, bytes, "{name}");
    }
}

/// A BA2 name is one string, decoded the same way.
#[test]
fn a_windows_1252_ba2_name_is_listed_and_extracted() {
    let path = output("ba2-windows-1252");
    let mut name = b"textures\\".to_vec();
    name.extend_from_slice(WINDOWS_1252_FILE);
    let file: fo4::File = [fo4::Chunk::from_decompressed(b"general bytes".as_slice())]
        .into_iter()
        .collect();
    let archive: fo4::Archive = [(fo4::ArchiveKey::from(name), file)].into_iter().collect();
    let options = fo4::ArchiveOptions::builder()
        .format(fo4::Format::GNRL)
        .version(fo4::Version::v1)
        .strings(true)
        .build();
    archive
        .write(&mut std::fs::File::create(&path).unwrap(), &options)
        .unwrap();

    let read = open(&path);
    let assets = read.archived_assets().unwrap();

    assert_eq!(assets.len(), 1);
    assert_eq!(assets[0].name, r"textures\café.dds");
    let mut extracted = Vec::new();
    read.extract(&assets[0].name, &mut extracted).unwrap();
    assert_eq!(extracted, b"general bytes");
}
