//! Run Evidence: the owned factual record of one Optimization Run.
//!
//! Ported from `src/Run/RunEvidence.h`. [`MutableRunEvidence`] is the sole
//! mutable owner while the run executes; consuming it after Safety Cleanup
//! yields the immutable [`RunEvidence`]. Each fact is retained before it is
//! published through the [`RunObservationSink`], so an observer never sees a
//! fact the evidence could later lose.
//!
//! Structural violations are programming errors. They return
//! [`Error::EvidenceInvariant`], which the Run Executor turns into a panic only
//! after Safety Cleanup.
//!
//! Archive Collisions, extraction attempts and Archive Finalization results
//! join this record with their slices (#496, #497, #498).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;

use crate::Error;
use crate::execution::MutationState;
use crate::routing::{RoutingLedger, SkipReason};
use crate::run::{
    MutationKind, MutationSummary, RoutedAssetAttempt, RunDiagnostic, RunDiagnosticCode,
    RunFailure, RunFailureCode, RunPhase, RunPhaseRecord, RunPhaseStatus, RunPreparation,
    RunProgress, panic_message,
};

/// Receives live facts only after Run Evidence has retained them.
///
/// A panicking sink is contained: the evidence retains an `ObserverFailed`
/// diagnostic and the run continues.
pub trait RunObservationSink {
    /// Observes one accepted phase transition or progress update.
    fn record_phase(&self, phase: &RunPhaseRecord);
    /// Observes one retained Run Failure.
    fn record_failure(&self, failure: &RunFailure);
    /// Observes one retained diagnostic.
    fn record_diagnostic(&self, diagnostic: &RunDiagnostic);
}

/// The non-derived Archive facts established when discovery returns.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ArchiveDiscoveryEvidence {
    /// Recognized Archives excluded by Routing Policy, by Skip Reason.
    pub skipped_archive_counts: BTreeMap<SkipReason, usize>,
    /// Unsupported paths the caller named explicitly as files.
    pub unsupported_explicit_paths: Vec<PathBuf>,
    /// Distinct Archives that appeared only after extraction.
    pub nested_archive_count: usize,
}

