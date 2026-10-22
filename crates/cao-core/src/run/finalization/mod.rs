//! Archive Finalization: the Apply-only Run Phase that packs Loose Assets into
//! new Archives and maintains their Loading Plugins.
//!
//! Ported from `src/Run/ArchiveFinalization.cpp` and its private parts
//! (`ArchiveFinalizationPlanning.cpp`, `ArchiveFinalizationAttempt.cpp`,
//! `ArchiveFinalizationLoadingPlugins.cpp`), plus the empty-directory pruning
//! of `FilesystemOperations::deleteEmptyDirectories`. Archives are written
//! only through the [`ArchivePacker`](crate::run::ArchivePacker) seam and
//! read back only through the [`ArchiveReader`](crate::run::ArchiveReader),
//! so none of this needs `ba2`.
//!
//! When the Routing Policy requests Archive creation, the phase freezes an
//! output plan for every Mod Root without mutating anything, checks staging
//! capacity per volume, then runs one atomic attempt per output: write and
//! no-replace publication of the Archive, its Loading Plugin, then packed
//! source cleanup. Loading Plugins of existing Archives are maintained after
//! the last output. Empty directories are pruned whether or not Archive
//! creation is requested. The phase records its output total, each attempt
//! before the next output starts, and one final phase-level account into Run
//! Evidence through the handle that opened its phase, which reaches nothing
//! else. Run Evidence is the only holder of the attempts.

mod attempt;
mod planning;
mod plugins;
#[cfg(test)]
mod tests;

use std::os::windows::ffi::OsStrExt as _;
use std::os::windows::fs::MetadataExt as _;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use cao_winfs::is_reparse_point;

use crate::execution::MutationState;
use crate::routing::RequestedWork;
use crate::run::capacity::{CapacityCheck, Shortfall};
use crate::run::staging::has_staging_component;
use crate::run::{
    ArchiveFinalizationHandle, ArchiveSeams, RunPreparation, TemporaryArtifactRegistry,
    take_panic_message,
};
use crate::{AttemptOutcome, Error, OperationFailure};

use attempt::attempt_output;
use planning::{FinalizationPlan, PlanningStop, plan_finalization};
use plugins::maintain_existing_loading_plugins;

/// Why Archive Finalization, or one of its output attempts, failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArchiveFinalizationFailure {
    /// A Capacity Check found too little space before any mutation.
    InsufficientCapacity,
    /// The Archive could not be written to staging.
    WriteFailed,
    /// The staged Archive could not be published.
    CommitFailed,
    /// A Loading Plugin could not be found or created.
    PluginCreationFailed,
    /// A Dummy Plugin could not be removed.
    PluginRemovalFailed,
    /// A packed source could not be removed.
    SourceCleanupFailed,
    /// The phase failed outside any attempt it could classify.
    UnexpectedException,
}

/// One complete output attempt, including durable mutation after a cleanup failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveFinalizationAttempt {
    /// The planned output Archive.
    pub archive_path: PathBuf,
    /// The frozen plan's canonical Mod Root, independent of directory depth.
    pub mod_root: PathBuf,
    /// What the attempt left behind: its mutation, its Operation Failure if
    /// it failed, and whether the run may continue after it. Public because
    /// the Attempt Outcome protects its own invariants.
    pub outcome: AttemptOutcome<ArchiveFinalizationFailure>,
}

impl ArchiveFinalizationAttempt {
    /// The completed attempt at `archive_path` in `mod_root`, which left
    /// `outcome` behind.
    pub fn new(
        archive_path: PathBuf,
        mod_root: PathBuf,
        outcome: AttemptOutcome<ArchiveFinalizationFailure>,
    ) -> Self {
        Self {
            archive_path,
            mod_root,
            outcome,
        }
    }

    /// Whether staging, publication, the Loading Plugin and any requested
    /// source cleanup all finished.
    pub fn succeeded(&self) -> bool {
        self.outcome.succeeded()
    }

