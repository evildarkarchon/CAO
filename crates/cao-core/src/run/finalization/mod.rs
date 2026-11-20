//! Archive Finalization: the Run Phase that packs Loose Assets into new
//! Archives and maintains their Loading Plugins under Apply, and under Dry
//! Run plans its output Archives, runs the Apply run's Capacity Check over
//! them without failing, and stops ([`plan_archive_creation`]).
//!
//! Archives are written only through the
//! [`ArchivePacker`](crate::run::ArchivePacker) seam and read back only
//! through the [`ArchiveReader`](crate::run::ArchiveReader), so none of this
//! needs `ba2`.
//!
//! When the Routing Policy requests Archive creation, the phase freezes an
//! output plan for every Mod Root without mutating anything, checks staging
//! capacity per volume, then runs one atomic attempt per output: write and
//! no-replace publication of the Archive, its Loading Plugin, then packed
//! source cleanup. Loading Plugins of existing Archives are maintained after
//! the last output. Last, the folders this run emptied are pruned, whether
//! or not Archive creation is requested: those packing or Archive extraction
//! deleted the last file from, never a folder a mod ships empty (audit
//! FS-7). The phase records its output total, each attempt
//! before the next output starts, and one final phase-level account into Run
//! Evidence through the handle that opened its phase, which reaches nothing
//! else. Run Evidence is the only holder of the attempts.

mod attempt;
mod planning;
mod plugins;
#[cfg(test)]
mod tests;

use std::collections::{BTreeSet, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use cao_winfs::{OrdinalIgnoreCase, is_reparse_point};

use crate::execution::MutationState;
use crate::routing::RequestedWork;
use crate::run::capacity::{
    CapacityCheck, CapacityShortage, CapacityStage, CapacityVerdict, Demand,
};
use crate::run::staging::has_staging_component;
use crate::run::{
    ArchiveExtractionPlan, ArchiveFinalizationHandle, ArchiveSeams, PlannedOutputArchive,
    PlannedPruning, RunDiagnostic, RunDiagnosticCode, RunPhase, RunPreparation, SourceCleanup,
    TemporaryArtifactRegistry, take_panic_message,
};
use crate::{AttemptOutcome, Error, OperationFailure};

use attempt::attempt_output;
use planning::{
    FinalizationPlan, PlanningCancelled, UnplannedRoot, describe_case_twins, plan_finalization,
};
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
    /// A packed source could not be removed.
    SourceCleanupFailed,
    /// Planning could not plan one or more Mod Roots, such as one with a
    /// directory that cannot be read. Planning only reads, so those roots
    /// are unchanged; they get no output, Loading Plugin maintenance or
    /// pruning, while every other root is finalized. Safe to continue.
    PlanningFailed,
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
}

/// One completed effect that no output attempt accounts for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveFinalizationMutation {
    pub mod_root: PathBuf,
    /// The plugin created, or the pruned Mod Root.
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
    /// A phase-level failure, with the detail it reported. It has no output
    /// attempt and does not advance progress.
    pub failure: Option<OperationFailure<ArchiveFinalizationFailure>>,
    pub cancelled: bool,
    pub safe_to_continue: bool,
    pub mutations: Vec<ArchiveFinalizationMutation>,
}

impl ArchiveFinalizationResult {
    /// The public result of a phase that streamed `attempts` and finished
    /// with the phase-level `account`: the one place the two halves are put
    /// together, so the account's fields map onto the result's in one spot.
    pub(crate) fn from_parts(
        attempts: Vec<ArchiveFinalizationAttempt>,
        account: ArchiveFinalizationAccount,
    ) -> Self {
        let ArchiveFinalizationAccount {
            failure,
            cancelled,
            safe_to_continue,
            mutations,
        } = account;
        Self {
            attempts,
            failure,
            cancelled,
            safe_to_continue,
            mutations,
        }
    }

    /// The stable category of the phase-level failure, or `None` when the
    /// phase did not fail outside its attempts; mirrors
    /// [`AttemptOutcome::failure_category`].
    pub fn failure_category(&self) -> Option<ArchiveFinalizationFailure> {
        self.failure.as_ref().map(OperationFailure::category)
    }
}