impl ArchiveDiscoveryEvidence {
    /// The recognized Archives excluded for one reason.
    pub fn skipped_archive_count(&self, reason: SkipReason) -> usize {
        self.skipped_archive_counts
            .get(&reason)
            .copied()
            .unwrap_or(0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Storage {
    preparation: Option<RunPreparation>,
    phases: Vec<RunPhaseRecord>,
    diagnostics: Vec<RunDiagnostic>,
    failures: Vec<RunFailure>,
    archive_discovery: Option<ArchiveDiscoveryEvidence>,
    routing_ledger: Option<RoutingLedger>,
    asset_attempts: Vec<RoutedAssetAttempt>,
    safety_cleanup_failures: Vec<RunFailure>,
    cancellation_observed: bool,
    // Derived when the evidence is consumed, after every producer has stopped.
    cleanup_failures: Vec<RunFailure>,
    mutation_summaries: Vec<MutationSummary>,
    skipped_asset_counts: BTreeMap<SkipReason, usize>,
}

impl Storage {
    fn phase(&self, phase: RunPhase) -> Option<&RunPhaseRecord> {
        self.phases.iter().find(|record| record.phase() == phase)
    }

    fn current_phase(&self) -> Option<RunPhase> {
        self.phases.last().map(RunPhaseRecord::phase)
    }

    fn skipped_asset_count(&self, reason: SkipReason) -> usize {
        self.archive_discovery
            .as_ref()
            .map_or(0, |discovery| discovery.skipped_archive_count(reason))
            + self
                .routing_ledger
                .as_ref()
                .map_or(0, |ledger| ledger.skipped_asset_count(reason))
    }
}

/// The immutable factual record of one finished Optimization Run.
///
/// The Run Outcome is deliberately absent: classification belongs to the Run
/// Executor, not to the facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunEvidence {
    storage: Storage,
}

impl RunEvidence {
    /// Successful Preparing facts, or `None` when Preparing did not complete.
    pub fn preparation(&self) -> Option<&RunPreparation> {
        self.storage.preparation.as_ref()
    }

    /// Each traversed phase once, in traversal order, with its latest account.
    pub fn phases(&self) -> &[RunPhaseRecord] {
        &self.storage.phases
    }

    /// The latest record of one phase, or `None` when it was never reached.
    pub fn phase(&self, phase: RunPhase) -> Option<&RunPhaseRecord> {
        self.storage.phase(phase)
    }

    /// Diagnostics in the order the evidence accepted them.
    pub fn diagnostics(&self) -> &[RunDiagnostic] {
        &self.storage.diagnostics
    }

    /// Run Failures in the order the evidence accepted them.
    pub fn failures(&self) -> &[RunFailure] {
        &self.storage.failures
    }

    /// Archive discovery facts, or `None` when discovery did not return.
    pub fn archive_discovery(&self) -> Option<&ArchiveDiscoveryEvidence> {
        self.storage.archive_discovery.as_ref()
    }

    /// The definitive routing, or `None` when it did not complete. An empty
    /// ledger still means routing completed with no Routed Assets.
    pub fn routing_ledger(&self) -> Option<&RoutingLedger> {
        self.storage.routing_ledger.as_ref()
    }

    /// Completed Asset attempts in execution order, including unsafe failures.
    pub fn asset_attempts(&self) -> &[RoutedAssetAttempt] {
        &self.storage.asset_attempts
    }

    /// Recognized exclusions from Archive discovery and routing together.
    pub fn skipped_asset_count(&self, reason: SkipReason) -> usize {
        self.storage
            .skipped_asset_counts
            .get(&reason)
            .copied()
            .unwrap_or(0)
    }

    /// Mutation counts derived from completed attempts, ordered by Mod Root and kind.
    pub fn mutation_summaries(&self) -> &[MutationSummary] {
        &self.storage.mutation_summaries
    }

    /// Failures of the final Safety Cleanup pass, in attempted order.
    pub fn safety_cleanup_failures(&self) -> &[RunFailure] {
        &self.storage.safety_cleanup_failures
    }

    /// Attempt-local cleanup failures followed by final Safety Cleanup failures.
    pub fn cleanup_failures(&self) -> &[RunFailure] {
        &self.storage.cleanup_failures
    }

    /// Whether cooperative cancellation was observed before the evidence was sealed.
    pub fn cancellation_observed(&self) -> bool {
        self.storage.cancellation_observed
    }
}

/// The sole mutable owner of one run's facts while it executes.
pub struct MutableRunEvidence<'a> {
    storage: Storage,
    observations: Option<&'a dyn RunObservationSink>,
    published_diagnostics: usize,
    diagnostic_publications: Vec<usize>,
}

/// Rejects progress that cannot be a valid account.
fn validate_progress(record: &RunPhaseRecord) -> Result<(), Error> {
    match record.progress() {
        Some(progress) if progress.completed() > progress.total() => Err(Error::EvidenceInvariant(
            "Run Phase progress exceeds its immutable total",
        )),
        _ => Ok(()),
    }
}

/// Rejects a same-phase replacement that would rewrite or regress accepted facts.
fn validate_replacement(
    previous: &RunPhaseRecord,
    replacement: &RunPhaseRecord,
) -> Result<(), Error> {
    if previous.status() != replacement.status()
        || previous.skip_reason() != replacement.skip_reason()
    {
        return Err(Error::EvidenceInvariant(
            "Run Phase status and skip reason are immutable after traversal",
        ));
    }
    validate_progress(replacement)?;
    let Some(before) = previous.progress() else {
        if replacement
            .progress()
            .is_some_and(|after| after.completed() != 0)
        {
            return Err(Error::EvidenceInvariant(
                "Determinate Run Phase progress must begin at zero",
            ));
        }
        return Ok(());
    };
    let Some(after) = replacement.progress() else {
        return Err(Error::EvidenceInvariant(
            "Run Phase progress cannot become indeterminate",
        ));
    };
    if before.total() != after.total() {
        return Err(Error::EvidenceInvariant(
            "Run Phase progress total cannot change",
        ));
    }
    if after.succeeded() < before.succeeded() || after.failed() < before.failed() {
        return Err(Error::EvidenceInvariant(
            "Run Phase progress cannot regress",
        ));
    }
    Ok(())
}

impl<'a> MutableRunEvidence<'a> {
    /// The sole mutable owner, optionally publishing through a sink.
    pub fn new(observations: Option<&'a dyn RunObservationSink>) -> Self {
        Self {
            storage: Storage::default(),
            observations,
            published_diagnostics: 0,
            diagnostic_publications: Vec::new(),
        }
    }

