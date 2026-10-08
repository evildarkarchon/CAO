//! Volume identity, free space, and MSVC-canonical text for volumes with no
//! drive letter, against the host's real volumes.

mod common;

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use cao_winfs::{available_space, msvc_canonical, volume_guid_path, volume_mount_point};
use common::scratch_dir;

#[test]
fn a_scratch_directory_is_on_its_drive_root_volume() {
    let dir = scratch_dir("volume-mount");
    let nested = dir.join("a").join("b");
    std::fs::create_dir_all(&nested).unwrap();

    let mount = volume_mount_point(&nested).unwrap();
    let text = mount.to_str().unwrap();
    assert!(text.ends_with('\\'), "{text}");
    assert!(dir.starts_with(&mount), "{text}");

    let guid = volume_guid_path(&nested).unwrap();
    let guid_text = guid.to_str().unwrap();
    assert!(
        guid_text.starts_with(r"\\?\Volume{") && guid_text.ends_with("}\\"),
        "{guid_text}"
    );
    assert_eq!(volume_guid_path(&dir).unwrap(), guid);
}

#[test]
fn available_space_reads_the_volume() {
    let dir = scratch_dir("volume-space");
    assert!(available_space(&dir).unwrap() > 0);
    assert!(available_space(&dir.join("missing")).is_err());
}

/// Each volume's GUID path with its DOS mount points.
fn volumes() -> Vec<(PathBuf, Vec<String>)> {
    use windows_sys::Win32::Foundation::{INVALID_HANDLE_VALUE, MAX_PATH};
    use windows_sys::Win32::Storage::FileSystem::{
        FindFirstVolumeW, FindNextVolumeW, FindVolumeClose, GetVolumePathNamesForVolumeNameW,
    };

    let mut volumes = Vec::new();
    let mut name = [0u16; MAX_PATH as usize];
    // SAFETY: every buffer passed holds the length given with it, and the
    // find handle is closed once enumeration ends.
    unsafe {
        let find = FindFirstVolumeW(name.as_mut_ptr(), MAX_PATH);
        assert!(find != INVALID_HANDLE_VALUE, "FindFirstVolumeW failed");
        loop {
            let mut mounts = vec![0u16; 4096];
            let mut length = 0;
            let listed = GetVolumePathNamesForVolumeNameW(
                name.as_ptr(),
                mounts.as_mut_ptr(),
                mounts.len() as u32,
                &mut length,
            );
            let end = name.iter().position(|&unit| unit == 0).unwrap();
            let volume = PathBuf::from(OsString::from_wide(&name[..end]));
            let mounts = if listed == 0 {
                Vec::new()
            } else {
                mounts[..length as usize]
                    .split(|&unit| unit == 0)
                    .filter(|mount| !mount.is_empty())
                    .map(String::from_utf16_lossy)
                    .collect()
            };
            volumes.push((volume, mounts));
            if FindNextVolumeW(find, name.as_mut_ptr(), MAX_PATH) == 0 {
                break;
            }
        }
        FindVolumeClose(find);
    }
    volumes
}

/// MSVC `canonical` names a volume with no DOS name through
/// `\\?\GLOBALROOT\Device\...\` and a lettered one by its drive root; this was
/// confirmed against `canonical_probe volumes` on the recording host. A host
/// whose every volume has a drive letter only exercises the lettered form.
#[test]
fn volume_roots_canonicalize_as_msvc_does() {
    let mut checked_without_letter = 0;
    for (volume, mounts) in volumes() {
        let Ok(canonical) = msvc_canonical(&volume) else {
            // Removable drives with no media, for example, cannot be opened.
            continue;
        };
        let text = canonical.to_str().unwrap().to_owned();
        if mounts.is_empty() {
            assert!(
                text.starts_with(r"\\?\GLOBALROOT\Device\") && text.ends_with('\\'),
                "{}: {text}",
                volume.display()
            );
            checked_without_letter += 1;
        } else {
            assert!(
                mounts.contains(&text),
                "{}: {text} not in {mounts:?}",
                volume.display()
            );
        }
    }
    eprintln!("checked {checked_without_letter} volume(s) with no mount point");
}
