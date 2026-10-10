//! The HDPT reader (#505): the Headpart Meshes a plugin names, and deviation
//! 17's parser fixes. C++ overflowed a 1024-byte buffer on a long or
//! unterminated MODL, looped forever on HDPT framing that ran past its end,
//! and misread compressed records.
//!
//! The plugins are built byte by byte by `common::plugin`.

mod common;

use std::io::Cursor;

use cao_optimizers::plugins::{PluginError, headparts};
use common::plugin::{COMPRESSED, compressed, field, group, hdpt, modl, plugin, record};

/// The Headpart Meshes `bytes` names.
fn read(bytes: &[u8]) -> Result<Vec<String>, PluginError> {
    headparts(&mut Cursor::new(bytes))
}

/// C++: every MODL path in the HDPT group is a Headpart Mesh, `/`-separated
/// and under `meshes/`, whether the plugin wrote that prefix or not. Other
/// groups and fields are skipped.
#[test]
fn hdpt_records_name_headpart_meshes() {
    let bytes = plugin(&[
        group(b"GMST", &record(b"GMST", 0, &field(b"EDID", b"fSetting\0"))),
        group(
            b"HDPT",
            &[
                hdpt("Actors\\Character\\Hair\\HairMale01.nif"),
                hdpt("Meshes\\Actors\\Character\\Beards\\Beard01.nif"),
            ]
            .concat(),
        ),
    ]);

    assert_eq!(
        read(&bytes).unwrap(),
        [
            "meshes/Actors/Character/Hair/HairMale01.nif",
            "Meshes/Actors/Character/Beards/Beard01.nif",
        ]
    );
}

/// Deviation 17: a MODL path is read to its first NUL or the end of its
/// field, however long. C++ copied it into a 1024-byte buffer and, without a
/// NUL, read past the field.
#[test]
fn long_and_unterminated_modl_paths_are_read_whole() {
    let long = format!("Actors\\{}\\Hair.nif", "Deep\\".repeat(400));
    let unterminated = field(b"MODL", b"Actors\\Character\\Hair\\Bald.nif");
    let bytes = plugin(&[group(
        b"HDPT",
        &[hdpt(&long), record(b"HDPT", 0, &unterminated)].concat(),
    )]);

    let expected_long = format!("meshes/Actors/{}Hair.nif", "Deep/".repeat(400));
    assert!(long.len() > 1024);
    assert_eq!(
        read(&bytes).unwrap(),
        [
            expected_long.as_str(),
            "meshes/Actors/Character/Hair/Bald.nif"
        ]
    );
}

/// Deviation 17: HDPT framing that runs past what encloses it makes the plugin
/// unreadable. C++ looped forever on each of these.
#[test]
fn truncated_hdpt_framing_makes_the_plugin_unreadable() {
    let modl_record = |declared: u32| {
        let mut bytes = hdpt("Actors\\Hair.nif");
        bytes[4..8].copy_from_slice(&declared.to_le_bytes());
        bytes
    };
    let mut long_field = field(b"EDID", b"HairMale01\0");
    long_field[4..6].copy_from_slice(&200_u16.to_le_bytes());
    let mut small_group = group(b"HDPT", &hdpt("Actors\\Hair.nif"));
    small_group[4..8].copy_from_slice(&10_u32.to_le_bytes());
    let mut long_group = group(b"HDPT", &hdpt("Actors\\Hair.nif"));
    long_group[4..8].copy_from_slice(&5000_u32.to_le_bytes());

    let cases = [
        ("a group past the end of the file", plugin(&[long_group])),
        ("a group smaller than its header", plugin(&[small_group])),
        (
            "a record past the end of its group",
            plugin(&[group(b"HDPT", &modl_record(4000))]),
        ),
        (
            "a record header cut by its group's end",
            plugin(&[group(b"HDPT", &hdpt("Actors\\Hair.nif")[..20])]),
        ),
        (
            "a field past the end of its record",
            plugin(&[group(b"HDPT", &record(b"HDPT", 0, &long_field))]),
        ),
        (
            "a field header cut by its record's end",
            plugin(&[group(b"HDPT", &record(b"HDPT", 0, b"MOD"))]),
        ),
    ];
    for (case, bytes) in cases {
        assert!(
            matches!(read(&bytes), Err(PluginError::Truncated(_))),
            "{case}: {:?}",
            read(&bytes)
        );
    }
}

