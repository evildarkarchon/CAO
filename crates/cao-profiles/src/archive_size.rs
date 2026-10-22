//! A Profile's maximum Archive size (`[BSA] maxBsaUncompressedSize`): a whole
//! number of bytes, so no floating-point value leaves the read of `profile.ini`.

use crate::error::ProfileError;
use crate::ini::{IniFile, Value};

/// The largest uncompressed Archive a Profile creates, in whole bytes.
///
/// `profile.ini` has always stored it as a double (the shipped Profiles hold
/// `2104533975.04`); the packer truncated it to whole bytes, so the read does
/// the same and the fraction never reaches the model. A hand-edited Profile may
/// also give it with a unit, such as `8GB`, counted in 1024s like the window's
/// gigabytes: a copied Profile may raise its Archives well past the game's
/// table maximum (FO4's 4 GB is only the Creation Kit's limit), and a unit is
/// how a person writes such a size. A missing key reads as `0`. Any other text,
/// or a negative, non-finite or overflowing size, makes the Profile unreadable
/// with [`ProfileError::UnsupportedValue`].
///
/// A size read with a unit remembers its text, and writing puts that text back,
/// so a save that leaves the size alone leaves the user's line alone. Every
/// other size is written as integer bytes, which older builds read as the same
/// double; they read a unit as `0`, and so fall back to the game's maximum.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct ArchiveSize {
    bytes: u64,
    /// The text this size was read from, when that text has a unit.
    written: Option<String>,
}

impl ArchiveSize {
    /// The size of `bytes` bytes, written as integer bytes.
    pub const fn from_bytes(bytes: u64) -> Self {
        Self {
            bytes,
            written: None,
        }
    }

    /// The size in bytes.
    pub const fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Reads `key` (`Section/key`) of `profile.ini` as a size: whole or
    /// fractional bytes, or a number with a unit. A fraction of a byte is
    /// truncated. A missing key reads as `0`, which the game's Archive-table
    /// maximum then raises, as it always has.
    ///
    /// # Errors
    /// [`ProfileError::UnsupportedValue`] when the value is neither, or is
    /// negative, not finite or past [`u64::MAX`] bytes.
    pub(crate) fn read(ini: &IniFile, key: &str) -> Result<Self, ProfileError> {
        let value = ini.value(key);
        if matches!(value, Value::Invalid) {
            return Ok(Self::default());
        }
        let text = value.to_qstring();
        let text = text.trim();
        let unsupported = || ProfileError::unsupported_value(key, value, ACCEPTED);
        match split_unit(text) {
            None => whole_bytes(text)
                .map(Self::from_bytes)
                .ok_or_else(unsupported),
            Some((number, unit)) => {
                let bytes = unit_scale(unit)
                    .and_then(|scale| scaled_bytes(number, scale))
                    .ok_or_else(unsupported)?;
                Ok(Self {
                    bytes,
                    written: Some(text.to_owned()),
                })
            }
        }
    }

    /// The text `profile.ini` holds for this size: the user's text when it was
    /// read with a unit, otherwise integer bytes.
    pub(crate) fn to_written(&self) -> String {
        self.written
            .clone()
            .unwrap_or_else(|| self.bytes.to_string())
    }
}

/// Splits `text` into its number and its unit, the trailing run of letters,
/// with any space between them dropped; `None` when `text` has no unit.
///
/// A bare `inf` or `nan` is all letters, so it splits into an empty number and
/// is rejected as one, just as `-inf` and `1e400` are rejected as numbers.
fn split_unit(text: &str) -> Option<(&str, &str)> {
    let number = text.trim_end_matches(|c: char| c.is_ascii_alphabetic());
    (number.len() < text.len()).then(|| (number.trim_end(), &text[number.len()..]))
}

/// The bytes one `unit` stands for, in any case: `B`, then `KB`, `MB`, `GB`
/// and `TB`, each 1024 of the one before, as the window counts gigabytes. The
/// IEC spellings (`KiB` and so on) are accepted as the same units.
fn unit_scale(unit: &str) -> Option<u64> {
    let power = match unit.to_ascii_uppercase().as_str() {
        "B" => 0,
        "KB" | "KIB" => 1,
        "MB" | "MIB" => 2,
        "GB" | "GIB" => 3,
        "TB" | "TIB" => 4,
        _ => return None,
    };
    Some(1024_u64.pow(power))
}

/// The whole bytes in `number` units of `scale` bytes, or `None` when `number`
/// is not a size or the bytes overflow.
fn scaled_bytes(number: &str, scale: u64) -> Option<u64> {
    // A whole number of units multiplies exactly; a double would round sizes
    // past 2^53 bytes.
    if let Ok(units) = number.parse::<u64>() {
        return units.checked_mul(scale);
    }
    let units: f64 = number.parse().ok()?;
    in_range(units * scale as f64)
}

/// The whole bytes `text` holds, read as `QVariant::toDouble()` reads it,
/// except that text which does not parse is rejected rather than read as `0`.
fn whole_bytes(text: &str) -> Option<u64> {
    // Whole bytes, as this crate writes them, parse exactly: a double would
    // round sizes past 2^53, and turn `u64::MAX` into an overflow.
    if let Ok(bytes) = text.parse::<u64>() {
        return Some(bytes);
    }
    in_range(text.parse().ok()?)
}

/// `size` truncated to whole bytes, or `None` when it is negative, not finite
/// or past [`u64::MAX`].
fn in_range(size: f64) -> Option<u64> {
    // NaN and both infinities fall outside the range too. `as` truncates the
    // fraction, as the packer always has.
    (0.0..PAST_THE_LARGEST)
        .contains(&size)
        .then_some(size as u64)
}

/// What a rejection says the size accepts, phrased to follow "expected".
const ACCEPTED: &str = "a whole number of bytes from 0 to 18446744073709551615, or a size \
                        such as `8GB` in B, KB, MB, GB or TB, counted in 1024s";

/// 2^64, the first double past [`u64::MAX`]; a double cannot hold `u64::MAX`
/// itself, so the bound is exclusive.
const PAST_THE_LARGEST: f64 = 18_446_744_073_709_551_616.0;
