//! The Run Executor's work phases: Archive-first discovery, definitive routing,
//! Asset attempts and Archive Finalization.
//!
//! The executor calls [`run_work_phases`] with its Run Toolset; the
//! function walks each Mod Root for Archives, extracts the enabled ones in
//! Apply through [`RunToolset::extract_archive`] or, in a Dry Run, records
//! each extraction as Planned Archive Work, routes the Effective Asset
//! Tree once, offers every Routed Asset to the Asset attempt port in Texture,
//! Mesh, Animation order, recording each completed attempt before reporting
//! it, and in Apply runs Archive Finalization through
//! [`RunToolset::finalize_archives`], or in Dry Run plans it through
//! [`RunToolset::plan_archive_creation`]. The Toolset runs each over its
//! seams, so this module never reaches into them.
//!
//! Loose Assets take precedence over Archived Assets, and Archive Precedence
//! decides between Archives in one Mod Root. Before the first extraction, a
//! preflight reads every selected manifest, rejects any Unsafe Game Path,
//! records the Archive Collisions in Run Evidence and runs the Capacity
//! Check; any failure stops the run with nothing extracted. A Dry Run runs
//! the same preflight, so it fails where the Apply run would, except that a
//! Capacity Check shortage, which the Dry Run itself cannot hit, is a
//! warning-level Run Diagnostic and the Dry Run goes on to plan.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs::Metadata;
use std::io;
use std::ops::ControlFlow;
use std::os::windows::fs::FileTypeExt as _;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::Stop;
use crate::Error;
use crate::execution::{AssetExecutionFailure, AssetExecutionResult, MutationState};
use crate::routing::{
    AssetRouter, ExecutionMode, OptimizerTarget, RoutingDecision, RoutingLedger, SkipReason,
};
use crate::run::archives::{Preflight, RootArchives, archive_order_key, relative_game_path};
use crate::run::finalization::{CreationPlanning, PlannedExtractionEffects};
use crate::run::{
    ArchiveExtractionPlan, ArchiveFinalizationChoices, AssetInitializationCancelled, ByteSize,
    CancellationToken, MutableRunEvidence, OutputPlanBasis, PlannedArchiveExtraction,
    PlannedOutputArchive, PlannedPruning, RoutedAssetAttempt, RunDiagnostic, RunDiagnosticCode,
    RunFailure, RunFailureCode, RunPhase, RunPhaseRecord, RunPreparation, RunToolset,
    SourceCleanup, TemporaryArtifactRegistry, WorkChoices, is_staging_name, take_panic_message,
};

/// Walks Mod Roots for regular files under the scope rules both discovery passes share.
///
/// It holds no evidence of its own: each walk is lent the run's evidence for
/// its exclusion and unreadable-directory diagnostics, since other work
/// phases record between the two passes.
struct Discovery<'r> {
    /// The run's token, polled per entry. The discovery phases read it from
    /// here rather than taking it again beside their walker.
    stop: &'r CancellationToken,
    /// The phase discovery diagnostics are attributed to.
    phase: RunPhase,
    /// Each excluded linked entry and each unreadable directory is diagnosed
    /// once, across both passes.
    diagnosed: HashSet<PathBuf>,
}

/// What discovery does with an entry in scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Visit {
    /// Hands the file to the pass's visitor.
    File,
    /// Walks the directory.
    Directory,
}

/// What discovery makes of one directory entry, judged from its attributes
/// read without following links.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryScope {
    /// A file to visit as it stands.
    File,
    /// A directory to walk.
    Directory,
    /// A link to a directory, which discovery never follows.
    DirectoryLink,
    /// A reparse-point file, a file link above all, visited only once it
    /// resolves inside the Mod Root and outside CAO staging.
    LinkedFile,
}

impl EntryScope {
    /// Classifies an entry from its attributes: whether it is a reparse
    /// point, whether it carries the directory attribute, and whether its
    /// reparse tag is a name surrogate, the tag bit of symlinks, junctions
    /// and mounted folders, which stand for another named object.
    ///
    /// Only a name-surrogate directory is a directory link. A directory with
    /// any other reparse tag, such as a cloud-sync placeholder, is the
    /// directory itself and is walked like any other; excluding it as a link
    /// would quietly leave its whole subtree out of the run. Every
    /// reparse-point file is still resolved before it is visited.
    fn classify(reparse_point: bool, directory: bool, name_surrogate: bool) -> Self {
        match (reparse_point, directory) {
            (false, false) => Self::File,
            (false, true) => Self::Directory,
            (true, true) if name_surrogate => Self::DirectoryLink,
            (true, true) => Self::Directory,
            (true, false) => Self::LinkedFile,
        }
    }

    /// Classifies the entry `metadata` describes, read without following
    /// links, as a directory entry's own metadata is.
    fn of(metadata: &Metadata) -> Self {
        let file_type = metadata.file_type();
        // std reports a name-surrogate reparse point as a symlink and never
        // as a directory, so the directory attribute is read through
        // `is_symlink_dir` for those and `is_dir` for every other entry.
        Self::classify(
            cao_winfs::is_reparse_point(metadata),
            file_type.is_dir() || file_type.is_symlink_dir(),
            file_type.is_symlink(),
        )
    }
}

