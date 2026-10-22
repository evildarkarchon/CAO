//! Closed sets: `profile.ini` values CAO accepts only from a fixed list.
//!
//! An axis such as the Mesh Target's user version is an enum implementing
//! [`ClosedSet`]. [`read`] reads its key strictly, and a number outside the set
//! (or a missing key, which reads as `0`) makes the Profile unreadable with a
//! [`ProfileError::UnsupportedValue`] naming the key, its value and the accepted
//! values; [`read_list`] does the same for each entry of a list key. Writing
//! stores [`ClosedSet::number`], the number the file has always held, so files
//! stay readable by older builds.

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

    /// `members` with each repeat dropped, keeping every member where it first
    /// occurs: how a Profile holds a list of members, whether read from
    /// `profile.ini` or from the window.
    fn first_occurrences(members: impl IntoIterator<Item = Self>) -> Vec<Self> {
        let mut kept = Vec::new();
        for member in members {
            if !kept.contains(&member) {
                kept.push(member);
            }
        }
        kept
    }
}

/// Reads `key` of `profile.ini` (`Section/key`) as a member of `T`.
///
/// # Errors
/// [`ProfileError::UnsupportedValue`] when the value is not one of `T`'s numbers
/// (see [`member`]); a missing key reads as `0`, which no set holds.
pub(crate) fn read<T: ClosedSet>(ini: &IniFile, key: &str) -> Result<T, ProfileError> {
    let value = ini.value(key);
    member(value).ok_or_else(|| ProfileError::unsupported_value(key, value, T::accepted()))
}

/// Reads `key` of `profile.ini` (`Section/key`) as a list of members of `T`, in
/// the file's order, keeping only the first occurrence of a repeated member
/// ([`ClosedSet::first_occurrences`]).
///
/// The list has the shapes [`Value::list_entries`] reads, so a missing key or
/// an empty value is an empty list. Each entry is checked as [`read`] checks a
/// whole value, so an entry past 32 bits is rejected rather than wrapped into
/// the set, and text is rejected as written rather than read as `0`.
///
/// # Errors
/// [`ProfileError::UnsupportedValue`] for the first entry that is not one of
/// `T`'s numbers, carrying that entry as written; or for the whole value when
/// it is encoded in a way no list can be read from (a broken `@Variant(…)`),
/// since reading that as empty would let the next save overwrite it.
pub(crate) fn read_list<T: ClosedSet>(ini: &IniFile, key: &str) -> Result<Vec<T>, ProfileError> {
    let value = ini.value(key);
    let entries = value.list_entries().ok_or_else(|| {
        ProfileError::unsupported_value(
            key,
            value,
            format!("a list whose entries are each {}", T::accepted()),
        )
    })?;
    let members = entries
        .iter()
        .map(|entry| {
            member(entry).ok_or_else(|| {
                ProfileError::unsupported_value(
                    key,
                    entry,
                    format!("each entry to be {}", T::accepted()),
                )
            })
        })
        .collect::<Result<Vec<T>, _>>()?;
    // A harmless repeat is dropped rather than making the Profile unreadable.
    Ok(T::first_occurrences(members))
}

/// The member of `T` that `value` holds, if any.
///
/// The text is read as `QVariant::toLongLong()` reads it, so surrounding
/// whitespace is allowed, but text, a fraction or a number past 32 bits is not
/// a member: a narrower read would wrap `2^32 + 85` to `85` and accept it.
fn member<T: ClosedSet>(value: &Value) -> Option<T> {
    u32::try_from(value.to_i64()).ok().and_then(T::from_number)
}
