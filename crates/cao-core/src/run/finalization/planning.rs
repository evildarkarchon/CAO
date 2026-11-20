//! Freezing Archive Finalization's outputs before any mutation.
//!
//! Ported from `src/Run/ArchiveFinalizationPlanning.cpp`: source selection,
//! output naming, the planned Loading Plugin names and the capacity estimate.
//! Classification, splitting and merging are the packer's.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use cao_winfs::{OrdinalIgnoreCase, canonical, is_reparse_point};

use super::ArchiveFinalizationChoices;
use super::plugins::{
    LoadingPluginStrength, existing_entry, find_loading_plugin, has_exact_dummy_bytes,
};
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
    /// Dummy Plugin destination. Set whenever a plugin must load the output:
    /// when Dummy Plugins are created, and when they are not but the packed
    /// sources will be deleted, which only a plugin that loads the Archive
    /// justifies. Empty otherwise.
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
    /// The Mod Roots whose planning failed, in run order. None of them is in
    /// `roots`, so none is packed, maintained or pruned.
    pub unplanned: Vec<UnplannedRoot>,
    /// The outputs left out because nothing would load them: Dummy Plugins
    /// are not created, the sources would be deleted, and no plugin at any
    /// recognized name loads the Archive. Their sources stay loose.
    pub unloadable: Vec<UnloadableOutput>,
    /// The planned Mod Roots holding plugins or Archives whose names differ
    /// only in case, in run order. Planning goes on with them: they are only
    /// reported.
    pub case_twins: Vec<CaseTwins>,
}

/// A Mod Root whose plugins or Archives include names that differ only in
/// case, as a case-sensitive directory can hold: a Linux mod manager's, or one
/// WSL or `fsutil` made. The game, under Windows, Wine or Proton, sees each
/// group as one name and loads only one of its files.
pub(super) struct CaseTwins {
    pub mod_root: PathBuf,
    /// Each group's file names, as on disk and in ordinal order, two or more
    /// per group; the groups are ordered by their first name.
    pub groups: Vec<Vec<String>>,
}

/// An output planning left out because no plugin would load it, so packing
/// it and deleting its sources could make the mod stop loading them.
pub(super) struct UnloadableOutput {
    pub mod_root: PathBuf,
    /// The Archive that would have been created.
    pub archive_path: PathBuf,
    /// How many loose files it would have packed, which stay loose.
    pub file_count: usize,
    /// The suffix-free plugin name that would load it, where a Dummy Plugin
    /// would have gone.
    pub plugin_path: PathBuf,
}

/// A Mod Root planning could not plan, and why.
pub(super) struct UnplannedRoot {
    /// The Mod Root as the run gave it; it may not even canonicalize.
    pub root: PathBuf,
    /// What failed, such as a directory that could not be read.
    pub detail: String,
}

/// What planning one Mod Root produced, before it joins the plan.
struct RootPlan {
    /// The canonical Mod Root.
    root: PathBuf,
    /// The plugin allowance the root's existing Archives may need.
    dummy_capacity: u64,
    outputs: Vec<FinalizationOutput>,
    unloadable: Vec<UnloadableOutput>,
    /// The root's plugin and Archive names that differ only in case.
    case_twins: Vec<Vec<String>>,
}

/// Cancellation was observed between planning inputs; nothing was mutated
/// and no plan is returned.
#[derive(Debug)]
pub(super) struct PlanningCancelled;