impl Discovery<'_> {
    /// Retains one discovery diagnostic about `path` in `evidence` the first
    /// time it is observed.
    fn diagnose_once(
        &mut self,
        evidence: &mut MutableRunEvidence<'_>,
        code: RunDiagnosticCode,
        path: &Path,
        detail: impl Into<String>,
    ) {
        if self.diagnosed.insert(path.to_path_buf()) {
            evidence
                .retain_diagnostic(RunDiagnostic::new(code, self.phase, detail).with_path(path));
        }
    }

    /// Retains one linked-entry exclusion in `evidence` the first time it is observed.
    fn exclude(&mut self, evidence: &mut MutableRunEvidence<'_>, path: &Path, detail: &str) {
        self.diagnose_once(
            evidence,
            RunDiagnosticCode::LinkedEntryExcluded,
            path,
            detail,
        );
    }

    /// Notes that the directory at `path` could not be read with `error`.
    ///
    /// A directory that vanished while work ran is simply absent, as the
    /// tree can change during a run. Any other error, such as access being
    /// denied, leaves a subtree out of the run, so it is retained as a
    /// warning-level [`RunDiagnosticCode::DirectoryUnreadable`], once per
    /// directory, rather than letting the run succeed with fewer Assets and
    /// no word of why.
    fn unreadable(
        &mut self,
        evidence: &mut MutableRunEvidence<'_>,
        path: &Path,
        error: &io::Error,
    ) {
        if error.kind() == io::ErrorKind::NotFound {
            return;
        }
        self.diagnose_once(
            evidence,
            RunDiagnosticCode::DirectoryUnreadable,
            path,
            format!(
                "The directory could not be read, so nothing beneath it was discovered: {error}"
            ),
        );
    }

    /// Decides whether the entry at `path`, which `metadata` describes
    /// without following links, is visited, walked or excluded.
    ///
    /// Rejects directory links, and file links not provably inside the Mod
    /// Root or that lead into CAO staging, retaining each exclusion once.
    /// Returns `None` for an excluded entry.
    fn within_scope(
        &mut self,
        evidence: &mut MutableRunEvidence<'_>,
        path: &Path,
        metadata: &Metadata,
        canonical_root: &Path,
    ) -> Option<Visit> {
        let scope = EntryScope::of(metadata);
        // A file link can name a directory too; following it would walk
        // into a tree the Mod Root does not hold, so it counts as a
        // directory link. Only reparse-point files pay for this second query.
        let scope = if scope == EntryScope::LinkedFile
            && std::fs::metadata(path).is_ok_and(|followed| followed.is_dir())
        {
            EntryScope::DirectoryLink
        } else {
            scope
        };
        match scope {
            EntryScope::File => return Some(Visit::File),
            EntryScope::Directory => return Some(Visit::Directory),
            EntryScope::DirectoryLink => {
                self.exclude(
                    evidence,
                    path,
                    "Directory links are not followed during discovery.",
                );
                return None;
            }
            EntryScope::LinkedFile => {}
        }
        let Ok(resolved) = cao_winfs::canonical(path) else {
            self.exclude(
                evidence,
                path,
                "Linked entry could not be resolved within the Mod Root.",
            );
            return None;
        };
        let Ok(relative) = resolved.strip_prefix(canonical_root) else {
            self.exclude(
                evidence,
                path,
                "Linked entry resolves outside the Mod Root.",
            );
            return None;
        };
        // A contained alias can still point into staging; its own name would hide that.
        if relative
            .components()
            .any(|component| is_staging_name(component.as_os_str()))
        {
            self.exclude(
                evidence,
                path,
                "Linked entry resolves into excluded CAO staging.",
            );
            return None;
        }
        Some(Visit::File)
    }

    /// Visits each regular file beneath `root` depth-first, in enumeration
    /// order, retaining linked-entry exclusions and unreadable directories in
    /// `evidence`. Returns `false` as soon as cancellation is observed.
    fn visit(
        &mut self,
        evidence: &mut MutableRunEvidence<'_>,
        root: &Path,
        visitor: &mut dyn FnMut(&Path),
    ) -> bool {
        if root.file_name().is_some_and(is_staging_name) {
            return true;
        }
        self.visit_directory(evidence, root, root, visitor)
    }

    /// Visits each regular file beneath `directory`, a directory within
    /// `root`, depth-first, as [`Self::visit`] does. Returns `false` as soon
    /// as cancellation is observed.
    ///
    /// Each entry's attributes come from the directory listing itself, so
    /// deciding its scope costs no query of its own, except for a
    /// reparse-point file, which is resolved.
    fn visit_directory(
        &mut self,
        evidence: &mut MutableRunEvidence<'_>,
        directory: &Path,
        root: &Path,
        visitor: &mut dyn FnMut(&Path),
    ) -> bool {
        // A tree can change while work runs, so an unreadable directory is
        // absent from this pass rather than failing the run; `unreadable`
        // decides whether that deserves a diagnostic.
        let entries = match std::fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) => {
                self.unreadable(evidence, directory, &error);
                return true;
            }
        };
        for entry in entries {
            // Poll for every entry: directories and unsupported files may be the
            // whole tree, so no other seam is guaranteed to observe cancellation.
            if self.stop.is_cancelled() {
                return false;
            }
            // An enumeration that fails part way leaves the rest of this
            // directory unread, as a failed open leaves all of it.
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    self.unreadable(evidence, directory, &error);
                    return true;
                }
            };
            if is_staging_name(&entry.file_name()) {
                continue;
            }
            // On Windows the listing already carries the entry's attributes,
            // so this asks nothing more of the filesystem; an entry that
            // still cannot be described has vanished.
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            let path = entry.path();
            match self.within_scope(evidence, &path, &metadata, root) {
                Some(Visit::File) => visitor(&path),
                // A walk that observed cancellation stops this one too.
                Some(Visit::Directory) if !self.visit_directory(evidence, &path, root, visitor) => {
                    return false;
                }
                // A directory walked to its end, or an excluded entry, which
                // `within_scope` has diagnosed.
                Some(Visit::Directory) | None => {}
            }
        }
        true
    }
}

