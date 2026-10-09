//! The Asset Run: Archive-first discovery, definitive routing and Asset attempts.
//!
//! Ported from `src/Run/AssetRun.h` and the Loose Asset half of
//! `src/Run/ArchiveFirstAssetDiscovery.h`. A Run Work Service calls
//! [`execute_asset_run`] with its adapters; the function walks each Mod Root,
//! routes the Effective Asset Tree once, and offers every Routed Asset to the
//! adapters in Texture, Mesh, Animation order, recording each completed attempt
//! before reporting it.
//!
//! The run passes through every lifecycle phase the C++ run does, so run facts
//! compare in order with the parity oracle. Archives are recognized and counted
//! here; selecting one for extraction in Apply fails the run before any
//! mutation until Archive discovery and extraction are ported (#496, #497).

use std::collections::{BTreeMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use crate::Error;
use crate::execution::{AssetExecutionFailure, AssetExecutionResult, MutationState};
use crate::routing::{
    AssetKind, AssetRouter, ExecutionMode, OptimizerTarget, RoutedAsset, RoutedAssetPhase,
    RoutingDecision, SkipReason,
};
use crate::run::{
    ArchiveDiscoveryEvidence, CancellationToken, RunDiagnostic, RunDiagnosticCode, RunFailure,
    RunFailureCode, RunPhase, RunPhaseRecord, RunPreparation, RunWorkEvidence, RunWorkMilestones,
    take_panic_message,
};

/// One completed attempt: its routed identity, resolved Mod Root and durable outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutedAssetAttempt {
    /// The canonical Mod Root the attempt is attributed to, frozen before it ran.
    pub mod_root: PathBuf,
    pub asset: RoutedAsset,
    pub result: AssetExecutionResult,
}

/// Completed Routed Asset attempts against the routed-only total of one phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AssetRunProgress {
    pub phase: RoutedAssetPhase,
    pub completed: usize,
    pub total: usize,
}

/// Cancellation observed while an Asset backend initialized, before any attempt existed.
///
/// Initialization is read-only, so the run records cancellation without
/// inventing an attempt or a mutation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AssetInitializationCancelled;

/// Executes one Routed Asset against its frozen canonical Mod Root.
pub type ExecuteAsset<'a> = Box<
    dyn FnMut(&RoutedAsset, &Path) -> Result<AssetExecutionResult, AssetInitializationCancelled>
        + 'a,
>;

/// Observes one lifecycle boundary of an Asset Run.
pub type ReportPhase<'a> = Box<dyn FnMut(&RunPhaseRecord) + 'a>;

/// The adapters a Run Work Service supplies to one Asset Run.
///
/// `execute_asset` is required; the rest are optional. A panic in any adapter
/// is contained: in `execute_asset` it becomes an unsafe Operation Failure with
/// unknown mutation, and in a presentation adapter an `ObserverFailed`
/// diagnostic.
pub struct AssetRunAdapters<'a> {
    pub execute_asset: ExecuteAsset<'a>,
    /// Observes completed attempts against the routed total.
    pub report_progress: Option<Box<dyn FnMut(AssetRunProgress) + 'a>>,
    /// Observes each lifecycle boundary, including empty phases, before work.
    pub report_phase: Option<ReportPhase<'a>>,
    /// An extra cancellation source, combined with the run's token.
    pub is_cancelled: Option<Box<dyn Fn() -> bool + 'a>>,
    /// Runs Archive Finalization, which records its own output total,
    /// attempts and result into the same evidence. It is called only in
    /// Apply, once the executed phase is recorded; its errors reach the Run
    /// Executor unchanged. Without one, Apply reports the phase as having no
    /// requested work.
    pub finalize_archive_lifecycle: Option<FinalizeArchiveLifecycle<'a>>,
}

