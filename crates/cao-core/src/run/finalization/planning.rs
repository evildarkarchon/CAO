//! Freezing Archive Finalization's outputs before any mutation.
//!
//! Ported from `src/Run/ArchiveFinalizationPlanning.cpp`: source selection,
//! output naming, the planned Loading Plugin names and the capacity estimate.
//! Classification, splitting and merging are the packer's.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use cao_winfs::{is_reparse_point, msvc_canonical};

use super::ArchiveFinalizationSettings;
use super::plugins::{LoadingPluginStrength, find_loading_plugin, has_exact_dummy_bytes};
use crate::run::staging::has_staging_component;
use crate::run::{
    ArchiveMerge, ArchiveName, ArchiveNameKind, ArchiveNamingRules, ArchivePacker,
    PackedArchiveKind, PackedFile, PlannedArchive,
};

/// One frozen output and the complete source set its atomic attempt consumes.
pub(super) struct FinalizationOutput {
    pub mod_root: PathBuf,
    pub archive_path: PathBuf,
    /// The output's contents, whose files are its packed sources.
    pub archive: PlannedArchive,
    /// The fallback Dummy Plugin destination, when planning found no Loading
    /// Plugin. The attempt rechecks every recognized name before creation and
    /// source deletion, since a plugin may appear or disappear meanwhile.
    pub plugin_path: Option<PathBuf>,
    /// A conservative content and framing allowance, not a reservation.
    pub estimated_capacity_bytes: u64,
    /// The recognized Loading Plugin names; the last is the suffix-free
    /// Dummy Plugin destination. Empty when Dummy Plugins are not created.
    pub loading_plugin_paths: Vec<PathBuf>,
}

/// Every output name, partition and choice, frozen before any mutation. The
/// output count is the phase's progress total.
pub(super) struct FinalizationPlan {
    pub outputs: Vec<FinalizationOutput>,
    /// The canonical Mod Roots, in run order.
    pub roots: Vec<PathBuf>,
    pub rules: ArchiveNamingRules,
    pub compress: bool,
    pub delete_sources: bool,
    pub create_dummies: bool,
    /// The plugin allowance each root's existing Archives may need.
    pub dummy_capacity_by_root: BTreeMap<PathBuf, u64>,
}

/// Why planning returned no plan.
pub(super) enum PlanningStop {
    /// Cancellation was observed between inputs; nothing was mutated.
    Cancelled,
    /// An input could not be read or planned.
    Failed(String),
}

/// The counters tried, from 0, when every plugin-derived output name is
/// taken; C++ gave up after 255.
const NAME_COUNTERS: u32 = 255;

/// The fixed staging allowance of the capacity estimate: once per output for
/// framing and once more per source for its staging and name tables.
const CAPACITY_ALLOWANCE: u64 = 65_536;