impl Default for ArchiveFinalizationResult {
    /// No attempts, with the default phase-level account: not failed, not
    /// cancelled and safe to continue after.
    fn default() -> Self {
        Self::from_parts(Vec::new(), ArchiveFinalizationAccount::default())
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
    /// A phase-level failure, with the detail it reported, such as a
    /// capacity shortage or the escaped panic's message. It has no output
    /// attempt and does not advance progress. Archive Finalization records
    /// it only through [`fail`](Self::fail), and the Operation Failure keeps
    /// a category from ever travelling without its detail.
    pub(crate) failure: Option<OperationFailure<ArchiveFinalizationFailure>>,
    /// Whether the phase observed cancellation; it marks the run's
    /// cancellation observed when the phase finishes.
    pub(crate) cancelled: bool,
    /// The phase-level verdict on whether the run may continue after the
    /// phase: false after an unsafe output attempt, an unsafe Loading Plugin
    /// publication, or a phase-level Unexpected Exception, whose durable
    /// effects have no reliable boundary.
    pub(crate) safe_to_continue: bool,
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
            mutations: Vec::new(),
        }
    }
}

impl ArchiveFinalizationAccount {
    /// Records `failure` as the phase-level failure. Every site that fails
    /// the phase outside an output attempt goes through here; cancellation
    /// and the continuation verdict stay the caller's to set beside it.
    pub(crate) fn fail(&mut self, failure: OperationFailure<ArchiveFinalizationFailure>) {
        self.failure = Some(failure);
    }
}

/// The run's Archive Finalization choices, captured from its options before
/// scheduling. Whether packing runs at all is the Routing Policy's Archive
/// creation request, not one of these choices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArchiveFinalizationChoices {
    /// Compress Standard and Textures Archives (`bBsaCompress`).
    pub compress: bool,
    /// Delete packed Loose Assets once their Archive is loadable (`bBsaDeleteSource`).
    pub delete_sources: bool,
    /// Create missing Dummy Plugins (`bBsaCreateDummies`). Off, no plugin is
    /// created or removed, and an output whose sources would be deleted is
    /// packed only when a plugin already loads it.
    pub create_dummy_plugins: bool,
    /// Merge Incompressible files into the Standard Archive (`bBsaMergeIncomp`).
    pub merge_incompressible: bool,
    /// Merge Textures into the Standard Archive (`bBsaMergeTexture`), unless
    /// the game keeps them separate (deviation 21).
    pub merge_textures: bool,
}

impl Default for ArchiveFinalizationChoices {
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
/// Finalization's only mutating entry point: the Run Toolset calls it once
/// per Apply run, and a Dry Run calls [`plan_archive_creation`] instead.
///
/// It runs over the run's seams: `seams.packer` holds the game's rules and
/// writes Archives, `seams.reader` verifies a committed Archive is readable,
/// and `capacity`, the run's Capacity Check, is the phase's only other
/// filesystem seam. [`CapacityCheck`] is the only caller of
/// `seams.volume_probe`: the phase never samples the probe itself, so every
/// check and recheck groups Mod Roots by the volume identities the run froze.
/// `choices` are the run's Archive Finalization choices.
///
/// The Packing Exclusions come from the Selected Profile
/// ([`SelectedProfile::packing_exclusions`]) in `preparation`. Each rule is
/// matched as a case-insensitive substring of the `/`-separated path within
/// the Mod Root.
///
/// `phase` must have just opened the executed Archive Finalization phase,
/// unplanned; the phase is finished through it. `cancelled` is polled
/// between outputs and between Mod Roots, and inside an output attempt only
/// while its Archive is written, before anything is published (audit BE-8),
/// so an attempt is still atomic; cancelled planning finishes the phase as cancelled without an
/// output total. A failure or panic of the phase's own work finishes it
/// once as a phase-level `UnexpectedException` that keeps every attempt
/// already recorded and forbids continuation. A Mod Root planning could not
/// plan is left out while the others are finalized, and the phase then fails
/// with `PlanningFailed`, which is safe to continue, unless it already
/// failed some other way. The caller owns `artifacts` through Safety
/// Cleanup.
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
    choices: &ArchiveFinalizationChoices,
    preparation: &RunPreparation,
    mut phase: ArchiveFinalizationHandle<'_, '_>,
    artifacts: &mut TemporaryArtifactRegistry,
    cancelled: &(dyn Fn() -> bool + Sync),
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
            choices,
            preparation,
            &mut phase,
            artifacts,
            cancelled,
            &mut account,
        )
    }));
    let detail = match outcome {
        Ok(Ok(())) => return phase.finish(account),
        Ok(Err(Escape::Evidence(error))) => return Err(error),
        Ok(Err(Escape::Work(detail))) => detail,
        Err(payload) => take_panic_message(payload),
    };
    // The escaped failure gives no reliable boundary for further durable
    // effects. Every attempt was already streamed into the evidence, so the
    // phase-level failure keeps them all.
    account.fail(OperationFailure::new(
        ArchiveFinalizationFailure::UnexpectedException,
        detail,
    ));
    account.safe_to_continue = false;
    phase.finish(account)
}

