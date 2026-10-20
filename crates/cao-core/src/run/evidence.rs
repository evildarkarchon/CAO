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

use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;

use crate::Error;
use crate::execution::{AssetExecutionResult, MutationState};
use crate::routing::{RoutedAsset, RoutingLedger, SkipReason};
use crate::run::{
    ArchiveCollision, ArchiveExtractionResult, ArchiveFinalizationAttempt,
    ArchiveFinalizationRecorder, ArchiveFinalizationResult, MutationKind, MutationSummary,
    RunDiagnostic, RunDiagnosticCode, RunFailure, RunFailureCode, RunPhase, RunPhaseRecord,
    RunPhaseStatus, RunPreparation, RunProgress, take_panic_message,
};

/// One completed attempt: its routed identity, resolved Mod Root and durable outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutedAssetAttempt {
    /// The canonical Mod Root the attempt is attributed to, frozen before it ran.
    pub mod_root: PathBuf,
    pub asset: RoutedAsset,
    pub result: AssetExecutionResult,
}

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
    archive_collisions: Vec<ArchiveCollision>,
    archive_collisions_recorded: bool,
    archive_extraction_attempts: Vec<ArchiveExtractionResult>,
    archive_discovery: Option<ArchiveDiscoveryEvidence>,
    routing_ledger: Option<RoutingLedger>,
    asset_attempts: Vec<RoutedAssetAttempt>,
    /// The phase's streamed attempts once its total is recorded, then its
    /// returned result.
    archive_finalization: Option<ArchiveFinalizationResult>,
    archive_finalization_total: Option<usize>,
    archive_finalization_completed: bool,
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

    /// The preflight Archive Collisions, in Mod Root and game-path order.
    /// Empty when there were none or the preflight never completed.
    pub fn archive_collisions(&self) -> &[ArchiveCollision] {
        &self.storage.archive_collisions
    }

    /// Completed Archive extraction attempts, in attempt order.
    pub fn archive_extraction_attempts(&self) -> &[ArchiveExtractionResult] {
        &self.storage.archive_extraction_attempts
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

    /// Archive Finalization's result, or `None` when the phase recorded none.
    pub fn archive_finalization(&self) -> Option<&ArchiveFinalizationResult> {
        self.storage.archive_finalization.as_ref()
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
pub(crate) struct MutableRunEvidence<'a> {
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

/// The kinds of attempt whose Run Progress the shared progress step
/// accounts for, each streamed during its own executed Run Phase against the
/// determinate total recorded when that phase was planned.
#[derive(Debug, Clone, Copy)]
enum AttemptKind {
    ArchiveExtraction,
    Asset,
    ArchiveFinalization,
}

impl AttemptKind {
    /// The Run Phase this kind of attempt is recorded during.
    fn phase(self) -> RunPhase {
        match self {
            Self::ArchiveExtraction => RunPhase::ExtractingArchives,
            Self::Asset => RunPhase::ProcessingAssets,
            Self::ArchiveFinalization => RunPhase::ArchiveFinalization,
        }
    }

    /// The invariant an attempt breaks when its Run Phase is not the
    /// current, executed one.
    fn outside_phase(self) -> &'static str {
        match self {
            Self::ArchiveExtraction => {
                "Archive extraction attempts must be recorded during Extracting Archives"
            }
            Self::Asset => "Asset attempts must be recorded during Processing Assets",
            Self::ArchiveFinalization => {
                "Archive Finalization attempts must be recorded during Archive Finalization"
            }
        }
    }

    /// The invariant an attempt breaks when its Run Phase has no planned
    /// total yet.
    fn before_plan(self) -> &'static str {
        match self {
            Self::ArchiveExtraction => "Archive extraction attempts require a planned total",
            Self::Asset => "Asset attempts require a planned routed-work total",
            Self::ArchiveFinalization => {
                "Archive Finalization attempts require a recorded output total"
            }
        }
    }

    /// The invariant an attempt breaks when the planned total is already
    /// complete.
    fn beyond_total(self) -> &'static str {
        match self {
            Self::ArchiveExtraction => {
                "Archive extraction attempts cannot exceed the planned total"
            }
            Self::Asset => "Asset attempts cannot exceed the routed-work total",
            Self::ArchiveFinalization => {
                "Archive Finalization attempts cannot exceed the planned total"
            }
        }
    }
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
        let planned = self.storage.archive_finalization_total.is_none()
            && self.storage.phases.last().is_some_and(|record| {
                record.phase() == RunPhase::ArchiveFinalization
                    && record.status() == RunPhaseStatus::Executed
                    && record.progress().is_none()
            });
        if !planned {
            return Err(Error::EvidenceInvariant(
                "Archive Finalization needs an executed phase and one immutable plan",
            ));
        }
        self.storage.archive_finalization_total = Some(total);
        self.storage.archive_finalization = Some(ArchiveFinalizationResult::default());
        self.record_phase(RunPhaseRecord::executed(
            RunPhase::ArchiveFinalization,
            Some(RunProgress::determinate(total, 0, 0)),
        ))
    }

    /// Retains one completed Archive Finalization output attempt, then
    /// advances progress against the immutable output total (C++
    /// `recordArchiveFinalizationAttempt`).
    ///
    /// # Errors
    /// A Run Evidence invariant violation: Archive Finalization is not the
    /// current, executed phase, its output total is not recorded, its result
    /// is already recorded, or the attempts would exceed the total.
    pub fn record_archive_finalization_attempt(
        &mut self,
        attempt: ArchiveFinalizationAttempt,
    ) -> Result<(), Error> {
        // A recorded result closes the phase to attempts but leaves it the
        // current, executed phase, so the shared step alone would accept one.
        if self.storage.archive_finalization_completed {
            return Err(Error::EvidenceInvariant(
                "Archive Finalization attempts cannot follow its recorded result",
            ));
        }
        let succeeded = attempt.succeeded();
        self.record_attempt_progress(AttemptKind::ArchiveFinalization, succeeded, |storage| {
            // The plan opened this result; the default only covers a
            // progress record that bypassed the plan.
            storage
                .archive_finalization
                .get_or_insert_with(ArchiveFinalizationResult::default)
                .attempts
                .push(attempt);
        })
    }

    /// The one progress step every attempt kind shares: finds `kind`'s
    /// current, executed Run Phase, reads the total its plan recorded,
    /// rejects an attempt beyond that total, retains the attempt through
    /// `retain`, then publishes progress advanced by one succeeded or failed
    /// attempt.
    ///
    /// The total is read from the phase's determinate Run Progress, so no
    /// caller can supply one that disagrees with the plan. `retain` runs
    /// only once every check has passed, so a rejected attempt is never
    /// retained.
    ///
    /// # Errors
    /// A Run Evidence invariant violation, with `kind`'s own message, when
    /// its phase is not the current, executed one, has no plan yet, or the
    /// planned total is already complete.
    fn record_attempt_progress(
        &mut self,
        kind: AttemptKind,
        succeeded: bool,
        retain: impl FnOnce(&mut Storage),
    ) -> Result<(), Error> {
        let Some(current) = self.storage.phases.last().filter(|record| {
            record.phase() == kind.phase() && record.status() == RunPhaseStatus::Executed
        }) else {
            return Err(Error::EvidenceInvariant(kind.outside_phase()));
        };
        let Some(progress) = current.progress() else {
            return Err(Error::EvidenceInvariant(kind.before_plan()));
        };
        if progress.completed() >= progress.total() {
            return Err(Error::EvidenceInvariant(kind.beyond_total()));
        }
        retain(&mut self.storage);
        self.record_phase(RunPhaseRecord::executed(
            kind.phase(),
            Some(RunProgress::determinate(
                progress.total(),
                progress.succeeded() + usize::from(succeeded),
                progress.failed() + usize::from(!succeeded),
            )),
        ))
    }

    /// Retains Archive Finalization's one returned result (C++
    /// `recordArchiveFinalization`).
    ///
    /// Its attempts must begin with every attempt already streamed, in order;
    /// any it adds are recorded as attempts first. A result returned before
    /// any output total, because planning failed or was cancelled, may have
    /// no attempts. A cancelled result marks cancellation observed.
    pub fn record_archive_finalization(
        &mut self,
        result: ArchiveFinalizationResult,
    ) -> Result<(), Error> {
        let executing = self.storage.phases.last().is_some_and(|record| {
            record.phase() == RunPhase::ArchiveFinalization
                && record.status() == RunPhaseStatus::Executed
        });
        if self.storage.archive_finalization_completed || !executing {
            return Err(Error::EvidenceInvariant(
                "Archive Finalization results require the executed work phase exactly once",
            ));
        }
        let Some(total) = self.storage.archive_finalization_total else {
            // A finalizer may fail or cancel before planning establishes a
            // trustworthy total; keep its phase-level status without
            // inventing zero planned outputs.
            if !result.attempts.is_empty() {
                return Err(Error::EvidenceInvariant(
                    "Archive Finalization attempts require a recorded output total",
                ));
            }
            self.complete_archive_finalization(result);
            return Ok(());
        };
        let streamed = self
            .storage
            .archive_finalization
            .as_ref()
            .map_or(0, |finalization| finalization.attempts.len());
        let matches = self
            .storage
            .archive_finalization
            .as_ref()
            .is_some_and(|finalization| {
                result.attempts.len() <= total
                    && streamed <= result.attempts.len()
                    && finalization
                        .attempts
                        .iter()
                        .zip(&result.attempts)
                        .all(|(retained, returned)| retained == returned)
            });
        if !matches {
            return Err(Error::EvidenceInvariant(
                "Returned Archive Finalization attempts must match retained attempt order",
            ));
        }
        for attempt in &result.attempts[streamed..] {
            self.record_archive_finalization_attempt(attempt.clone())?;
        }
        self.complete_archive_finalization(result);
        Ok(())
    }

    /// Whether Archive Finalization's one result is already recorded, so a
    /// contained port panic knows no further result may be recorded.
    pub(crate) fn archive_finalization_completed(&self) -> bool {
        self.storage.archive_finalization_completed
    }

    /// The finalization recorder lent this evidence for as long as it lives,
    /// which the run hands to Archive Finalization in place of the whole
    /// store, so the phase reaches only its own facts.
    pub(crate) fn finalization_recorder(&mut self) -> ArchiveFinalizationRecorder<'_, 'a> {
        ArchiveFinalizationRecorder::new(self)
    }

    /// Retains the phase's final result, once.
    fn complete_archive_finalization(&mut self, result: ArchiveFinalizationResult) {
        if result.cancelled {
            self.storage.cancellation_observed = true;
        }
        self.storage.archive_finalization = Some(result);
        self.storage.archive_finalization_completed = true;
    }

    /// Archive Finalization's attempts and result so far, once its output
    /// total or result is recorded.
    pub fn archive_finalization(&self) -> Option<&ArchiveFinalizationResult> {
        self.storage.archive_finalization.as_ref()
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

    /// Retains the complete preflight collision plan, once, while Discovering
    /// Archives is current, so the evidence holds it before any extraction.
    pub fn record_archive_collisions(
        &mut self,
        collisions: &[ArchiveCollision],
    ) -> Result<(), Error> {
        if self.storage.current_phase() != Some(RunPhase::DiscoveringArchives) {
            return Err(Error::EvidenceInvariant(
                "Archive Collisions must be recorded during Discovering Archives",
            ));
        }
        if self.storage.archive_collisions_recorded {
            return Err(Error::EvidenceInvariant(
                "Archive Collisions can only be recorded once",
            ));
        }
        self.storage.archive_collisions = collisions.to_vec();
        self.storage.archive_collisions_recorded = true;
        Ok(())
    }

    /// Retains one completed extraction attempt, then advances progress
    /// against the immutable planned total.
    ///
    /// # Errors
    /// A Run Evidence invariant violation: Extracting Archives is not the
    /// current, executed phase, has no planned total, or the attempts would
    /// exceed it.
    pub fn record_archive_extraction_attempt(
        &mut self,
        attempt: ArchiveExtractionResult,
    ) -> Result<(), Error> {
        let succeeded = attempt.succeeded();
        self.record_attempt_progress(AttemptKind::ArchiveExtraction, succeeded, |storage| {
            storage.archive_extraction_attempts.push(attempt);
        })
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
    ///
    /// The routed total is the one [`Self::record_asset_processing_plan`]
    /// checked against the definitive Routing Ledger.
    ///
    /// # Errors
    /// A Run Evidence invariant violation: Processing Assets is not the
    /// current, executed phase, has no planned total, or the attempts would
    /// exceed it.
    pub fn record_asset_attempt(&mut self, attempt: RoutedAssetAttempt) -> Result<(), Error> {
        let succeeded = attempt.result.succeeded();
        self.record_attempt_progress(AttemptKind::Asset, succeeded, |storage| {
            storage.asset_attempts.push(attempt);
        })
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
    // Only the evidence suite and the `test_support` finalization harness
    // read a phase back from the crate-private store (#561).
    #[cfg(any(test, feature = "test-support"))]
    pub fn phase(&self, phase: RunPhase) -> Option<&RunPhaseRecord> {
        self.storage.phase(phase)
    }

    /// The current lifecycle position, or `None` before traversal starts.
    pub fn current_phase(&self) -> Option<&RunPhaseRecord> {
        self.storage.phases.last()
    }

    /// Definitive routing, once it has completed.
    pub fn routing_ledger(&self) -> Option<&RoutingLedger> {
        self.storage.routing_ledger.as_ref()
    }

    /// Runs one presentation callback, retaining a panic as an unpublished
    /// `ObserverFailed` diagnostic.
    pub fn report_safely(&mut self, phase: RunPhase, publication: impl FnOnce()) {
        if let Err(payload) = catch_unwind(AssertUnwindSafe(publication)) {
            self.retain_observer_failure(phase, take_panic_message(payload));
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
        storage.mutation_summaries = derive_mutation_summaries(
            &storage.archive_extraction_attempts,
            &storage.asset_attempts,
            storage.archive_finalization.as_ref(),
        );
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

/// Groups durable effects of completed attempts, and finalization effects
/// outside its output attempts, by Mod Root and kind.
fn derive_mutation_summaries(
    extractions: &[ArchiveExtractionResult],
    attempts: &[RoutedAssetAttempt],
    finalization: Option<&ArchiveFinalizationResult>,
) -> Vec<MutationSummary> {
    let mut grouped: BTreeMap<(PathBuf, MutationKind), MutationSummary> = BTreeMap::new();
    let finalization_attempts = finalization.map_or(&[][..], |result| &result.attempts);
    let finalization_mutations = finalization.map_or(&[][..], |result| &result.mutations);
    let effects = extractions
        .iter()
        .map(|attempt| {
            (
                &attempt.mod_root,
                MutationKind::ArchiveExtraction,
                attempt.mutation,
                1,
            )
        })
        .chain(attempts.iter().map(|attempt| {
            (
                &attempt.mod_root,
                MutationKind::AssetProcessing,
                attempt.result.mutation_state(),
                1,
            )
        }))
        .chain(finalization_attempts.iter().map(|attempt| {
            (
                &attempt.mod_root,
                MutationKind::ArchiveFinalization,
                attempt.mutation,
                1,
            )
        }))
        .chain(finalization_mutations.iter().map(|mutation| {
            (
                &mutation.mod_root,
                MutationKind::ArchiveFinalization,
                mutation.mutation,
                mutation.count,
            )
        }));
    for (mod_root, kind, mutation, count) in effects {
        if mutation == MutationState::None {
            continue;
        }
        let summary = grouped
            .entry((mod_root.clone(), kind))
            .or_insert_with(|| MutationSummary {
                mod_root: mod_root.clone(),
                kind,
                committed: 0,
                partial_or_unknown: 0,
            });
        if mutation == MutationState::Committed {
            summary.committed += count;
        } else {
            summary.partial_or_unknown += count;
        }
    }
    grouped.into_values().collect()
}

#[cfg(test)]
mod tests;
