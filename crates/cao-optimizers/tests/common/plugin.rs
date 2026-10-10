//! Plugins built byte by byte, in the layout Skyrim and Fallout 4 write:
//! 24-byte record and group headers, and 6-byte field headers (#505).

use std::io::Write;

/// The record flag marking compressed record data.
pub const COMPRESSED: u32 = 0x0004_0000;

/// A record: its 24-byte header (`signature`, data size, `flags`, form id,
/// version control, version, unknown), then `data`.
pub fn record(signature: &[u8; 4], flags: u32, data: &[u8]) -> Vec<u8> {
    let mut bytes = signature.to_vec();
    bytes.extend(u32::try_from(data.len()).unwrap().to_le_bytes());
    bytes.extend(flags.to_le_bytes());
    bytes.extend(0x0000_0801_u32.to_le_bytes());
    bytes.extend([0; 4]);
    bytes.extend(44_u16.to_le_bytes());
    bytes.extend([0; 2]);
    bytes.extend(data);
    bytes
}

/// A top-level group of `label` records: its 24-byte header, whose size
/// counts the header itself, then `contents`.
pub fn group(label: &[u8; 4], contents: &[u8]) -> Vec<u8> {
    let mut bytes = b"GRUP".to_vec();
    bytes.extend(u32::try_from(24 + contents.len()).unwrap().to_le_bytes());
    bytes.extend(label);
    bytes.extend([0; 12]);
    bytes.extend(contents);
    bytes
}

/// A field: its 6-byte header (`signature`, `u16` size), then `data`.
pub fn field(signature: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut bytes = signature.to_vec();
    bytes.extend(u16::try_from(data.len()).unwrap().to_le_bytes());
    bytes.extend(data);
    bytes
}

/// A NUL-terminated MODL field naming `path`.
pub fn modl(path: &str) -> Vec<u8> {
    let mut data = path.as_bytes().to_vec();
    data.push(0);
    field(b"MODL", &data)
}

/// An HDPT record with an editor id and one MODL field naming `path`.
pub fn hdpt(path: &str) -> Vec<u8> {
    let mut data = field(b"EDID", b"HairMale01\0");
    data.extend(modl(path));
    data.extend(field(b"DATA", &[1]));
    record(b"HDPT", 0, &data)
}

/// A plugin: its `TES4` header record, then `groups`.
pub fn plugin(groups: &[Vec<u8>]) -> Vec<u8> {
    let mut bytes = record(b"TES4", 0, &field(b"HEDR", &[0; 12]));
    for group in groups {
        bytes.extend(group);
    }
    bytes
}

/// A plugin whose HDPT group holds one record per path in `paths`.
pub fn headpart_plugin(paths: &[&str]) -> Vec<u8> {
    let records: Vec<u8> = paths.iter().flat_map(|path| hdpt(path)).collect();
    plugin(&[group(b"HDPT", &records)])
}

/// `fields` as compressed record data: their size, then their zlib stream.
pub fn compressed(fields: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(fields).unwrap();
    let mut data = u32::try_from(fields.len()).unwrap().to_le_bytes().to_vec();
    data.extend(encoder.finish().unwrap());
    data
}