/// What a Dry Run's planned Archive extractions would leave in the Mod Roots
/// by the time an Apply run prunes: the source Archives Source Cleanup would
/// remove, and every folder that would hold a file the extractions publish.
///
/// Apply prunes the folders a removed source Archive vacated, but an
/// extraction publishes its winning entries beneath the Archive's own
/// folder, so that folder empties only when its Archive was alone there and
/// won nothing. A Dry Run therefore needs both halves to report the same
/// folders ([`planned_pruning`]).
#[derive(Debug, Default)]
pub(crate) struct PlannedExtractionEffects {
    /// The source Archives Apply would delete after extracting them.
    removed: Vec<PathBuf>,
    /// Each folder that would hold a published entry, at any depth, keyed as
    /// NTFS compares names: a merge publishes beneath an existing folder
    /// whatever the case its entry names it in.
    occupied: BTreeSet<OrdinalIgnoreCase<PathBuf>>,
}

impl PlannedExtractionEffects {
    /// The effects of extracting every one of `plans` with `cleanup`, as a
    /// Dry Run assumes each extraction succeeds. Only
    /// [`SourceCleanup::Remove`] deletes a source; a backup stays in the
    /// Archive's folder, so it vacates nothing.
    pub(crate) fn of(plans: &[ArchiveExtractionPlan], cleanup: SourceCleanup) -> Self {
        let mut effects = Self::default();
        for plan in plans {
            let Some(folder) = plan.archive_path.parent() else {
                continue;
            };
            if cleanup == SourceCleanup::Remove {
                effects.removed.push(plan.archive_path.clone());
            }
            for entry in &plan.merge_entries {
                // Entries are `/`-separated and relative to the Archive's
                // folder; only the folders above the file are occupied.
                let mut directory = folder.to_path_buf();
                let mut components = entry.split('/').peekable();
                while let Some(component) = components.next() {
                    if components.peek().is_none() {
                        break;
                    }
                    directory.push(component);
                    effects
                        .occupied
                        .insert(OrdinalIgnoreCase(directory.clone()));
                }
                // The Archive's folder and its parents up to the Mod Root
                // hold the published file too.
                for ancestor in folder.ancestors() {
                    if !ancestor.starts_with(&plan.mod_root) {
                        break;
                    }
                    if !effects
                        .occupied
                        .insert(OrdinalIgnoreCase(ancestor.to_path_buf()))
                    {
                        break;
                    }
                }
            }
        }
        effects
    }

    /// Whether a published entry would lie somewhere beneath `directory`.
    fn occupies(&self, directory: &Path) -> bool {
        self.occupied
            .contains(&OrdinalIgnoreCase(directory.to_path_buf()))
    }
}

/// What planning a Dry Run's Archive creation produced.
#[derive(Debug)]
pub(crate) enum CreationPlanning {
    /// The Routing Policy requests no Archive creation, so nothing was
    /// planned. An Apply run still prunes the folders its Archive
    /// extraction's source removal emptied, which `pruning` lists
    /// ([`planned_pruning`]), in the order they would go.
    NotRequested {
        /// The folders the Apply run would prune.
        pruning: Vec<PlannedPruning>,
    },
    /// The output Archives an Apply run over the Mod Roots as they are now
    /// would create, in plan order. Empty when nothing would be packed.
    Planned {
        /// The planned output Archives, in plan order.
        outputs: Vec<PlannedOutputArchive>,
        /// The Dry Run's warning, ready to record, of the shortage the Apply
        /// run's Capacity Check would fail its Archive creation on before
        /// any root mutates: the same check over the same
        /// [`creation_demand`], so `None` exactly when that Apply run would
        /// pass it.
        capacity_warning: Option<RunDiagnostic>,
        /// One warning per Mod Root planning could not plan, in run order:
        /// the roots an Apply run would leave out with
        /// [`ArchiveFinalizationFailure::PlanningFailed`].
        unplanned: Vec<RunDiagnostic>,
        /// The plan's own warnings, as [`plan_warnings`] lists them: the
        /// same ones an Apply run records over the same Mod Roots.
        plan_warnings: Vec<RunDiagnostic>,
        /// The folders an Apply run would prune once the planned packing
        /// and the planned Archive extractions deleted their sources
        /// ([`planned_pruning`]), in the order they would go.
        pruning: Vec<PlannedPruning>,
    },
    /// Cancellation was observed while planning; nothing was planned.
    Cancelled,
    /// Planning panicked, with this message: the failure an Apply run
    /// reports as the phase's Unexpected Exception. A Mod Root that merely
    /// could not be planned is one of [`Self::Planned`]'s `unplanned`.
    Failed(String),
}