    /// The Attempt Outcome's verdict on whether the run may start the next
    /// output after this attempt; partial or unknown mutation is never safe,
    /// whatever the producer claimed.
    pub fn safe_to_continue(&self) -> bool {
        self.outcome.safe_to_continue()
    }
}

/// The kinds of finalization effects outside planned output attempts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArchiveFinalizationMutationKind {
    /// Empty directories removed from one Mod Root.
    EmptyDirectoryPruning,
    /// A Dummy Plugin published.
    PluginCreation,
    /// An exact Dummy Plugin removed.
    PluginRemoval,
}

/// One completed effect that no output attempt accounts for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveFinalizationMutation {
    pub mod_root: PathBuf,
    /// The plugin created or removed, or the pruned Mod Root.
    pub path: PathBuf,
    pub kind: ArchiveFinalizationMutationKind,
    pub mutation: MutationState,
    /// How many effects this fact stands for: the directories pruned, or 1.
    pub count: usize,
}

/// The phase's evidence, owned independently of its plan and staging.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveFinalizationResult {
    /// Completed output attempts, in plan order.
    pub attempts: Vec<ArchiveFinalizationAttempt>,
    /// A phase-level failure. It has no output attempt and does not advance
    /// progress.
    pub failure: Option<ArchiveFinalizationFailure>,
    pub cancelled: bool,
    pub safe_to_continue: bool,
    pub detail: String,
    pub mutations: Vec<ArchiveFinalizationMutation>,
}

impl Default for ArchiveFinalizationResult {
    fn default() -> Self {
        Self {
            attempts: Vec::new(),
            failure: None,
            cancelled: false,
            safe_to_continue: true,
            detail: String::new(),
            mutations: Vec::new(),
        }
    }
}

/// The phase-level account Archive Finalization finishes its phase with:
/// every field of [`ArchiveFinalizationResult`] except its attempts.
///
/// Run Evidence is the only holder of the output attempts, streamed through
/// the phase's handle as each one completes, and puts the public result
/// together from them plus this account. Crate-private, so the public result
/// keeps its shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArchiveFinalizationAccount {
    /// A phase-level failure. It has no output attempt and does not advance
    /// progress.
    pub(crate) failure: Option<ArchiveFinalizationFailure>,
    /// Whether the phase observed cancellation; it marks the run's
    /// cancellation observed when the phase finishes.
    pub(crate) cancelled: bool,
    pub(crate) safe_to_continue: bool,
    pub(crate) detail: String,
    /// Completed effects that no output attempt accounts for.
    pub(crate) mutations: Vec<ArchiveFinalizationMutation>,
}

impl Default for ArchiveFinalizationAccount {
    /// A phase that has not failed, been cancelled or mutated anything
    /// outside its attempts, and so is safe to continue after.
    fn default() -> Self {
        Self {
            failure: None,
            cancelled: false,
            safe_to_continue: true,
            detail: String::new(),
            mutations: Vec::new(),
        }
    }
}

/// The run's Archive Finalization choices, captured from its options before
/// scheduling. Whether packing runs at all is the Routing Policy's Archive
/// creation request, not a setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArchiveFinalizationSettings {
    /// Compress Standard and Textures Archives (`bBsaCompress`).
    pub compress: bool,
    /// Delete packed Loose Assets once their Archive is loadable (`bBsaDeleteSource`).
    pub delete_sources: bool,
    /// Create missing Dummy Plugins; otherwise remove exact ones (`bBsaCreateDummies`).
    pub create_dummy_plugins: bool,
    /// Merge Incompressible files into the Standard Archive (`bBsaMergeIncomp`).
    pub merge_incompressible: bool,
    /// Merge Textures into the Standard Archive (`bBsaMergeTexture`), unless
    /// the game keeps them separate (deviation 21).
    pub merge_textures: bool,
}

