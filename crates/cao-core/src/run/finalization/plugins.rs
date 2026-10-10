//! Loading Plugins and Dummy Plugins for new and existing Archives.
//!
//! Ported from `src/Run/ArchiveFinalizationLoadingPlugins.cpp`. A Loading
//! Plugin is a plugin at one of the profile-recognized names that makes the
//! game load an Archive; a Dummy Plugin is the profile's canonical 49-byte
//! plugin CAO publishes when no other plugin does. Every Dummy Plugin is
//! staged and published with no-replace, so a late occupant always wins, and
//! only a file whose complete bytes are canonical is ever removed.

use std::collections::BTreeMap;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use cao_winfs::{Access, FileFacts, Open, Share, delete_by_handle};

use super::planning::{FinalizationOutput, FinalizationPlan, loading_plugin_names};
use super::{
    ArchiveFinalizationFailure, ArchiveFinalizationMutation, ArchiveFinalizationMutationKind,
    ArchiveFinalizationResult,
};
use crate::execution::MutationState;
use crate::run::source_pin::{absolute_normal, pin_directories};
use crate::run::{
    ArchiveNameKind, ArchivePacker, PublicationPolicy, PublicationState, TemporaryArtifactRegistry,
};

/// How strong the evidence must be before a recognized name counts as
/// loading an Archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LoadingPluginStrength {
    /// Any regular file at the name, following links. Planning and
    /// existing-Archive maintenance use it.
    Present,
    /// An ordinary, nonempty file that is not a link. An attribute-only
    /// reparse write can retarget a symlink through a read pin, so deleting
    /// Loose Assets needs this.
    DurableForSourceDeletion,
}

/// Whether something, even a dangling link, occupies `path`.
///
/// # Errors
/// The entry cannot be inspected for any reason but its absence.
fn entry_exists(path: &Path) -> Result<bool, String> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("Cannot inspect `{}`: {error}", path.display())),
    }
}

/// Whether the recognized name `path` loads its Archive at `strength`.
///
/// # Errors
/// The entry cannot be inspected for any reason but its absence.
fn is_loading_plugin(path: &Path, strength: LoadingPluginStrength) -> Result<bool, String> {
    let inspect = |metadata: std::io::Result<std::fs::Metadata>| match metadata {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("Cannot inspect `{}`: {error}", path.display())),
    };
    match strength {
        LoadingPluginStrength::DurableForSourceDeletion => Ok(inspect(std::fs::symlink_metadata(
            path,
        ))?
        .is_some_and(|metadata| {
            metadata.is_file() && !metadata.is_symlink() && metadata.len() != 0
        })),
        // Loading follows links to regular plugins; exact Dummy Plugin
        // recognition does not.
        LoadingPluginStrength::Present => {
            Ok(inspect(std::fs::metadata(path))?.is_some_and(|metadata| metadata.is_file()))
        }
    }
}

/// The first of `names` that loads its Archive at `strength`, skipping every
/// name equal to `excluded`; probing stops at the first match.
///
/// Exclusion is by value: without an Archive suffix the Dummy Plugin
/// destination appears twice.
///
/// # Errors
/// A name cannot be inspected.
pub(super) fn find_loading_plugin(
    names: &[PathBuf],
    strength: LoadingPluginStrength,
    excluded: Option<&Path>,
) -> Result<Option<PathBuf>, String> {
    for name in names {
        if Some(name.as_path()) != excluded && is_loading_plugin(name, strength)? {
            return Ok(Some(name.clone()));
        }
    }
    Ok(None)
}

/// Whether `path` is an ordinary file, not a link, whose complete bytes are
/// exactly `bytes`. Any read failure is simply "no": an unverified file stays
/// a Loading Plugin name during planning, not a proven dummy.
pub(super) fn has_exact_dummy_bytes(path: &Path, bytes: &[u8]) -> bool {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if !metadata.is_file() || metadata.is_symlink() || metadata.len() != bytes.len() as u64 {
        return false;
    }
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    // One byte more than expected proves the file did not grow since the stat.
    let mut contents = Vec::with_capacity(bytes.len() + 1);
    file.take(bytes.len() as u64 + 1)
        .read_to_end(&mut contents)
        .is_ok_and(|_| contents == bytes)
}

