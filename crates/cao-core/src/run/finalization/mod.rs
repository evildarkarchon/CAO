//! Archive Finalization: the Apply-only Run Phase that packs Loose Assets into
//! new Archives and maintains their Loading Plugins.
//!
//! Ported from `src/Run/ArchiveFinalization.cpp` and its private parts
//! (`ArchiveFinalizationPlanning.cpp`, `ArchiveFinalizationAttempt.cpp`,
//! `ArchiveFinalizationLoadingPlugins.cpp`), plus the empty-directory pruning
//! of `FilesystemOperations::deleteEmptyDirectories`. Archives are written
//! only through the [`ArchivePacker`] seam and read back only through the
//! [`ArchiveReader`], so none of this needs `ba2`.
//!
//! When the Routing Policy requests Archive creation, the phase freezes an
//! output plan for every Mod Root without mutating anything, checks staging
//! capacity per volume, then runs one atomic attempt per output: write and
//! no-replace publication of the Archive, its Loading Plugin, then packed
//! source cleanup. Loading Plugins of existing Archives are maintained after
//! the last output. Empty directories are pruned whether or not Archive
//! creation is requested. The phase records its output total, each attempt
//! before the next output starts, and one final result into Run Evidence.

mod attempt;
mod planning;
mod plugins;

use std::collections::BTreeMap;
use std::os::windows::ffi::OsStrExt as _;
use std::os::windows::fs::MetadataExt as _;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use cao_winfs::is_reparse_point;

use crate::Error;
use crate::execution::MutationState;
use crate::routing::RequestedWork;
use crate::run::archives::{CapacityRequirements, capacity_detail};
use crate::run::staging::has_staging_component;
use crate::run::{
    ArchivePacker, ArchiveReader, CapacityProbe, RunPreparation, RunWorkEvidence,
    TemporaryArtifactRegistry, VolumeIdentityProbe, take_panic_message,
};

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
    pub mutation: MutationState,
    pub failure: Option<ArchiveFinalizationFailure>,
    /// Whether the run may continue after this attempt.
    pub safe_to_continue: bool,
    pub detail: String,
}

impl ArchiveFinalizationAttempt {
    /// A successful attempt at `archive_path` that has mutated nothing yet.
    pub fn new(archive_path: PathBuf, mod_root: PathBuf) -> Self {
        Self {
            archive_path,
            mod_root,
            mutation: MutationState::None,
            failure: None,
            safe_to_continue: true,
            detail: String::new(),
        }
    }

    /// Whether staging, publication, the Loading Plugin and any requested
    /// source cleanup all finished.
    pub fn succeeded(&self) -> bool {
        self.failure.is_none()
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

/// The seams one finalization reads and writes through.
#[derive(Clone, Copy)]
pub(crate) struct Seams<'a> {
    pub packer: &'a dyn ArchivePacker,
    pub reader: &'a dyn ArchiveReader,
    pub capacity: &'a dyn CapacityProbe,
}

/// The Archive Finalization Run Phase for one Optimization Run.
pub struct ArchiveFinalization<'a> {
    seams: Seams<'a>,
    volume_identity: &'a dyn VolumeIdentityProbe,
    settings: ArchiveFinalizationSettings,
    /// The Packing Exclusions, lowercase with `/` separators.
    files_to_not_pack: Vec<String>,
}

impl<'a> ArchiveFinalization<'a> {
    /// The phase over the run's seams: `packer` holds the game's rules and
    /// writes Archives, `reader` verifies a committed Archive is readable, and
    /// the probes are the phase's only other filesystem seams.
    ///
    /// `files_to_not_pack` holds the profile's Packing Exclusion rules, read
    /// from `FilesToNotPack.txt` during Preparing. **Deviation 16:** each is
    /// matched as a case-insensitive substring of the `/`-separated path
    /// within the Mod Root, never the absolute path, so a rule cannot match
    /// the folder the mods live in.
    pub fn new(
        packer: &'a dyn ArchivePacker,
        reader: &'a dyn ArchiveReader,
        capacity: &'a dyn CapacityProbe,
        volume_identity: &'a dyn VolumeIdentityProbe,
        settings: ArchiveFinalizationSettings,
        files_to_not_pack: &[String],
    ) -> Self {
        Self {
            seams: Seams {
                packer,
                reader,
                capacity,
            },
            volume_identity,
            settings,
            files_to_not_pack: files_to_not_pack
                .iter()
                .map(|rule| rule.replace('\\', "/").to_lowercase())
                .collect(),
        }
    }