impl Default for ArchiveFinalizationSettings {
    /// The options model's defaults.
    fn default() -> Self {
        Self {
            compress: true,
            delete_sources: true,
            create_dummy_plugins: true,
            merge_incompressible: true,
            merge_textures: false,
        }
    }
}

/// Runs the Archive Finalization Run Phase over the preparation's Mod Roots,
/// writing its facts through `phase`, the handle that opened it. Archive
/// Finalization's only entry point: the Run Toolset calls it once per Apply
/// run.
///
/// It runs over the run's seams: `seams.packer` holds the game's rules and
/// writes Archives, `seams.reader` verifies a committed Archive is readable,
/// and `capacity`, the run's Capacity Check, is the phase's only other
/// filesystem seam. [`CapacityCheck`] is the only caller of
/// `seams.volume_probe`: the phase never samples the probe itself, so every
/// check and recheck groups Mod Roots by the volume identities the run froze.
/// `settings` are the run's Archive Finalization choices.
///
/// The Packing Exclusions come from the Selected Profile
/// ([`SelectedProfile::packing_exclusions`]) in `preparation`. Each rule is
/// matched as a case-insensitive substring of the `/`-separated path within
/// the Mod Root.
///
/// `phase` must have just opened the executed Archive Finalization phase,
/// unplanned; the phase is finished through it. `cancelled` is polled
/// between outputs and between Mod Roots, never inside an atomic output
/// attempt; cancelled planning finishes the phase as cancelled without an
/// output total. A failure or panic of the phase's own work finishes it
/// once as a phase-level `UnexpectedException` that keeps every attempt
/// already recorded and forbids continuation. The caller owns `artifacts`
/// through Safety Cleanup.
///
/// # Errors
/// Only a Run Evidence invariant violation, returned unchanged so the Run
/// Executor still performs Safety Cleanup. The phase is then left
/// unfinished: its streamed attempts stay in the evidence with default
/// phase-level fields.
///
/// [`SelectedProfile::packing_exclusions`]: crate::run::SelectedProfile::packing_exclusions
pub(crate) fn finalize_archives(
    seams: &ArchiveSeams,
    capacity: &CapacityCheck<'_>,
    settings: &ArchiveFinalizationSettings,
    preparation: &RunPreparation,
    mut phase: ArchiveFinalizationHandle<'_, '_>,
    artifacts: &mut TemporaryArtifactRegistry,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), Error> {
    // The handle and the account are owned here, outside the panic
    // containment, and only lent to the contained work: a panic after
    // planning then still finishes the phase that holds every attempt
    // streamed so far, with every mutation fact established.
    let mut account = ArchiveFinalizationAccount::default();
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        run_phase(
            seams,
            capacity,
            settings,
            preparation,
            &mut phase,
            artifacts,
            cancelled,
            &mut account,
        )
    }));
    account.detail = match outcome {
        Ok(Ok(())) => return phase.finish(account),
        Ok(Err(Escape::Evidence(error))) => return Err(error),
        Ok(Err(Escape::Work(detail))) => detail,
        Err(payload) => take_panic_message(payload),
    };
    // The escaped failure gives no reliable boundary for further durable
    // effects. Every attempt was already streamed into the evidence, so the
    // phase-level failure keeps them all.
    account.failure = Some(ArchiveFinalizationFailure::UnexpectedException);
    account.safe_to_continue = false;
    phase.finish(account)
}

