//! Opening pins, reading their facts, and the staging owner lock, against real
//! directories.

mod common;

use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

use cao_winfs::{
    Access, FileFacts, FileIdentity, Open, OwnerLock, OwnerLockError, Share, is_sharing_violation,
};
use common::{scratch_dir, write};

const ERROR_FILE_EXISTS: i32 = windows_sys::Win32::Foundation::ERROR_FILE_EXISTS as i32;

/// An ordinary writer, as another process would open the file.
fn open_for_write(path: &Path) -> std::io::Result<std::fs::File> {
    OpenOptions::new().write(true).open(path)
}

#[test]
fn a_read_pin_denies_writers_and_renames_but_not_readers() {
    let dir = scratch_dir("read-pin");
    let source = dir.join("source.dds");
    write(&source, b"texture");

    let _pin = Open::new(Access::READ, Share::READ).open(&source).unwrap();

    let writer = open_for_write(&source).unwrap_err();
    assert!(is_sharing_violation(&writer), "{writer}");
    let rename = std::fs::rename(&source, dir.join("moved.dds")).unwrap_err();
    assert!(is_sharing_violation(&rename), "{rename}");
    assert_eq!(std::fs::read(&source).unwrap(), b"texture");
}

/// Attribute-only access takes no part in sharing checks, which is why every
/// C++ pin also asks for read access.
#[test]
fn an_attribute_only_open_does_not_deny_renames() {
    let dir = scratch_dir("attribute-pin");
    let source = dir.join("source.dds");
    write(&source, b"texture");

    let _pin = Open::new(Access::READ_ATTRIBUTES, Share::READ)
        .open(&source)
        .unwrap();

    std::fs::rename(&source, dir.join("moved.dds")).unwrap();
}

#[test]
fn a_directory_pin_denies_renaming_the_directory() {
    let dir = scratch_dir("directory-pin");
    let parent = dir.join("textures");
    std::fs::create_dir(&parent).unwrap();

    let pin = Open::new(
        Access::LIST_DIRECTORY | Access::READ_ATTRIBUTES,
        Share::READ | Share::WRITE,
    )
    .directory()
    .open(&parent)
    .unwrap();

    let rename = std::fs::rename(&parent, dir.join("moved")).unwrap_err();
    assert!(is_sharing_violation(&rename), "{rename}");
    // The pin still allows work inside the directory.
    write(&parent.join("child.dds"), b"texture");
    assert!(FileFacts::of(&pin).unwrap().is_directory());
}

#[test]
fn create_new_never_replaces_an_entry() {
    let dir = scratch_dir("create-new");
    let path = dir.join("scratch.manifest");

    let mut created = Open::new(Access::WRITE, Share::NONE)
        .create_new()
        .write_through()
        .open(&path)
        .unwrap();
    created.write_all(b"v3").unwrap();
    created.sync_all().unwrap();
    drop(created);

    let error = Open::new(Access::WRITE, Share::NONE)
        .create_new()
        .open(&path)
        .unwrap_err();
    assert_eq!(error.raw_os_error(), Some(ERROR_FILE_EXISTS));

    // Opening the existing entry for writing does not truncate it either.
    let mut existing = Open::new(Access::READ | Access::WRITE, Share::NONE)
        .open(&path)
        .unwrap();
    let mut bytes = Vec::new();
    existing.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"v3");
}

#[test]
fn facts_report_links_and_size() {
    let dir = scratch_dir("facts-links");
    let first = dir.join("first.nif");
    write(&first, b"12345");
    std::fs::hard_link(&first, dir.join("second.nif")).unwrap();

    let file = Open::new(Access::READ, Share::READ).open(&first).unwrap();
    let facts = FileFacts::of(&file).unwrap();
    assert_eq!(facts.link_count(), 2);
    assert_eq!(facts.size(), 5);
    assert!(facts.is_ordinary_file());
}

/// The change time catches an edit that keeps the size and restores the
/// last-write time, which C++ relies on to notice in-place edits.
#[test]
fn unchanged_since_catches_in_place_edits() {
    let dir = scratch_dir("facts-change");
    let path = dir.join("source.ba2");
    write(&path, b"before");
    let pin = || {
        Open::new(Access::READ, Share::READ | Share::WRITE)
            .open(&path)
            .unwrap()
    };
    let earlier = FileFacts::of(&pin()).unwrap();

    assert!(FileFacts::of(&pin()).unwrap().unchanged_since(&earlier));

    // Let the clock move on so the edit gets a new change time.
    std::thread::sleep(Duration::from_millis(50));
    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
    let mut writer = open_for_write(&path).unwrap();
    writer.write_all(b"after!").unwrap();
    writer.set_modified(modified).unwrap();
    drop(writer);

    let current = FileFacts::of(&pin()).unwrap();
    assert_eq!(current.size(), earlier.size());
    assert!(!current.unchanged_since(&earlier));
}

#[test]
fn identities_follow_the_file_object() {
    let dir = scratch_dir("identity");
    let first = dir.join("first.nif");
    let other = dir.join("other.nif");
    let link = dir.join("link.nif");
    write(&first, b"a");
    write(&other, b"a");
    std::fs::hard_link(&first, &link).unwrap();

    let identity = |path: &Path| {
        let file = Open::new(Access::READ_ATTRIBUTES, Share::READ)
            .open(path)
            .unwrap();
        FileIdentity::of(&file).unwrap()
    };
    let first_identity = identity(&first);
    assert!(first_identity.is_full_file_id(), "NTFS provides FileIdInfo");
    assert_eq!(identity(&first), first_identity);
    assert_eq!(identity(&link), first_identity);
    assert_ne!(identity(&other), first_identity);
}

#[test]
fn a_second_owner_lock_open_is_a_sharing_violation() {
    let dir = scratch_dir("owner-lock");
    let path = dir.join("owner.lock");

    let held = OwnerLock::create(&path).unwrap();

    match OwnerLock::open_existing(&path) {
        Err(OwnerLockError::Active(error)) => assert!(is_sharing_violation(&error), "{error}"),
        other => panic!("expected StagingActive, got {other:?}"),
    }
    // Any other share-mode-0 open, such as a C++ run's, fails the same way.
    let raw = Open::new(Access::READ, Share::NONE)
        .open(&path)
        .unwrap_err();
    assert!(is_sharing_violation(&raw), "{raw}");

    drop(held);
    OwnerLock::open_existing(&path).unwrap();
}

#[test]
fn creating_an_owner_lock_over_an_entry_fails() {
    let dir = scratch_dir("owner-lock-exists");
    let path = dir.join("owner.lock");
    write(&path, b"");

    match OwnerLock::create(&path) {
        Err(OwnerLockError::Open(error)) => {
            assert_eq!(error.raw_os_error(), Some(ERROR_FILE_EXISTS));
        }
        other => panic!("expected an open failure, got {other:?}"),
    }
}

#[test]
fn a_hard_linked_owner_lock_is_not_ordinary() {
    let dir = scratch_dir("owner-lock-linked");
    let path = dir.join("owner.lock");
    write(&path, b"");
    std::fs::hard_link(&path, dir.join("alias")).unwrap();

    assert!(matches!(
        OwnerLock::open_existing(&path),
        Err(OwnerLockError::NotOrdinary)
    ));
}

#[test]
fn a_missing_owner_lock_is_an_open_failure() {
    let dir = scratch_dir("owner-lock-missing");
    assert!(matches!(
        OwnerLock::open_existing(&dir.join("owner.lock")),
        Err(OwnerLockError::Open(_))
    ));
}