    /// Retains the one successful preparation, during Preparing.
    pub fn record_preparation(&mut self, preparation: RunPreparation) -> Result<(), Error> {
        if self.storage.preparation.is_some() {
            return Err(Error::EvidenceInvariant(
                "Successful Run preparation can only be recorded once",
            ));
        }
        if self.storage.current_phase() != Some(RunPhase::Preparing) {
            return Err(Error::EvidenceInvariant(
                "Successful Run preparation must be recorded during Preparing",
            ));
        }
        self.storage.preparation = Some(preparation);
        Ok(())
    }

    /// Accepts a canonical transition, or a monotonic update of the current phase.
    ///
    /// A repeated phase replaces its latest account in place. Regression, status
    /// or skip-reason changes and invalid progress are invariant violations.
    pub fn record_phase(&mut self, record: RunPhaseRecord) -> Result<(), Error> {
        validate_progress(&record)?;
        match self.storage.phases.last_mut() {
            None => {
                // A scheduling failure traverses no Preparing but still owes cleanup.
                if record.phase() != RunPhase::Preparing
                    && record.phase() != RunPhase::SafetyCleanup
                {
                    return Err(Error::EvidenceInvariant(
                        "Run Phase traversal must begin at Preparing",
                    ));
                }
                if record
                    .progress()
                    .is_some_and(|progress| progress.completed() != 0)
                {
                    return Err(Error::EvidenceInvariant(
                        "Determinate Run Phase progress must begin at zero",
                    ));
                }
                self.storage.phases.push(record);
            }
            Some(latest) if latest.phase() == record.phase() => {
                validate_replacement(latest, &record)?;
                *latest = record;
            }
            Some(latest) => {
                if record.phase() <= latest.phase() {
                    return Err(Error::EvidenceInvariant(
                        "Run Phases must follow canonical traversal order",
                    ));
                }
                if record
                    .progress()
                    .is_some_and(|progress| progress.completed() != 0)
                {
                    return Err(Error::EvidenceInvariant(
                        "Determinate Run Phase progress must begin at zero",
                    ));
                }
                self.storage.phases.push(record);
            }
        }
        if let Some(sink) = self.observations {
            self.report_safely(record.phase(), || sink.record_phase(&record));
        }
        Ok(())
    }

    /// Retains and immediately publishes one diagnostic.
    pub fn record_diagnostic(&mut self, diagnostic: RunDiagnostic) {
        self.retain_diagnostic(diagnostic);
        self.publish_diagnostics();
    }

    /// Retains a diagnostic now, leaving its publication for a later boundary.
    pub fn retain_diagnostic(&mut self, diagnostic: RunDiagnostic) {
        self.storage.diagnostics.push(diagnostic);
        self.diagnostic_publications
            .push(self.storage.diagnostics.len() - 1);
    }

