//! A Profile's maximum Archive size (`[BSA] maxBsaUncompressedSize`): a whole
//! number of bytes, so no floating-point value leaves the read of `profile.ini`.

use crate::error::ProfileError;
use crate::ini::{IniFile, Value};

/// The largest uncompressed Archive a Profile creates, in whole bytes.
///
/// `profile.ini` has always stored it as a double (the shipped Profiles hold
/// `2104533975.04`); the packer truncated it to whole bytes, so the read does
/// the same and the fraction never reaches the model. A missing key reads as
/// `0`. Text that is not a number, or a negative, non-finite or overflowing
/// number, makes the Profile unreadable with
/// [`ProfileError::UnsupportedValue`]. Writing stores the bytes as integer
/// text, which older builds read as the same double.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct ArchiveSize(u64);

impl ArchiveSize {
    /// The size of `bytes` bytes.
    pub const fn from_bytes(bytes: u64) -> Self {
        Self(bytes)
    }

    /// The size in bytes.
    pub const fn bytes(self) -> u64 {
        self.0
    }

    /// Reads `key` (`Section/key`) of `profile.ini` as a size, truncating a
    /// fraction. A missing key reads as `0`, which the game's Archive-table
    /// maximum then raises, as it always has.
    ///
    /// # Errors
    /// [`ProfileError::UnsupportedValue`] when the value is not a number, or is
    /// negative, not finite or past [`u64::MAX`] bytes.
    pub(crate) fn read(ini: &IniFile, key: &str) -> Result<Self, ProfileError> {
        let value = ini.value(key);
        whole_bytes(value)
            .map(Self)
            .ok_or_else(|| ProfileError::unsupported_value(key, value, ACCEPTED))
    }
}

/// The whole bytes `value` holds, or `None` when it holds no size.
///
/// The text is read as `QVariant::toDouble()` reads it, with surrounding
/// whitespace allowed, except that text which does not parse is rejected
/// rather than read as `0`.
fn whole_bytes(value: &Value) -> Option<u64> {
    if matches!(value, Value::Invalid) {
        return Some(0);
    }
    let text = value.to_qstring();
    let text = text.trim();
    // Whole bytes, as this crate writes them, parse exactly: a double would
    // round sizes past 2^53, and turn `u64::MAX` into an overflow.
    if let Ok(bytes) = text.parse::<u64>() {
        return Some(bytes);
    }
    let size: f64 = text.parse().ok()?;
    // NaN and both infinities fall outside the range too. `as` truncates the
    // fraction, as the packer always has.
    (0.0..PAST_THE_LARGEST)
        .contains(&size)
        .then_some(size as u64)
}

/// What a rejection says the size accepts, phrased to follow "expected".
const ACCEPTED: &str = "a whole number of bytes from 0 to 18446744073709551615";

/// 2^64, the first double past [`u64::MAX`]; a double cannot hold `u64::MAX`
/// itself, so the bound is exclusive.
const PAST_THE_LARGEST: f64 = 18_446_744_073_709_551_616.0;