    /// Runs the phase over the preparation's Mod Roots.
    ///
    /// Evidence must already be in the executed Archive Finalization phase.
    /// `cancelled` is polled between outputs and between Mod Roots, never
    /// inside an atomic output attempt; cancelled planning records a cancelled
    /// result without an output total. A failure or panic of the phase's own
    /// work is recorded once as a phase-level `UnexpectedException` that keeps
    /// every attempt already recorded and forbids continuation. The caller
    /// owns `artifacts` through Safety Cleanup.
    ///
    /// # Errors
    /// Only a Run Evidence invariant violation, returned unchanged so the Run
    /// Executor still performs Safety Cleanup.
    pub fn run(
        &self,
        preparation: &RunPreparation,
        evidence: &RunWorkEvidence<'_, '_>,
        artifacts: &mut TemporaryArtifactRegistry,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), Error> {
        let mut recorder = Recorder { evidence, total: 0 };
        let mut result = ArchiveFinalizationResult::default();
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            self.run_phase(
                preparation,
                &mut recorder,
                artifacts,
                cancelled,
                &mut result,
            )
        }));
        result.detail = match outcome {
            Ok(Ok(())) => return Ok(()),
            Ok(Err(Escape::Evidence(error))) => return Err(error),
            Ok(Err(Escape::Work(detail))) => detail,
            Err(payload) => take_panic_message(payload),
        };
        // The escaped failure gives no reliable boundary for further durable
        // effects. Every attempt in `result` was already recorded, so the
        // phase-level failure keeps that prefix.
        result.failure = Some(ArchiveFinalizationFailure::UnexpectedException);
        result.safe_to_continue = false;
        evidence.record_archive_finalization(result)
    }

    /// The phase's work. `result` accumulates in the caller, so an escaping
    /// failure or panic keeps every attempt and mutation fact established.
    fn run_phase(
        &self,
        preparation: &RunPreparation,
        recorder: &mut Recorder<'_, '_, '_>,
        artifacts: &mut TemporaryArtifactRegistry,
        cancelled: &dyn Fn() -> bool,
        result: &mut ArchiveFinalizationResult,
    ) -> Result<(), Escape> {
        if !preparation
            .policy()
            .requests(RequestedWork::ArchiveCreation)
        {
            // Empty-directory pruning is the legacy Apply finalization even
            // without packing.
            recorder.plan(0)?;
            prune_empty_directories(preparation.mod_roots(), cancelled, result);
        } else {
            // The list only filters packing, so its absence matters only when
            // packing runs.
            if self.files_to_not_pack.is_empty() {
                log::error!(
                    "FilesToNotPack.txt not found. This can cause a number of issues. For \
                     example, for Skyrim, animations will be packed to BSA, preventing them from \
                     being detected by FNIS and Nemesis."
                );
            }
            let plan = match plan_finalization(
                preparation.mod_roots(),
                &self.settings,
                self.seams.packer,
                &self.files_to_not_pack,
                cancelled,
            ) {
                Ok(plan) => plan,
                Err(PlanningStop::Cancelled) => {
                    // Planning made no mutation and has no trustworthy output
                    // total to publish.
                    result.cancelled = true;
                    return recorder.result(std::mem::take(result));
                }
                Err(PlanningStop::Failed(detail)) => return Err(Escape::Work(detail)),
            };
            recorder.plan(plan.outputs.len())?;
            self.finalize_plan(&plan, artifacts, cancelled, recorder, result)?;
        }
        recorder.result(std::mem::take(result))
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
        &self,
        plan: &FinalizationPlan,
        artifacts: &mut TemporaryArtifactRegistry,
        cancelled: &dyn Fn() -> bool,
        recorder: &mut Recorder<'_, '_, '_>,
        result: &mut ArchiveFinalizationResult,
    ) -> Result<(), Escape> {
        if cancelled() {
            result.cancelled = true;
            return Ok(());
        }
        // Freeze each root's volume once for the preflight and every later
        // recheck, so Several Mods never repeats native volume queries.
        let volumes = CachedVolumes(
            plan.roots
                .iter()
                .map(|root| (root.clone(), self.volume_identity.volume_identity(root)))
                .collect(),
        );
        let mut phase_capacity = CapacityRequirements::new(&volumes);
        let mut dummy_capacity = CapacityRequirements::new(&volumes);
        for root in &plan.roots {
            let required = plan.dummy_capacity_by_root[root];
            phase_capacity.add(root, required);
            dummy_capacity.add(root, required);
        }
        for output in &plan.outputs {
            phase_capacity.add(&output.mod_root, output.estimated_capacity_bytes);
        }
        for root in &plan.roots {
            let output = plan.outputs.iter().find(|output| output.mod_root == *root);
            // A Mod Root with no planned writes cannot run out of staging
            // space during this phase.
            if output.is_none() && plan.dummy_capacity_by_root[root] == 0 {
                continue;
            }
            let archive = output.map(|output| output.archive_path.as_path());
            if !self.has_capacity(
                root,
                archive,
                phase_capacity.required_at(root),
                recorder,
                result,
            )? {
                return Ok(());
            }
        }

        for (index, output) in plan.outputs.iter().enumerate() {
            if cancelled() {
                result.cancelled = true;
                break;
            }
            let attempt = attempt_output(
                plan,
                index,
                artifacts,
                self.seams,
                dummy_capacity.required_at(&output.mod_root),
                &mut result.mutations,
            );
            // Only the attempt's own capacity recheck reports this, before
            // any mutation; it ends the phase without plugin work or pruning.
            let capacity_rejected =
                attempt.failure == Some(ArchiveFinalizationFailure::InsufficientCapacity);
            if !capacity_rejected {
                result.safe_to_continue = attempt.safe_to_continue;
            }
            result.attempts.push(attempt);
            // Evidence owns each atomic result before the next output starts.
            recorder.attempt(result.attempts.last().expect("just pushed"))?;
            if capacity_rejected {
                return Ok(());
            }
            if !result.safe_to_continue {
                break;
            }
        }
        result.cancelled = result.cancelled || cancelled();
        if result.cancelled || !result.safe_to_continue {
            return Ok(());
        }

        let cleanup = catch_unwind(AssertUnwindSafe(|| {
            self.maintain_and_prune(plan, &volumes, artifacts, cancelled, recorder, result)
        }));
        let detail = match cleanup {
            Ok(Ok(())) => return Ok(()),
            // Run Evidence failures stay distinct from this pass's own.
            Ok(Err(Escape::Evidence(error))) => return Err(Escape::Evidence(error)),
            Ok(Err(Escape::Work(detail))) => detail,
            Err(payload) => take_panic_message(payload),
        };
        result.safe_to_continue = false;
        result.failure = Some(ArchiveFinalizationFailure::UnexpectedException);
        result.detail = detail;
        Ok(())
    }

    /// Maintains each Mod Root's Loading Plugins, then prunes every root once
    /// all of them finished. Returns early after a recorded plugin failure,
    /// shortage or cancellation.
    fn maintain_and_prune(
        &self,
        plan: &FinalizationPlan,
        volumes: &CachedVolumes,
        artifacts: &mut TemporaryArtifactRegistry,
        cancelled: &dyn Fn() -> bool,
        recorder: &mut Recorder<'_, '_, '_>,
        result: &mut ArchiveFinalizationResult,
    ) -> Result<(), Escape> {
        for (index, root) in plan.roots.iter().enumerate() {
            if cancelled() {
                result.cancelled = true;
                break;
            }
            if plan.dummy_capacity_by_root[root] != 0 {
                // Earlier roots have already used their plugin allowance, so
                // rechecking it would reject later roots despite a sufficient
                // phase budget.
                let mut remaining = CapacityRequirements::new(volumes);
                for later in &plan.roots[index..] {
                    remaining.add(later, plan.dummy_capacity_by_root[later]);
                }
                if !self.has_capacity(root, None, remaining.required_at(root), recorder, result)? {
                    return Ok(());
                }
            }
            // A probe may block while cancellation arrives; do not start the
            // next root's plugin mutation after it.
            if cancelled() {
                result.cancelled = true;
                break;
            }
            if !maintain_existing_loading_plugins(
                plan,
                root,
                self.seams.packer,
                artifacts,
                cancelled,
                result,
            )
            .map_err(Escape::Work)?
            {
                return Ok(());
            }
        }
        // Recoverable attempts keep their source evidence; cancellation keeps
        // every path.
        if !result.cancelled {
            prune_empty_directories(&plan.roots, cancelled, result);
        }
        Ok(())
    }

    /// Checks `required` bytes at `root`, recording a known shortfall as the
    /// attempt of `archive`, or as a phase-level failure when the root has no
    /// planned output. Returns whether the phase may go on.
    fn has_capacity(
        &self,
        root: &Path,
        archive: Option<&Path>,
        required: u64,
        recorder: &mut Recorder<'_, '_, '_>,
        result: &mut ArchiveFinalizationResult,
    ) -> Result<bool, Escape> {
        let Some(shortfall) = capacity_shortfall(self.seams.capacity, root, required) else {
            return Ok(true);
        };
        match archive {
            None => {
                result.failure = Some(ArchiveFinalizationFailure::InsufficientCapacity);
                result.detail = shortfall;
            }
            Some(archive) => {
                let mut attempt =
                    ArchiveFinalizationAttempt::new(archive.to_path_buf(), root.to_path_buf());
                attempt.failure = Some(ArchiveFinalizationFailure::InsufficientCapacity);
                attempt.detail = shortfall;
                result.attempts.push(attempt);
                recorder.attempt(result.attempts.last().expect("just pushed"))?;
            }
        }
        Ok(false)
    }
}

