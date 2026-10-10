//! The HDPT reader, ported from C++ `PluginsOperations::listHeadparts` (#505).
//!
//! A plugin is a `TES4` header record followed by top-level groups. Each MODL
//! field of a record in the `HDPT` group names a Headpart Mesh. [`headparts`]
//! reads them as C++ did, `/`-separated under `meshes/`, with deviation 17's
//! fixes:
//!
//! - **No overflow.** C++ copied each MODL field into a 1024-byte buffer and
//!   read it up to a NUL that might not be there. Here a MODL path ends at its
//!   first NUL or at the end of its field, whatever its length.
//! - **No hang.** C++ looped forever on HDPT framing that ran past the end of
//!   the file or of its group. Here every header is checked against what
//!   encloses it, and each step moves past at least one header, so a read
//!   always ends. A truncated HDPT group, record or field makes the plugin
//!   unreadable ([`PluginError::Truncated`]).
//! - **Compressed records.** C++ read a compressed record's zlib stream as if
//!   it were fields. Here it is decompressed first.
//!
//! Outside the HDPT group nothing changes: a file that does not start with
//! `TES4` names no Headpart Mesh, and framing cut short before the HDPT group
//! only ends the read early, as it ended C++'s.

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
/// Only the first top-level `HDPT` group is read, as C++ read only that one;
/// a game writes at most one.
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
    // Each top-level group needs a whole header. As in C++, the walk ends at
    // the first entry it cannot read or that is not a group.
    while let Some(header) = read_header(plugin, at, length)?
        && &header[..4] == b"GRUP"
    {
        // A group's size counts its own header.
        let size = u64::from(u32_at(&header, 4));
        if &header[8..12] != b"HDPT" {
            if size < HEADER as u64 {
                // C++'s unsigned skip wrapped around here, which ended its read.
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

/// The Headpart Mesh a MODL field names, as C++ built it: the text up to the
/// first NUL, read as UTF-8 (Qt's `QString(const char*)`), under `meshes/`
/// unless it already starts with `meshes` in any case, then cleaned with
/// [`clean_path`].
fn modl_path(data: &[u8]) -> String {
    let text = data.split(|&byte| byte == 0).next().unwrap_or_default();
    let text = String::from_utf8_lossy(text);
    let under_meshes = text
        .get(..6)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("meshes"));
    if under_meshes {
        clean_path(&text)
    } else {
        clean_path(&format!("meshes/{text}"))
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