    /// Publishes every retained diagnostic not yet published, once, in order.
    pub fn publish_diagnostics(&mut self) {
        while self.published_diagnostics < self.diagnostic_publications.len() {
            // Claim before calling out, so a failing sink cannot see it twice.
            let index = self.diagnostic_publications[self.published_diagnostics];
            self.published_diagnostics += 1;
            let diagnostic = self.storage.diagnostics[index].clone();
            if let Some(sink) = self.observations {
                self.report_safely(diagnostic.phase, || sink.record_diagnostic(&diagnostic));
            }
        }
    }

    /// Retains and publishes one Run Failure.
    pub fn record_failure(&mut self, failure: RunFailure) {
        self.storage.failures.push(failure.clone());
        if let Some(sink) = self.observations {
            self.report_safely(failure.phase, || sink.record_failure(&failure));
        }
    }

    /// Retains a Run Failure without publishing it, for a failure found after
    /// Safety Cleanup was published, when no further failure event may follow.
    pub(crate) fn retain_failure(&mut self, failure: RunFailure) {
        self.storage.failures.push(failure);
    }

    /// Accepts that Archive discovery started.
    pub fn record_archive_discovery_started(&mut self) -> Result<(), Error> {
        self.record_phase(RunPhaseRecord::executed(
            RunPhase::DiscoveringArchives,
            None,
        ))
    }

    /// Starts Archive extraction against its immutable planned total.
    pub fn record_archive_extraction_plan(&mut self, total: usize) -> Result<(), Error> {
        self.record_phase(RunPhaseRecord::executed(
            RunPhase::ExtractingArchives,
            Some(RunProgress::determinate(total, 0, 0)),
        ))
    }

    /// Records Archive Finalization's output total as its determinate progress.
    ///
    /// The phase must be the current, executed phase with no progress yet, so
    /// the plan is recorded once and never changed (C++
    /// `recordArchiveFinalizationPlan`).
    pub fn record_archive_finalization_plan(&mut self, total: usize) -> Result<(), Error> {
        let planned = self.storage.phases.last().is_some_and(|record| {
            record.phase() == RunPhase::ArchiveFinalization
                && record.status() == super::RunPhaseStatus::Executed
                && record.progress().is_none()
        });
        if !planned {
            return Err(Error::EvidenceInvariant(
                "Archive Finalization needs an executed phase and one immutable plan",
            ));
        }
        self.record_phase(RunPhaseRecord::executed(
            RunPhase::ArchiveFinalization,
            Some(RunProgress::determinate(total, 0, 0)),
        ))
    }

    /// Accepts that Dry Run made Archive extraction inapplicable.
    pub fn record_dry_run_archive_extraction(&mut self) -> Result<(), Error> {
        self.record_phase(RunPhaseRecord::skipped(
            RunPhase::ExtractingArchives,
            super::PhaseSkipReason::DryRun,
        ))
    }

    /// Accepts that definitive Effective Asset Tree discovery started.
    pub fn record_effective_asset_tree_started(&mut self) -> Result<(), Error> {
        self.record_phase(RunPhaseRecord::executed(
            RunPhase::BuildingEffectiveAssetTree,
            None,
        ))
    }

    /// Retains the Archive discovery facts of one returned discovery call.
    pub fn record_archive_discovery(
        &mut self,
        discovery: ArchiveDiscoveryEvidence,
    ) -> Result<(), Error> {
        match self.storage.current_phase() {
            Some(phase)
                if (RunPhase::DiscoveringArchives..=RunPhase::BuildingEffectiveAssetTree)
                    .contains(&phase) => {}
            _ => {
                return Err(Error::EvidenceInvariant(
                    "Archive discovery evidence must be recorded during Archive discovery",
                ));
            }
        }
        if self.storage.archive_discovery.is_some() {
            return Err(Error::EvidenceInvariant(
                "Archive discovery evidence can only be recorded once",
            ));
        }
        self.storage.archive_discovery = Some(discovery);
        Ok(())
    }