/// Plans Archive Finalization's outputs for a Dry Run, from the same inputs
/// as [`finalize_archives`]: the preparation's Mod Roots and Packing
/// Exclusions, `choices`, and `seams.packer`.
///
/// It stops where [`finalize_archives`] would start mutating: it never
/// writes, commits or cleans an output, maintains no existing Loading
/// Plugin and prunes no directory, so it reaches only the packer's `rules`,
/// `list_names` and `partition`; it only lists the folders it reports
/// Apply would prune. `cancelled` is polled as planning polls it under
/// Apply.
///
/// It then runs the Capacity Check the Apply run would run before mutating,
/// through `capacity`, the run's Capacity Check, over the same
/// [`creation_demand`], and returns the warning it gives beside the outputs
/// rather than failing: the Dry Run itself needs no space. The check only
/// reads the volume probe, whose panics it contains.
///
/// Planning has no panic containment of its own, and a Dry Run has no
/// attempt-phase handle to finish as an Unexpected Exception, so a panic of
/// the packer or of planning is contained here and returned as
/// [`CreationPlanning::Failed`] with its message.
pub(crate) fn plan_archive_creation(
    seams: &ArchiveSeams,
    capacity: &CapacityCheck<'_>,
    choices: &ArchiveFinalizationChoices,
    preparation: &RunPreparation,
    extractions: &PlannedExtractionEffects,
    cancelled: &dyn Fn() -> bool,
) -> CreationPlanning {
    if !preparation
        .policy()
        .requests(RequestedWork::ArchiveCreation)
    {
        // Apply's phase still prunes over every Mod Root without packing.
        return CreationPlanning::NotRequested {
            pruning: planned_pruning(preparation.mod_roots(), None, extractions),
        };
    }
    let exclusions = checked_packing_exclusions(preparation);
    let planned = catch_unwind(AssertUnwindSafe(|| {
        let plan = plan_finalization(
            preparation.mod_roots(),
            choices,
            &*seams.packer,
            &exclusions,
            cancelled,
        )?;
        // The plan is held, not finalized, so the demand comes from the
        // frozen plan exactly as an Apply run's does before it mutates.
        let verdict = capacity.check(
            CapacityStage::BeforeCreation,
            &creation_demand(capacity, &plan).phase,
        );
        Ok((
            planned_outputs(&plan),
            verdict,
            unplanned_warnings(&plan),
            plan_warnings(&plan),
            planned_pruning(&plan.roots, Some(&plan), extractions),
        ))
    }));
    match planned {
        Ok(Ok((outputs, verdict, unplanned, plan_warnings, pruning))) => {
            // Matched outside the containment above, so an impossible arm
            // panics rather than passing as a planning failure.
            let capacity_warning = match verdict {
                CapacityVerdict::Fits => None,
                CapacityVerdict::Warn(warning) => Some(warning),
                // The Run Toolset's mode-aware Archive steps are expected to
                // remove these arms by sharing one entry point with
                // `finalize_archives`: card 3, "The Run Toolset's Archive
                // work knows its mode", of the 2026-10-30 architecture
                // review, deferred by #652.
                CapacityVerdict::FailRun(_) | CapacityVerdict::FailWork(_) => unreachable!(
                    "Archive creation is planned only in a Dry Run, whose Capacity Check warns \
                     before creation"
                ),
            };
            CreationPlanning::Planned {
                outputs,
                capacity_warning,
                unplanned,
                plan_warnings,
                pruning,
            }
        }
        Ok(Err(PlanningCancelled)) => CreationPlanning::Cancelled,
        Err(payload) => CreationPlanning::Failed(take_panic_message(payload)),
    }
}

