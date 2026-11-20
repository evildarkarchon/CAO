//! An exclusive holder refuses every data open, whatever the open's flags.
//!
//! The Asset capture (audit CORE-2) relies on this: a file another program
//! holds without sharing must fail the capture's snapshot open, so the attempt
//! is a contained `StagingFailed` before anything is staged. On failure the
//! message lists every variant's result and the volume's file system, since the
//! answer has differed between hosts.

mod common;

use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::path::Path;

use cao_winfs::{Access, Open, Share, is_sharing_violation, volume_mount_point};
use common::{scratch_dir, write};
use windows_sys::Win32::Storage::FileSystem::GetVolumeInformationW;

/// The file system of the volume holding `path`, such as `NTFS` or `ReFS`.
fn file_system(path: &Path) -> String {
    let mount: Vec<u16> = volume_mount_point(path)
        .unwrap()
        .as_os_str()
        .encode_wide()
        .chain([0])
        .collect();
    let mut name = [0u16; 64];
    // SAFETY: `mount` is NUL-terminated, `name` holds the length passed, and
    // every other output is null.
    let read = unsafe {
        GetVolumeInformationW(
            mount.as_ptr(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            name.as_mut_ptr(),
            name.len() as u32,
        )
    };
    if read == 0 {
        return format!("unknown ({})", std::io::Error::last_os_error());
    }
    let length = name
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(name.len());
    String::from_utf16_lossy(&name[..length])
}

#[test]
fn an_exclusive_holder_refuses_every_data_open() {
    let dir = scratch_dir("sharing-exclusive");
    let path = dir.join("held.nif");
    write(&path, b"held");
    let _holder = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&path)
        .unwrap();

    let data = Access::READ_DATA | Access::READ_ATTRIBUTES;
    let every = Share::READ | Share::WRITE | Share::DELETE;
    let attempts = [
        (
            "snapshot: read data, share read, backup semantics",
            Open::new(data, Share::READ).directory().open(&path),
        ),
        (
            "read data, share read",
            Open::new(data, Share::READ).open(&path),
        ),
        (
            "read data, share all, backup semantics",
            Open::new(data, every).directory().open(&path),
        ),
        ("std File::open", std::fs::File::open(&path)),
    ];
    let report: Vec<String> = attempts
        .iter()
        .map(|(label, result)| match result {
            Ok(_) => format!("{label}: opened"),
            Err(error) => format!("{label}: {error}"),
        })
        .collect();
    let all_refused = attempts
        .iter()
        .all(|(_, result)| result.as_ref().is_err_and(is_sharing_violation));
    assert!(
        all_refused,
        "on {}:\n{}",
        file_system(&path),
        report.join("\n")
    );
}
