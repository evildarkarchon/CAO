//! Reparse points are rejected by attribute, whatever their tag.
//!
//! Junctions need no privilege, so those tests always run. File symlinks need
//! `SeCreateSymbolicLinkPrivilege` or Developer Mode; without it build.rs
//! leaves `cfg(symlink_privilege)` unset and those tests are reported as
//! ignored, never as passing.

mod common;

use std::os::windows::io::AsRawHandle;
use std::path::Path;

use cao_winfs::{
    Access, FileFacts, Open, OwnerLock, OwnerLockError, Share, is_reparse_point, msvc_canonical,
};
use common::{junction, scratch_dir, write};

const NEEDS_SYMLINKS: &str =
    "creating file symlinks needs SeCreateSymbolicLinkPrivilege or Developer Mode";

/// The directory pin every C++ ancestor walk opens.
fn directory_pin(path: &Path) -> std::fs::File {
    Open::new(
        Access::LIST_DIRECTORY | Access::READ_ATTRIBUTES,
        Share::READ | Share::WRITE,
    )
    .directory()
    .open(path)
    .unwrap()
}

#[test]
fn junctions_are_rejected() {
    let dir = scratch_dir("reparse-junction");
    let target = dir.join("target");
    let link = dir.join("link");
    std::fs::create_dir(&target).unwrap();
    junction(&link, &target);

    assert!(is_reparse_point(&std::fs::symlink_metadata(&link).unwrap()));
    assert!(!is_reparse_point(
        &std::fs::symlink_metadata(&target).unwrap()
    ));
    // Pins open the junction itself, so an ancestor walk sees it.
    let pin = directory_pin(&link);
    assert!(FileFacts::of(&pin).unwrap().is_reparse_point());
    assert!(is_reparse_point(&pin.metadata().unwrap()));
    assert!(
        !FileFacts::of(&directory_pin(&target))
            .unwrap()
            .is_reparse_point()
    );
}

#[test]
fn a_junction_cannot_stand_in_for_owner_lock() {
    let dir = scratch_dir("reparse-junction-lock");
    let target = dir.join("target");
    std::fs::create_dir(&target).unwrap();
    junction(&dir.join("owner.lock"), &target);

    let result = OwnerLock::open_existing(&dir.join("owner.lock"));
    assert!(
        matches!(
            result,
            Err(OwnerLockError::Open(_) | OwnerLockError::NotOrdinary)
        ),
        "{result:?}"
    );
}

#[test]
#[cfg_attr(
    not(symlink_privilege),
    ignore = "creating file symlinks needs SeCreateSymbolicLinkPrivilege or Developer Mode"
)]
fn file_symlinks_are_rejected() {
    let dir = scratch_dir("reparse-symlink");
    let target = dir.join("target.esp");
    let link = dir.join("link.esp");
    write(&target, b"plugin");
    std::os::windows::fs::symlink_file(&target, &link).expect(NEEDS_SYMLINKS);

    assert!(is_reparse_point(&std::fs::symlink_metadata(&link).unwrap()));
    let pin = Open::new(Access::READ, Share::READ).open(&link).unwrap();
    let facts = FileFacts::of(&pin).unwrap();
    assert!(facts.is_reparse_point());
    assert!(!facts.is_ordinary_file());
    // MSVC canonical follows links, as C++ did.
    assert_eq!(
        msvc_canonical(&link).unwrap(),
        msvc_canonical(&target).unwrap()
    );
}

#[test]
#[cfg_attr(
    not(symlink_privilege),
    ignore = "creating file symlinks needs SeCreateSymbolicLinkPrivilege or Developer Mode"
)]
fn a_file_symlink_cannot_stand_in_for_owner_lock() {
    let dir = scratch_dir("reparse-symlink-lock");
    let target = dir.join("target");
    write(&target, b"");
    std::os::windows::fs::symlink_file(&target, dir.join("owner.lock")).expect(NEEDS_SYMLINKS);

    assert!(matches!(
        OwnerLock::open_existing(&dir.join("owner.lock")),
        Err(OwnerLockError::NotOrdinary)
    ));
}

/// Sets a third-party reparse tag that is not a name surrogate. Unlike a
/// symlink tag, setting one needs only write access to the file.
fn set_custom_reparse_tag(path: &Path) {
    use windows_sys::Win32::Storage::FileSystem::REPARSE_GUID_DATA_BUFFER;
    use windows_sys::Win32::System::IO::DeviceIoControl;
    use windows_sys::Win32::System::Ioctl::FSCTL_SET_REPARSE_POINT;

    const TAG: u32 = 0x0000_1234; // No Microsoft bit, no name-surrogate bit.
    const DATA: [u8; 4] = *b"CAO!";
    let header = std::mem::offset_of!(REPARSE_GUID_DATA_BUFFER, GenericReparseBuffer);
    let mut buffer = vec![0u8; header + DATA.len()];
    buffer[..4].copy_from_slice(&TAG.to_le_bytes());
    buffer[4..6].copy_from_slice(&(DATA.len() as u16).to_le_bytes());
    // Bytes 8..24 are the GUID; any value names the third-party owner.
    buffer[8..24].copy_from_slice(b"cao-winfs-tests!");
    buffer[header..].copy_from_slice(&DATA);

    let file = Open::new(Access::WRITE, Share::NONE).open(path).unwrap();
    let mut returned = 0;
    // SAFETY: the handle stays open for the call, `buffer` holds the input
    // size passed, and there is no output buffer or overlapped I/O.
    let set = unsafe {
        DeviceIoControl(
            file.as_raw_handle(),
            FSCTL_SET_REPARSE_POINT,
            buffer.as_ptr().cast(),
            buffer.len() as u32,
            std::ptr::null_mut(),
            0,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    assert!(
        set != 0,
        "FSCTL_SET_REPARSE_POINT failed: {}",
        std::io::Error::last_os_error()
    );
}

/// `FileType::is_symlink` only reports name-surrogate tags, so this file
/// looks ordinary to it; the attribute test still rejects it.
#[test]
fn reparse_points_that_are_not_symlinks_are_rejected() {
    let dir = scratch_dir("reparse-custom");
    let path = dir.join("placeholder.dds");
    write(&path, b"");
    set_custom_reparse_tag(&path);

    let metadata = std::fs::symlink_metadata(&path).unwrap();
    assert!(!metadata.file_type().is_symlink());
    assert!(is_reparse_point(&metadata));
    let pin = Open::new(Access::READ_ATTRIBUTES, Share::READ)
        .open(&path)
        .unwrap();
    assert!(FileFacts::of(&pin).unwrap().is_reparse_point());
}