/// The phase's work, which plans the phase and streams each output attempt
/// through `phase`. `account` accumulates in the caller, so an escaping
/// failure or panic keeps every mutation fact established; the caller
/// finishes the phase with it.
#[expect(
    clippy::too_many_arguments,
    reason = "the phase's inputs, plus the handle and account its caller keeps across a panic"
)]
fn run_phase(
    seams: &ArchiveSeams,
    capacity: &CapacityCheck<'_>,
    settings: &ArchiveFinalizationSettings,
    preparation: &RunPreparation,
    phase: &mut ArchiveFinalizationHandle<'_, '_>,
    artifacts: &mut TemporaryArtifactRegistry,
    cancelled: &dyn Fn() -> bool,
    account: &mut ArchiveFinalizationAccount,
) -> Result<(), Escape> {
    if !preparation
        .policy()
        .requests(RequestedWork::ArchiveCreation)
    {
        // Empty-directory pruning is the legacy Apply finalization even
        // without packing.
        phase.plan(0).map_err(Escape::Evidence)?;
        prune_empty_directories(preparation.mod_roots(), cancelled, account);
    } else {
        let exclusions = packing_exclusions(preparation);
        // The list only filters packing, so its absence matters only when
        // packing runs.
        if exclusions.is_empty() {
            log::error!(
                "FilesToNotPack.txt not found. This can cause a number of issues. For \
                 example, for Skyrim, animations will be packed to BSA, preventing them from \
                 being detected by FNIS and Nemesis."
            );
        }
        let plan = match plan_finalization(
            preparation.mod_roots(),
            settings,
            &*seams.packer,
            &exclusions,
            cancelled,
        ) {
            Ok(plan) => plan,
            Err(PlanningStop::Cancelled) => {
                // Planning made no mutation and has no trustworthy output
                // total to publish.
                account.cancelled = true;
                return Ok(());
            }
            Err(PlanningStop::Failed(detail)) => return Err(Escape::Work(detail)),
        };
        phase.plan(plan.outputs.len()).map_err(Escape::Evidence)?;
        finalize_plan(seams, capacity, &plan, artifacts, cancelled, phase, account)?;
    }
    Ok(())
}

/// Publishes each planned output in one atomic attempt, then maintains
/// Loading Plugins for existing Archives and prunes empty directories.
///
/// A volume must fit its whole phase before any root mutates: planned
/// source deletion is never counted as space before it happens. Each
/// attempt rechecks capacity first; a known shortage there stops the phase
/// before mutation, with no plugin maintenance or pruning. An unsafe
/// attempt stops later outputs. Plugin maintenance runs per Mod Root after
/// every output, even with no outputs, rechecking only the plugin
/// allowance the remaining roots still need. Pruning waits for all of it
/// and is skipped after cancellation.
fn finalize_plan(
    seams: &ArchiveSeams,
    capacity: &CapacityCheck<'_>,
    plan: &FinalizationPlan,
    artifacts: &mut TemporaryArtifactRegistry,
    cancelled: &dyn Fn() -> bool,
    phase: &mut ArchiveFinalizationHandle<'_, '_>,
    account: &mut ArchiveFinalizationAccount,
) -> Result<(), Escape> {
    if cancelled() {
        account.cancelled = true;
        return Ok(());
    }
    // Each root's volume is frozen the first time the Capacity Check asks
    // for it, for this check and every later recheck, so Several Mods
    // never repeats native volume queries. A panicking query is an
    // unknown volume, not the phase's Unexpected Exception: the
    // unknown-volume rule keeps the check conservative.
    let mut phase_demand = capacity.demand();
    let mut dummy_demand = capacity.demand();
    for root in &plan.roots {
        let required = plan.dummy_capacity_by_root[root];
        phase_demand.add(root, required);
        dummy_demand.add(root, required);
    }
    for output in &plan.outputs {
        phase_demand.add(&output.mod_root, output.estimated_capacity_bytes);
    }
    // A Mod Root with no planned writes cannot run out of staging space
    // during this phase, so only roots with demand are checked, in root
    // order.
    if let Some(shortfall) = capacity.first_shortfall(&phase_demand) {
        let archive = plan
            .outputs
            .iter()
            .find(|output| output.mod_root == shortfall.root)
            .map(|output| output.archive_path.as_path());
        return record_shortfall(shortfall, archive, phase, account);
    }

    for (index, _) in plan.outputs.iter().enumerate() {
        if cancelled() {
            account.cancelled = true;
            break;
        }
        let attempt = attempt_output(
            plan,
            index,
            artifacts,
            seams,
            capacity,
            &dummy_demand,
            &mut account.mutations,
        );
        // Only the attempt's own capacity recheck reports this, before
        // any mutation; it ends the phase without plugin work or pruning.
        let capacity_rejected = attempt.outcome.failure_category()
            == Some(ArchiveFinalizationFailure::InsufficientCapacity);
        if !capacity_rejected {
            // The verdict, not the producer's raw claim: partial or unknown
            // mutation stops the phase even when the attempt claims safety.
            // It is taken before the attempt moves into the evidence, so
            // the phase never reads an attempt back.
            account.safe_to_continue = attempt.safe_to_continue();
        }
        // Evidence owns each atomic result before the next output starts.
        phase.record(attempt).map_err(Escape::Evidence)?;
        if capacity_rejected {
            return Ok(());
        }
        if !account.safe_to_continue {
            break;
        }
    }
    account.cancelled = account.cancelled || cancelled();
    if account.cancelled || !account.safe_to_continue {
        return Ok(());
    }

    let cleanup = catch_unwind(AssertUnwindSafe(|| {
        maintain_and_prune(seams, capacity, plan, artifacts, cancelled, phase, account)
    }));
    let detail = match cleanup {
        Ok(Ok(())) => return Ok(()),
        // Run Evidence failures stay distinct from this pass's own.
        Ok(Err(Escape::Evidence(error))) => return Err(Escape::Evidence(error)),
        Ok(Err(Escape::Work(detail))) => detail,
        Err(payload) => take_panic_message(payload),
    };
    account.safe_to_continue = false;
    account.failure = Some(ArchiveFinalizationFailure::UnexpectedException);
    account.detail = detail;
    Ok(())
}