/// One Dummy Plugin publication attempt after staging succeeded.
struct DummyPluginPublication {
    /// Whether the plugin was published and its Temporary Ownership released.
    completed: bool,
    /// The publication's continuation verdict.
    safe_to_continue: bool,
    /// Why publication did not complete; empty when it did.
    detail: String,
}

/// Stages the canonical Dummy Plugin bytes under Temporary Ownership and
/// publishes them at `destination` with no-replace, so a late occupant always
/// wins. A committed destination is appended to `mutations` as a
/// `PluginCreation` fact before this returns, even when releasing its
/// ownership then fails.
///
/// # Errors
/// Only before publication starts, when no destination mutation can have
/// happened.
fn publish_dummy_plugin(
    artifacts: &mut TemporaryArtifactRegistry,
    mod_root: &Path,
    destination: &Path,
    bytes: &[u8],
    mutations: &mut Vec<ArchiveFinalizationMutation>,
) -> Result<DummyPluginPublication, String> {
    let receipt = artifacts
        .stage_archive_file_for_publication(mod_root)
        .map_err(|error| error.to_string())?;
    let staged = receipt.path().map_err(|error| error.to_string())?;
    // Staging created the file empty; open it as it is, exclusively and never
    // through a link that replaced it, as every other staged write does.
    let stage_failed = |_| "Could not stage the loading plugin.".to_owned();
    let mut file = Open::new(Access::WRITE, Share::NONE)
        .open(staged)
        .map_err(stage_failed)?;
    file.write_all(bytes).map_err(stage_failed)?;
    // Close the handle before the staged file is renamed.
    drop(file);

    let published = receipt.publish(destination, PublicationPolicy::NoReplace);
    // The receipt, not a later directory listing, identifies the exact
    // committed path. A successful rename is a durable effect of its own even
    // if releasing its temporary ownership then fails.
    if published.mutation() == MutationState::Committed {
        mutations.push(ArchiveFinalizationMutation {
            mod_root: mod_root.to_path_buf(),
            path: destination.to_path_buf(),
            kind: ArchiveFinalizationMutationKind::PluginCreation,
            mutation: MutationState::Committed,
            count: 1,
        });
    }
    let completed = published.state == PublicationState::PublishedAndReleased;
    Ok(DummyPluginPublication {
        completed,
        safe_to_continue: published.safe_to_continue(),
        detail: if completed {
            String::new()
        } else if published.error_detail.is_empty() {
            "Loading plugin publication did not complete.".to_owned()
        } else {
            published.error_detail
        },
    })
}

/// Rechecks a published output's recognized names and publishes the fallback
/// Dummy Plugin when no other Loading Plugin loads it. Returns the entry that
/// loads the Archive, which the caller pins before deleting sources.
///
/// `output.loading_plugin_paths` must not be empty. A plugin at another
/// recognized name can arrive after planning, or a planned one disappear, so
/// all are probed again. When sources will be deleted, a link or empty file
/// at the Dummy Plugin destination is left alone and the fallback moves to
/// the first recognized name with no entry at all.
///
/// # Errors
/// Any failure; a plugin publication with durable effects is appended to
/// `mutations` first.
pub(super) fn ensure_output_loading_plugin(
    plan: &FinalizationPlan,
    output: &FinalizationOutput,
    artifacts: &mut TemporaryArtifactRegistry,
    mutations: &mut Vec<ArchiveFinalizationMutation>,
) -> Result<PathBuf, String> {
    let bytes = &plan.rules.dummy_plugin;
    let names = &output.loading_plugin_paths;
    let plugin = names
        .last()
        .expect("an output that maintains Loading Plugins has recognized names");
    let strength = if plan.delete_sources {
        LoadingPluginStrength::DurableForSourceDeletion
    } else {
        LoadingPluginStrength::Present
    };
    // The chosen dummy destination is checked separately, so a late
    // non-dummy occupant never becomes our successful publication.
    if let Some(loaded_elsewhere) = find_loading_plugin(names, strength, Some(plugin))? {
        return Ok(loaded_elsewhere);
    }
    let mut destination = plugin.clone();
    if strength == LoadingPluginStrength::DurableForSourceDeletion
        && entry_exists(plugin)?
        && !is_loading_plugin(plugin, strength)?
    {
        let mut free = None;
        for name in names {
            if !entry_exists(name)? {
                free = Some(name.clone());
                break;
            }
        }
        destination =
            free.ok_or_else(|| "No ordinary loading plugin name is available.".to_owned())?;
    }
    if destination == *plugin && entry_exists(plugin)? {
        if output.plugin_path.is_some() && !has_exact_dummy_bytes(plugin, bytes) {
            return Err("The planned loading plugin is occupied.".to_owned());
        }
        if !std::fs::metadata(plugin).is_ok_and(|metadata| metadata.is_file()) {
            return Err("The planned loading plugin is not a file.".to_owned());
        }
        return Ok(destination);
    }
    // Publication rechecks the leaf natively: a newcomer wins.
    let published =
        publish_dummy_plugin(artifacts, &output.mod_root, &destination, bytes, mutations)?;
    if !published.completed {
        return Err(published.detail);
    }
    Ok(destination)
}