/// The Mod Root an Asset is attributed to: the longest prepared root
/// containing it once links are resolved.
///
/// Resolution happens as the attempt starts, before it can remove a converted
/// source, so a folder swapped for a link after discovery is attributed to
/// the root it really leads into, or to none (C++ `weakly_canonical`). A path
/// that cannot be resolved is attributed by its own spelling.
fn attributed_mod_root<'p>(path: &Path, mod_roots: &'p [PathBuf]) -> Option<&'p PathBuf> {
    let resolved = cao_winfs::weakly_canonical(path).unwrap_or_else(|_| path.to_path_buf());
    mod_roots
        .iter()
        .filter(|root| resolved.starts_with(root))
        .max_by_key(|root| root.as_os_str().len())
}

/// What a work Run Phase hands the run: whether the next phase runs, and what
/// it needs from this one.
///
/// [`ControlFlow::Continue`] carries the phase's handoff to the next phase,
/// which is why `std`'s type is used rather than a bespoke continue-or-stop
/// enum. [`ControlFlow::Break`] stops the work with nothing left to record,
/// carrying the [`Stop`] that says why: every stop simply returns, since the
/// executor records cancellation observed from `stop`, or from a
/// [`Stop::BackendCancelled`], and publishes the retained Run Diagnostics
/// once the work has returned. A stop is an ordinary end of the work, so it
/// is kept apart from the outer `Err`, which only ever carries a Run
/// Evidence invariant violation for the executor to raise after Safety
/// Cleanup.
type PhaseFlow<T = ()> = Result<ControlFlow<Stop, T>, Error>;

/// What Archive discovery hands the later work phases.
struct DiscoveredArchives {
    /// Every Archive the Archive pass recognized, enabled or not. The
    /// definitive pass counts any other Archive as nested.
    recognized: HashSet<PathBuf>,
    /// The Archives Apply extracts, in order; a Dry Run records the same
    /// plans as Planned Archive Work instead.
    plans: Vec<ArchiveExtractionPlan>,
}

/// Runs Archive-first discovery, definitive routing, Asset attempts and
/// Archive Finalization through `toolset`'s Asset attempt port and over its
/// Archive seams. Archive extraction and Archive Finalization follow
/// `choices`, the Run Request's Work Choices.
///
/// Completed facts enter `evidence`, which the executor lends for the whole
/// call, before the run's Run Observation Sink sees them. The work phases
/// enter, skip and plan their own Run Phases by recording them in
/// `evidence`, which enforces the canonical Run Phase order. Cancellation,
/// from `stop`, is observed between entries and attempts and once more after
/// the last attempt; an attempt in flight always finishes. An unsafe attempt
/// stops all further work.
///
/// Every exit simply returns: the executor records cancellation observed
/// from `stop` at Safety Cleanup, and publishes the Run Diagnostics retained
/// here once the work has returned. An Asset backend's own report of
/// cancellation, which `stop` need not show, is returned as
/// [`Stop::BackendCancelled`] for the executor to record at that same
/// sample, so the work phases record no cancellation at all. The Archive
/// discovery facts are recorded where they are established, the skipped
/// Archive counts at the end of the Archive pass and the nested Archive
/// count after a complete definitive pass, so no exit carries them either.
///
/// An Asset attempt's panic is contained here, as an unsafe Operation
/// Failure with unknown mutation. Archive extraction and Archive
/// Finalization contain their own panics, so they are called without a
/// second layer, as is Archive Finalization's planning-only step. Archive
/// Finalization runs only in Apply, once its executed phase is recorded; a
/// Dry Run records it as Planned and only plans its outputs. Errors are Run
/// Evidence invariant violations, which the executor raises after Safety
/// Cleanup.
///
/// Each work Run Phase runs in its own function returning a [`PhaseFlow`];
/// this function only chains them, stopping at the first `Break` and
/// returning its [`Stop`].
pub(super) fn run_work_phases(
    preparation: &RunPreparation,
    choices: &WorkChoices,
    toolset: &mut RunToolset,
    evidence: &mut MutableRunEvidence<'_>,
    artifacts: &mut TemporaryArtifactRegistry,
    stop: &CancellationToken,
) -> Result<ControlFlow<Stop>, Error> {
    let policy = preparation.policy();
    let mode = policy.execution_mode();
    // The provider builds the Toolset, and only it knows which mode it gave
    // it; every Capacity Check of the run decides from that mode, so a
    // provider wired with the wrong one would apply the other mode's rule.
    debug_assert_eq!(
        toolset.mode(),
        mode,
        "the Run Toolset's Execution Mode must be the Run Request's"
    );
    let router = AssetRouter::new(policy.clone());
    // Both discovery passes share one walker, so each excluded linked entry
    // is diagnosed once across them.
    let mut discovery = Discovery {
        stop,
        phase: RunPhase::DiscoveringArchives,
        diagnosed: HashSet::new(),
    };

    // `?` cannot unwrap a `ControlFlow` on stable Rust, so each stop is
    // returned by hand, keeping its reason for the executor.
    let archives = match discover_archives(preparation, toolset, evidence, &router, &mut discovery)?
    {
        ControlFlow::Continue(archives) => archives,
        ControlFlow::Break(stopped) => return Ok(ControlFlow::Break(stopped)),
    };
    if let stopped @ ControlFlow::Break(_) = extract_archives(
        mode,
        &archives.plans,
        choices.archive_extraction,
        toolset,
        evidence,
        artifacts,
        stop,
    )? {
        return Ok(stopped);
    }
    let ledger = match build_effective_asset_tree(
        preparation,
        evidence,
        &router,
        &mut discovery,
        &archives.recognized,
    )? {
        ControlFlow::Continue(ledger) => ledger,
        ControlFlow::Break(stopped) => return Ok(ControlFlow::Break(stopped)),
    };
    if let stopped @ ControlFlow::Break(_) =
        process_assets(preparation, toolset, evidence, ledger, artifacts, stop)?
    {
        return Ok(stopped);
    }
    // Archive Finalization is the last work phase, so whether it stopped
    // short changes nothing that follows; its stop is returned as it is.
    finalize_archives(
        preparation,
        mode,
        &choices.archive_finalization,
        (&archives.plans, choices.archive_extraction),
        toolset,
        evidence,
        artifacts,
        stop,
    )
}