/// Runs Archive Finalization against the run's evidence.
pub type FinalizeArchiveLifecycle<'a> =
    Box<dyn FnMut(&RunWorkEvidence<'_, '_>) -> Result<(), Error> + 'a>;

impl<'a> AssetRunAdapters<'a> {
    /// Adapters that only execute Assets.
    pub fn new(execute_asset: ExecuteAsset<'a>) -> Self {
        Self {
            execute_asset,
            report_progress: None,
            report_phase: None,
            is_cancelled: None,
            finalize_archive_lifecycle: None,
        }
    }
}

/// Reserves the `.cao-staging` namespace, case-insensitively and for any suffix.
///
/// Discovery skips the whole namespace: stale or unverified temporary material
/// must never become optimization input.
pub fn is_staging_name(name: &std::ffi::OsStr) -> bool {
    name.to_string_lossy()
        .to_ascii_lowercase()
        .starts_with(".cao-staging")
}

/// Walks Mod Roots for regular files under the scope rules both discovery passes share.
struct Discovery<'r, 'e, 'x, 'y> {
    evidence: &'e RunWorkEvidence<'x, 'y>,
    stop: &'r dyn Fn() -> bool,
    /// The phase linked-entry diagnostics are attributed to.
    phase: RunPhase,
    /// Each excluded linked entry is diagnosed once, across both passes.
    diagnosed: HashSet<PathBuf>,
}

impl Discovery<'_, '_, '_, '_> {
    /// Retains one linked-entry exclusion the first time it is observed.
    fn exclude(&mut self, path: &Path, detail: &str) {
        if self.diagnosed.insert(path.to_path_buf()) {
            self.evidence.retain_diagnostic(
                RunDiagnostic::new(RunDiagnosticCode::LinkedEntryExcluded, self.phase, detail)
                    .with_path(path),
            );
        }
    }

    /// Rejects directory links, and file links not provably inside the Mod Root.
    /// Entries that vanish or cannot be read are silently absent.
    fn within_scope(&mut self, path: &Path, canonical_root: &Path) -> bool {
        let Ok(metadata) = std::fs::symlink_metadata(path) else {
            return false;
        };
        // Reparse points are rejected by attribute: junctions are not always symlinks.
        if !metadata.file_type().is_symlink() && !cao_winfs::is_reparse_point(&metadata) {
            return true;
        }
        if path.is_dir() {
            self.exclude(
                path,
                "Directory links and reparse points are not followed during discovery.",
            );
            return false;
        }
        let Ok(resolved) = cao_winfs::msvc_canonical(path) else {
            self.exclude(
                path,
                "Linked entry could not be resolved within the Mod Root.",
            );
            return false;
        };
        let Ok(relative) = resolved.strip_prefix(canonical_root) else {
            self.exclude(path, "Linked entry resolves outside the Mod Root.");
            return false;
        };
        // A contained alias can still point into staging; its own name would hide that.
        if relative
            .components()
            .any(|component| is_staging_name(component.as_os_str()))
        {
            self.exclude(path, "Linked entry resolves into excluded CAO staging.");
            return false;
        }
        true
    }

    /// Visits each regular file beneath `root` depth-first, in enumeration
    /// order. Returns `false` as soon as cancellation is observed.
    fn visit(&mut self, root: &Path, visitor: &mut dyn FnMut(&Path)) -> bool {
        if root.file_name().is_some_and(is_staging_name) {
            return true;
        }
        self.visit_directory(root, root, visitor)
    }

