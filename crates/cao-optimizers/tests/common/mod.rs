//! Helpers shared by the composition-root scenarios: app directories with real
//! profiles, synthetic Textures, and tree snapshots.

#![allow(dead_code)] // Each test binary uses a different subset.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use cao_profiles::{Options, Profiles};
use directxtex::{CP_FLAGS_NONE, DDS_FLAGS_NONE, DXGI_FORMAT, ScratchImage};

pub mod plugin;

/// Serializes scenarios that start runs: one run may be active per process,
/// and a test binary runs its tests on parallel threads.
pub fn serial() -> MutexGuard<'static, ()> {
    static SERIAL: Mutex<()> = Mutex::new(());
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A fresh, empty directory under the target directory for one scenario.
pub fn scratch_dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("cao-optimizers")
        .join(name);
    // A missing directory is the expected case; anything else surfaces below.
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// An app directory holding a copy of the repository's shipped `profiles/`.
pub fn app_dir(name: &str) -> PathBuf {
    let app = scratch_dir(name);
    let shipped = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../profiles");
    copy_tree(&shipped, &app.join("profiles"));
    app
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// Replaces one `key=value` line of a profile's `profile.ini`.
pub fn edit_profile(app: &Path, profile: &str, key: &str, value: &str) {
    let path = app.join("profiles").join(profile).join("profile.ini");
    let text = std::fs::read_to_string(&path).unwrap();
    let prefix = format!("{key}=");
    assert!(text.lines().any(|line| line.starts_with(&prefix)), "{key}");
    let edited: Vec<String> = text
        .lines()
        .map(|line| {
            if line.starts_with(&prefix) {
                format!("{prefix}{value}")
            } else {
                line.to_owned()
            }
        })
        .collect();
    std::fs::write(&path, edited.join("\r\n")).unwrap();
}

/// The options a profile's `settings.ini` holds, as the GUI loads them.
pub fn profile_options(app: &Path, profile: &str) -> Options {
    Profiles::new(app)
        .open(profile)
        .load_options(&Options::default())
        .unwrap()
}

/// Dry Run options over the single Mod Root `mod_root`, with every Texture
/// option on and every other kind of work off.
pub fn dry_run_textures(app: &Path, mod_root: &Path) -> Options {
    let mut options = profile_options(app, "SSE");
    options.dry_run = true;
    options.user_path = mod_root.to_string_lossy().into_owned();
    options.textures_necessary = true;
    options.textures_compress = true;
    options.textures_mipmaps = true;
    options
}

/// Writes a `size`×`size` single-mip DDS in `format` with a gradient.
pub fn write_dds(path: &Path, format: DXGI_FORMAT, size: usize) {
    let mut scratch = ScratchImage::default();
    scratch
        .initialize_2d(format, size, size, 1, 1, CP_FLAGS_NONE)
        .unwrap();
    for (index, byte) in scratch.pixels_mut().iter_mut().enumerate() {
        *byte = (index * 7) as u8;
    }
    let blob = scratch.save_dds(DDS_FLAGS_NONE).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, blob.buffer()).unwrap();
}

/// Writes `bytes` at `root/relative`, creating parent directories.
pub fn write(root: &Path, relative: &str, bytes: &[u8]) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, bytes).unwrap();
}

/// The names left in `mod_root/.cao-staging`, sorted. After a clean Apply only
/// [`STAGING_CONTROL_FILES`] remain.
pub fn staging_leftovers(mod_root: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(mod_root.join(".cao-staging"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// The staging control files a run leaves in `.cao-staging`, sorted.
pub const STAGING_CONTROL_FILES: [&str; 2] = ["owner.lock", "ownership.manifest"];

/// Every file under `root` with its bytes, for proving a tree was not touched.
pub fn snapshot_tree(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                let bytes = std::fs::read(&path).unwrap();
                files.insert(path.strip_prefix(root).unwrap().to_path_buf(), bytes);
            }
        }
    }
    files
}