/// Runs the Discovering Archives phase: the Archive pass over every Mod Root
/// and the preflight that plans extraction, in Apply and Dry Run alike.
///
/// The skipped Archive counts are recorded once the pass ends, complete or
/// not. A cancelled pass, a failed preflight, recorded as the run's failure,
/// or a cancelled preflight stops the work with nothing extracted; an Apply
/// run's Capacity Check shortage is such a failed preflight. A Dry Run's
/// capacity warning is recorded after the collisions, and the run continues.
fn discover_archives(
    preparation: &RunPreparation,
    toolset: &RunToolset,
    evidence: &mut MutableRunEvidence<'_>,
    router: &AssetRouter,
    discovery: &mut Discovery<'_>,
) -> PhaseFlow<DiscoveredArchives> {
    evidence.record_phase(RunPhaseRecord::executed(
        RunPhase::DiscoveringArchives,
        None,
    ))?;

    let mut skipped_archive_counts: BTreeMap<SkipReason, usize> = BTreeMap::new();
    let mut recognized_archives: HashSet<PathBuf> = HashSet::new();
    let mut root_archives: Vec<RootArchives> = Vec::new();

    // The Archive pass: recognize every Archive, and every Loose Asset an
    // Archived Asset must not replace, before any extraction.
    let mut archive_pass_complete = true;
    for root in preparation.mod_roots() {
        let mut selected: Vec<PathBuf> = Vec::new();
        let mut loose = BTreeSet::new();
        archive_pass_complete = discovery.visit(evidence, root, &mut |path| {
            // The extension alone tells Archives apart, so only they are routed here.
            if !router.names_archive(path) {
                loose.insert(cao_winfs::OrdinalIgnoreCase(relative_game_path(root, path)));
                return;
            }
            if !recognized_archives.insert(path.to_path_buf()) {
                return;
            }
            match router.route(path) {
                RoutingDecision::Routed(_) => selected.push(path.to_path_buf()),
                RoutingDecision::Skipped(asset) => {
                    *skipped_archive_counts.entry(asset.reason()).or_default() += 1
                }
                // `names_archive` already confirmed routing recognizes it.
                RoutingDecision::Unsupported => {}
            }
        });
        if !archive_pass_complete {
            break;
        }
        // Root order belongs to the caller; only this root's batch is sorted,
        // by its relative names rather than filesystem enumeration order.
        selected.sort_by_cached_key(|path| archive_order_key(&relative_game_path(root, path)));
        root_archives.push(RootArchives {
            root: root.clone(),
            archives: selected,
            loose,
        });
    }
    // The Archive pass establishes the skipped counts, so they are recorded
    // here once and no later stop has to carry them. A cancelled pass keeps
    // the exclusions it counted before cancellation.
    evidence.record_skipped_archive_counts(skipped_archive_counts)?;
    if !archive_pass_complete {
        return Ok(ControlFlow::Break(Stop::Halted));
    }

    // Both modes run the same preflight: a Dry Run reads the manifests and
    // fails Archive Precedence or an Unsafe Game Path as the Apply run would,
    // so a Dry Run that succeeds means the Apply run gets past those checks.
    // The preflight only reads, so a Dry Run still mutates nothing.
    let plans = match toolset.preflight_archives(
        root_archives,
        preparation.archive_precedence(),
        discovery.stop,
    ) {
        Preflight::Failed(failure) => {
            // A failed preflight has no trustworthy tree, so the run stops
            // before any extraction, routing or finalization.
            evidence.record_failure(failure);
            return Ok(ControlFlow::Break(Stop::Halted));
        }
        Preflight::Cancelled => return Ok(ControlFlow::Break(Stop::Halted)),
        Preflight::Planned {
            plans,
            collisions,
            capacity_warning,
        } => {
            // Retained before the extraction phase is observed, so a
            // failing observer or a later unwind cannot lose the plan.
            evidence.record_archive_collisions(&collisions)?;
            if let Some(warning) = capacity_warning {
                record_capacity_warning(evidence, warning);
            }
            plans
        }
    };
    Ok(ControlFlow::Continue(DiscoveredArchives {
        recognized: recognized_archives,
        plans,
    }))
}