    /// Retains definitive routing after a successful Effective Asset Tree discovery.
    pub fn record_routing_ledger(&mut self, ledger: RoutingLedger) -> Result<(), Error> {
        if self.storage.current_phase() != Some(RunPhase::BuildingEffectiveAssetTree)
            || self.storage.archive_discovery.is_none()
            || !self.storage.failures.is_empty()
        {
            return Err(Error::EvidenceInvariant(
                "Routing Ledger requires completed, successful Effective Asset Tree discovery",
            ));
        }
        if self.storage.routing_ledger.is_some() {
            return Err(Error::EvidenceInvariant(
                "Definitive routing can only be recorded once",
            ));
        }
        self.storage.routing_ledger = Some(ledger);
        Ok(())
    }

    /// Starts Processing Assets with the retained ledger's routed total.
    pub fn record_asset_processing_plan(&mut self, total: usize) -> Result<(), Error> {
        if self
            .storage
            .routing_ledger
            .as_ref()
            .map(|ledger| ledger.routed_assets().len())
            != Some(total)
        {
            return Err(Error::EvidenceInvariant(
                "Processing Assets total must match the definitive Routing Ledger",
            ));
        }
        self.record_phase(RunPhaseRecord::executed(
            RunPhase::ProcessingAssets,
            Some(RunProgress::determinate(total, 0, 0)),
        ))
    }

    /// Retains a completed Asset attempt, then publishes progress against the routed total.
    pub fn record_asset_attempt(
        &mut self,
        attempt: RoutedAssetAttempt,
        total: usize,
    ) -> Result<(), Error> {
        let Some(current) = self
            .storage
            .phases
            .last()
            .filter(|record| record.phase() == RunPhase::ProcessingAssets)
        else {
            return Err(Error::EvidenceInvariant(
                "Asset attempts must be recorded during Processing Assets",
            ));
        };
        let routed = self
            .storage
            .routing_ledger
            .as_ref()
            .map(|ledger| ledger.routed_assets().len());
        let Some(progress) = current
            .progress()
            .filter(|progress| progress.total() == total && routed == Some(total))
        else {
            return Err(Error::EvidenceInvariant(
                "Asset attempts must use the immutable routed-work total",
            ));
        };
        if self.storage.asset_attempts.len() >= total {
            return Err(Error::EvidenceInvariant(
                "Asset attempts cannot exceed the routed-work total",
            ));
        }
        let succeeded = attempt.result.succeeded();
        self.storage.asset_attempts.push(attempt);
        self.record_phase(RunPhaseRecord::executed(
            RunPhase::ProcessingAssets,
            Some(RunProgress::determinate(
                total,
                progress.succeeded() + usize::from(succeeded),
                progress.failed() + usize::from(!succeeded),
            )),
        ))
    }

    /// Retains one final Safety Cleanup failure without publishing it as a Run Failure.
    pub fn record_safety_cleanup_failure(&mut self, failure: RunFailure) -> Result<(), Error> {
        if self.storage.current_phase() != Some(RunPhase::SafetyCleanup)
            || failure.phase != RunPhase::SafetyCleanup
            || !matches!(
                failure.code,
                RunFailureCode::TemporaryArtifactCleanupFailed
                    | RunFailureCode::SafetyCleanupServiceFailed
            )
        {
            return Err(Error::EvidenceInvariant(
                "Safety Cleanup failures require the cleanup phase and a cleanup failure code",
            ));
        }
        self.storage.safety_cleanup_failures.push(failure);
        Ok(())
    }

    /// Retains that cancellation was observed, without choosing an outcome.
    pub fn record_cancellation_observation(&mut self) {
        self.storage.cancellation_observed = true;
    }

    /// The latest record of one phase, or `None` when it was never reached.
    pub fn phase(&self, phase: RunPhase) -> Option<&RunPhaseRecord> {
        self.storage.phase(phase)
    }

    /// The current lifecycle position, or `None` before traversal starts.
    pub fn current_phase(&self) -> Option<&RunPhaseRecord> {
        self.storage.phases.last()
    }