/// Maintains each Mod Root's Loading Plugins, then prunes every root once
/// all of them finished. Returns early after a recorded plugin failure,
/// shortage or cancellation.
fn maintain_and_prune(
    seams: &ArchiveSeams,
    capacity: &CapacityCheck<'_>,
    plan: &FinalizationPlan,
    artifacts: &mut TemporaryArtifactRegistry,
    cancelled: &dyn Fn() -> bool,
    phase: &mut ArchiveFinalizationHandle<'_, '_>,
    account: &mut ArchiveFinalizationAccount,
) -> Result<(), Escape> {
    for (index, root) in plan.roots.iter().enumerate() {
        if cancelled() {
            account.cancelled = true;
            break;
        }
        if plan.dummy_capacity_by_root[root] != 0 {
            // Earlier roots have already used their plugin allowance, so
            // rechecking it would reject later roots despite a sufficient
            // phase budget.
            let mut remaining = capacity.demand();
            for later in &plan.roots[index..] {
                remaining.add(later, plan.dummy_capacity_by_root[later]);
            }
            if let Some(shortfall) = capacity.shortfall_at(&remaining, root) {
                return record_shortfall(shortfall, None, phase, account);
            }
        }
        // A probe may block while cancellation arrives; do not start the
        // next root's plugin mutation after it.
        if cancelled() {
            account.cancelled = true;
            break;
        }
        if !maintain_existing_loading_plugins(
            plan,
            root,
            &*seams.packer,
            artifacts,
            cancelled,
            account,
        )
        .map_err(Escape::Work)?
        {
            return Ok(());
        }
    }
    // Recoverable attempts keep their source evidence; cancellation keeps
    // every path.
    if !account.cancelled {
        prune_empty_directories(&plan.roots, cancelled, account);
    }
    Ok(())
}