/// Runs the Extracting Archives phase. Apply opens it with its plan and
/// extracts each planned Archive in order through
/// [`RunToolset::extract_archive`], over `toolset`'s seams and the run's
/// Capacity Check with `cleanup`, the Run Request's Archive extraction
/// choice, retaining every completed attempt
/// before it is reported. A Dry Run records the phase as Planned instead,
/// opening no handle, through [`plan_archive_extractions`].
///
/// Cancellation is observed before and after the phase is recorded, between
/// attempts and after the last one. An attempt in flight always finishes, so
/// cancellation never leaves a partial Archive. Extraction contains its own
/// panics, classified by how far the attempt got, so it is called without a
/// second layer. Extraction stops after any attempt whose Attempt Outcome is
/// not safe to continue, so `PartialOrUnknown` mutation stops it even when
/// the attempt claims otherwise. Returns `Continue` when every plan was
/// attempted and the run may continue.
///
/// Attempts are recorded through the open phase's handle, which this call
/// owns, so the phase closes when it returns.
fn extract_archives(
    mode: ExecutionMode,
    plans: &[ArchiveExtractionPlan],
    cleanup: SourceCleanup,
    toolset: &RunToolset,
    evidence: &mut MutableRunEvidence<'_>,
    artifacts: &mut TemporaryArtifactRegistry,
    stop: &CancellationToken,
) -> PhaseFlow {
    if stop.is_cancelled() {
        return Ok(ControlFlow::Break(Stop::Halted));
    }
    // A Dry Run plans Archive extraction, so only Apply opens its handle.
    if mode == ExecutionMode::DryRun {
        return plan_archive_extractions(plans, cleanup, evidence, stop);
    }
    let mut extraction = evidence.open_archive_extraction(plans.len())?;
    if stop.is_cancelled() {
        return Ok(ControlFlow::Break(Stop::Halted));
    }

    // Cancellation stops before the definitive pass, and an unsafe attempt
    // never reaches routing or finalization.
    if plans.is_empty() {
        // No attempt ran, so there is none to re-sample cancellation after:
        // the checks around this phase's record observe it.
        return Ok(ControlFlow::Continue(()));
    }
    for plan in plans {
        // Cancellation observed between attempts stops extraction.
        if stop.is_cancelled() {
            return Ok(ControlFlow::Break(Stop::Halted));
        }
        let mut attempt = toolset.extract_archive(plan, cleanup, artifacts);
        // The attempt's own continuation claim is retained in its Attempt
        // Outcome for diagnosis, but the stop follows the shared verdict, so
        // partial mutation stops the run regardless.
        let unsafe_attempt = !attempt.safe_to_continue();
        attempt.mod_root = plan.mod_root.clone();
        extraction.record(attempt)?;
        if unsafe_attempt {
            // The attempt left unknown mutation or claimed it was unsafe to
            // continue.
            return Ok(ControlFlow::Break(Stop::Halted));
        }
    }
    // Re-sampled after the final attempt and its progress record, so
    // cancellation during the last Archive still skips the definitive pass.
    Ok(if stop.is_cancelled() {
        ControlFlow::Break(Stop::Halted)
    } else {
        ControlFlow::Continue(())
    })
}

/// Runs a Dry Run's Extracting Archives phase: records it as Planned, then
/// retains one Planned Archive Work fact for each of `plans`, in order, and
/// logs what the Apply run would extract. It attempts nothing and never
/// reaches the extraction seams, so no Archive, staging or Mod Root changes.
///
/// `cleanup` decides whether each Archive would be deleted afterwards. With
/// no plans, as when the run requests other work but no Archive extraction,
/// the phase is still Planned and holds no fact, as an Apply run executes it
/// with no attempt.
///
/// Cancellation observed once the Planned record is published stops before
/// any fact is retained, as an Apply run stops before its first attempt;
/// otherwise it is re-sampled after the last fact.
fn plan_archive_extractions(
    plans: &[ArchiveExtractionPlan],
    cleanup: SourceCleanup,
    evidence: &mut MutableRunEvidence<'_>,
    stop: &CancellationToken,
) -> PhaseFlow {
    evidence.record_phase(RunPhaseRecord::planned(RunPhase::ExtractingArchives))?;
    if stop.is_cancelled() {
        return Ok(ControlFlow::Break(Stop::Halted));
    }
    for plan in plans {
        // The files the extraction would add are the entries the Archive
        // wins, not its whole manifest.
        log::info!(
            "would extract {} into {} ({} files)",
            plan.archive_path.display(),
            plan.mod_root.display(),
            plan.merge_entries.len()
        );
        evidence.record_planned_extraction(PlannedArchiveExtraction {
            archive_path: plan.archive_path.clone(),
            mod_root: plan.mod_root.clone(),
            entry_count: plan.entries.len(),
            winner_count: plan.merge_entries.len(),
            estimated_capacity_bytes: plan.estimated_capacity_bytes,
            deletes_source_archive: cleanup == SourceCleanup::Remove,
        })?;
    }
    Ok(if stop.is_cancelled() {
        ControlFlow::Break(Stop::Halted)
    } else {
        ControlFlow::Continue(())
    })
}

