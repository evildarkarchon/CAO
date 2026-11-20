//! The host dependence WS-1 removed: a *plain* path past `MAX_PATH` reaches
//! Win32 only when the machine-wide `LongPathsEnabled` value is 1, even in
//! this `longPathAware` test binary. CAO's own raw calls pass such paths in
//! extended-length form instead (`extended_length_path`), which is why the
//! long-path scenarios in `mutation.rs` and `volumes.rs` pass either way.
//!
//! CI turns the value off before running the tests, so every long-path
//! scenario there proves the conversion; this test proves the switch reached
//! the test processes. It runs, and passes, on any host.

mod common;

use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use common::scratch_dir;
use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::Storage::FileSystem::{GetFileAttributesW, INVALID_FILE_ATTRIBUTES};
use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RegGetValueW};

/// The host's `LongPathsEnabled` value; a missing value means off, as on a
/// default install.
fn long_paths_enabled() -> bool {
    let wide = |text: &str| -> Vec<u16> { text.encode_utf16().chain([0]).collect() };
    let key = wide(r"SYSTEM\CurrentControlSet\Control\FileSystem");
    let name = wide("LongPathsEnabled");
    let mut value = 0u32;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: both strings are NUL-terminated, and `value` is a writable
    // DWORD whose size `size` states.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&raw mut value).cast(),
            &mut size,
        )
    };
    status == ERROR_SUCCESS && value == 1
}

/// Whether Win32 finds the existing `path` when given its plain text.
fn plain_path_resolves(path: &Path) -> bool {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    // SAFETY: `wide` is NUL-terminated and outlives the call.
    unsafe { GetFileAttributesW(wide.as_ptr()) != INVALID_FILE_ATTRIBUTES }
}

#[test]
fn plain_long_paths_reach_win32_only_when_the_host_enables_them() {
    let dir = scratch_dir("plain-long-path");
    let deep = dir.join("p".repeat(150)).join("q".repeat(150));
    // std reaches it on any host: it converts long paths itself.
    std::fs::create_dir_all(&deep).unwrap();
    assert!(deep.as_os_str().len() > 260);

    let enabled = long_paths_enabled();
    assert_eq!(
        plain_path_resolves(&deep),
        enabled,
        "LongPathsEnabled is {}, but a plain long path {} Win32",
        u8::from(enabled),
        if enabled { "did not reach" } else { "reached" }
    );
}
