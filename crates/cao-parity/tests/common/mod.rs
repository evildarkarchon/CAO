//! Helpers shared by the filesystem-backed tests.

#![allow(dead_code)] // Each test binary uses a different subset.

use std::path::{Path, PathBuf};

/// A fresh directory under the system temp dir, removed on drop.
pub struct TempDir(PathBuf);

impl TempDir {
    /// Creates `<temp>/cao-parity-tests/<name>-<pid>`, replacing any leftover.
    pub fn new(name: &str) -> Self {
        let path = std::env::temp_dir()
            .join("cao-parity-tests")
            .join(format!("{name}-{}", std::process::id()));
        // A previous run of the same test may have been killed mid-way.
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        // Best effort: a failed cleanup must not hide the test's own result.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Writes `bytes` at `root/relative`, creating parent directories.
pub fn write(root: &Path, relative: &str, bytes: &[u8]) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, bytes).unwrap();
}