/// Maintains the Loading Plugins of every existing Archive in `root`: creates
/// the missing Dummy Plugins when they are wanted, otherwise removes only
/// exact Dummy Plugins. Each plugin action is a separate mutation fact,
/// never an output attempt.
///
/// Returns `Ok(false)` when finalization must stop without pruning, after
/// recording a plugin failure or cancellation in `result`.
///
/// # Errors
/// When creating, a failure to list the root's Archives; removal records any
/// listing failure as `PluginRemovalFailed` instead.
pub(super) fn maintain_existing_loading_plugins(
    plan: &FinalizationPlan,
    root: &Path,
    packer: &dyn ArchivePacker,
    artifacts: &mut TemporaryArtifactRegistry,
    cancelled: &dyn Fn() -> bool,
    result: &mut ArchiveFinalizationResult,
) -> Result<bool, String> {
    if plan.create_dummies {
        create_missing_dummy_plugins(plan, root, packer, artifacts, cancelled, result)
    } else {
        Ok(remove_dummy_plugins(plan, root, packer, cancelled, result))
    }
}

/// Publishes a Dummy Plugin for each existing Archive in `root` that no
/// recognized name loads. Returns `Ok(false)` after recording a plugin
/// failure or cancellation.
fn create_missing_dummy_plugins(
    plan: &FinalizationPlan,
    root: &Path,
    packer: &dyn ArchivePacker,
    artifacts: &mut TemporaryArtifactRegistry,
    cancelled: &dyn Fn() -> bool,
    result: &mut ArchiveFinalizationResult,
) -> Result<bool, String> {
    let archives = packer
        .list_names(root, ArchiveNameKind::Archive)
        .map_err(|error| error.to_string())?;
    for archive in archives {
        if cancelled() {
            result.cancelled = true;
            return Ok(false);
        }
        let names = loading_plugin_names(&archive, &plan.rules);
        let destination = names.last().expect("every game has a plugin extension");
        let created = (|| -> Result<Option<DummyPluginPublication>, String> {
            if find_loading_plugin(&names, LoadingPluginStrength::Present, None)?.is_some() {
                return Ok(None);
            }
            // A non-plugin entry at the chosen name must survive, even though
            // the read-only probe above did not recognize it.
            if entry_exists(destination)? {
                return Err("The loading plugin name is occupied.".to_owned());
            }
            publish_dummy_plugin(
                artifacts,
                root,
                destination,
                &plan.rules.dummy_plugin,
                &mut result.mutations,
            )
            .map(Some)
        })();
        match created {
            Ok(None) => {}
            Ok(Some(published)) if published.completed => {}
            Ok(Some(published)) => {
                result.failure = Some(ArchiveFinalizationFailure::PluginCreationFailed);
                // Publication's verdict stands.
                result.safe_to_continue = published.safe_to_continue;
                result.detail = published.detail;
                result.cancelled = cancelled();
                return Ok(false);
            }
            Err(detail) => {
                // Staging and the checks above fail before publication, so
                // nothing was mutated.
                result.failure = Some(ArchiveFinalizationFailure::PluginCreationFailed);
                result.detail = detail;
                result.cancelled = cancelled();
                return Ok(false);
            }
        }
    }
    Ok(true)
}

