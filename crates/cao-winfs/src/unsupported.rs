//! Recognising a file system that does not implement a query or a flag.

use std::io;

use windows_sys::Win32::Foundation::{
    ERROR_CALL_NOT_IMPLEMENTED, ERROR_INVALID_FUNCTION, ERROR_INVALID_PARAMETER,
    ERROR_NOT_SUPPORTED,
};

/// Whether `error` says the file system does not support the information
/// class asked for, rather than that the call failed on this file.
///
/// Windows reports an unimplemented class in several ways: FAT and some
/// network redirectors answer `ERROR_INVALID_PARAMETER` (an unknown class),
/// others `ERROR_INVALID_FUNCTION` or `ERROR_NOT_SUPPORTED`, and Wine
/// `ERROR_CALL_NOT_IMPLEMENTED`. A caller falls back to an older class only
/// on these, so a real failure such as access denied is never masked.
pub(crate) fn is_unsupported(error: &io::Error) -> bool {
    error.raw_os_error().is_some_and(|code| {
        [
            ERROR_INVALID_FUNCTION,
            ERROR_NOT_SUPPORTED,
            ERROR_INVALID_PARAMETER,
            ERROR_CALL_NOT_IMPLEMENTED,
        ]
        .contains(&(code as u32))
    })
}

#[cfg(test)]
mod tests {
    use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_SHARING_VIOLATION};

    use super::*;

    #[test]
    fn unimplemented_classes_are_unsupported() {
        for code in [
            ERROR_INVALID_FUNCTION,
            ERROR_NOT_SUPPORTED,
            ERROR_INVALID_PARAMETER,
            ERROR_CALL_NOT_IMPLEMENTED,
        ] {
            assert!(is_unsupported(&io::Error::from_raw_os_error(code as i32)));
        }
    }

    #[test]
    fn failures_on_the_file_are_not() {
        for code in [ERROR_ACCESS_DENIED, ERROR_SHARING_VIOLATION] {
            assert!(!is_unsupported(&io::Error::from_raw_os_error(code as i32)));
        }
        assert!(!is_unsupported(&io::Error::other("no OS code")));
    }
}
