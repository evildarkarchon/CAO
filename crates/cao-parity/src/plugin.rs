//! The bytes of a `plugin` recipe entry (#529): a port of the C++ tests'
//! `writeHeadpartPlugin` (`tests/MainOptimizerTests.cpp:152-173`), or a game's
//! Dummy Plugin from `cao-archive`'s tables.
//!
//! The materialiser writes them, and the deviation guard reads them before
//! anything is written, so both see the same bytes.

use cao_archive::Settings;

use crate::guard::PLUGIN_EXTENSIONS;
use crate::recipe::PluginEntry;

/// The size of a record or group header.
const HEADER: u32 = 24;
/// The size of a field header: its signature and a `u16` size.
const FIELD_HEADER: u32 = 6;
/// C++'s HDPT parser copies a MODL field into a `char buffer[1024]`, so a
/// field this long or longer is deviation 17's overflow.
const MODL_BUFFER: usize = 1024;

/// Builds a [`PluginEntry`]'s bytes, before any fault.
///
/// # Errors
/// A message for a plugin the recipe vocabulary cannot write: a path that is
/// not a plugin's, a Dummy Plugin with headparts or extra groups, a
/// `flip_byte` without `dummy` or past its end, a MODL path holding a NUL or
/// too long for C++'s buffer, or an extra group label that is not four ASCII
/// characters or is `HDPT`.
pub(crate) fn plugin_bytes(plugin: &PluginEntry) -> Result<Vec<u8>, String> {
    let extension = plugin
        .path
        .rsplit_once('.')
        .map_or("", |(_, extension)| extension);
    if !PLUGIN_EXTENSIONS
        .iter()
        .any(|known| extension.eq_ignore_ascii_case(known))
    {
        return Err("a plugin's path must end in `.esp`, `.esm` or `.esl`".into());
    }
    if let Some(game) = plugin.dummy {
        if !plugin.headparts.is_empty() || !plugin.extra_groups.is_empty() {
            return Err("a Dummy Plugin takes no `headparts` or `extra_groups`".into());
        }
        let mut bytes = Settings::get(game.game()).dummy_plugin.to_vec();
        if let Some(offset) = plugin.flip_byte {
            let length = bytes.len();
            let byte = bytes.get_mut(offset).ok_or_else(|| {
                format!("`flip_byte` {offset} is past the Dummy Plugin's {length} bytes")
            })?;
            *byte ^= 0x01;
        }
        return Ok(bytes);
    }
    if plugin.flip_byte.is_some() {
        return Err("`flip_byte` makes a near-dummy, so it needs `dummy`".into());
    }

    let mut bytes = header(b"TES4", 0, None);
    for label in &plugin.extra_groups {
        let label: &[u8; 4] = label
            .as_bytes()
            .try_into()
            .ok()
            .filter(|label: &&[u8; 4]| label.is_ascii() && *label != b"HDPT")
            .ok_or_else(|| {
                format!("extra group `{label}` is not a four-character ASCII label other than HDPT")
            })?;
        bytes.extend(header(b"GRUP", HEADER, Some(label)));
    }
    if plugin.headparts.is_empty() {
        return Ok(bytes);
    }
    let mut records = Vec::new();
    for model in &plugin.headparts {
        if model.contains('\0') {
            return Err(format!("headpart `{model}` holds a NUL"));
        }
        // The field holds the path and its terminating NUL.
        let field_size = model.len() + 1;
        if field_size >= MODL_BUFFER {
            return Err(format!(
                "headpart `{model}` makes a MODL field of {field_size} bytes, which overflows \
                 C++'s {MODL_BUFFER}-byte buffer (deviation 17)"
            ));
        }
        let field_size = u16::try_from(field_size).expect("under 1024");
        records.extend(header(b"HDPT", FIELD_HEADER + u32::from(field_size), None));
        records.extend_from_slice(b"MODL");
        records.extend_from_slice(&field_size.to_le_bytes());
        records.extend_from_slice(model.as_bytes());
        records.push(0);
    }
    // A group's size counts its own header.
    let group_size = HEADER + u32::try_from(records.len()).map_err(|_| "too many headparts")?;
    bytes.extend(header(b"GRUP", group_size, Some(b"HDPT")));
    bytes.extend(records);
    Ok(bytes)
}

/// A 24-byte record header (a signature, its data size, then zeros for the
/// flags, form id and version fields), or, with a `label`, a group header
/// (`GRUP`, its size counting the header, the label, then zeros).
fn header(signature: &[u8; 4], size: u32, label: Option<&[u8; 4]>) -> Vec<u8> {
    let mut header = Vec::with_capacity(HEADER as usize);
    header.extend_from_slice(signature);
    header.extend_from_slice(&size.to_le_bytes());
    header.extend_from_slice(label.unwrap_or(&[0; 4]));
    header.extend_from_slice(&[0; 12]);
    header
}