/// Runs the Building Effective Asset Tree phase: the definitive pass over
/// every Mod Root, after extraction, then the definitive routing of the
/// paths it found, which it hands to Processing Assets as the Routing Ledger.
///
/// Cancellation is observed once the phase is recorded and per entry; a
/// partial pass stops the work. Only a complete pass records the nested
/// Archive count: the Archives found that are not among the `recognized`
/// ones from the Archive pass.
fn build_effective_asset_tree(
    preparation: &RunPreparation,
    evidence: &mut MutableRunEvidence<'_>,
    router: &AssetRouter,
    discovery: &mut Discovery<'_>,
    recognized: &HashSet<PathBuf>,
) -> PhaseFlow<RoutingLedger> {
    evidence.record_phase(RunPhaseRecord::executed(
        RunPhase::BuildingEffectiveAssetTree,
        None,
    ))?;
    if discovery.stop.is_cancelled() {
        return Ok(ControlFlow::Break(Stop::Halted));
    }

    // The definitive pass. No Archive enters the tree: one the Archive pass did
    // not see appeared only through extraction, and the game never reads an
    // Archive nested inside another, so it is counted rather than worked on.
    discovery.phase = RunPhase::BuildingEffectiveAssetTree;
    let mut effective_paths = Vec::new();
    let mut nested_archives: HashSet<PathBuf> = HashSet::new();
    let mut complete = true;
    for root in preparation.mod_roots() {
        complete = discovery.visit(evidence, root, &mut |path| {
            // Each other file is routed once, by `route_all` below.
            if router.names_archive(path) {
                if !recognized.contains(path) {
                    nested_archives.insert(path.to_path_buf());
                }
            } else {
                effective_paths.push(path.to_path_buf());
            }
        });
        if !complete {
            break;
        }
    }
    if !complete {
        // A partial scan is not a definitive tree and never becomes work.
        return Ok(ControlFlow::Break(Stop::Halted));
    }
    // Only a complete definitive pass establishes the nested count; a
    // partial one records none, so it reports no nested Archives.
    evidence.record_nested_archive_count(nested_archives.len())?;
    Ok(ControlFlow::Continue(router.route_all(&effective_paths)))
}

/// Runs the Processing Assets phase: opens it with `ledger` and offers every
/// Routed Asset to `toolset`'s Asset attempt port in Texture, Mesh, Animation
/// order, recording each completed attempt through the phase's handle before
/// reporting it.
///
/// Cancellation is observed before each attempt and once more after the
/// last; an attempt in flight always finishes. An attempt's panic is
/// contained as an unsafe Operation Failure with unknown mutation, and an
/// attempt that is not safe to continue stops the work. An Asset backend's
/// report of cancellation stops it too, as [`Stop::BackendCancelled`], the
/// one stop whose reason the executor needs.
fn process_assets(
    preparation: &RunPreparation,
    toolset: &mut RunToolset,
    evidence: &mut MutableRunEvidence<'_>,
    ledger: RoutingLedger,
    artifacts: &mut TemporaryArtifactRegistry,
    stop: &CancellationToken,
) -> PhaseFlow {
    // The evidence retains the ledger it opens Processing Assets with; this
    // shared handle on it lets the loop iterate it while the phase's handle
    // records attempts, without copying every Routed Asset.
    let ledger = Arc::new(ledger);
    let mut assets = evidence.open_processing_assets(Arc::clone(&ledger))?;
    for target in [
        OptimizerTarget::Texture,
        OptimizerTarget::Mesh,
        OptimizerTarget::Animation,
    ] {
        for asset in ledger.routed_assets_for(target) {
            if stop.is_cancelled() {
                return Ok(ControlFlow::Break(Stop::Halted));
            }
            let path = asset.execution_path();
            let attempt = match attributed_mod_root(path, preparation.mod_roots()) {
                // The Asset is never opened, so nothing changed; but the tree
                // is no longer the one discovery saw, so the run stops.
                None => (
                    PathBuf::new(),
                    AssetExecutionResult::failed(
                        AssetExecutionFailure::OutsideModRoots,
                        "Routed Asset is outside prepared Mod Roots",
                    )
                    .with_safe_to_continue(false)
                    .with_path(path),
                ),
                Some(mod_root) => {
                    // An Asset attempt's diagnostics are retained, not published,
                    // so they publish with the run's other deferred diagnostics.
                    // The open phase's handle holds the evidence, so they are
                    // retained through it.
                    let mut retain =
                        |diagnostic: RunDiagnostic| assets.retain_diagnostic(diagnostic);
                    match catch_unwind(AssertUnwindSafe(|| {
                        toolset.asset_attempt().attempt(
                            asset,
                            mod_root,
                            artifacts,
                            stop,
                            &mut retain,
                        )
                    })) {
                        Ok(Ok(result)) => (mod_root.clone(), result),
                        Ok(Err(AssetInitializationCancelled)) => {
                            // The backend reported this cancellation; the token
                            // need not show it, so the stop carries it to the
                            // executor's one sample at Safety Cleanup rather
                            // than the work phases recording it themselves.
                            return Ok(ControlFlow::Break(Stop::BackendCancelled));
                        }
                        // A panic cannot establish whether durable bytes were committed.
                        Err(payload) => (
                            mod_root.clone(),
                            AssetExecutionResult::failed(
                                AssetExecutionFailure::BackendException,
                                take_panic_message(payload),
                            )
                            .with_mutation(MutationState::PartialOrUnknown)
                            .with_safe_to_continue(false)
                            .with_path(path),
                        ),
                    }
                }
            };
            let (mod_root, result) = attempt;
            let safe_to_continue = result.safe_to_continue();
            assets.record(RoutedAssetAttempt {
                mod_root,
                asset: asset.clone(),
                result,
            })?;
            // A failed attempt still completes progress, but uncertain mutation
            // makes every later attempt and Archive Finalization unsafe.
            if !safe_to_continue {
                return Ok(ControlFlow::Break(Stop::Halted));
            }
        }
    }
    // Cancellation raised during the last attempt has no later loop head to
    // observe it, and a finalizer need not check it either.
    Ok(if stop.is_cancelled() {
        ControlFlow::Break(Stop::Halted)
    } else {
        ControlFlow::Continue(())
    })
}

