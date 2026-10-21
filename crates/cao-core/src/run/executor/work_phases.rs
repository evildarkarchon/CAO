//! The Run Executor's work phases: Archive-first discovery, definitive routing,
//! Asset attempts and Archive Finalization.
//!
//! Ported from `src/Run/AssetRun.h` and `src/Run/ArchiveFirstAssetDiscovery.h`.
//! The executor calls [`run_work_phases`] with its Run Toolset; the
//! function walks each Mod Root for Archives, extracts the enabled ones in
//! Apply with the core [`ArchiveExtractor`](crate::run::ArchiveExtractor)
//! over the Toolset's seams, routes the Effective Asset Tree once, offers
//! every Routed Asset to the Asset attempt port in Texture, Mesh, Animation
//! order, recording each completed attempt before reporting it, and in Apply
//! runs [`ArchiveFinalization`](crate::run::ArchiveFinalization) over the
//! same seams. The Toolset assembles both over its seams, so this module
//! never reaches into them.
//!
//! Loose Assets take precedence over Archived Assets, and Archive Precedence
//! decides between Archives in one Mod Root. Before the first extraction, a
//! preflight reads every selected manifest, rejects any Unsafe Game Path,
//! records the Archive Collisions in Run Evidence and runs the Capacity
//! Check; any failure stops the run with nothing extracted.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use crate::Error;
use crate::execution::{AssetExecutionFailure, AssetExecutionResult, MutationState};
use crate::routing::{
    AssetKind, AssetRouter, ExecutionMode, OptimizerTarget, RoutingDecision, SkipReason,
};
use crate::run::archives::{Preflight, RootArchives, archive_order_key, relative_game_path};
use crate::run::{
    ArchiveDiscoveryEvidence, ArchiveExtractionPlan, AssetInitializationCancelled,
    CancellationToken, MutableRunEvidence, PhaseSkipReason, RoutedAssetAttempt, RunDiagnostic,
    RunDiagnosticCode, RunPhase, RunPhaseRecord, RunPreparation, RunToolset,
    TemporaryArtifactRegistry, is_staging_name, take_panic_message,
};

/// Walks Mod Roots for regular files under the scope rules both discovery passes share.
///
/// It holds no evidence of its own: each walk is lent the run's evidence for
/// its linked-entry exclusion diagnostics, since other work phases record
/// between the two passes.
struct Discovery<'r> {
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
        let Ok(resolved) = cao_winfs::msvc_canonical(path) else {
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
    let resolved = cao_winfs::msvc_weakly_canonical(path).unwrap_or_else(|_| path.to_path_buf());
    mod_roots
        .iter()
        .filter(|root| resolved.starts_with(root))
        .max_by_key(|root| root.as_os_str().len())
}