/// Freezes output names and source partitions for every Mod Root, in order,
/// without mutating anything.
///
/// `files_to_not_pack` holds the Packing Exclusions, already lowercase with
/// `/` separators. `cancelled` is polled between inputs: planning returns
/// [`PlanningStop::Cancelled`] rather than an incomplete plan.
pub(super) fn plan_finalization(
    roots: &[PathBuf],
    choices: &ArchiveFinalizationSettings,
    packer: &dyn ArchivePacker,
    files_to_not_pack: &[String],
    cancelled: &dyn Fn() -> bool,
) -> Result<FinalizationPlan, PlanningStop> {
    let rules = packer.rules().clone();
    let check = || {
        if cancelled() {
            Err(PlanningStop::Cancelled)
        } else {
            Ok(())
        }
    };
    let failed = |error: &dyn std::fmt::Display| PlanningStop::Failed(error.to_string());
    // Deviation 21: some games never merge Textures, whatever was asked.
    let merge = ArchiveMerge {
        incompressible: choices.merge_incompressible,
        textures: choices.merge_textures && !rules.separate_textures,
    };
    let mut plan = FinalizationPlan {
        outputs: Vec::new(),
        roots: Vec::new(),
        rules: rules.clone(),
        compress: choices.compress,
        delete_sources: choices.delete_sources,
        create_dummies: choices.create_dummy_plugins,
        dummy_capacity_by_root: BTreeMap::new(),
    };
    let dummy_len = rules.dummy_plugin.len() as u64;
    let mut reserved: BTreeSet<PathBuf> = BTreeSet::new();
    for input_root in roots {
        check()?;
        let root = msvc_canonical(input_root).map_err(|error| failed(&error))?;
        plan.roots.push(root.clone());
        let mut dummy_capacity = 0u64;
        if plan.create_dummies {
            // Existing Archives can also need plugins in the final pass, even
            // with no new outputs. Count each, without assuming plugin reuse.
            check()?;
            let existing = packer
                .list_names(&root, ArchiveNameKind::Archive)
                .map_err(|error| failed(&error))?;
            dummy_capacity = (existing.len() as u64).saturating_mul(dummy_len);
        }
        plan.dummy_capacity_by_root
            .insert(root.clone(), dummy_capacity);

        let listed = packer
            .list_names(&root, ArchiveNameKind::Plugin)
            .map_err(|error| failed(&error))?;
        // Ignore only proven Dummy Plugin names while planning; their files
        // stay until every output attempt finishes, so cancellation cannot
        // strand an existing Archive.
        let mut plugins = Vec::with_capacity(listed.len());
        for plugin in listed {
            check()?;
            if !has_exact_dummy_bytes(&plugin.full_path(), &rules.dummy_plugin) {
                plugins.push(plugin);
            }
        }
        plugins.sort();
        if plugins.is_empty() {
            let name = root
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| {
                    PlanningStop::Failed(format!(
                        "The Mod Root name of `{}` is not valid Unicode.",
                        root.display()
                    ))
                })?;
            plugins.push(ArchiveName {
                dir: root.clone(),
                name: name.to_owned(),
                suffix: String::new(),
                ext: ".esp".to_owned(),
                counter: None,
            });
        }

        let sources = collect_sources(&root, files_to_not_pack, &check)?;
        check()?;
        let archives = packer
            .partition(&root, sources, merge)
            .map_err(|error| failed(&error))?;
        for archive in archives {
            check()?;
            let suffix = match archive.kind {
                PackedArchiveKind::Textures => rules.texture_suffix.clone(),
                PackedArchiveKind::Standard | PackedArchiveKind::Incompressible => {
                    rules.suffix.clone()
                }
            }
            .unwrap_or_default();
            let available = |candidate: &ArchiveName| -> Result<bool, PlanningStop> {
                let path = candidate.full_path();
                match std::fs::symlink_metadata(&path) {
                    Ok(_) => Ok(false),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        Ok(!reserved.contains(&path))
                    }
                    Err(error) => Err(PlanningStop::Failed(format!(
                        "Cannot plan output Archive `{}`: {error}",
                        path.display()
                    ))),
                }
            };
            let mut selected = None;
            for plugin in &plugins {
                check()?;
                let candidate = ArchiveName {
                    ext: rules.extension.clone(),
                    suffix: suffix.clone(),
                    ..plugin.clone()
                };
                if available(&candidate)? {
                    selected = Some(candidate);
                    break;
                }
            }
            if selected.is_none() {
                let mut candidate = ArchiveName {
                    ext: rules.extension.clone(),
                    suffix: suffix.clone(),
                    ..plugins[0].clone()
                };
                for counter in 0..NAME_COUNTERS {
                    check()?;
                    candidate.counter = Some(counter);
                    if available(&candidate)? {
                        selected = Some(candidate);
                        break;
                    }
                }
            }
            let Some(selected) = selected else {
                return Err(PlanningStop::Failed(
                    "No available output Archive name.".to_owned(),
                ));
            };
            let archive_path = selected.full_path();
            reserved.insert(archive_path.clone());
            let mut plugin_path = None;
            let mut loading_plugin_paths = Vec::new();
            if plan.create_dummies {
                check()?;
                loading_plugin_paths = loading_plugin_names(&selected, &rules);
                if find_loading_plugin(&loading_plugin_paths, LoadingPluginStrength::Present, None)
                    .map_err(PlanningStop::Failed)?
                    .is_none()
                {
                    plugin_path = loading_plugin_paths.last().cloned();
                }
            }
            let mut output = FinalizationOutput {
                mod_root: root.clone(),
                archive_path,
                archive,
                plugin_path,
                estimated_capacity_bytes: 0,
                loading_plugin_paths,
            };
            output.estimated_capacity_bytes = estimate_packed_capacity(&output, dummy_len, &check)?;
            plan.outputs.push(output);
        }
    }
    check()?;
    Ok(plan)
}