/// How the phase's work stopped short of recording its own result.
enum Escape {
    /// Run Evidence rejected a fact: a producer bug the executor reports
    /// after Safety Cleanup. Never converted into a finalization result.
    Evidence(Error),
    /// The phase's own work failed outside any attempt.
    Work(String),
}

/// Submits the phase's evidence protocol (output total, attempts, one
/// result), keeping every Run Evidence failure distinct from work failures.
struct Recorder<'r, 'e, 'a> {
    evidence: &'r RunWorkEvidence<'e, 'a>,
    total: usize,
}

impl Recorder<'_, '_, '_> {
    /// Starts determinate progress with the frozen output total.
    fn plan(&mut self, total: usize) -> Result<(), Escape> {
        self.evidence
            .record_archive_finalization_plan(total)
            .map_err(Escape::Evidence)?;
        self.total = total;
        Ok(())
    }

    /// Retains one completed attempt before the next output starts.
    fn attempt(&self, attempt: &ArchiveFinalizationAttempt) -> Result<(), Escape> {
        self.evidence
            .record_archive_finalization_attempt(attempt.clone(), self.total)
            .map_err(Escape::Evidence)
    }

    /// Retains the phase's single result, whose attempts match those recorded.
    fn result(&self, result: ArchiveFinalizationResult) -> Result<(), Escape> {
        self.evidence
            .record_archive_finalization(result)
            .map_err(Escape::Evidence)
    }
}

