//! Helpers shared by the filesystem-backed tests.

#![allow(dead_code)] // Each test binary uses a different subset.

use std::path::{Path, PathBuf};

/// The directory the transcripts under `tests/transcripts/` were captured in.
/// Each scenario ran with `CAPTURE/<scenario>` as the oracle's working
/// directory, so every absolute path in a transcript starts with it.
pub const CAPTURE: &str = "C:/Users/evild/AppData/Local/Temp/cao480/work";

/// Reads a captured oracle transcript: its stdout, byte for byte, CRLF included.
pub fn transcript(name: &str) -> String {
    let path = format!(
        "{}/tests/transcripts/{name}.stdout",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"))
}

/// Serializes tests that start a run: one run may be active per process, and
/// a test binary runs its tests on parallel threads.
pub fn serial() -> std::sync::MutexGuard<'static, ()> {
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

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

/// A pool with nothing pinned, for tests whose cases use no local assets.
pub fn empty_pool() -> &'static cao_parity::local_assets::LocalAssetPool {
    use cao_parity::local_assets::{LocalAssetPool, PinnedList};
    static POOL: std::sync::OnceLock<LocalAssetPool> = std::sync::OnceLock::new();
    POOL.get_or_init(|| LocalAssetPool::new(PathBuf::from("no-pool"), PinnedList::default()))
}

/// The repository's shipped `profiles/`.
pub fn shipped_profiles() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../profiles")
}

/// Copies a tree of plain files and directories from `from` to `to`.
pub fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// Writes `bytes` at `root/relative`, creating parent directories.
pub fn write(root: &Path, relative: &str, bytes: &[u8]) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, bytes).unwrap();
}
