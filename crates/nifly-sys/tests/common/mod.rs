//! Helpers shared by the nifly-sys integration tests.

#![allow(dead_code)] // Each test binary uses a different subset.

use std::path::{Path, PathBuf};

/// The path of one of nifly's own fixtures, e.g. `fixture("TestNifFile_Static_SE.nif")`.
pub fn fixture(file_name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("vendor/nifly/tests")
        .join(file_name)
}

/// A fresh, empty directory under the target directory for one test.
///
/// Each test names its own directory, so the harness's parallel threads never
/// share one.
pub fn scratch_dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("nifly-sys")
        .join(name);
    // A missing directory is the expected case; anything else surfaces in create_dir_all.
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Asserts two files hold the same bytes, reporting the first difference
/// rather than dumping both files.
pub fn assert_same_bytes(actual: &Path, expected: &Path) {
    let actual_bytes = std::fs::read(actual).unwrap();
    let expected_bytes = std::fs::read(expected).unwrap();
    if actual_bytes == expected_bytes {
        return;
    }
    let first_difference = actual_bytes
        .iter()
        .zip(&expected_bytes)
        .position(|(a, e)| a != e)
        .unwrap_or(actual_bytes.len().min(expected_bytes.len()));
    panic!(
        "{} differs from {}: {} vs {} bytes, first difference at offset {first_difference}",
        actual.display(),
        expected.display(),
        actual_bytes.len(),
        expected_bytes.len(),
    );
}
