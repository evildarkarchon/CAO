//! Helpers shared by the cao-archive integration tests.

#![allow(dead_code)] // Each test binary uses a different subset.

use std::path::{Path, PathBuf};

/// A fresh, empty directory under the target directory for one test.
///
/// Each test names its own directory, so the harness's parallel threads never
/// share one.
pub fn scratch_dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("cao-archive")
        .join(name);
    // A missing directory is the expected case; anything else surfaces in create_dir_all.
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Writes `bytes` at `root/relative`, creating parent directories, and returns
/// the file's path.
pub fn write(root: &Path, relative: &str, bytes: &[u8]) -> PathBuf {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, bytes).unwrap();
    path
}