    fn visit_directory(
        &mut self,
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
            if (self.stop)() {
                return false;
            }
            let Ok(entry) = entry else { continue };
            let path = entry.path();
            if is_staging_name(&entry.file_name()) || !self.within_scope(&path, root) {
                continue;
            }
            if path.is_file() {
                visitor(&path);
            } else if path.is_dir() && !self.visit_directory(&path, root, visitor) {
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

/// Runs Archive-first discovery, definitive routing and Asset attempts beneath the Run Executor.
///
/// Completed facts enter the evidence before any presentation adapter sees
/// them, and the milestones let the executor own phase position and progress.
/// Cancellation, from `stop` or the adapters, is observed between entries and
/// attempts and once more after the last attempt; an attempt in flight always
/// finishes. An unsafe attempt stops all further work. Errors are Run Evidence
/// invariant violations, which the executor raises after Safety Cleanup.
pub fn execute_asset_run(
    preparation: &RunPreparation,
    evidence: &RunWorkEvidence<'_, '_>,
    milestones: &dyn RunWorkMilestones,
    stop: &CancellationToken,
    adapters: &mut AssetRunAdapters<'_>,
) -> Result<(), Error> {
    let policy = preparation.policy();
    let mode = policy.execution_mode();
    let router = AssetRouter::new(policy.clone());
    let extra_cancellation = adapters.is_cancelled.take();
    let cancelled = || {
        stop.is_cancelled()
            || extra_cancellation
                .as_ref()
                .is_some_and(|is_cancelled| is_cancelled())
    };
    let mut report_phase = adapters.report_phase.take();
    let mut report = |record: RunPhaseRecord| {
        if let Some(report_phase) = report_phase.as_mut() {
            evidence.report_safely(record.phase(), || report_phase(&record));
        }
    };
    // Every return publishes the diagnostics discovery retained, exactly once.
    let finish = |cancellation_observed: bool| -> Result<(), Error> {
        if cancellation_observed {
            evidence.record_cancellation_observation();
        }
        evidence.publish_diagnostics();
        Ok(())
    };
    // Cancellation before the definitive pass: keep the Archive exclusions the
    // pass already counted, but never a partial tree.
    let interrupted = |skipped_archive_counts: BTreeMap<SkipReason, usize>| -> Result<(), Error> {
        evidence.record_archive_discovery(ArchiveDiscoveryEvidence {
            skipped_archive_counts,
            ..ArchiveDiscoveryEvidence::default()
        })?;
        finish(true)
    };

    report(milestones.archive_discovery_started()?);

    let mut discovery = Discovery {
        evidence,
        stop: &cancelled,
        phase: RunPhase::DiscoveringArchives,
        diagnosed: HashSet::new(),
    };
    let mut skipped_archive_counts: BTreeMap<SkipReason, usize> = BTreeMap::new();
    let mut recognized_archives: HashSet<PathBuf> = HashSet::new();
    let mut selected_archives: Vec<PathBuf> = Vec::new();

    // The Archive pass: recognize every Archive before any extraction.
    for root in preparation.mod_roots() {
        let complete = discovery.visit(root, &mut |path| {
            if !names_an_archive(&router, path) || !recognized_archives.insert(path.to_path_buf()) {
                return;
            }
            match router.route(path) {
                RoutingDecision::Routed(_) => selected_archives.push(path.to_path_buf()),
                RoutingDecision::Skipped(asset) => {
                    *skipped_archive_counts.entry(asset.reason()).or_default() += 1
                }
                // `names_an_archive` already confirmed routing recognizes it.
                RoutingDecision::Unsupported => {}
            }
        });
        if !complete {
            return interrupted(skipped_archive_counts);
        }
    }

    if mode == ExecutionMode::Apply && !selected_archives.is_empty() {
        // A selected Archive cannot be extracted in this build. Failing before
        // extraction keeps the tree untouched, as any discovery failure does.
        evidence.record_archive_discovery(ArchiveDiscoveryEvidence {
            skipped_archive_counts,
            ..ArchiveDiscoveryEvidence::default()
        })?;
        evidence.record_failure(
            RunFailure::new(
                RunFailureCode::RequestedWorkUnavailable,
                RunPhase::DiscoveringArchives,
                "Archive extraction is not available in this build",
            )
            .with_path(&selected_archives[0]),
        );
        return finish(false);
    }

    if cancelled() {
        return interrupted(skipped_archive_counts);
    }
    report(match mode {
        ExecutionMode::DryRun => milestones.dry_run_archive_extraction()?,
        ExecutionMode::Apply => milestones.archive_extraction_planned(selected_archives.len())?,
    });
    if cancelled() {
        return interrupted(skipped_archive_counts);
    }

    report(milestones.effective_asset_tree_started()?);
    if cancelled() {
        return interrupted(skipped_archive_counts);
    }

    // The definitive pass. No Archive enters the tree: one the Archive pass did
    // not see appeared only through extraction, and the game never reads an
    // Archive nested inside another, so it is counted rather than worked on.
    discovery.phase = RunPhase::BuildingEffectiveAssetTree;
    let mut effective_paths = Vec::new();
    let mut nested_archives: HashSet<PathBuf> = HashSet::new();
    let mut complete = true;
    for root in preparation.mod_roots() {
        complete = discovery.visit(root, &mut |path| {
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
        return finish(true);
    }
    evidence.record_routing_ledger(router.route_all(&effective_paths))?;

    let ledger = evidence
        .routing_ledger()
        .expect("the Routing Ledger was just recorded");
    let total = ledger.routed_assets().len();
    report(milestones.asset_processing_planned(total)?);
    let mut completed = 0;
    for target in [
        OptimizerTarget::Texture,
        OptimizerTarget::Mesh,
        OptimizerTarget::Animation,
    ] {
        for asset in ledger.routed_assets_for(target) {
            if cancelled() {
                return finish(true);
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
                    match catch_unwind(AssertUnwindSafe(|| {
                        (adapters.execute_asset)(asset, mod_root)
                    })) {
                        Ok(Ok(result)) => (mod_root.clone(), result),
                        Ok(Err(AssetInitializationCancelled)) => return finish(true),
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
            evidence.record_asset_attempt(
                RoutedAssetAttempt {
                    mod_root,
                    asset: asset.clone(),
                    result,
                },
                total,
            )?;
            completed += 1;
            if let Some(current) = evidence.current_phase() {
                report(current);
            }
            if let Some(report_progress) = adapters.report_progress.as_mut() {
                evidence.report_safely(RunPhase::ProcessingAssets, || {
                    report_progress(AssetRunProgress {
                        phase: RoutedAssetPhase::LooseAssetProcessing,
                        completed,
                        total,
                    })
                });
            }
            // A failed attempt still completes progress, but uncertain mutation
            // makes every later attempt and Archive Finalization unsafe.
            if !safe_to_continue {
                return finish(cancelled());
            }
        }
    }
    // Cancellation raised during the last attempt has no later loop head to
    // observe it, and a finalizer need not check it either.
    if cancelled() {
        return finish(true);
    }
    evidence.publish_diagnostics();
    // An observer of those diagnostics may cancel; the run then stops before
    // entering Archive Finalization rather than recording a phase it skips.
    if cancelled() {
        return finish(true);
    }

    let finalizer = adapters.finalize_archive_lifecycle.as_mut();
    report(milestones.archive_finalization_available(mode, finalizer.is_some())?);
    // Archive Finalization is a separate mutation boundary: observe cancellation before it.
    if cancelled() {
        return finish(true);
    }
    // The immutable policy decides: Dry Run never finalizes, whatever the adapters.
    if mode == ExecutionMode::Apply
        && let Some(finalize) = finalizer
    {
        finalize(evidence)?;
    }
    finish(cancelled())
}

/// Reports whether routing recognizes the path as an Archive, enabled or not.
fn names_an_archive(router: &AssetRouter, path: &Path) -> bool {
    match router.route(path) {
        RoutingDecision::Routed(asset) => asset.kind() == AssetKind::Archive,
        RoutingDecision::Skipped(asset) => asset.kind() == AssetKind::Archive,
        RoutingDecision::Unsupported => false,
    }
}