/// Removes every exact Dummy Plugin in `root` through the guarded removal.
/// Returns false after recording a removal failure or cancellation.
fn remove_dummy_plugins(
    plan: &FinalizationPlan,
    root: &Path,
    packer: &dyn ArchivePacker,
    cancelled: &dyn Fn() -> bool,
    result: &mut ArchiveFinalizationResult,
) -> bool {
    let failed = |result: &mut ArchiveFinalizationResult, detail: String| {
        result.failure = Some(ArchiveFinalizationFailure::PluginRemovalFailed);
        result.detail = detail;
        result.cancelled = cancelled();
        false
    };
    let plugins = match packer.list_names(root, ArchiveNameKind::Plugin) {
        Ok(plugins) => plugins,
        Err(error) => return failed(result, error.to_string()),
    };
    for plugin in plugins {
        if cancelled() {
            result.cancelled = true;
            return false;
        }
        let path = plugin.full_path();
        match remove_exact_dummy_plugin(&path, root, &plan.rules.dummy_plugin) {
            Ok(DummyPluginRemoval::NotDummy) => {}
            Ok(DummyPluginRemoval::Removed) => {
                result.mutations.push(ArchiveFinalizationMutation {
                    mod_root: root.to_path_buf(),
                    path,
                    kind: ArchiveFinalizationMutationKind::PluginRemoval,
                    mutation: MutationState::Committed,
                    count: 1,
                });
            }
            Err(detail) => return failed(result, detail),
        }
    }
    true
}

/// What the guarded removal found.
enum DummyPluginRemoval {
    /// The file is not an exact Dummy Plugin, so it was kept.
    NotDummy,
    /// The exact Dummy Plugin was deleted.
    Removed,
}

/// Deletes `plugin` only if it is an exact Dummy Plugin, through one handle
/// that pins it while its bytes are checked.
///
/// Its parents inside `mod_root` stay pinned, so the pathname cannot escape
/// through a junction or switch parents during the delete. A hard-linked or
/// changed file is refused: the same file could be reachable under another
/// user's pathname. Every error happens before the delete, so it always
/// describes a known no-mutation failure.
///
/// C++ also reported an uncertain removal when closing the deleted handle
/// failed; dropping a Rust `File` reports no close error, so a disposition
/// that succeeded is a removal.
fn remove_exact_dummy_plugin(
    plugin: &Path,
    mod_root: &Path,
    bytes: &[u8],
) -> Result<DummyPluginRemoval, String> {
    let path = absolute_normal(plugin).map_err(|error| error.to_string())?;
    let root = absolute_normal(mod_root).map_err(|error| error.to_string())?;
    let mut directories = BTreeMap::new();
    pin_directories(&path, &root, Share::READ, &mut directories)
        .map_err(|error| error.to_string())?;
    let io_error =
        |action: &str, error: std::io::Error| format!("{action} `{}`: {error}", path.display());
    let mut entry = Open::new(Access::READ | Access::DELETE, Share::READ)
        .open(&path)
        .map_err(|error| io_error("Could not pin Dummy Plugin for removal", error))?;
    let original =
        FileFacts::of(&entry).map_err(|error| io_error("Could not inspect Dummy Plugin", error))?;
    if !original.is_ordinary_file() {
        return Err(format!(
            "A Dummy Plugin is no longer an ordinary file: {}",
            path.display()
        ));
    }
    if original.link_count() != 1 {
        return Err("A linked Dummy Plugin cannot be removed.".to_owned());
    }
    if original.size() != bytes.len() as u64 {
        return Ok(DummyPluginRemoval::NotDummy);
    }
    let mut contents = Vec::with_capacity(bytes.len());
    (&mut entry)
        .take(bytes.len() as u64)
        .read_to_end(&mut contents)
        .map_err(|error| io_error("Could not read pinned Dummy Plugin", error))?;
    if contents.len() != bytes.len() {
        return Err("A Dummy Plugin changed during verification.".to_owned());
    }
    if contents != bytes {
        return Ok(DummyPluginRemoval::NotDummy);
    }
    let verified =
        FileFacts::of(&entry).map_err(|error| io_error("Could not inspect Dummy Plugin", error))?;
    if verified.link_count() != 1 || !verified.unchanged_since(&original) {
        return Err("A Dummy Plugin changed during verification.".to_owned());
    }
    delete_by_handle(&entry)
        .map_err(|error| io_error("Could not remove pinned Dummy Plugin", error))?;
    Ok(DummyPluginRemoval::Removed)
}
