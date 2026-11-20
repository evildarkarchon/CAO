//! Scratch directories for scenarios.

use std::path::PathBuf;

/// Makes `dir` a fresh, empty directory and returns it.
///
/// Whatever an earlier run left there is removed first, so every scenario
/// starts from nothing, and a scenario that names a stable directory reuses
/// it on the next run rather than leaving another beside it. Integration
/// tests pass a directory under `CARGO_TARGET_TMPDIR`, so `cargo clean`
/// removes it too.
///
/// # Panics
///
/// When the directory cannot be created.
pub fn fresh_dir(dir: PathBuf) -> PathBuf {
    // A missing directory is the expected case; anything else surfaces in create_dir_all.
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A fresh, empty directory at `relative` beneath the scratch root of
/// `cao-core`'s own unit tests, made by [`fresh_dir`].
///
/// Unit tests have no `CARGO_TARGET_TMPDIR`, so the root is found from the
/// test executable, which Cargo places at `<target>/<profile>/deps/`: the
/// directories go under `<target>/tmp/cao-core-unit`, beside the integration
/// tests' own. The name is stable across runs, so a rerun clears and reuses
/// it, `cargo clean` removes it, and each worktree's target directory keeps
/// its own.
///
/// # Panics
///
/// When the test executable's path is not that deep, or the directory cannot
/// be created.
#[cfg(test)]
pub(crate) fn unit_scratch_dir(relative: &str) -> PathBuf {
    let executable = std::env::current_exe().expect("the test executable has a path");
    let target = executable
        .ancestors()
        .nth(3)
        .expect("Cargo places unit tests in <target>/<profile>/deps");
    // Joined a component at a time, so the path is spelled with backslashes
    // only: a tool such as `mklink` reads a forward slash as a switch.
    let dir = relative.split('/').fold(
        target.join("tmp").join("cao-core-unit"),
        |dir, component| dir.join(component),
    );
    fresh_dir(dir)
}