/// Deviation 17: a plugin cut anywhere ends the read promptly. Cut inside its
/// HDPT group, it is unreadable; cut before the group, it names nothing, as
/// C++'s read ended early there too.
#[test]
fn a_plugin_cut_anywhere_ends_the_read() {
    let before = plugin(&[group(
        b"GMST",
        &record(b"GMST", 0, &field(b"EDID", b"fSetting\0")),
    )]);
    let bytes = [
        before.clone(),
        group(
            b"HDPT",
            &[hdpt("Actors\\Hair01.nif"), hdpt("Actors\\Hair02.nif")].concat(),
        ),
    ]
    .concat();

    for cut in 0..bytes.len() {
        let result = read(&bytes[..cut]);
        if cut < before.len() + 24 {
            assert!(
                result.as_ref().is_ok_and(Vec::is_empty),
                "cut at {cut}: {result:?}"
            );
        } else {
            assert!(
                matches!(result, Err(PluginError::Truncated(_))),
                "cut at {cut}: {result:?}"
            );
        }
    }
    assert_eq!(read(&bytes).unwrap().len(), 2);
}

/// A file that is not a plugin, or a plugin without an HDPT group, names no
/// Headpart Mesh, as in C++.
#[test]
fn files_without_hdpt_records_name_nothing() {
    for bytes in [
        Vec::new(),
        b"not a plugin at all, just some text".to_vec(),
        plugin(&[]),
        plugin(&[group(
            b"NPC_",
            &record(b"NPC_", 0, &modl("Actors\\Npc.nif")),
        )]),
    ] {
        assert!(read(&bytes).unwrap().is_empty());
    }
}

/// Deviation 17: a compressed record whose stream is not zlib, or holds less
/// than it declares, makes the plugin unreadable.
#[test]
fn corrupt_compressed_records_make_the_plugin_unreadable() {
    let fields = modl("Actors\\Hair.nif");
    let mut short = compressed(&fields);
    short[..4].copy_from_slice(&500_u32.to_le_bytes());
    let mut garbage = 20_u32.to_le_bytes().to_vec();
    garbage.extend(b"definitely not zlib");

    for data in [short, garbage] {
        let bytes = plugin(&[group(b"HDPT", &record(b"HDPT", COMPRESSED, &data))]);
        assert!(
            matches!(read(&bytes), Err(PluginError::Corrupt(_))),
            "{:?}",
            read(&bytes)
        );
    }
    let bytes = plugin(&[group(b"HDPT", &record(b"HDPT", COMPRESSED, &[1, 0]))]);
    assert!(matches!(read(&bytes), Err(PluginError::Truncated(_))));
}

/// Deviation 17: a compressed HDPT record is decompressed before its fields
/// are read, so its MODL path is a Headpart Mesh like any other. C++ read the
/// zlib stream as fields.
#[test]
fn compressed_hdpt_records_are_decompressed() {
    let mut fields = field(b"EDID", b"HairFemale01\0");
    fields.extend(modl("Actors\\Character\\Hair\\HairFemale01.nif"));
    let bytes = plugin(&[group(
        b"HDPT",
        &[
            record(b"HDPT", COMPRESSED, &compressed(&fields)),
            hdpt("Actors\\Character\\Hair\\HairMale01.nif"),
        ]
        .concat(),
    )]);

    assert_eq!(
        read(&bytes).unwrap(),
        [
            "meshes/Actors/Character/Hair/HairFemale01.nif",
            "meshes/Actors/Character/Hair/HairMale01.nif",
        ]
    );
}
