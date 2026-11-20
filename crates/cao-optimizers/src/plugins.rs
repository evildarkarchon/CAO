//! The HDPT reader (#505).
//!
//! A plugin is a `TES4` header record followed by top-level groups. Each MODL
//! field of a record in the `HDPT` group names a Headpart Mesh. [`headparts`]
//! reads them `/`-separated under `meshes/` (deviation 17):
//!
//! - A MODL path ends at its first NUL or at the end of its field, whatever
//!   its length, so a field without a NUL is never read past.
//! - Every header is checked against what encloses it, and each step moves
//!   past at least one header, so a read always ends. A truncated HDPT group,
//!   record or field makes the plugin unreadable ([`PluginError::Truncated`]).
//! - A compressed record is decompressed before its fields are read.
//! - A path's text is UTF-8 when it is valid UTF-8, and Windows-1252
//!   otherwise, the code page plugin strings are written in
//!   ([`decode_game_text`], audit BE-13).
//!
//! A file that does not start with `TES4` names no Headpart Mesh, and framing
//! cut short before the HDPT group only ends the read early: a plugin's other
//! groups are none of this reader's business.

use std::io::{self, Read, Seek, SeekFrom};

/// The size of a record or group header.
const HEADER: usize = 24;
/// The size of a field header: its signature and a `u16` size.
const FIELD_HEADER: usize = 6;
/// The record flag marking compressed record data.
const COMPRESSED: u32 = 0x0004_0000;

/// Why a plugin's Headpart Meshes cannot be read.
#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    /// The file could not be opened or read.
    #[error("the plugin could not be read: {0}")]
    Io(#[from] io::Error),
    /// HDPT framing runs past the end of what encloses it: the file, the
    /// group or the record.
    #[error("its HDPT {0} is truncated")]
    Truncated(&'static str),
    /// A compressed HDPT record cannot be decompressed.
    #[error("{0}")]
    Corrupt(String),
}

/// The most a compressed HDPT record may declare once decompressed. A real one
/// holds an editor id, a model path and a few small fields, well under a
/// kilobyte; the cap only keeps a corrupt size from allocating gigabytes.
const MAX_DECOMPRESSED: u32 = 16 * 1024 * 1024;

/// The Headpart Meshes `plugin`'s HDPT records name, in record order, each
/// `/`-separated and under `meshes/` (see [`modl_path`]).
///
/// Only the first top-level `HDPT` group is read; a game writes at most one.
///
/// # Errors
/// [`PluginError::Truncated`] when the HDPT group, one of its records or one
/// of their fields runs past the end of what encloses it, and
/// [`PluginError::Io`] when reading fails.
pub fn headparts<R: Read + Seek>(plugin: &mut R) -> Result<Vec<String>, PluginError> {
    let length = plugin.seek(SeekFrom::End(0))?;
    let Some(header) = read_header(plugin, 0, length)? else {
        return Ok(Vec::new());
    };
    if &header[..4] != b"TES4" {
        // Not a plugin.
        return Ok(Vec::new());
    }
    let mut at = HEADER as u64 + u64::from(u32_at(&header, 4));
    // Each top-level group needs a whole header. The walk ends at the first
    // entry it cannot read or that is not a group: past it, nothing says
    // where the next group starts.
    while let Some(header) = read_header(plugin, at, length)?
        && &header[..4] == b"GRUP"
    {
        // A group's size counts its own header.
        let size = u64::from(u32_at(&header, 4));
        if &header[8..12] != b"HDPT" {
            if size < HEADER as u64 {
                // A group smaller than its own header cannot be skipped
                // past, so the read ends here.
                break;
            }
            at += size;
            continue;
        }
        if size < HEADER as u64 || at + size > length {
            return Err(PluginError::Truncated("group"));
        }
        let mut group = vec![0; usize::try_from(size).expect("a u32 fits a usize") - HEADER];
        plugin.seek(SeekFrom::Start(at + HEADER as u64))?;
        plugin.read_exact(&mut group)?;
        return group_headparts(&group);
    }
    Ok(Vec::new())
}

/// The header at `at`, or `None` when the file ends before all of it.
fn read_header<R: Read + Seek>(
    plugin: &mut R,
    at: u64,
    length: u64,
) -> io::Result<Option<[u8; HEADER]>> {
    if at.saturating_add(HEADER as u64) > length {
        return Ok(None);
    }
    let mut header = [0; HEADER];
    plugin.seek(SeekFrom::Start(at))?;
    plugin.read_exact(&mut header)?;
    Ok(Some(header))
}

/// The little-endian `u32` at `at` in `bytes`, which holds all four of its
/// bytes.
fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes"))
}

/// The `length` bytes of `bytes` from `at`, or `None` when they run past its
/// end.
fn slice(bytes: &[u8], at: usize, length: usize) -> Option<&[u8]> {
    bytes.get(at..at.checked_add(length)?)
}

/// The MODL paths of every record in an HDPT group's contents.
fn group_headparts(group: &[u8]) -> Result<Vec<String>, PluginError> {
    let mut headparts = Vec::new();
    let mut at = 0;
    while at < group.len() {
        let header = slice(group, at, HEADER).ok_or(PluginError::Truncated("record header"))?;
        let size = u32_at(header, 4) as usize;
        let data = slice(group, at + HEADER, size).ok_or(PluginError::Truncated("record"))?;
        if u32_at(header, 8) & COMPRESSED != 0 {
            record_headparts(&decompress(data)?, &mut headparts)?;
        } else {
            record_headparts(data, &mut headparts)?;
        }
        at += HEADER + size;
    }
    Ok(headparts)
}