/// Why planning one Mod Root, or rechecking one output's estimate, stopped.
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
/// [`PlanningCancelled`] rather than an incomplete plan.
///
/// A Mod Root whose planning fails, because a directory cannot be read or
/// the packer refuses it, is left out of the plan and recorded in
/// [`FinalizationPlan::unplanned`]; the other roots are planned as if it
/// were not there. Planning only reads, so the failed root is unchanged, and
/// it gets no output, no Loading Plugin maintenance and no pruning.
///
/// Without Dummy Plugins, an output whose sources would be deleted is planned
/// only when a plugin at one of its recognized names already loads it;
/// otherwise it is recorded in [`FinalizationPlan::unloadable`] and its
/// sources stay loose (audit FS-4).
pub(super) fn plan_finalization(
    roots: &[PathBuf],
    choices: &ArchiveFinalizationChoices,
    packer: &dyn ArchivePacker,
    files_to_not_pack: &[String],
    cancelled: &dyn Fn() -> bool,
) -> Result<FinalizationPlan, PlanningCancelled> {
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
        unplanned: Vec::new(),
        unloadable: Vec::new(),
        case_twins: Vec::new(),
    };
    let create_dummies = plan.create_dummies;
    let delete_sources = plan.delete_sources;
    let dummy_len = rules.dummy_plugin.len() as u64;
    // A fallback Dummy Plugin is only ever published when they are created.
    let dummy_allowance = if create_dummies { dummy_len } else { 0 };
    // Names this plan already gave an output, compared as Windows compares
    // them, so two plugins differing only in case never get twin Archives.
    let mut reserved: BTreeSet<OrdinalIgnoreCase<PathBuf>> = BTreeSet::new();
    // One root's plan: its canonical path, its Dummy Plugin allowance, its
    // outputs and the outputs nothing would load. An error leaves `plan`
    // untouched by this root.
    let mut plan_root = |input_root: &Path| -> Result<RootPlan, PlanningStop> {
        let root = canonical(input_root).map_err(|error| failed(&error))?;
        let mut outputs = Vec::new();
        let mut unloadable = Vec::new();
        let mut dummy_capacity = 0u64;
        if create_dummies {
            // Existing Archives can also need plugins in the final pass, even
            // with no new outputs. Count each, without assuming plugin reuse.
            check()?;
            let existing = packer
                .list_names(&root, ArchiveNameKind::Archive)
                .map_err(|error| failed(&error))?;
            dummy_capacity = (existing.len() as u64).saturating_mul(dummy_len);
        }

        check()?;
        let case_twins = case_twins(&root, &rules).map_err(|error| failed(&error))?;
        let listed = packer
            .list_names(&root, ArchiveNameKind::Plugin)
            .map_err(|error| failed(&error))?;
        // Ignore only proven Dummy Plugin names while naming outputs; the
        // files themselves are never removed, so they keep loading the
        // Archives earlier runs packed.
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
            // Taken under any spelling: in a case-sensitive directory an
            // existing `MyMod.BSA` must still keep `MyMod.bsa` from being
            // created beside it.
            let available = |candidate: &ArchiveName| -> Result<bool, PlanningStop> {
                let path = candidate.full_path();
                match existing_entry(&path) {
                    Ok(Some(_)) => Ok(false),
                    Ok(None) => Ok(!reserved.contains(&OrdinalIgnoreCase(path))),
                    Err(detail) => Err(PlanningStop::Failed(format!(
                        "Cannot plan output Archive `{}`: {detail}",
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
            let mut plugin_path = None;
            let mut loading_plugin_paths = Vec::new();
            if !create_dummies && delete_sources {
                check()?;
                let names = loading_plugin_names(&selected, &rules);
                // The strength the attempt requires before deleting
                // sources, so a planned output can only fail there if
                // its plugin changes meanwhile.
                let loaded = find_loading_plugin(
                    &names,
                    LoadingPluginStrength::DurableForSourceDeletion,
                    None,
                )
                .map_err(PlanningStop::Failed)?;
                if loaded.is_none() {
                    // Packing it would delete loose files the game then
                    // loads from nowhere; leave them loose instead.
                    unloadable.push(UnloadableOutput {
                        mod_root: root.clone(),
                        archive_path,
                        file_count: archive.files.len(),
                        plugin_path: names
                            .last()
                            .expect("every game has a plugin extension")
                            .clone(),
                    });
                    continue;
                }
                loading_plugin_paths = names;
            }
            reserved.insert(OrdinalIgnoreCase(archive_path.clone()));
            if create_dummies {
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
            output.estimated_capacity_bytes =
                estimate_packed_capacity(&output, dummy_allowance, &check)?;
            outputs.push(output);
        }
        Ok(RootPlan {
            root,
            dummy_capacity,
            outputs,
            unloadable,
            case_twins,
        })
    };
    for input_root in roots {
        if cancelled() {
            return Err(PlanningCancelled);
        }
        match plan_root(input_root) {
            Ok(RootPlan {
                root,
                dummy_capacity,
                outputs,
                unloadable,
                case_twins,
            }) => {
                if !case_twins.is_empty() {
                    log::warn!(
                        "{} holds plugins or Archives whose names differ only in case: {}",
                        root.display(),
                        describe_case_twins(&case_twins)
                    );
                    plan.case_twins.push(CaseTwins {
                        mod_root: root.clone(),
                        groups: case_twins,
                    });
                }
                plan.roots.push(root.clone());
                plan.dummy_capacity_by_root.insert(root, dummy_capacity);
                plan.outputs.extend(outputs);
                for output in &unloadable {
                    log::warn!(
                        "Archive creation skips {}: no plugin loads it and Dummy Plugins are \
                         off, so its {} loose files stay loose",
                        output.archive_path.display(),
                        output.file_count
                    );
                }
                plan.unloadable.extend(unloadable);
            }
            Err(PlanningStop::Cancelled) => return Err(PlanningCancelled),
            Err(PlanningStop::Failed(detail)) => {
                log::error!("Archive creation skips {}: {detail}", input_root.display());
                plan.unplanned.push(UnplannedRoot {
                    root: input_root.clone(),
                    detail,
                });
            }
        }
    }
    if cancelled() {
        return Err(PlanningCancelled);
    }
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
/// plus `dummy_allowance`: the fallback Dummy Plugin's size when Dummy
/// Plugins are created, since a Loading Plugin can disappear before
/// publication and require one, and 0 when they are not.
///
/// Zlib and LZ4 framing, up to four BA2 texture chunks, and BSA name tables
/// need space beyond source bytes, so each source counts twice, plus a fixed
/// allowance and three bytes per name byte. The formula is internal; only
/// conservatism is contract. Every source is stat'ed again, polling `check`
/// between them; a source that cannot be sized fails the estimate.
pub(super) fn estimate_packed_capacity(
    output: &FinalizationOutput,
    dummy_allowance: u64,
    check: &dyn Fn() -> Result<(), PlanningStop>,
) -> Result<u64, PlanningStop> {
    let mut estimate: u64 = CAPACITY_ALLOWANCE.saturating_add(dummy_allowance);
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

/// The groups of plugin and Archive file names directly in `root` that differ
/// only in case, as `rules` recognise them: each group's names as on disk,
/// in ordinal order, and the groups ordered by their first name. Empty on any
/// case-insensitive directory, which cannot hold two such names.
///
/// The names are read from the directory itself rather than from the
/// packer's parsed names, which re-render a counter and drop its leading
/// zeros, so two different files never look alike.
///
/// # Errors
/// `root` cannot be listed.
fn case_twins(root: &Path, rules: &ArchiveNamingRules) -> std::io::Result<Vec<Vec<String>>> {
    let recognised = |name: &str| {
        let extension = name.rfind('.').map_or("", |dot| &name[dot..]);
        rules.extension.eq_ignore_ascii_case(extension)
            || rules
                .plugin_extensions
                .iter()
                .any(|known| known.eq_ignore_ascii_case(extension))
    };
    let mut groups: BTreeMap<OrdinalIgnoreCase<String>, Vec<String>> = BTreeMap::new();
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        // A directory is never a plugin or Archive, whatever its name.
        if entry.file_type()?.is_dir() {
            continue;
        }
        // A name that is not Unicode is no plugin or Archive name.
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if recognised(&name) {
            groups
                .entry(OrdinalIgnoreCase(name.clone()))
                .or_default()
                .push(name);
        }
    }
    Ok(groups
        .into_values()
        .filter(|names| names.len() > 1)
        .map(|mut names| {
            names.sort();
            names
        })
        .collect())
}

/// `groups` as one line for the log and the warning: each group's names
/// joined by " and ", the groups by "; ".
pub(super) fn describe_case_twins(groups: &[Vec<String>]) -> String {
    groups
        .iter()
        .map(|names| {
            names
                .iter()
                .map(|name| format!("`{name}`"))
                .collect::<Vec<_>>()
                .join(" and ")
        })
        .collect::<Vec<_>>()
        .join("; ")
}
