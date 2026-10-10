//! A run's Headpart Meshes, ported from C++ `MeshesOptimizer::listHeadparts`
//! (#505).
//!
//! A Mesh is a Headpart Mesh when the profile's `customHeadparts.txt` lists
//! it, when an HDPT record in any plugin across the Mod Selection names it, or
//! when it lies on a facegen path. [`scan_headparts`] gathers the first two
//! into one [`HeadpartList`] for the whole run: a patch's plugin can name
//! meshes that live in another mod, so the list is not kept per Mod Root (the
//! one exception the glossary records on Mod Root). The facegen rule is
//! applied per Mesh by [`crate::meshes`].
//!
//! The plugin scan walks the selected folder itself, so mods with a Mod
//! Exclusion are still scanned, as in C++. Deviation 17 changes two things:
//! the scan skips the `.cao-staging` namespace, and a plugin whose HDPT
//! records cannot be read is returned as an [`UnreadablePlugin`] for the run
//! to report while the scan carries on.

use std::collections::HashSet;
use std::fs::File;
use std::path::{Path, PathBuf};

use cao_core::run::is_staging_name;

use crate::plugins::{self, PluginError, clean_path};

/// The Headpart Meshes a run's profile and plugins name, as `meshes/...`
/// paths.
///
/// Matching ignores case, as Qt's `QStringList::contains` did with
/// `Qt::CaseInsensitive`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HeadpartList {
    /// Each path folded with `to_lowercase`, so a lookup folds only the Mesh.
    folded: HashSet<String>,
}

impl HeadpartList {
    /// A list of `paths`, each cleaned as C++ cleaned them (`QDir::cleanPath`).
    pub fn new<I, S>(paths: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        Self {
            folded: paths
                .into_iter()
                .map(|path| clean_path(path.as_ref()).to_lowercase())
                .collect(),
        }
    }

    /// Whether the list names the Mesh at `game_path`, a `/`-separated path
    /// within its Mod Root such as `meshes/actors/hair.nif`.
    pub fn contains(&self, game_path: &str) -> bool {
        self.folded.contains(&game_path.to_lowercase())
    }
}

/// A plugin the scan found whose Headpart Meshes cannot be read.
#[derive(Debug)]
pub struct UnreadablePlugin {
    /// The plugin, as the scan found it beneath the selected folder.
    pub path: PathBuf,
    /// Why its HDPT records cannot be read.
    pub error: PluginError,
}

/// What [`scan_headparts`] found.
#[derive(Debug, Default)]
pub struct HeadpartScan {
    /// The profile's list and every readable plugin's MODL paths.
    pub headparts: HeadpartList,
    /// The plugins left out of the list, in scan order.
    pub unreadable: Vec<UnreadablePlugin>,
}

/// Cancellation observed during the scan; nothing was read past it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanCancelled;

/// Lists the run's Headpart Meshes: `custom`, the profile's
/// `customHeadparts.txt`, and the MODL paths of every plugin under
/// `selection`, the selected folder.
///
/// A plugin is a file whose name ends, in any case, in one of
/// `plugin_extensions` (each with its dot, as `cao-archive`'s per-game tables
/// list them). The walk skips the `.cao-staging` namespace (deviation 17) and
/// does not follow directory links or junctions, as Qt's `QDirIterator`
/// without `FollowSymlinks` did not; a folder it cannot read is passed over.
/// An empty `custom` is logged as C++ logged it.
///
/// # Errors
/// [`ScanCancelled`] as soon as `cancelled` reports true; it is polled for
/// every entry and every plugin.
pub fn scan_headparts(
    custom: &[String],
    selection: &Path,
    plugin_extensions: &[&str],
    cancelled: &dyn Fn() -> bool,
) -> Result<HeadpartScan, ScanCancelled> {
    if custom.is_empty() {
        log::error!(
            "customHeadparts.txt not found. This can cause issue when optimizing meshes, as \
             some headparts won't be detected."
        );
    }
    let mut plugins = Vec::new();
    list_plugins(selection, plugin_extensions, cancelled, &mut plugins)?;

    let mut paths: Vec<String> = custom.to_vec();
    let mut unreadable = Vec::new();
    for path in plugins {
        if cancelled() {
            return Err(ScanCancelled);
        }
        match read_plugin(&path) {
            Ok(headparts) => paths.extend(headparts),
            Err(error) => {
                log::error!(
                    "Cannot read the headparts of the plugin {}: {error}",
                    path.display()
                );
                unreadable.push(UnreadablePlugin { path, error });
            }
        }
    }
    Ok(HeadpartScan {
        headparts: HeadpartList::new(paths),
        unreadable,
    })
}

/// The Headpart Meshes the plugin at `path` names.
fn read_plugin(path: &Path) -> Result<Vec<String>, PluginError> {
    plugins::headparts(&mut std::io::BufReader::new(File::open(path)?))
}

/// Appends every plugin beneath `directory` to `plugins`, depth-first.
fn list_plugins(
    directory: &Path,
    extensions: &[&str],
    cancelled: &dyn Fn() -> bool,
    plugins: &mut Vec<PathBuf>,
) -> Result<(), ScanCancelled> {
    // A folder that vanished or cannot be read holds no plugin for this scan,
    // as an unreadable folder held none for Qt's iterator.
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Ok(());
    };
    for entry in entries {
        if cancelled() {
            return Err(ScanCancelled);
        }
        // An entry that cannot be read, or that vanished before its metadata
        // was, is passed over like an unreadable folder.
        let Ok(entry) = entry else { continue };
        let name = entry.file_name();
        if is_staging_name(&name) {
            continue;
        }
        let path = entry.path();
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.is_dir() {
            // Junctions are reparse points without being symlinks.
            if !metadata.file_type().is_symlink() && !cao_winfs::is_reparse_point(&metadata) {
                list_plugins(&path, extensions, cancelled, plugins)?;
            }
        } else if path.is_file() && is_plugin_name(&name.to_string_lossy(), extensions) {
            plugins.push(path);
        }
    }
    Ok(())
}

/// Whether `name` ends in one of `extensions`, ignoring ASCII case: mods do
/// ship `Plugin.ESP`.
fn is_plugin_name(name: &str, extensions: &[&str]) -> bool {
    extensions.iter().any(|extension| {
        name.len() >= extension.len()
            && name.as_bytes()[name.len() - extension.len()..]
                .eq_ignore_ascii_case(extension.as_bytes())
    })
}