/// The fields of a compressed record: its `data` is their `u32` size, then
/// their zlib stream.
///
/// # Errors
/// [`PluginError::Truncated`] when `data` cannot hold the size, and
/// [`PluginError::Corrupt`] when the stream is not zlib, ends before the size
/// it declares, or declares more than [`MAX_DECOMPRESSED`].
fn decompress(data: &[u8]) -> Result<Vec<u8>, PluginError> {
    let size = slice(data, 0, 4).ok_or(PluginError::Truncated("compressed record"))?;
    let size = u32_at(size, 0);
    if size > MAX_DECOMPRESSED {
        return Err(PluginError::Corrupt(format!(
            "a compressed HDPT record declares {size} bytes"
        )));
    }
    let mut fields = Vec::new();
    // `take` stops a stream that inflates past its declared size; a shorter
    // one is caught below.
    flate2::read::ZlibDecoder::new(&data[4..])
        .take(u64::from(size))
        .read_to_end(&mut fields)
        .map_err(|error| PluginError::Corrupt(format!("a compressed HDPT record: {error}")))?;
    if fields.len() != size as usize {
        return Err(PluginError::Corrupt(format!(
            "a compressed HDPT record holds {} of its {size} bytes",
            fields.len()
        )));
    }
    Ok(fields)
}

/// Appends the MODL paths among one record's fields to `headparts`.
fn record_headparts(fields: &[u8], headparts: &mut Vec<String>) -> Result<(), PluginError> {
    let mut at = 0;
    while at < fields.len() {
        let header =
            slice(fields, at, FIELD_HEADER).ok_or(PluginError::Truncated("field header"))?;
        let size = usize::from(u16::from_le_bytes([header[4], header[5]]));
        let data = slice(fields, at + FIELD_HEADER, size).ok_or(PluginError::Truncated("field"))?;
        if &header[..4] == b"MODL" {
            headparts.push(modl_path(data));
        }
        at += FIELD_HEADER + size;
    }
    Ok(())
}

/// The Headpart Mesh a MODL field names: the text up to the first NUL,
/// decoded with [`decode_game_text`], under `meshes/` unless its first folder
/// already is `meshes` in any case, then cleaned with [`clean_path`].
///
/// Only a whole first folder counts, so `MeshesHD/hair.nif` is placed under
/// `meshes/` like any other path relative to it (audit BE-13).
fn modl_path(data: &[u8]) -> String {
    let text = data.split(|&byte| byte == 0).next().unwrap_or_default();
    let text = decode_game_text(text);
    let bytes = text.as_bytes();
    let under_meshes = bytes.len() > 6
        && bytes[..6].eq_ignore_ascii_case(b"meshes")
        && matches!(bytes[6], b'/' | b'\\');
    if under_meshes {
        clean_path(&text)
    } else {
        clean_path(&format!("meshes/{text}"))
    }
}

/// The characters Windows-1252 gives the bytes 0x80 to 0x9F; the five
/// bytes it leaves undefined map to the C1 control of the same value, as
/// Windows' own conversion maps them. Every other byte is the Latin-1
/// character of the same value.
const WINDOWS_1252_HIGH: [char; 32] = [
    '\u{20AC}', '\u{81}', '\u{201A}', '\u{192}', '\u{201E}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{2C6}', '\u{2030}', '\u{160}', '\u{2039}', '\u{152}', '\u{8D}', '\u{17D}', '\u{8F}',
    '\u{90}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{2DC}', '\u{2122}', '\u{161}', '\u{203A}', '\u{153}', '\u{9D}', '\u{17E}', '\u{178}',
];

/// Decodes a path stored in a plugin or a NIF: as UTF-8 when the bytes are
/// valid UTF-8, and otherwise as Windows-1252, the code page the games' tools
/// write paths in, so a path with an accented letter still names its file
/// (audit BE-13). Decoding the ANSI bytes lossily as UTF-8 would turn each
/// such letter into U+FFFD, which no file name holds.
pub(crate) fn decode_game_text(bytes: &[u8]) -> std::borrow::Cow<'_, str> {
    match std::str::from_utf8(bytes) {
        Ok(text) => std::borrow::Cow::Borrowed(text),
        Err(_) => bytes
            .iter()
            .map(|&byte| match byte {
                0x80..=0x9F => WINDOWS_1252_HIGH[usize::from(byte - 0x80)],
                _ => char::from(byte),
            })
            .collect::<String>()
            .into(),
    }
}

/// Qt's `QDir::cleanPath` on Windows: `\` becomes `/`, repeated separators
/// and `.` components go, each `..` removes the component before it (one
/// with nothing before it in a relative path is kept), and no separator is
/// left at the end. An empty result is `.`, unless `path` was empty.
pub(crate) fn clean_path(path: &str) -> String {
    let path = path.replace('\\', "/");
    if path.is_empty() {
        return path;
    }
    let absolute = path.starts_with('/');
    let mut components: Vec<&str> = Vec::new();
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => match components.last() {
                Some(&last) if last != ".." => {
                    components.pop();
                }
                // Nothing lies above the root.
                _ if absolute => {}
                _ => components.push(".."),
            },
            component => components.push(component),
        }
    }
    let joined = components.join("/");
    match (absolute, joined.is_empty()) {
        (true, _) => format!("/{joined}"),
        (false, true) => ".".to_owned(),
        (false, false) => joined,
    }
}