/// Records a known `shortfall` at its Mod Root as the attempt of `archive`,
/// or as a phase-level failure when the root has no planned output. The
/// phase stops after it, before any further mutation.
fn record_shortfall(
    shortfall: Shortfall,
    archive: Option<&Path>,
    phase: &mut ArchiveFinalizationHandle<'_, '_>,
    account: &mut ArchiveFinalizationAccount,
) -> Result<(), Escape> {
    let detail = shortfall.to_string();
    match archive {
        None => {
            account.failure = Some(ArchiveFinalizationFailure::InsufficientCapacity);
            account.detail = detail;
        }
        Some(archive) => {
            // A known shortage is found before any mutation, so the producer
            // still claims it safe; the phase stops after it regardless.
            let outcome = AttemptOutcome::failed(
                OperationFailure::new(ArchiveFinalizationFailure::InsufficientCapacity, detail),
                MutationState::None,
                true,
            );
            let attempt =
                ArchiveFinalizationAttempt::new(archive.to_path_buf(), shortfall.root, outcome);
            phase.record(attempt).map_err(Escape::Evidence)?;
        }
    }
    Ok(())
}

/// How the phase's work stopped short of finishing normally.
enum Escape {
    /// Run Evidence rejected a fact: a producer bug the executor reports
    /// after Safety Cleanup. Never converted into a finalization account,
    /// so the phase is left unfinished.
    Evidence(Error),
    /// The phase's own work failed outside any attempt.
    Work(String),
}

/// The Selected Profile's Packing Exclusion rules, read from
/// `FilesToNotPack.txt` during Preparing, lowercase with `/` separators.
///
/// **Deviation 16:** each is matched as a case-insensitive substring of the
/// `/`-separated path within the Mod Root, never the absolute path, so a rule
/// cannot match the folder the mods live in.
fn packing_exclusions(preparation: &RunPreparation) -> Vec<String> {
    preparation
        .selected_profile()
        .packing_exclusions()
        .iter()
        .map(|rule| rule.replace('\\', "/").to_lowercase())
        .collect()
}

/// Prunes empty directories beneath each Mod Root, recording one mutation per
/// root that lost any. Cancellation stops before the next root.
fn prune_empty_directories(
    roots: &[PathBuf],
    cancelled: &dyn Fn() -> bool,
    account: &mut ArchiveFinalizationAccount,
) {
    for root in roots {
        if cancelled() {
            account.cancelled = true;
            break;
        }
        let pruned = delete_empty_directories(root);
        if pruned != 0 {
            account.mutations.push(ArchiveFinalizationMutation {
                mod_root: root.clone(),
                path: root.clone(),
                kind: ArchiveFinalizationMutationKind::EmptyDirectoryPruning,
                mutation: MutationState::Committed,
                count: pruned,
            });
        }
    }
}

/// Windows' hidden attribute, which C++'s `QDirIterator` neither listed nor
/// descended into without `QDir::Hidden`.
const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;

/// Removes every empty directory beneath `root`, deepest first, and returns
/// how many went (C++ `FilesystemOperations::deleteEmptyDirectories`).
///
/// The root itself is never removed. Reserved staging belongs to Safety
/// Cleanup and is skipped, as are links and other reparse points, which are
/// neither descended into nor removed, and hidden directories, as Qt skipped
/// them. Removal is nonrecursive, so a directory that is not empty, or that
/// cannot be listed, simply stays. **Deviation 20:** C++ also skipped any
/// directory whose full path contained "separator", case-insensitively;
/// that rule is gone.
fn delete_empty_directories(root: &Path) -> usize {
    let mut directories = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            // `DirEntry::metadata` does not follow links on Windows.
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if !metadata.is_dir()
                || metadata.is_symlink()
                || is_reparse_point(&metadata)
                || metadata.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0
            {
                continue;
            }
            let path = entry.path();
            if has_staging_component(path.strip_prefix(root).unwrap_or(&path)) {
                continue;
            }
            directories.push(path.clone());
            pending.push(path);
        }
    }
    // A child's path is always longer than its parent's, so longest first
    // empties each child before its parent is tried, as C++'s length-keyed
    // map iterated from the back did.
    directories.sort_by_key(|path| std::cmp::Reverse(path.as_os_str().encode_wide().count()));
    directories
        .iter()
        .filter(|directory| std::fs::remove_dir(directory).is_ok())
        .count()
}
