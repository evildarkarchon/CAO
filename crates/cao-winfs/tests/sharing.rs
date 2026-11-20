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