/// The regular files beneath `root` that may be packed and later deleted as
/// packed sources.
///
/// Reserved staging, links and other reparse points are skipped without
/// being descended into; files directly in the Mod Root are never packed
/// (bethutil's `default_is_allowed_path`); and Packing Exclusions are
/// matched against the path within the Mod Root (deviation 16). Each file
/// carries the size splitting counts.
fn collect_sources(
    root: &Path,
    files_to_not_pack: &[String],
    check: &dyn Fn() -> Result<(), PlanningStop>,
) -> Result<Vec<PackedFile>, PlanningStop> {
    let failed = |path: &Path, error: std::io::Error| {
        PlanningStop::Failed(format!(
            "Cannot inspect finalization input `{}`: {error}",
            path.display()
        ))
    };
    let mut sources = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let entries = std::fs::read_dir(&directory).map_err(|error| failed(&directory, error))?;
        for entry in entries {
            check()?;
            let entry = entry.map_err(|error| failed(&directory, error))?;
            let path = entry.path();
            let relative = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
            let metadata =
                std::fs::symlink_metadata(&path).map_err(|error| failed(&path, error))?;
            // Never descend into reserved staging or directory links just to
            // filter their files.
            if has_staging_component(&relative)
                || metadata.is_symlink()
                || is_reparse_point(&metadata)
            {
                continue;
            }
            if metadata.is_dir() {
                pending.push(path);
            } else if metadata.is_file()
                && relative.components().nth(1).is_some()
                && !is_packing_excluded(&relative, files_to_not_pack)
            {
                sources.push(PackedFile {
                    path,
                    size: metadata.len(),
                });
            }
        }
    }
    Ok(sources)
}

/// Whether a Packing Exclusion matches `relative`, a path within its Mod
/// Root: a case-insensitive substring of its `/`-separated form.
fn is_packing_excluded(relative: &Path, files_to_not_pack: &[String]) -> bool {
    let text = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
        .to_lowercase();
    let rule = files_to_not_pack
        .iter()
        .find(|rule| !rule.is_empty() && text.contains(rule.as_str()));
    if let Some(rule) = rule {
        log::trace!("{text} ignored because of filesToNotPack. Rule: {rule}");
    }
    rule.is_some()
}

/// The profile-recognized Loading Plugin names for `archive`: each plugin
/// extension with, then without, the Archive's suffix. The last is the
/// suffix-free Dummy Plugin destination. A name repeats when the Archive has
/// no suffix.
pub(super) fn loading_plugin_names(
    archive: &ArchiveName,
    rules: &ArchiveNamingRules,
) -> Vec<PathBuf> {
    let mut names = Vec::with_capacity(rules.plugin_extensions.len() * 2);
    for extension in &rules.plugin_extensions {
        let mut plugin = ArchiveName {
            ext: extension.clone(),
            ..archive.clone()
        };
        names.push(plugin.full_path());
        plugin.suffix.clear();
        names.push(plugin.full_path());
    }
    names
}

/// Estimates an output's source content plus format and staging overhead,
/// and the fallback Dummy Plugin when the output maintains Loading Plugins.
///
/// Zlib and LZ4 framing, up to four BA2 texture chunks, and BSA name tables
/// need space beyond source bytes, so each source counts twice, plus a fixed
/// allowance and three bytes per name byte. The formula is internal; only
/// conservatism is contract. Every source is stat'ed again, polling `check`
/// between them; a source that cannot be sized fails the estimate.
pub(super) fn estimate_packed_capacity(
    output: &FinalizationOutput,
    dummy_len: u64,
    check: &dyn Fn() -> Result<(), PlanningStop>,
) -> Result<u64, PlanningStop> {
    let mut estimate: u64 = CAPACITY_ALLOWANCE;
    // A Loading Plugin can disappear before publication, requiring the dummy.
    if !output.loading_plugin_paths.is_empty() {
        estimate = estimate.saturating_add(dummy_len);
    }
    for source in &output.archive.files {
        check()?;
        let size = std::fs::metadata(&source.path)
            .map_err(|error| {
                PlanningStop::Failed(format!(
                    "Cannot size packed source `{}`: {error}",
                    source.path.display()
                ))
            })?
            .len();
        let name = source
            .path
            .strip_prefix(&output.mod_root)
            .unwrap_or(&source.path)
            .to_string_lossy()
            .len() as u64;
        let overhead = CAPACITY_ALLOWANCE.saturating_add(name.saturating_mul(3));
        estimate = estimate.saturating_add(size.saturating_mul(2).saturating_add(overhead));
    }
    Ok(estimate)
}