/// Volume identities frozen once per Mod Root for the whole phase.
struct CachedVolumes(BTreeMap<PathBuf, Option<String>>);

impl VolumeIdentityProbe for CachedVolumes {
    fn volume_identity(&self, root: &Path) -> Option<String> {
        self.0.get(root).cloned().flatten()
    }
}

/// Samples `root`'s capacity against `required`: the rejection detail when
/// the estimate is known not to fit, `None` when it fits or capacity is
/// unknown. A panicking probe is unknown capacity, as a throwing one was in
/// C++: the atomic writer still handles real I/O errors.
pub(crate) fn capacity_shortfall(
    capacity: &dyn CapacityProbe,
    root: &Path,
    required: u64,
) -> Option<String> {
    let available = catch_unwind(AssertUnwindSafe(|| capacity.available_bytes(root)))
        .unwrap_or_else(|payload| {
            take_panic_message(payload);
            None
        })?;
    (available < required).then(|| capacity_detail(required, available))
}

/// Prunes empty directories beneath each Mod Root, recording one mutation per
/// root that lost any. Cancellation stops before the next root.
fn prune_empty_directories(
    roots: &[PathBuf],
    cancelled: &dyn Fn() -> bool,
    result: &mut ArchiveFinalizationResult,
) {
    for root in roots {
        if cancelled() {
            result.cancelled = true;
            break;
        }
        let pruned = delete_empty_directories(root);
        if pruned != 0 {
            result.mutations.push(ArchiveFinalizationMutation {
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
