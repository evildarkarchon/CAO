//! A run's Headpart Meshes (#505).
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
//! Exclusion are still scanned: an excluded mod's plugin can still name a
//! Headpart Mesh another mod ships. The scan skips the `.cao-staging`
//! namespace, and a plugin whose HDPT records cannot be read is returned as
//! an [`UnreadablePlugin`] for the run to report while the scan carries on
//! (deviation 17).

use std::collections::HashSet;
use std::fs::File;
use std::path::{Path, PathBuf};

use cao_core::run::is_staging_name;

use crate::plugins::{self, PluginError, clean_path};

/// The Headpart Meshes a run's profile and plugins name, as `meshes/...`
/// paths.
///
/// Matching ignores case, as Windows and the game resolve paths.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HeadpartList {
    /// Each path folded with `to_lowercase`, so a lookup folds only the Mesh.
    folded: HashSet<String>,
}

impl HeadpartList {
    /// A list of `paths`, each cleaned with [`clean_path`], so spellings of
    /// one path match.
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
/// does not follow directory links or junctions, so a link cannot pull a
/// folder outside the selection, or a cycle, into the scan; a folder it
/// cannot read is passed over. An empty `custom` is logged, since some
/// Headpart Meshes are then only found through the plugins.
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
        // The profile reader gives a missing file and an empty one alike.
        log::error!(
            "customHeadparts.txt is missing or empty. This can cause issue when optimizing \
             meshes, as some headparts won't be detected."
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
///
/// The directory entry's own metadata, which on Windows comes with the
/// listing and is never followed through a link, tells folders from files,
/// and a file's name is checked before anything else is asked of it, so most
/// entries cost no system call of their own (audit BE-14).
fn list_plugins(
    directory: &Path,
    extensions: &[&str],
    cancelled: &dyn Fn() -> bool,
    plugins: &mut Vec<PathBuf>,
) -> Result<(), ScanCancelled> {
    // A folder that vanished or cannot be read holds no plugin for this scan.
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
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_dir() {
            // Junctions are reparse points without being symlinks.
            if !metadata.file_type().is_symlink() && !cao_winfs::is_reparse_point(&metadata) {
                list_plugins(&entry.path(), extensions, cancelled, plugins)?;
            }
        } else if is_plugin_name(&name.to_string_lossy(), extensions) {
            let path = entry.path();
            // A link to a plugin counts as the plugin, so only a link costs
            // a second look, through it.
            let is_file = metadata.is_file()
                || (metadata.file_type().is_symlink()
                    && std::fs::metadata(&path).is_ok_and(|target| target.is_file()));
            if is_file {
                plugins.push(path);
            }
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
