//! Closed sets: `profile.ini` values CAO accepts only from a fixed list.
//!
//! An axis such as the Mesh Target's user version is an enum implementing
//! [`ClosedSet`]. [`read`] reads its key strictly, and a number outside the set
//! (or a missing key, which reads as `0`) makes the Profile unreadable with a
//! [`ProfileError::UnsupportedValue`] naming the key, its value and the accepted
//! values. Writing stores [`ClosedSet::number`], the number the file has always
//! held, so files stay readable by older builds.

use crate::error::ProfileError;
use crate::ini::{IniFile, Value};

/// A `profile.ini` value with a closed set of accepted members, each stored as
/// a number.
pub trait ClosedSet: Copy + Eq + 'static {
    /// Every accepted member, in the order the GUI offers them.
    const ALL: &'static [Self];

    /// The number `profile.ini` stores for this member.
    fn number(self) -> u32;

    /// This member as a rejection lists it among the accepted values; its
    /// number unless a hint helps the reader pick one.
    fn describe(self) -> String {
        self.number().to_string()
    }

    /// The member stored as `number`, if any.
    fn from_number(number: u32) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|member| member.number() == number)
    }

    /// The accepted values as a rejection's message ends them, such as
    /// `one of 11, 12`.
    fn accepted() -> String {
        let members: Vec<String> = Self::ALL.iter().map(|member| member.describe()).collect();
        format!("one of {}", members.join(", "))
    }
}

/// Reads `key` of `profile.ini` (`Section/key`) as a member of `T`.
///
/// The text is read as `QVariant::toLongLong()` reads it, so surrounding
/// whitespace is allowed but text, a fraction or a number past 32 bits is not a
/// member. A missing key reads as `0`, which no set holds.
///
/// # Errors
/// [`ProfileError::UnsupportedValue`] when the value is not one of `T`'s numbers.
pub(crate) fn read<T: ClosedSet>(ini: &IniFile, key: &str) -> Result<T, ProfileError> {
    let value = ini.value(key);
    u32::try_from(value.to_i64())
        .ok()
        .and_then(T::from_number)
        .ok_or_else(|| ProfileError::UnsupportedValue {
            file: PROFILE_INI.to_owned(),
            key: key.to_owned(),
            value: raw(value),
            accepted: T::accepted(),
        })
}

/// The file every closed set is read from.
const PROFILE_INI: &str = "profile.ini";

/// The text of a value as written, or `None` when the key is missing.
fn raw(value: &Value) -> Option<String> {
    match value {
        Value::Invalid => None,
        value => Some(value.to_qstring()),
    }
}