/// One line per unplanned Mod Root, naming it and what failed, for the
/// phase-level [`ArchiveFinalizationFailure::PlanningFailed`].
fn unplanned_detail(unplanned: &[UnplannedRoot]) -> String {
    unplanned
        .iter()
        .map(|root| format!("{}: {}", root.root.display(), root.detail))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A Dry Run's warning for each Mod Root `plan` could not plan, as a
/// [`RunDiagnosticCode::ArchiveCreationUnplanned`] at that root.
fn unplanned_warnings(plan: &FinalizationPlan) -> Vec<RunDiagnostic> {
    plan.unplanned
        .iter()
        .map(|root| {
            RunDiagnostic::new(
                RunDiagnosticCode::ArchiveCreationUnplanned,
                RunPhase::ArchiveFinalization,
                root.detail.clone(),
            )
            .with_path(&root.root)
        })
        .collect()
}

/// The warnings `plan` records, Apply and Dry Run alike: one
/// [`RunDiagnosticCode::CaseTwinNames`] per Mod Root holding plugin or
/// Archive names that differ only in case, then one
/// [`RunDiagnosticCode::ArchiveCreationSkipped`] per output left out because
/// nothing would load it, each at its Mod Root, then one
/// [`RunDiagnosticCode::ArchiveCreationSkipped`] per loose file too large for
/// any Archive, at the file (audit BE-16), all in plan order.
fn plan_warnings(plan: &FinalizationPlan) -> Vec<RunDiagnostic> {
    let case_twins = plan.case_twins.iter().map(|root| {
        RunDiagnostic::new(
            RunDiagnosticCode::CaseTwinNames,
            RunPhase::ArchiveFinalization,
            format!(
                "These names differ only in case: {}. The game, under Windows, Wine or \
                 Proton, sees each group as one name and loads only one of its files. Keep \
                 one file of each group.",
                describe_case_twins(&root.groups)
            ),
        )
        .with_path(&root.mod_root)
    });
    let skipped = plan.unloadable.iter().map(|output| {
        RunDiagnostic::new(
            RunDiagnosticCode::ArchiveCreationSkipped,
            RunPhase::ArchiveFinalization,
            format!(
                "{} was not created: Create Dummy Plugins is off and no plugin loads it, so \
                     packing it and deleting its {} loose files could stop the mod loading \
                     them. The files stay loose. Add {} or turn Create Dummy Plugins on.",
                output.archive_path.display(),
                output.file_count,
                output.plugin_path.display()
            ),
        )
        .with_path(&output.mod_root)
    });
    let oversized = plan.oversized.iter().map(|asset| {
        RunDiagnostic::new(
            RunDiagnosticCode::ArchiveCreationSkipped,
            RunPhase::ArchiveFinalization,
            format!(
                "{} stays loose: at {} bytes it is past its game's {}-byte Archive Size Cap, \
                 so no Archive the game can load may hold it. The rest of its Mod Root is \
                 packed.",
                asset.path.display(),
                asset.size,
                asset.cap
            ),
        )
        .with_path(&asset.path)
    });
    case_twins.chain(skipped).chain(oversized).collect()
}

/// Archive creation's capacity demand over one frozen plan, as
/// [`creation_demand`] computes it for both Apply and Dry Run.
struct CreationDemand {
    /// Everything the phase may write: each Mod Root's Dummy Plugin
    /// allowance plus each planned output's estimate. A volume must fit all
    /// of it before any root mutates.
    phase: Demand,
    /// Only the Dummy Plugin allowances, to which each output attempt's
    /// recheck adds its own output.
    dummies: Demand,
}

/// The capacity demand of Archive creation over `plan`, computed without
/// finalizing it, so an Apply run's Capacity Check and a Dry Run's warning
/// test the same bytes at the same Mod Roots.
///
/// Roots enter the demand in plan order, each with its Dummy Plugin
/// allowance, and each output's `estimated_capacity_bytes` is added to its
/// own root. Planned source deletion is never counted as space.
fn creation_demand(capacity: &CapacityCheck<'_>, plan: &FinalizationPlan) -> CreationDemand {
    let mut phase = capacity.demand();
    let mut dummies = capacity.demand();
    for root in &plan.roots {
        let required = plan.dummy_capacity_by_root[root];
        phase.add(root, required);
        dummies.add(root, required);
    }
    for output in &plan.outputs {
        phase.add(&output.mod_root, output.estimated_capacity_bytes);
    }
    CreationDemand { phase, dummies }
}

/// The Planned Archive Work facts of `plan`'s outputs, in plan order: what
/// each would pack, as planning measured it, and the Dummy Plugin it would
/// need.
fn planned_outputs(plan: &FinalizationPlan) -> Vec<PlannedOutputArchive> {
    plan.outputs
        .iter()
        .map(|output| PlannedOutputArchive {
            archive_path: output.archive_path.clone(),
            mod_root: output.mod_root.clone(),
            kind: output.archive.kind,
            file_count: output.archive.files.len(),
            total_bytes: output
                .archive
                .files
                .iter()
                .fold(0u64, |total, file| total.saturating_add(file.size)),
            dummy_plugin_path: output.plugin_path.clone(),
            deletes_packed_sources: plan.delete_sources,
        })
        .collect()
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
    choices: &ArchiveFinalizationChoices,
    preparation: &RunPreparation,
    phase: &mut ArchiveFinalizationHandle<'_, '_>,
    artifacts: &mut TemporaryArtifactRegistry,
    cancelled: &(dyn Fn() -> bool + Sync),
    account: &mut ArchiveFinalizationAccount,
) -> Result<(), Escape> {
    if !preparation
        .policy()
        .requests(RequestedWork::ArchiveCreation)
    {
        // Without packing the phase plans no output, but Archive extraction
        // may still have emptied a folder by removing its source Archive.
        phase.plan(0).map_err(Escape::Evidence)?;
        prune_vacated_directories(preparation.mod_roots(), artifacts, cancelled, account);
    } else {
        let exclusions = checked_packing_exclusions(preparation);
        let plan = match plan_finalization(
            preparation.mod_roots(),
            choices,
            &*seams.packer,
            &exclusions,
            cancelled,
        ) {
            Ok(plan) => plan,
            Err(PlanningCancelled) => {
                // Planning made no mutation and has no trustworthy output
                // total to publish.
                account.cancelled = true;
                return Ok(());
            }
        };
        phase.plan(plan.outputs.len()).map_err(Escape::Evidence)?;
        // Planning already logged each output it left out and each root
        // with names that differ only in case; the evidence carries them as
        // warnings, as a Dry Run's does.
        for warning in plan_warnings(&plan) {
            phase.retain_diagnostic(warning);
        }
        finalize_plan(seams, capacity, &plan, artifacts, cancelled, phase, account)?;
        // Recorded after the other roots are finalized, and only when the
        // phase found no failure of its own: a shortage, plugin failure or
        // Unexpected Exception says more about the run, and every unplanned
        // root was already logged when planning skipped it.
        if account.failure.is_none() && !plan.unplanned.is_empty() {
            account.fail(OperationFailure::new(
                ArchiveFinalizationFailure::PlanningFailed,
                unplanned_detail(&plan.unplanned),
            ));
        }
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
    cancelled: &(dyn Fn() -> bool + Sync),
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
    let CreationDemand {
        phase: phase_demand,
        dummies: dummy_demand,
    } = creation_demand(capacity, plan);
    // A Mod Root with no planned writes cannot run out of staging space
    // during this phase, so only roots with demand are checked, in root
    // order.
    match capacity.check(CapacityStage::BeforeCreation, &phase_demand) {
        CapacityVerdict::Fits => {}
        CapacityVerdict::FailWork(shortage) => {
            let archive = plan
                .outputs
                .iter()
                .find(|output| output.mod_root == shortage.root)
                .map(|output| output.archive_path.as_path());
            return record_shortage(shortage, archive, phase, account);
        }
        verdict @ (CapacityVerdict::FailRun(_) | CapacityVerdict::Warn(_)) => unreachable!(
            "Archive Finalization runs only in an Apply run, whose shortage before creation \
             fails only the phase's work, never {verdict:?}"
        ),
    }

    for (index, _) in plan.outputs.iter().enumerate() {
        if cancelled() {
            account.cancelled = true;
            break;
        }
        let Some(attempt) = attempt_output(
            plan,
            index,
            artifacts,
            seams,
            capacity,
            &dummy_demand,
            &mut account.mutations,
            cancelled,
        ) else {
            // Cancelled inside the write, before anything was published.
            account.cancelled = true;
            break;
        };
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
    account.fail(OperationFailure::new(
        ArchiveFinalizationFailure::UnexpectedException,
        detail,
    ));
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
            if let Some(shortage) = capacity.recheck_at(&remaining, root) {
                return record_shortage(shortage, None, phase, account);
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
        .map_err(|error| Escape::Work(error.to_string()))?
        {
            return Ok(());
        }
    }
    // Recoverable attempts keep their source evidence; cancellation keeps
    // every path.
    if !account.cancelled {
        prune_vacated_directories(&plan.roots, artifacts, cancelled, account);
    }
    Ok(())
}

/// Records a known `shortage` at its Mod Root as the attempt of `archive`,
/// or as a phase-level failure when the root has no planned output. The
/// phase stops after it, before any further mutation.
fn record_shortage(
    shortage: CapacityShortage,
    archive: Option<&Path>,
    phase: &mut ArchiveFinalizationHandle<'_, '_>,
    account: &mut ArchiveFinalizationAccount,
) -> Result<(), Escape> {
    let root = shortage.root.clone();
    let failure = OperationFailure::from_capacity_shortage(
        ArchiveFinalizationFailure::InsufficientCapacity,
        shortage,
    );
    match archive {
        None => account.fail(failure),
        Some(archive) => {
            // A known shortage is found before any mutation, so the producer
            // still claims it safe; the phase stops after it regardless.
            let outcome = AttemptOutcome::failed(failure, MutationState::None, true);
            let attempt = ArchiveFinalizationAttempt::new(archive.to_path_buf(), root, outcome);
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

/// [`packing_exclusions`], logging an Application Log error when the
/// Selected Profile has none. Called only where packing is planned, under
/// Apply or Dry Run.
///
/// An empty list here means `FilesToNotPack.txt` was missing or listed no
/// rules: one that exists but cannot be read already refused the run at
/// Preparing, so the message never blames an unreadable file.
fn checked_packing_exclusions(preparation: &RunPreparation) -> Vec<String> {
    let exclusions = packing_exclusions(preparation);
    // The list only filters packing, so its absence matters only when
    // packing runs or is planned.
    if exclusions.is_empty() {
        log::error!(
            "FilesToNotPack.txt is missing or lists no rules, so no Packing Exclusion \
             applies. This can cause a number of issues. For example, for Skyrim, \
             animations will be packed to BSA, preventing them from being detected by FNIS \
             and Nemesis."
        );
    }
    exclusions
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

/// Prunes, beneath each of `roots`, the folders this run emptied, recording
/// one mutation per root that lost any. Cancellation stops before the next
/// root.
///
/// The folders are those `artifacts` recorded as vacated: each one a file
/// was deleted from by packing or by Archive extraction's source removal.
/// One is removed only if it is now empty, and then each parent its removal
/// left empty, up to but never including the Mod Root ([`prune_chains`]).
/// A folder that held nothing before the run, which a mod may ship on
/// purpose, is never touched (audit FS-7).
fn prune_vacated_directories(
    roots: &[PathBuf],
    artifacts: &TemporaryArtifactRegistry,
    cancelled: &dyn Fn() -> bool,
    account: &mut ArchiveFinalizationAccount,
) {
    for root in roots {
        if cancelled() {
            account.cancelled = true;
            break;
        }
        let pruned = prune_chains(
            root,
            artifacts.vacated_directories(root),
            remove_emptied_directory,
        )
        .len();
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

/// Walks the pruning chain of each of `vacated` beneath `root`, deepest
/// first: the folder itself, then each parent while `remove` removed the
/// folder below it. A chain stops at the Mod Root, which is never removed, at
/// reserved staging, which belongs to Safety Cleanup, and at the first folder
/// `remove` keeps. Returns the folders removed, in the order they went.
///
/// Apply passes [`remove_emptied_directory`]; a Dry Run passes a check
/// against what its planned packing and Archive extractions would delete and
/// publish ([`planned_pruning`]), so both walk the same chains. Deepest first, a folder vacated inside another
/// is tried before it. A chain that reaches a folder an earlier chain already
/// removed stops there: that chain already tried every parent above it after
/// the removal.
fn prune_chains(
    root: &Path,
    vacated: impl IntoIterator<Item = PathBuf>,
    mut remove: impl FnMut(&Path) -> bool,
) -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = vacated
        .into_iter()
        .filter(|directory| directory != root && directory.starts_with(root))
        .collect();
    candidates.sort_by_key(|directory| std::cmp::Reverse(directory.components().count()));
    candidates.dedup();
    let mut removed = Vec::new();
    for candidate in &candidates {
        let mut directory = candidate.as_path();
        while directory != root
            && let Ok(relative) = directory.strip_prefix(root)
            && !has_staging_component(relative)
            && remove(directory)
        {
            removed.push(directory.to_path_buf());
            let Some(parent) = directory.parent() else {
                break;
            };
            directory = parent;
        }
    }
    removed
}

/// Removes `directory` if it is still an empty ordinary folder, returning
/// whether it went.
///
/// A link or other reparse point now standing at the path is not the folder
/// the run emptied, so it stays. Removal is nonrecursive, so a folder that is
/// not empty simply stays; an empty one that cannot be removed stays too,
/// with an Application Log warning naming it.
fn remove_emptied_directory(directory: &Path) -> bool {
    match std::fs::symlink_metadata(directory) {
        Ok(metadata)
            if metadata.is_dir() && !metadata.is_symlink() && !is_reparse_point(&metadata) => {}
        // Absent, replaced or unreadable: nothing this run emptied to remove.
        _ => return false,
    }
    match std::fs::remove_dir(directory) {
        Ok(()) => true,
        // Most chains end at a folder that still holds something; trying it
        // is the walk.
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::DirectoryNotEmpty | std::io::ErrorKind::NotFound
            ) =>
        {
            false
        }
        // An empty folder that stays, held open or denied, is the user's to
        // know about: it used to vanish behind `is_ok` (FS-2).
        Err(error) => {
            log::warn!(
                "Could not remove empty directory {}: {error}",
                directory.display()
            );
            false
        }
    }
}

/// The folders an Apply run would prune beneath `roots` once it deleted
/// the sources its packing (`packed`, when Archive creation is planned) and
/// its Archive extractions (`extractions`) remove, for a Dry Run to report,
/// in the order they would go.
///
/// Every folder a deleted source lies in is walked as Apply walks the
/// folders it recorded as vacated ([`prune_chains`]), but nothing is
/// removed: a folder counts as emptied when no extraction would publish
/// beneath it and every entry in it is a deleted source or a folder already
/// counted. `roots` are the roots Apply would prune: the planned roots when
/// it packs, and every Mod Root when it does not. Packing without source
/// deletion and extraction with a backup vacate nothing.
///
/// The packing plan is an estimate from the Mod Roots as they are now,
/// before extraction: a file an extraction publishes is never among the
/// packed sources here, so a folder an Apply run would empty by packing such
/// a file and deleting it is not reported. That limit is the planned
/// outputs' ([`OutputPlanBasis::EstimateFromModRootsAsTheyAre`](crate::run::OutputPlanBasis)).
fn planned_pruning(
    roots: &[PathBuf],
    packed: Option<&FinalizationPlan>,
    extractions: &PlannedExtractionEffects,
) -> Vec<PlannedPruning> {
    let mut gone: HashSet<PathBuf> = extractions.removed.iter().cloned().collect();
    if let Some(plan) = packed.filter(|plan| plan.delete_sources) {
        gone.extend(
            plan.outputs
                .iter()
                .flat_map(|output| output.archive.files.iter().map(|file| file.path.clone())),
        );
    }
    let mut pruning = Vec::new();
    for root in roots {
        let vacated: BTreeSet<PathBuf> = gone
            .iter()
            .filter(|source| source.starts_with(root))
            .filter_map(|source| source.parent().map(Path::to_path_buf))
            .collect();
        let removed = prune_chains(root, vacated, |directory| {
            // A published entry keeps every folder above it, however empty
            // the folder is now.
            if extractions.occupies(directory) {
                return false;
            }
            let Ok(entries) = std::fs::read_dir(directory) else {
                return false;
            };
            let emptied = entries
                .into_iter()
                .all(|entry| entry.is_ok_and(|entry| gone.contains(&entry.path())));
            if emptied {
                gone.insert(directory.to_path_buf());
            }
            emptied
        });
        pruning.extend(removed.into_iter().map(|directory| PlannedPruning {
            directory,
            mod_root: root.clone(),
        }));
    }
    pruning
}

/// [`PlannedExtractionEffects`]: what a Dry Run assumes its planned
/// extractions leave behind, without touching the filesystem.
#[cfg(test)]
mod extraction_effects_tests {
    use std::path::{Path, PathBuf};

    use super::PlannedExtractionEffects;
    use crate::run::{ArchiveExtractionPlan, SourceCleanup};

    /// A plan for `archive` under `mod_root` that wins `winners`.
    fn plan(mod_root: &str, archive: &str, winners: &[&str]) -> ArchiveExtractionPlan {
        ArchiveExtractionPlan {
            archive_path: PathBuf::from(archive),
            mod_root: PathBuf::from(mod_root),
            entries: winners.iter().map(|winner| (*winner).to_owned()).collect(),
            merge_entries: winners.iter().map(|winner| (*winner).to_owned()).collect(),
            estimated_capacity_bytes: 0,
        }
    }

    /// Removal deletes the source and Backup keeps it; a winner occupies its
    /// own folders, the Archive's folder and every folder up to the Mod
    /// Root, whatever case it names them in, and nothing above or beside.
    #[test]
    fn winners_occupy_their_folders_up_to_the_mod_root_ignoring_case() {
        let plans = [
            plan(r"C:\mods\m", r"C:\mods\m\kept\a.bsa", &["Sub/Deeper/x.dds"]),
            plan(r"C:\mods\m", r"C:\mods\m\lonely\b.bsa", &[]),
        ];

        let removed = PlannedExtractionEffects::of(&plans, SourceCleanup::Remove);
        assert_eq!(
            removed.removed,
            [
                PathBuf::from(r"C:\mods\m\kept\a.bsa"),
                PathBuf::from(r"C:\mods\m\lonely\b.bsa"),
            ]
        );
        for occupied in [
            r"C:\mods\m",
            r"C:\mods\m\kept",
            r"C:\mods\m\KEPT\sub",
            r"c:\mods\m\kept\SUB\deeper",
        ] {
            assert!(removed.occupies(Path::new(occupied)), "{occupied}");
        }
        for free in [
            r"C:\mods",
            r"C:\mods\m\lonely",
            r"C:\mods\m\kept\sub\deeper\x.dds",
        ] {
            assert!(!removed.occupies(Path::new(free)), "{free}");
        }

        let backed_up = PlannedExtractionEffects::of(&plans, SourceCleanup::Backup);
        assert!(backed_up.removed.is_empty());
        assert!(backed_up.occupies(Path::new(r"C:\mods\m\kept\sub")));
    }
}
