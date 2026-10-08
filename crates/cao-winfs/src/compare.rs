//! Windows filename equivalence (#463).
//!
//! `CompareStringOrdinal` with `bIgnoreCase` uppercases through the OS table
//! one UTF-16 unit at a time, which is how NTFS compares names. Rust has no
//! equivalent: its full Unicode case mappings can expand a character
//! (`ß` uppercases to `SS`), which C++ CAO explicitly avoids.

use std::cmp::Ordering;
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

use windows_sys::Win32::Globalization::{CSTR_EQUAL, CompareStringOrdinal};

/// Compares two names or paths as a case-insensitive Windows volume would:
/// ordinal, not linguistic, with no case expansion.
///
/// # Panics
///
/// Panics if either side exceeds `i32::MAX` UTF-16 units or Windows rejects
/// the comparison; C++ throws in both cases.
pub fn compare_ordinal_ignore_case(left: impl AsRef<OsStr>, right: impl AsRef<OsStr>) -> Ordering {
    let left: Vec<u16> = left.as_ref().encode_wide().collect();
    let right: Vec<u16> = right.as_ref().encode_wide().collect();
    let length = |units: &[u16]| {
        i32::try_from(units.len()).expect("a compared name exceeds Windows comparison limits")
    };
    // SAFETY: each pointer is valid for the length passed with it, and Windows
    // reads no further, so neither side needs a terminator.
    let result = unsafe {
        CompareStringOrdinal(
            left.as_ptr(),
            length(&left),
            right.as_ptr(),
            length(&right),
            // bIgnoreCase: the docs reject any non-zero value other than TRUE.
            1,
        )
    };
    // 0 is failure; CSTR_LESS_THAN, CSTR_EQUAL and CSTR_GREATER_THAN are 1, 2
    // and 3, so subtracting CSTR_EQUAL leaves the ordering's sign.
    assert!(
        result != 0,
        "CompareStringOrdinal failed: {}",
        std::io::Error::last_os_error()
    );
    (result - CSTR_EQUAL).cmp(&0)
}

/// Orders a name by [`compare_ordinal_ignore_case`], so a `BTreeMap` or
/// `BTreeSet` keyed by it treats Windows-equivalent spellings as one key.
///
/// There is deliberately no `Hash`: a hash consistent with this equality would
/// need the OS uppercase table.
#[derive(Clone, Copy, Debug)]
pub struct OrdinalIgnoreCase<T>(pub T);

impl<T: AsRef<OsStr>> PartialEq for OrdinalIgnoreCase<T> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl<T: AsRef<OsStr>> Eq for OrdinalIgnoreCase<T> {}

impl<T: AsRef<OsStr>> PartialOrd for OrdinalIgnoreCase<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T: AsRef<OsStr>> Ord for OrdinalIgnoreCase<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        compare_ordinal_ignore_case(&self.0, &other.0)
    }
}