    /// Diagnostics accepted so far.
    pub fn diagnostics(&self) -> &[RunDiagnostic] {
        &self.storage.diagnostics
    }

    /// Run Failures accepted so far.
    pub fn failures(&self) -> &[RunFailure] {
        &self.storage.failures
    }

    /// Archive discovery facts, once discovery has returned.
    pub fn archive_discovery(&self) -> Option<&ArchiveDiscoveryEvidence> {
        self.storage.archive_discovery.as_ref()
    }

    /// Definitive routing, once it has completed.
    pub fn routing_ledger(&self) -> Option<&RoutingLedger> {
        self.storage.routing_ledger.as_ref()
    }

    /// Recognized exclusions from the discovery and routing facts accepted so far.
    pub fn skipped_asset_count(&self, reason: SkipReason) -> usize {
        self.storage.skipped_asset_count(reason)
    }

    /// Runs one presentation callback, retaining a panic as an unpublished
    /// `ObserverFailed` diagnostic.
    pub fn report_safely(&mut self, phase: RunPhase, publication: impl FnOnce()) {
        if let Err(payload) = catch_unwind(AssertUnwindSafe(publication)) {
            self.retain_observer_failure(phase, panic_message(payload.as_ref()));
        }
    }

    /// Retains an observer failure. It is deliberately never published, so the
    /// failing path cannot receive it recursively.
    fn retain_observer_failure(&mut self, phase: RunPhase, detail: String) {
        log::warn!("A Run Event observer failed: {detail}");
        self.storage.diagnostics.push(RunDiagnostic::new(
            RunDiagnosticCode::ObserverFailed,
            phase,
            detail,
        ));
    }

    /// Seals the evidence after Safety Cleanup, deriving summaries from the retained facts.
    pub fn consume(mut self) -> Result<RunEvidence, Error> {
        if self.storage.current_phase() != Some(RunPhase::SafetyCleanup) {
            return Err(Error::EvidenceInvariant(
                "Run Evidence can only be consumed after Safety Cleanup",
            ));
        }
        let storage = &mut self.storage;
        storage.mutation_summaries = derive_mutation_summaries(&storage.asset_attempts);
        storage.cleanup_failures = storage
            .asset_attempts
            .iter()
            .flat_map(|attempt| attempt.result.cleanup_failures().iter().cloned())
            .chain(storage.safety_cleanup_failures.iter().cloned())
            .collect();
        storage.skipped_asset_counts = SkipReason::ALL
            .into_iter()
            .map(|reason| (reason, storage.skipped_asset_count(reason)))
            .filter(|(_, count)| *count != 0)
            .collect();
        Ok(RunEvidence {
            storage: self.storage,
        })
    }
}

/// Groups durable effects of completed attempts by Mod Root and kind.
fn derive_mutation_summaries(attempts: &[RoutedAssetAttempt]) -> Vec<MutationSummary> {
    let mut grouped: BTreeMap<(PathBuf, MutationKind), MutationSummary> = BTreeMap::new();
    for attempt in attempts {
        let mutation = attempt.result.mutation_state();
        if mutation == MutationState::None {
            continue;
        }
        let kind = MutationKind::AssetProcessing;
        let summary = grouped
            .entry((attempt.mod_root.clone(), kind))
            .or_insert_with(|| MutationSummary {
                mod_root: attempt.mod_root.clone(),
                kind,
                committed: 0,
                partial_or_unknown: 0,
            });
        if mutation == MutationState::Committed {
            summary.committed += 1;
        } else {
            summary.partial_or_unknown += 1;
        }
    }
    grouped.into_values().collect()
}

/// The phase-restricted view of Run Evidence that a Run Work Service writes through.
///
/// Work submits complete facts here but cannot choose Run Phases; the executor
/// advances the lifecycle through [`super::RunWorkMilestones`]. Both share one
/// evidence owner for the duration of the synchronous work call.
pub struct RunWorkEvidence<'e, 'a> {
    evidence: &'e RefCell<MutableRunEvidence<'a>>,
}

impl<'e, 'a> RunWorkEvidence<'e, 'a> {
    /// Borrows the executor's evidence owner for one work call.
    pub(crate) fn new(evidence: &'e RefCell<MutableRunEvidence<'a>>) -> Self {
        Self { evidence }
    }