/// Runs the Archive Finalization phase, once the Run Diagnostics retained so
/// far are published. Apply executes it over `toolset`'s seams with
/// `choices`, the Run Request's Archive Finalization choices, through the
/// handle its executed record opens; Dry Run plans it with the same choices
/// ([`plan_archive_finalization`]), and with `extractions`, the run's
/// Archive extraction plans and source cleanup, whose source removals an
/// Apply run would prune after.
///
/// Cancellation is observed after that publication, before the phase is
/// recorded, and again before Apply finalizes or Dry Run plans. As the last
/// work phase it returns `Break` only to report that finalization or its
/// planning did not finish.
#[expect(
    clippy::too_many_arguments,
    reason = "the phase's inputs; the extraction plans matter only to a Dry Run's pruning plan"
)]
fn finalize_archives(
    preparation: &RunPreparation,
    mode: ExecutionMode,
    choices: &ArchiveFinalizationChoices,
    extractions: (&[ArchiveExtractionPlan], SourceCleanup),
    toolset: &RunToolset,
    evidence: &mut MutableRunEvidence<'_>,
    artifacts: &mut TemporaryArtifactRegistry,
    stop: &CancellationToken,
) -> PhaseFlow {
    evidence.publish_diagnostics();
    // An observer of those diagnostics may cancel; the run then stops before
    // entering Archive Finalization rather than recording a phase it skips.
    if stop.is_cancelled() {
        return Ok(ControlFlow::Break(Stop::Halted));
    }

    // The execution mode selects the final work phase's record: Apply
    // executes Archive Finalization over the Run Toolset's seams, and Dry
    // Run plans it. The immutable policy decides: Dry Run never finalizes,
    // whatever the Toolset holds.
    if mode == ExecutionMode::DryRun {
        let (plans, cleanup) = extractions;
        let effects = PlannedExtractionEffects::of(plans, cleanup);
        return plan_archive_finalization(preparation, choices, &effects, toolset, evidence, stop);
    }
    let finalization = evidence.open_archive_finalization()?;
    // Archive Finalization is a separate mutation boundary: observe
    // cancellation before it. The unplanned handle is then dropped, which
    // records nothing, so the phase reports no result.
    if stop.is_cancelled() {
        return Ok(ControlFlow::Break(Stop::Halted));
    }
    // Finalization reaches only its own facts, never the whole evidence, and
    // owns the phase it runs. It contains its own panics as an Unexpected
    // Exception, so it is called without a second layer.
    toolset.finalize_archives(preparation, choices, finalization, artifacts, &|| {
        stop.is_cancelled()
    })?;
    Ok(ControlFlow::Continue(()))
}

/// Records a Dry Run's Archive Finalization as Planned, then plans its
/// output Archives through `toolset`'s planning-only step with `choices` and
/// records each as Planned Archive Work, marked as an estimate made from the
/// Mod Roots as they are now: a Dry Run plans before any extraction or Loose
/// Asset optimization an Apply run would do first. Each recorded output and
/// each distinct Dummy Plugin it would need gets a "would create" line in
/// the Application Log.
///
/// Without an Archive creation request the phase is still Planned, with no
/// planned output. Nothing is packed, committed or cleaned, no Loading
/// Plugin is maintained and no directory pruned. Each folder the planned
/// packing or the planned Archive extractions (`extractions`) would empty is
/// recorded after the outputs, with a "would remove" line, since an Apply
/// run would prune it; with no Archive creation request those folders are
/// the phase's only record.
///
/// A shortage the Apply run's Capacity Check would fail Archive creation on
/// is recorded as a [`RunDiagnosticCode::CapacityShortage`] warning, and
/// logged, before the outputs; it neither fails the Dry Run nor drops any
/// planned output.
///
/// A Mod Root planning could not plan is recorded as a
/// [`RunDiagnosticCode::ArchiveCreationUnplanned`] warning, and the other
/// roots' outputs are still recorded: an Apply run would leave that root out
/// the same way.
///
/// Cancellation is observed before planning and by planning itself; either
/// stops with nothing planned, as an Apply run stops with no output total. A
/// contained planning panic is recorded as the run's Run Failure. Both
/// return `Break`.
fn plan_archive_finalization(
    preparation: &RunPreparation,
    choices: &ArchiveFinalizationChoices,
    extractions: &PlannedExtractionEffects,
    toolset: &RunToolset,
    evidence: &mut MutableRunEvidence<'_>,
    stop: &CancellationToken,
) -> PhaseFlow {
    evidence.record_phase(RunPhaseRecord::planned(RunPhase::ArchiveFinalization))?;
    if stop.is_cancelled() {
        return Ok(ControlFlow::Break(Stop::Halted));
    }
    // What the planned packing would empty, kept from the plan below; every
    // other arm returns before it is read.
    let pruning;
    let outputs = match toolset
        .plan_archive_creation(preparation, choices, extractions, &|| stop.is_cancelled())
    {
        CreationPlanning::NotRequested { pruning } => {
            record_planned_pruning(evidence, pruning)?;
            return Ok(ControlFlow::Continue(()));
        }
        CreationPlanning::Planned {
            outputs,
            capacity_warning,
            unplanned,
            plan_warnings,
            pruning: planned_pruning,
        } => {
            // As at Discovering Archives, the warning neither fails the Dry
            // Run nor drops a planned output: every one is recorded below.
            if let Some(warning) = capacity_warning {
                record_capacity_warning(evidence, warning);
            }
            // Planning already logged each root it left out; the evidence
            // carries it as a warning, as an Apply run carries it as a
            // contained PlanningFailed.
            for warning in unplanned {
                evidence.record_diagnostic(warning);
            }
            // So did each output nothing would load and each root with names
            // that differ only in case; an Apply run records the same
            // warnings.
            for warning in plan_warnings {
                evidence.record_diagnostic(warning);
            }
            pruning = planned_pruning;
            outputs
        }
        CreationPlanning::Cancelled => return Ok(ControlFlow::Break(Stop::Halted)),
        CreationPlanning::Failed(detail) => {
            evidence.record_failure(RunFailure::new(
                RunFailureCode::ArchiveCreationPlanningFailed,
                RunPhase::ArchiveFinalization,
                detail,
            ));
            return Ok(ControlFlow::Break(Stop::Halted));
        }
    };
    evidence.record_output_plan_basis(OutputPlanBasis::EstimateFromModRootsAsTheyAre)?;
    // Outputs that share a fallback Loading Plugin name, such as an
    // Archive and its Textures Archive, would need one Dummy Plugin, so each
    // is reported once.
    for (output, first_plugin) in PlannedOutputArchive::with_first_dummy_plugins(&outputs) {
        // Recorded before it is reported, so the log never names an output
        // the evidence refused.
        evidence.record_planned_output(output.clone())?;
        log::info!(
            "would create {} ({} files, {})",
            output.archive_path.display(),
            output.file_count,
            ByteSize(output.total_bytes)
        );
        if let Some(plugin) = first_plugin {
            log::info!("would create Dummy Plugin {}", plugin.display());
        }
    }
    record_planned_pruning(evidence, pruning)?;
    Ok(ControlFlow::Continue(()))
}