/// Runs Archive-first discovery, definitive routing, Asset attempts and
/// Archive Finalization through `toolset`'s Asset attempt port and over its
/// Archive seams.
///
/// Completed facts enter `evidence`, which the executor lends for the whole
/// call, before the run's Run Observation Sink sees them. The work phases
/// enter, skip and plan their own Run Phases by recording them in
/// `evidence`, which enforces the canonical Run Phase order. Cancellation,
/// from `stop`, is observed between entries and attempts and once more after
/// the last attempt; an attempt in flight always finishes. An unsafe attempt
/// stops all further work.
///
/// An Asset attempt's panic is contained here, as an unsafe Operation
/// Failure with unknown mutation. The Archive extractor and Archive
/// Finalization contain their own panics, so they are called without a
/// second layer. Archive Finalization runs only in Apply, once its executed
/// phase is recorded. Errors are Run Evidence invariant violations, which
/// the executor raises after Safety Cleanup.
pub(super) fn run_work_phases(
    preparation: &RunPreparation,
    toolset: &mut RunToolset,
    evidence: &mut MutableRunEvidence<'_>,
    artifacts: &mut TemporaryArtifactRegistry,
    stop: &CancellationToken,
) -> Result<(), Error> {
    let policy = preparation.policy();
    let mode = policy.execution_mode();
    let router = AssetRouter::new(policy.clone());

    evidence.record_archive_discovery_started()?;

    let mut discovery = Discovery {
        stop,
        phase: RunPhase::DiscoveringArchives,
        diagnosed: HashSet::new(),
    };
    let mut skipped_archive_counts: BTreeMap<SkipReason, usize> = BTreeMap::new();
    let mut recognized_archives: HashSet<PathBuf> = HashSet::new();
    let mut root_archives: Vec<RootArchives> = Vec::new();

    // The Archive pass: recognize every Archive, and every Loose Asset an
    // Archived Asset must not replace, before any extraction.
    for root in preparation.mod_roots() {
        let mut selected: Vec<PathBuf> = Vec::new();
        let mut loose = BTreeSet::new();
        let complete = discovery.visit(evidence, root, &mut |path| {
            if !names_an_archive(&router, path) {
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
        if !complete {
            return interrupted(evidence, skipped_archive_counts);
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

    // Dry Run ignores Archive Precedence and never reads a manifest,
    // calculates a collision or extracts.
    let mut plans: Vec<ArchiveExtractionPlan> = Vec::new();
    if mode == ExecutionMode::Apply {
        match toolset.preflight_archives(root_archives, preparation.archive_precedence(), stop) {
            Preflight::Failed(failure) => {
                // A failed preflight has no trustworthy tree, so the run stops
                // before any extraction, routing or finalization.
                evidence.record_archive_discovery(ArchiveDiscoveryEvidence {
                    skipped_archive_counts,
                    ..ArchiveDiscoveryEvidence::default()
                })?;
                evidence.record_failure(failure);
                return finish(evidence, false);
            }
            Preflight::Cancelled => return interrupted(evidence, skipped_archive_counts),
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

    if stop.is_cancelled() {
        return interrupted(evidence, skipped_archive_counts);
    }
    match mode {
        ExecutionMode::DryRun => evidence.record_dry_run_archive_extraction()?,
        ExecutionMode::Apply => evidence.record_archive_extraction_plan(plans.len())?,
    };
    if stop.is_cancelled() {
        return interrupted(evidence, skipped_archive_counts);
    }

    if let Some(halt) = extract_archives(&plans, toolset, evidence, artifacts, stop)? {
        return match halt {
            ExtractionStop::Cancelled => interrupted(evidence, skipped_archive_counts),
            // An unsafe attempt never reaches routing or finalization; only
            // separately observed cancellation is retained alongside it.
            ExtractionStop::Unsafe { cancellation } => {
                evidence.record_archive_discovery(ArchiveDiscoveryEvidence {
                    skipped_archive_counts,
                    ..ArchiveDiscoveryEvidence::default()
                })?;
                finish(evidence, cancellation)
            }
        };
    }

    evidence.record_effective_asset_tree_started()?;
    if stop.is_cancelled() {
        return interrupted(evidence, skipped_archive_counts);
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
            if names_an_archive(&router, path) {
                if !recognized_archives.contains(path) {
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
    evidence.record_archive_discovery(ArchiveDiscoveryEvidence {
        skipped_archive_counts,
        unsupported_explicit_paths: Vec::new(),
        nested_archive_count: if complete { nested_archives.len() } else { 0 },
    })?;
    if !complete {
        // A partial scan is not a definitive tree and never becomes work.
        return finish(evidence, true);
    }
    evidence.record_routing_ledger(router.route_all(&effective_paths))?;

    // A copy, so the loop can iterate it while recording attempts.
    let ledger = evidence
        .routing_ledger()
        .cloned()
        .expect("the Routing Ledger was just recorded");
    evidence.record_asset_processing_plan(ledger.routed_assets().len())?;
    for target in [
        OptimizerTarget::Texture,
        OptimizerTarget::Mesh,
        OptimizerTarget::Animation,
    ] {
        for asset in ledger.routed_assets_for(target) {
            if stop.is_cancelled() {
                return finish(evidence, true);
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
                    let mut retain =
                        |diagnostic: RunDiagnostic| evidence.retain_diagnostic(diagnostic);
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
                        Ok(Err(AssetInitializationCancelled)) => return finish(evidence, true),
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
            evidence.record_asset_attempt(RoutedAssetAttempt {
                mod_root,
                asset: asset.clone(),
                result,
            })?;
            // A failed attempt still completes progress, but uncertain mutation
            // makes every later attempt and Archive Finalization unsafe.
            if !safe_to_continue {
                return finish(evidence, stop.is_cancelled());
            }
        }
    }
    // Cancellation raised during the last attempt has no later loop head to
    // observe it, and a finalizer need not check it either.
    if stop.is_cancelled() {
        return finish(evidence, true);
    }
    evidence.publish_diagnostics();
    // An observer of those diagnostics may cancel; the run then stops before
    // entering Archive Finalization rather than recording a phase it skips.
    if stop.is_cancelled() {
        return finish(evidence, true);
    }

    // The execution mode selects the final work phase's record: Apply
    // executes Archive Finalization over the Run Toolset's seams, and Dry
    // Run skips it.
    evidence.record_phase(match mode {
        ExecutionMode::DryRun => {
            RunPhaseRecord::skipped(RunPhase::ArchiveFinalization, PhaseSkipReason::DryRun)
        }
        ExecutionMode::Apply => RunPhaseRecord::executed(RunPhase::ArchiveFinalization, None),
    })?;
    // Archive Finalization is a separate mutation boundary: observe cancellation before it.
    if stop.is_cancelled() {
        return finish(evidence, true);
    }
    // The immutable policy decides: Dry Run never finalizes, whatever the Toolset holds.
    if mode == ExecutionMode::Apply {
        // Finalization reaches only its own facts, never the whole evidence.
        // It contains its own panics as an Unexpected Exception, so it is
        // called without a second layer.
        toolset.finalize_archives(
            preparation,
            &mut evidence.finalization_recorder(),
            artifacts,
            &|| stop.is_cancelled(),
        )?;
    }
    finish(evidence, stop.is_cancelled())
}

/// Ends the work on any return: records cancellation in `evidence` when it
/// was observed, then publishes every diagnostic the work retained, exactly
/// once. Always `Ok`, so a return site can hand back its result directly.
fn finish(evidence: &mut MutableRunEvidence<'_>, cancellation_observed: bool) -> Result<(), Error> {
    if cancellation_observed {
        evidence.record_cancellation_observation();
    }
    evidence.publish_diagnostics();
    Ok(())
}

/// Ends the work after cancellation before the definitive pass: keeps the
/// Archive exclusions the Archive pass already counted, but never a partial
/// tree, then [`finish`]es with cancellation observed.
///
/// # Errors
/// A Run Evidence invariant violation recording the discovery facts.
fn interrupted(
    evidence: &mut MutableRunEvidence<'_>,
    skipped_archive_counts: BTreeMap<SkipReason, usize>,
) -> Result<(), Error> {
    evidence.record_archive_discovery(ArchiveDiscoveryEvidence {
        skipped_archive_counts,
        ..ArchiveDiscoveryEvidence::default()
    })?;
    finish(evidence, true)
}

/// Why Archive extraction stopped short of the definitive pass.
enum ExtractionStop {
    /// Cancellation was observed between attempts.
    Cancelled,
    /// An attempt left unknown mutation or claimed it was unsafe to continue;
    /// `cancellation` records whether cancellation was also observed.
    Unsafe { cancellation: bool },
}

/// Extracts each planned Archive in order through
/// [`RunToolset::extract_archive`], the core
/// [`ArchiveExtractor`](crate::run::ArchiveExtractor) over `toolset`'s reader
/// and the run's Capacity Check with its source cleanup, retaining every
/// completed attempt before it is reported.
///
/// An attempt in flight always finishes, so cancellation never leaves a
/// partial Archive. The extractor contains its own panics, classified by how
/// far the attempt got, so it is called without a second layer.
/// `PartialOrUnknown` mutation is unsafe even when the attempt claims
/// otherwise, and stops extraction. Returns `None` when every plan was
/// attempted and the run may continue.
fn extract_archives(
    plans: &[ArchiveExtractionPlan],
    toolset: &RunToolset,
    evidence: &mut MutableRunEvidence<'_>,
    artifacts: &mut TemporaryArtifactRegistry,
    stop: &CancellationToken,
) -> Result<Option<ExtractionStop>, Error> {
    if plans.is_empty() {
        // No attempt ran, so there is none to re-sample cancellation after:
        // the caller's own checks around this phase observe it.
        return Ok(None);
    }
    for plan in plans {
        if stop.is_cancelled() {
            return Ok(Some(ExtractionStop::Cancelled));
        }
        let mut attempt = toolset.extract_archive(plan, artifacts);
        // The attempt's own continuation claim is retained for diagnosis, but
        // partial mutation stops the run regardless.
        let unsafe_attempt = attempt.is_unsafe();
        attempt.mod_root = plan.mod_root.clone();
        evidence.record_archive_extraction_attempt(attempt)?;
        if unsafe_attempt {
            return Ok(Some(ExtractionStop::Unsafe {
                cancellation: stop.is_cancelled(),
            }));
        }
    }
    // Re-sampled after the final attempt and its progress record, so
    // cancellation during the last Archive still skips the definitive pass.
    Ok(stop.is_cancelled().then_some(ExtractionStop::Cancelled))
}

/// Reports whether routing recognizes the path as an Archive, enabled or not.
fn names_an_archive(router: &AssetRouter, path: &Path) -> bool {
    match router.route(path) {
        RoutingDecision::Routed(asset) => asset.kind() == AssetKind::Archive,
        RoutingDecision::Skipped(asset) => asset.kind() == AssetKind::Archive,
        RoutingDecision::Unsupported => false,
    }
}
