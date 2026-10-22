//! The Run Executor's work phases: Archive-first discovery, definitive routing,
//! Asset attempts and Archive Finalization.
//!
//! Ported from `src/Run/AssetRun.h` and `src/Run/ArchiveFirstAssetDiscovery.h`.
//! The executor calls [`run_work_phases`] with its Run Toolset; the
//! function walks each Mod Root for Archives, extracts the enabled ones in
//! Apply through [`RunToolset::extract_archive`], routes the Effective Asset
//! Tree once, offers every Routed Asset to the Asset attempt port in Texture,
//! Mesh, Animation order, recording each completed attempt before reporting
//! it, and in Apply runs Archive Finalization through
//! [`RunToolset::finalize_archives`]. The Toolset runs both over its seams,
//! so this module never reaches into them.
//!
//! Loose Assets take precedence over Archived Assets, and Archive Precedence
//! decides between Archives in one Mod Root. Before the first extraction, a
//! preflight reads every selected manifest, rejects any Unsafe Game Path,
//! records the Archive Collisions in Run Evidence and runs the Capacity
//! Check; any failure stops the run with nothing extracted.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::ops::ControlFlow;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use super::Stop;
use crate::Error;
use crate::execution::{AssetExecutionFailure, AssetExecutionResult, MutationState};
use crate::routing::{
    AssetKind, AssetRouter, ExecutionMode, OptimizerTarget, RoutingDecision, RoutingLedger,
    SkipReason,
};
use crate::run::archives::{Preflight, RootArchives, archive_order_key, relative_game_path};
use crate::run::{
    ArchiveExtractionPlan, ArchiveFinalizationChoices, AssetInitializationCancelled,
    CancellationToken, MutableRunEvidence, PhaseSkipReason, RoutedAssetAttempt, RunDiagnostic,
    RunDiagnosticCode, RunPhase, RunPhaseRecord, RunPreparation, RunToolset, SourceCleanup,
    TemporaryArtifactRegistry, WorkChoices, is_staging_name, take_panic_message,
};

/// Walks Mod Roots for regular files under the scope rules both discovery passes share.
///
/// It holds no evidence of its own: each walk is lent the run's evidence for
/// its linked-entry exclusion diagnostics, since other work phases record
/// between the two passes.
struct Discovery<'r> {
    /// The run's token, polled per entry. The discovery phases read it from
    /// here rather than taking it again beside their walker.
    stop: &'r CancellationToken,
    /// The phase linked-entry diagnostics are attributed to.
    phase: RunPhase,
    /// Each excluded linked entry is diagnosed once, across both passes.
    diagnosed: HashSet<PathBuf>,
}