/// Records each of a Dry Run's `pruning` folders as Planned Archive Work, in
/// order, and logs a "would remove" line for each. Each is recorded before
/// it is reported, as each planned output is, so the log never names a
/// folder the evidence refused.
fn record_planned_pruning(
    evidence: &mut MutableRunEvidence<'_>,
    pruning: Vec<PlannedPruning>,
) -> Result<(), Error> {
    for pruned in pruning {
        let directory = pruned.directory.clone();
        evidence.record_planned_pruning(pruned)?;
        log::info!("would remove empty folder {}", directory.display());
    }
    Ok(())
}

/// Records a Dry Run's capacity `warning`, the
/// [`RunDiagnosticCode::CapacityShortage`] its Capacity Check returned, in
/// `evidence`, and logs it to the Application Log as a warning; neither fails
/// the run.
fn record_capacity_warning(evidence: &mut MutableRunEvidence<'_>, warning: RunDiagnostic) {
    log::warn!("{}: {}", warning.path.display(), warning.detail);
    evidence.record_diagnostic(warning);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::unit_scratch_dir;

    /// Audit CORE-12: only a name-surrogate directory is a link that
    /// discovery refuses to follow. A directory with any other reparse tag,
    /// such as a cloud-sync placeholder, is walked; every reparse-point file
    /// is resolved before it is visited.
    #[test]
    fn only_a_name_surrogate_directory_is_a_directory_link() {
        // ((reparse point, directory, name surrogate), scope, example)
        let rows = [
            ((false, false, false), EntryScope::File, "an ordinary file"),
            (
                (false, true, false),
                EntryScope::Directory,
                "an ordinary directory",
            ),
            (
                (true, true, true),
                EntryScope::DirectoryLink,
                "a junction, directory symlink or mounted folder",
            ),
            (
                (true, true, false),
                EntryScope::Directory,
                "a cloud placeholder directory",
            ),
            (
                (true, false, true),
                EntryScope::LinkedFile,
                "a file symlink",
            ),
            (
                (true, false, false),
                EntryScope::LinkedFile,
                "a deduplicated or placeholder file",
            ),
        ];
        for ((reparse_point, directory, name_surrogate), expected, example) in rows {
            assert_eq!(
                EntryScope::classify(reparse_point, directory, name_surrogate),
                expected,
                "{example}"
            );
        }
    }

    /// A directory listing's own metadata, which discovery classifies
    /// without a query per entry, tells a file, a directory and a junction
    /// apart as [`EntryScope::classify`] expects.
    #[test]
    fn listing_metadata_classifies_files_directories_and_junctions() {
        let dir = unit_scratch_dir("work_phases/entry-scope");
        std::fs::write(dir.join("file.dds"), b"").unwrap();
        std::fs::create_dir(dir.join("directory")).unwrap();
        // Junctions need no privilege, unlike symlinks, so this runs anywhere.
        let created = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(dir.join("junction"))
            .arg(dir.join("directory"))
            .output()
            .expect("cmd runs");
        assert!(
            created.status.success(),
            "mklink /J failed: {}",
            String::from_utf8_lossy(&created.stderr)
        );

        let mut scopes: Vec<(String, EntryScope)> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (
                    entry.file_name().to_string_lossy().into_owned(),
                    EntryScope::of(&entry.metadata().unwrap()),
                )
            })
            .collect();
        scopes.sort_by(|left, right| left.0.cmp(&right.0));
        std::fs::remove_dir(dir.join("junction")).unwrap();

        assert_eq!(
            scopes,
            [
                ("directory".to_owned(), EntryScope::Directory),
                ("file.dds".to_owned(), EntryScope::File),
                ("junction".to_owned(), EntryScope::DirectoryLink),
            ]
        );
    }
}
