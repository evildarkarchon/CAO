//! Handle renames, identity-bound deletes and write-through moves, against
//! real directories.

mod common;

use std::io;
use std::path::Path;

use cao_winfs::{
    Access, Open, RenameMode, Share, delete_by_handle, move_file_write_through, rename_by_handle,
};
use common::{scratch_dir, write};

const ERROR_FILE_NOT_FOUND: i32 = windows_sys::Win32::Foundation::ERROR_FILE_NOT_FOUND as i32;
const ERROR_ALREADY_EXISTS: i32 = windows_sys::Win32::Foundation::ERROR_ALREADY_EXISTS as i32;

/// The staged-publication handle: exclusive, with the rights to flush and
/// rename.
fn publication_handle(path: &Path) -> std::fs::File {
    Open::new(
        Access::WRITE | Access::DELETE | Access::READ_ATTRIBUTES,
        Share::NONE,
    )
    .write_through()
    .open(path)
    .unwrap()
}

#[test]
fn no_replace_rename_publishes_to_a_free_name() {
    let dir = scratch_dir("rename-free");
    let staged = dir.join("staged.dds");
    let published = dir.join("published.dds");
    write(&staged, b"new");

    let handle = publication_handle(&staged);
    handle.sync_all().unwrap();
    rename_by_handle(&handle, &published, RenameMode::NoReplace).unwrap();
    drop(handle);

    assert!(!staged.exists());
    assert_eq!(std::fs::read(&published).unwrap(), b"new");
}

#[test]
fn no_replace_rename_refuses_an_occupied_name() {
    let dir = scratch_dir("rename-occupied");
    let staged = dir.join("staged.dds");
    let published = dir.join("published.dds");
    write(&staged, b"new");
    write(&published, b"old");

    let handle = publication_handle(&staged);
    let error = rename_by_handle(&handle, &published, RenameMode::NoReplace).unwrap_err();
    drop(handle);

    assert_eq!(error.raw_os_error(), Some(ERROR_ALREADY_EXISTS), "{error}");
    assert_eq!(std::fs::read(&staged).unwrap(), b"new");
    assert_eq!(std::fs::read(&published).unwrap(), b"old");
}

#[test]
fn replace_rename_replaces_an_existing_file() {
    let dir = scratch_dir("rename-replace");
    let staged = dir.join("staged.dds");
    let published = dir.join("published.dds");
    write(&staged, b"new");
    write(&published, b"old");

    let handle = publication_handle(&staged);
    rename_by_handle(&handle, &published, RenameMode::Replace).unwrap();
    drop(handle);

    assert!(!staged.exists());
    assert_eq!(std::fs::read(&published).unwrap(), b"new");
}

#[test]
fn rename_destinations_must_be_absolute() {
    let dir = scratch_dir("rename-relative");
    let staged = dir.join("staged.dds");
    write(&staged, b"new");

    let handle = publication_handle(&staged);
    let error =
        rename_by_handle(&handle, Path::new("published.dds"), RenameMode::NoReplace).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
}

/// The destination goes to Win32 without a `\\?\` prefix, as in C++, so this
/// also checks that the manifest's `longPathAware` reaches the test binary.
#[test]
fn renames_reach_paths_longer_than_max_path() {
    let dir = scratch_dir("rename-long");
    let deep = dir.join("d".repeat(120)).join("e".repeat(120));
    std::fs::create_dir_all(&deep).unwrap();
    let staged = dir.join("staged.dds");
    let published = deep.join("published.dds");
    assert!(published.as_os_str().len() > 260);
    write(&staged, b"new");

    let handle = publication_handle(&staged);
    rename_by_handle(&handle, &published, RenameMode::NoReplace).unwrap();
    drop(handle);

    assert_eq!(std::fs::read(&published).unwrap(), b"new");
}

/// The delete follows the object the handle verified, not whatever its old
/// path names by the time it runs.
#[test]
fn delete_by_handle_removes_the_opened_object_not_its_path() {
    let dir = scratch_dir("delete-identity");
    let source = dir.join("source.ba2");
    let moved = dir.join("moved.ba2");
    write(&source, b"verified");

    let handle = Open::new(
        Access::DELETE | Access::READ_ATTRIBUTES,
        Share::READ | Share::DELETE,
    )
    .open(&source)
    .unwrap();
    // Another process moves the verified file away and puts a new one in its place.
    std::fs::rename(&source, &moved).unwrap();
    write(&source, b"substitute");

    delete_by_handle(&handle).unwrap();
    drop(handle);

    assert!(!moved.exists());
    assert_eq!(std::fs::read(&source).unwrap(), b"substitute");
}

#[test]
fn write_through_moves_replace_the_destination() {
    let dir = scratch_dir("move-replace");
    let scratch = dir.join("ownership.manifest.tmp");
    let manifest = dir.join("ownership.manifest");
    write(&scratch, b"v3 new");
    write(&manifest, b"v3 old");

    move_file_write_through(&scratch, &manifest).unwrap();

    assert!(!scratch.exists());
    assert_eq!(std::fs::read(&manifest).unwrap(), b"v3 new");
}

#[test]
fn write_through_moves_report_a_missing_source() {
    let dir = scratch_dir("move-missing");
    let error =
        move_file_write_through(&dir.join("missing"), &dir.join("ownership.manifest")).unwrap_err();
    assert_eq!(error.raw_os_error(), Some(ERROR_FILE_NOT_FOUND));
}