impl Discovery<'_> {
    /// Retains one linked-entry exclusion in `evidence` the first time it is observed.
    fn exclude(&mut self, evidence: &mut MutableRunEvidence<'_>, path: &Path, detail: &str) {
        if self.diagnosed.insert(path.to_path_buf()) {
            evidence.retain_diagnostic(
                RunDiagnostic::new(RunDiagnosticCode::LinkedEntryExcluded, self.phase, detail)
                    .with_path(path),
            );
        }
    }

    /// Rejects directory links, and file links not provably inside the Mod Root.
    /// Entries that vanish or cannot be read are silently absent.
    fn within_scope(
        &mut self,
        evidence: &mut MutableRunEvidence<'_>,
        path: &Path,
        canonical_root: &Path,
    ) -> bool {
        let Ok(metadata) = std::fs::symlink_metadata(path) else {
            return false;
        };
        // Reparse points are rejected by attribute: junctions are not always symlinks.
        if !metadata.file_type().is_symlink() && !cao_winfs::is_reparse_point(&metadata) {
            return true;
        }
        if path.is_dir() {
            self.exclude(
                evidence,
                path,
                "Directory links and reparse points are not followed during discovery.",
            );
            return false;
        }
        let Ok(resolved) = cao_winfs::canonical(path) else {
            self.exclude(
                evidence,
                path,
                "Linked entry could not be resolved within the Mod Root.",
            );
            return false;
        };
        let Ok(relative) = resolved.strip_prefix(canonical_root) else {
            self.exclude(
                evidence,
                path,
                "Linked entry resolves outside the Mod Root.",
            );
            return false;
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
            return false;
        }
        true
    }

    /// Visits each regular file beneath `root` depth-first, in enumeration
    /// order, retaining linked-entry exclusions in `evidence`. Returns
    /// `false` as soon as cancellation is observed.
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

    fn visit_directory(
        &mut self,
        evidence: &mut MutableRunEvidence<'_>,
        directory: &Path,
        root: &Path,
        visitor: &mut dyn FnMut(&Path),
    ) -> bool {
        // A tree can change while work runs: unreadable or vanished directories
        // are absent from this pass rather than failing the run.
        let Ok(entries) = std::fs::read_dir(directory) else {
            return true;
        };
        for entry in entries {
            // Poll for every entry: directories and unsupported files may be the
            // whole tree, so no other seam is guaranteed to observe cancellation.
            if self.stop.is_cancelled() {
                return false;
            }
            let Ok(entry) = entry else { continue };
            let path = entry.path();
            if is_staging_name(&entry.file_name()) || !self.within_scope(evidence, &path, root) {
                continue;
            }
            if path.is_file() {
                visitor(&path);
            } else if path.is_dir() && !self.visit_directory(evidence, &path, root, visitor) {
                return false;
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
    /// The Archives Apply extracts, in order. Dry Run plans none.
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
/// second layer. Archive Finalization runs only in Apply, once its executed
/// phase is recorded. Errors are Run Evidence invariant violations, which
/// the executor raises after Safety Cleanup.
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
    let archives = match discover_archives(
        preparation,
        mode,
        toolset,
        evidence,
        &router,
        &mut discovery,
    )? {
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
        toolset,
        evidence,
        artifacts,
        stop,
    )
}

/// Runs the Discovering Archives phase: the Archive pass over every Mod Root
/// and, in Apply, the preflight that plans extraction.
///
/// The skipped Archive counts are recorded once the pass ends, complete or
/// not. A cancelled pass, a failed preflight, recorded as the run's failure,
/// or a cancelled preflight stops the work with nothing extracted.
fn discover_archives(
    preparation: &RunPreparation,
    mode: ExecutionMode,
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
            if !names_an_archive(router, path) {
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
                // `names_an_archive` already confirmed routing recognizes it.
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

    // Dry Run ignores Archive Precedence and never reads a manifest,
    // calculates a collision or extracts.
    let mut plans: Vec<ArchiveExtractionPlan> = Vec::new();
    if mode == ExecutionMode::Apply {
        match toolset.preflight_archives(
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
                plans: planned,
                collisions,
            } => {
                // Retained before the extraction phase is observed, so a
                // failing observer or a later unwind cannot lose the plan.
                evidence.record_archive_collisions(&collisions)?;
                plans = planned;
            }
        }
    }
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
/// before it is reported. Dry Run skips the phase, opening no handle, and
/// has no plans.
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
    // Dry Run skips Archive extraction, so only Apply opens its handle.
    let extraction = match mode {
        ExecutionMode::DryRun => {
            evidence.record_phase(RunPhaseRecord::skipped(
                RunPhase::ExtractingArchives,
                PhaseSkipReason::DryRun,
            ))?;
            None
        }
        ExecutionMode::Apply => Some(evidence.open_archive_extraction(plans.len())?),
    };
    if stop.is_cancelled() {
        return Ok(ControlFlow::Break(Stop::Halted));
    }

    // Cancellation stops before the definitive pass, and an unsafe attempt
    // never reaches routing or finalization.
    let Some(mut extraction) = extraction else {
        return Ok(ControlFlow::Continue(()));
    };
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
            if names_an_archive(router, path) {
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
    // copy lets the loop iterate it while the handle records attempts.
    let mut assets = evidence.open_processing_assets(ledger.clone())?;
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
                None => (
                    PathBuf::new(),
                    AssetExecutionResult::failed(
                        AssetExecutionFailure::BackendException,
                        "Routed Asset is outside prepared Mod Roots",
                    )
                    .with_mutation(MutationState::PartialOrUnknown)
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
/// handle its executed record opens; Dry Run skips it.
///
/// Cancellation is observed after that publication, before the phase is
/// recorded, and again before Apply finalizes. As the last work phase it
/// returns `Break` only to report that finalization did not run.
fn finalize_archives(
    preparation: &RunPreparation,
    mode: ExecutionMode,
    choices: &ArchiveFinalizationChoices,
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
    // Run skips it. The immutable policy decides: Dry Run never finalizes,
    // whatever the Toolset holds.
    if mode == ExecutionMode::DryRun {
        evidence.record_phase(RunPhaseRecord::skipped(
            RunPhase::ArchiveFinalization,
            PhaseSkipReason::DryRun,
        ))?;
        return Ok(ControlFlow::Continue(()));
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

/// Reports whether routing recognizes the path as an Archive, enabled or not.
fn names_an_archive(router: &AssetRouter, path: &Path) -> bool {
    match router.route(path) {
        RoutingDecision::Routed(asset) => asset.kind() == AssetKind::Archive,
        RoutingDecision::Skipped(asset) => asset.kind() == AssetKind::Archive,
        RoutingDecision::Unsupported => false,
    }
}