    /// Retains the Archive discovery facts once discovery returns.
    pub fn record_archive_discovery(
        &self,
        discovery: ArchiveDiscoveryEvidence,
    ) -> Result<(), Error> {
        self.evidence
            .borrow_mut()
            .record_archive_discovery(discovery)
    }

    /// Retains definitive routing before Asset processing starts.
    pub fn record_routing_ledger(&self, ledger: RoutingLedger) -> Result<(), Error> {
        self.evidence.borrow_mut().record_routing_ledger(ledger)
    }

    /// Retains a completed Asset attempt and advances its planned progress.
    pub fn record_asset_attempt(
        &self,
        attempt: RoutedAssetAttempt,
        total: usize,
    ) -> Result<(), Error> {
        self.evidence
            .borrow_mut()
            .record_asset_attempt(attempt, total)
    }

    /// Records Archive Finalization's immutable output total, once, after the
    /// executor accepted the executed phase.
    pub fn record_archive_finalization_plan(&self, total: usize) -> Result<(), Error> {
        self.evidence
            .borrow_mut()
            .record_archive_finalization_plan(total)
    }

    /// Retains and publishes a Run Failure.
    pub fn record_failure(&self, failure: RunFailure) {
        self.evidence.borrow_mut().record_failure(failure);
    }

    /// Retains a diagnostic whose publication boundary comes later in the work.
    pub fn retain_diagnostic(&self, diagnostic: RunDiagnostic) {
        self.evidence.borrow_mut().retain_diagnostic(diagnostic);
    }

    /// Publishes deferred diagnostics, once each.
    pub fn publish_diagnostics(&self) {
        self.evidence.borrow_mut().publish_diagnostics();
    }

    /// Retains cancellation independently of the eventual Run Outcome.
    pub fn record_cancellation_observation(&self) {
        self.evidence.borrow_mut().record_cancellation_observation();
    }

    /// Runs a work presentation callback, retaining a panic as a diagnostic.
    ///
    /// The evidence is not borrowed while the callback runs, so the callback
    /// may read the evidence through this view.
    pub fn report_safely(&self, phase: RunPhase, publication: impl FnOnce()) {
        if let Err(payload) = catch_unwind(AssertUnwindSafe(publication)) {
            self.evidence
                .borrow_mut()
                .retain_observer_failure(phase, panic_message(payload.as_ref()));
        }
    }

    /// The current executor-owned phase.
    pub fn current_phase(&self) -> Option<RunPhaseRecord> {
        self.evidence.borrow().current_phase().copied()
    }

    /// The diagnostics accepted so far.
    pub fn diagnostics(&self) -> Vec<RunDiagnostic> {
        self.evidence.borrow().diagnostics().to_vec()
    }

    /// A copy of the definitive Routing Ledger, once routing has completed. A
    /// copy, so the work can iterate it while recording attempts.
    pub fn routing_ledger(&self) -> Option<RoutingLedger> {
        self.evidence.borrow().routing_ledger().cloned()
    }

    /// Recognized exclusions accepted so far.
    pub fn skipped_asset_count(&self, reason: SkipReason) -> usize {
        self.evidence.borrow().skipped_asset_count(reason)
    }

    /// Reports whether a phase was executed rather than skipped, if traversed.
    pub fn phase_status(&self, phase: RunPhase) -> Option<RunPhaseStatus> {
        self.evidence
            .borrow()
            .phase(phase)
            .map(RunPhaseRecord::status)
    }
}
