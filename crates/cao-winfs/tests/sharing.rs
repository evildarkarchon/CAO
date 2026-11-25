//! An exclusive holder refuses every data open CAO makes of a file.
//!
//! Staging relies on this twice: a live run's `owner.lock` must refuse
//! another claim, and an Asset another program holds without sharing must
//! refuse the destination snapshot, so the attempt is a contained
//! `StagingFailed` before anything is staged (audit CORE-2). Those opens never
//! use backup semantics, which an elevated administrator's process can open
//! past a share mode with (see `Open::directory`); the GitHub Actions runner
//! is such a process, so CI checks this as the app runs elevated.

mod common;

use std::os::windows::fs::OpenOptionsExt;
use std::time::{Duration, Instant};

use cao_winfs::{Access, Open, OwnerLock, OwnerLockError, Share, is_sharing_violation};
use common::{scratch_dir, write};

#[test]
fn an_exclusive_holder_refuses_every_data_open_of_a_file() {
    let dir = scratch_dir("sharing-exclusive");
    let path = dir.join("held.nif");
    write(&path, b"held");
    let _holder = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&path)
        .unwrap();

    let data = Access::READ_DATA | Access::READ_ATTRIBUTES;
    let opens = [
        (
            "the destination snapshot's open",
            Open::new(data, Share::READ).open(&path),
        ),
        (
            "a data open sharing everything",
            Open::new(data, Share::READ | Share::WRITE | Share::DELETE).open(&path),
        ),
        ("std File::open", std::fs::File::open(&path)),
    ];
    for (label, opened) in opens {
        assert!(
            opened.as_ref().is_err_and(is_sharing_violation),
            "{label}: {opened:?}"
        );
    }
    assert!(matches!(
        OwnerLock::open_existing(&path),
        Err(OwnerLockError::Active(_))
    ));
}

/// Holds `path` as a real-time scanner does for a moment after a run writes
/// it: open, sharing nothing.
fn hold_exclusively(path: &std::path::Path) -> std::fs::File {
    std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(path)
        .unwrap()
}

/// An exclusive open of a staged file waits out a scanner's brief hold
/// instead of failing the Asset (FS-5).
#[test]
fn open_retrying_waits_out_a_brief_exclusive_hold() {
    let dir = scratch_dir("sharing-retry-brief");
    let path = dir.join("staged.dds");
    write(&path, b"staged");
    let held = hold_exclusively(&path);
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        drop(held);
    });

    let opened = Open::new(Access::WRITE | Access::DELETE, Share::NONE).open_retrying(&path);
    release.join().unwrap();

    opened.unwrap();
}

/// A hold that outlasts the wait still refuses the open, with its sharing
/// violation, after about a second.
#[test]
fn open_retrying_returns_a_lasting_sharing_violation() {
    let dir = scratch_dir("sharing-retry-lasting");
    let path = dir.join("staged.dds");
    write(&path, b"staged");
    let _held = hold_exclusively(&path);

    let started = Instant::now();
    let error = Open::new(Access::WRITE | Access::DELETE, Share::NONE)
        .open_retrying(&path)
        .unwrap_err();

    assert!(is_sharing_violation(&error), "{error}");
    assert!(
        started.elapsed() >= Duration::from_millis(500),
        "{:?}",
        started.elapsed()
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
}

/// Only sharing violations wait: a missing file is reported at once.
#[test]
fn open_retrying_reports_other_errors_at_once() {
    let dir = scratch_dir("sharing-retry-missing");
    let started = Instant::now();
    let error = Open::new(Access::READ, Share::READ)
        .open_retrying(&dir.join("missing.dds"))
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "{:?}",
        started.elapsed()
    );
}
