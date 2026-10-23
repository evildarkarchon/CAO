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

use crate::execution::{AssetExecutionResult, MutationState};
use crate::routing::{RoutedAsset, RoutingLedger, SkipReason};
use crate::run::{
    ArchiveCollision, ArchiveExtractionResult, ArchiveFinalizationAccount,
    ArchiveFinalizationResult, MutationKind, MutationSummary, RunDiagnostic, RunDiagnosticCode,
    RunFailure, RunFailureCode, RunPhase, RunPhaseRecord, RunPhaseStatus, RunPreparation,
    RunProgress, take_panic_message,
};
use crate::{AttemptFacts, Error};

mod attempt_phase;
mod planned_work;

use attempt_phase::{ATTEMPT_BEARING_PHASES, PLANNABLE_PHASES};
pub(crate) use attempt_phase::{ArchiveFinalizationHandle, AttemptPhaseHandle, PhaseAttempt};
pub use planned_work::{
    OutputPlanBasis, PlannedArchiveExtraction, PlannedArchiveWork, PlannedOutputArchive,
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

/// The non-derived Archive facts Archive discovery establishes: the Archive
/// pass's skipped counts, and the nested count of a complete definitive pass.
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

// Crate-visible in name only, so the sealed attempt trait can retain into
// it; every field stays private to this module.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Storage {
    preparation: Option<RunPreparation>,
    phases: Vec<RunPhaseRecord>,
    diagnostics: Vec<RunDiagnostic>,
    failures: Vec<RunFailure>,
    archive_collisions: Vec<ArchiveCollision>,
    archive_collisions_recorded: bool,
    archive_extraction_attempts: Vec<ArchiveExtractionResult>,
    /// Present once either Archive discovery fact is recorded; each fact
    /// fills its own field and keeps its own once-only flag below.
    archive_discovery: Option<ArchiveDiscoveryEvidence>,
    skipped_archive_counts_recorded: bool,
    nested_archive_count_recorded: bool,
    routing_ledger: Option<RoutingLedger>,
    asset_attempts: Vec<RoutedAssetAttempt>,
    /// Archive Finalization's public result, the only holder of its
    /// attempts: opened by the plan, or by an unplanned finish, it holds the
    /// streamed attempts with default phase-level fields until the finish
    /// writes the phase-level account into it. Stored rather than built on
    /// demand, as the sealed evidence lends it out by reference.
    archive_finalization: Option<ArchiveFinalizationResult>,
    archive_finalization_completed: bool,
    /// The Archive work a Dry Run planned instead of attempting. Kept apart
    /// from every attempt list, so neither classification nor mutation
    /// summaries, which read only [`Storage::completed_attempts`], count it.
    planned_archive_work: PlannedArchiveWork,
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

    /// Whether `phase` is the current Run Phase and is Planned.
    fn planning(&self, phase: RunPhase) -> bool {
        self.phases.last().is_some_and(|record| {
            record.phase() == phase && record.status() == RunPhaseStatus::Planned
        })
    }

    /// Whether the current Run Phase is one of Archive discovery's, from
    /// Discovering Archives through Building Effective Asset Tree.
    fn archive_discovery_current(&self) -> bool {
        self.current_phase().is_some_and(|phase| {
            (RunPhase::DiscoveringArchives..=RunPhase::BuildingEffectiveAssetTree).contains(&phase)
        })
    }

    /// Whether Archive discovery established both of its facts: the Archive
    /// pass's skipped counts and a complete definitive pass's nested count.
    fn archive_discovery_complete(&self) -> bool {
        self.skipped_archive_counts_recorded && self.nested_archive_count_recorded
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

    /// Every recorded Archive extraction, Asset and Archive Finalization
    /// output attempt, in that kind order and each kind in attempt order.
    ///
    /// This is the one place that projects the three attempt types onto
    /// their shared [`AttemptFacts`]; Run Outcome classification and mutation
    /// summaries both read it, so neither can disagree with the other about
    /// what an attempt mutated or whether it was safe to continue.
    /// Finalization effects outside any output attempt are not attempts and
    /// are absent.
    fn completed_attempts(&self) -> impl Iterator<Item = CompletedAttempt<'_>> {
        let extractions = self
            .archive_extraction_attempts
            .iter()
            .map(|attempt| CompletedAttempt {
                kind: MutationKind::ArchiveExtraction,
                mod_root: &attempt.mod_root,
                facts: attempt.outcome.facts(),
            });
        let assets = self.asset_attempts.iter().map(|attempt| CompletedAttempt {
            kind: MutationKind::AssetProcessing,
            mod_root: &attempt.mod_root,
            facts: attempt.result.outcome().facts(),
        });
        let finalizations = self
            .archive_finalization
            .iter()
            .flat_map(|finalization| &finalization.attempts)
            .map(|attempt| CompletedAttempt {
                kind: MutationKind::ArchiveFinalization,
                mod_root: &attempt.mod_root,
                facts: attempt.outcome.facts(),
            });
        extractions.chain(assets).chain(finalizations)
    }
}

/// One recorded attempt of any kind, reduced to what classification and
/// mutation summaries need: its kind, its Mod Root and its Attempt Outcome's
/// facts. Crate-private, so the view never becomes public API.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CompletedAttempt<'a> {
    /// The attempt's kind, which maps one-to-one onto the three attempt types.
    pub(crate) kind: MutationKind,
    /// The Mod Root the attempt is attributed to.
    pub(crate) mod_root: &'a PathBuf,
    /// The attempt's mutation, succeeded and safe-to-continue facts.
    pub(crate) facts: AttemptFacts,
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

    /// The Archive work a Dry Run planned instead of attempting; empty when
    /// the run planned none. Never part of the attempts or mutation summaries.
    pub fn planned_archive_work(&self) -> &PlannedArchiveWork {
        &self.storage.planned_archive_work
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

    /// Every completed Archive extraction, Asset and Archive Finalization
    /// output attempt as one view, for Run Outcome classification. The
    /// mutation summaries are derived from the same view when the evidence
    /// is sealed.
    pub(crate) fn completed_attempts(&self) -> impl Iterator<Item = CompletedAttempt<'_>> {
        self.storage.completed_attempts()
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
    ///
    /// Extracting Archives, Processing Assets and Archive Finalization
    /// execute only through their [`AttemptPhaseHandle`], so an executed
    /// record of any of them is refused here; their skips still enter
    /// through this transition.
    ///
    /// A Planned record enters here too, and only for Extracting Archives and
    /// Archive Finalization: they are the phases whose Archive work a Dry Run
    /// plans instead of attempting. Once planned, a phase cannot be opened
    /// through its handle, and its status never changes.
    pub fn record_phase(&mut self, record: RunPhaseRecord) -> Result<(), Error> {
        match record.status() {
            RunPhaseStatus::Executed if ATTEMPT_BEARING_PHASES.contains(&record.phase()) => {
                return Err(Error::EvidenceInvariant(
                    "An attempt-bearing Run Phase can only be executed through its attempt-phase handle",
                ));
            }
            RunPhaseStatus::Planned if !PLANNABLE_PHASES.contains(&record.phase()) => {
                return Err(Error::EvidenceInvariant(
                    "Only Extracting Archives and Archive Finalization can be planned",
                ));
            }
            _ => {}
        }
        self.accept_phase(record)
    }

    /// Enters an attempt-bearing Run Phase with its opening `record`, once.
    ///
    /// Every opener goes through here rather than straight to
    /// [`Self::accept_phase`], which would accept an opening of the current
    /// phase as a same-phase replacement: a phase that finished unplanned, or
    /// opened with no attempt yet, could then be reopened, publishing a
    /// duplicate phase record and handing out a second handle.
    ///
    /// # Errors
    /// A Run Evidence invariant violation, recording nothing, when the
    /// phase already has a record, executed or skipped, or cannot be entered
    /// next in canonical traversal order.
    fn open_attempt_phase(&mut self, record: RunPhaseRecord) -> Result<(), Error> {
        if self.storage.phase(record.phase()).is_some() {
            return Err(Error::EvidenceInvariant(
                "An attempt-bearing Run Phase can only be opened once",
            ));
        }
        self.accept_phase(record)
    }

    /// [`Self::record_phase`] without its attempt-phase check: the openers
    /// and the shared progress step enter and advance the attempt-bearing
    /// phases through it, with every other check and the publication intact.
    fn accept_phase(&mut self, record: RunPhaseRecord) -> Result<(), Error> {
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

    /// Opens Archive extraction against its immutable planned total, as
    /// determinate progress starting at zero.
    ///
    /// The returned handle is the only way to record the phase's attempts,
    /// and it holds this evidence until its borrow ends, which closes the
    /// phase.
    ///
    /// # Errors
    /// A Run Evidence invariant violation when Extracting Archives was
    /// already opened or skipped, or cannot be entered next in canonical
    /// traversal order.
    pub fn open_archive_extraction(
        &mut self,
        total: usize,
    ) -> Result<AttemptPhaseHandle<'_, 'a, ArchiveExtractionResult>, Error> {
        self.open_attempt_phase(RunPhaseRecord::executed(
            RunPhase::ExtractingArchives,
            Some(RunProgress::determinate(total, 0, 0)),
        ))?;
        Ok(AttemptPhaseHandle::new(self))
    }

    /// Opens Archive Finalization unplanned, as executed with no progress:
    /// its output total is known only once the phase has planned, through
    /// the returned handle.
    ///
    /// The handle is the only way to plan the phase, record its attempts and
    /// finish it, and it holds this evidence until its borrow ends, which
    /// closes the phase. Opening alone opens no result, so a phase that
    /// neither plans nor finishes reports none.
    ///
    /// # Errors
    /// A Run Evidence invariant violation when Archive Finalization was
    /// already opened, finished or skipped, or cannot be entered next in
    /// canonical traversal order.
    pub fn open_archive_finalization(
        &mut self,
    ) -> Result<ArchiveFinalizationHandle<'_, 'a>, Error> {
        self.open_attempt_phase(RunPhaseRecord::executed(
            RunPhase::ArchiveFinalization,
            None,
        ))?;
        Ok(AttemptPhaseHandle::new(self))
    }

    /// Records Archive Finalization's output total as its determinate progress.
    ///
    /// The phase must be the current, executed phase with no progress yet, so
    /// the plan is recorded once and never changed (C++
    /// `recordArchiveFinalizationPlan`). Nor may it follow the phase's
    /// finish, as cancelled planning finishes without a total: the plan would
    /// reopen the closed phase and replace its account. Finishing consumes
    /// the only handle and the phase cannot be opened again, so that check is
    /// defensive and no handle can reach it.
    ///
    /// The total lives only in that progress, which the shared progress step
    /// reads back for each attempt.
    fn plan_archive_finalization(&mut self, total: usize) -> Result<(), Error> {
        let planned = !self.storage.archive_finalization_completed
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
        self.storage.archive_finalization = Some(ArchiveFinalizationResult::default());
        self.accept_phase(RunPhaseRecord::executed(
            RunPhase::ArchiveFinalization,
            Some(RunProgress::determinate(total, 0, 0)),
        ))
    }

    /// Finishes Archive Finalization, once, writing its phase-level
    /// `account` into the public result beside the attempts already
    /// streamed (C++ `recordArchiveFinalization`, without the attempts).
    ///
    /// Valid with or without a plan: a phase that finishes unplanned, as
    /// cancelled or failed planning does, keeps no output total and has no
    /// attempts. A cancelled account marks cancellation observed.
    ///
    /// # Errors
    /// A Run Evidence invariant violation when Archive Finalization is not
    /// the current, executed phase or has already finished; nothing is then
    /// retained. Finishing consumes the only handle and the phase cannot be
    /// opened again, so both checks are defensive.
    fn finish_archive_finalization(
        &mut self,
        account: ArchiveFinalizationAccount,
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
        let cancelled = account.cancelled;
        // The attempts stay as streamed: the evidence is their only holder.
        let attempts = self
            .storage
            .archive_finalization
            .take()
            .map(|result| result.attempts)
            .unwrap_or_default();
        self.storage.archive_finalization =
            Some(ArchiveFinalizationResult::from_parts(attempts, account));
        if cancelled {
            self.storage.cancellation_observed = true;
        }
        self.storage.archive_finalization_completed = true;
        Ok(())
    }

    /// The one progress step every attempt kind shares: finds `T`'s
    /// current, executed Run Phase, reads the total its plan recorded,
    /// rejects an attempt beyond that total, retains the attempt in `T`'s
    /// own list, then publishes progress advanced by one succeeded or failed
    /// attempt.
    ///
    /// The total is read from the phase's determinate Run Progress, so no
    /// caller can supply one that disagrees with the plan. The attempt is
    /// retained only once every check has passed, so a rejected attempt is
    /// never retained.
    ///
    /// # Errors
    /// A Run Evidence invariant violation, with `T`'s own message, when its
    /// phase is not the current, executed one, has no plan yet, or the
    /// planned total is already complete.
    fn record_attempt_progress<T: PhaseAttempt>(&mut self, attempt: T) -> Result<(), Error> {
        let Some(current) = self.storage.phases.last().filter(|record| {
            record.phase() == T::PHASE && record.status() == RunPhaseStatus::Executed
        }) else {
            return Err(Error::EvidenceInvariant(T::OUTSIDE_PHASE));
        };
        let Some(progress) = current.progress() else {
            return Err(Error::EvidenceInvariant(T::BEFORE_PLAN));
        };
        if progress.completed() >= progress.total() {
            return Err(Error::EvidenceInvariant(T::BEYOND_TOTAL));
        }
        let succeeded = attempt.succeeded();
        attempt.retain(&mut self.storage);
        self.accept_phase(RunPhaseRecord::executed(
            T::PHASE,
            Some(RunProgress::determinate(
                progress.total(),
                progress.succeeded() + usize::from(succeeded),
                progress.failed() + usize::from(!succeeded),
            )),
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

    /// Retains the recognized Archives the Archive pass excluded by Routing
    /// Policy, by Skip Reason.
    ///
    /// Recorded where the Archive pass ends, so every later stop already
    /// carries them. A cancelled pass records the counts it reached.
    ///
    /// # Errors
    /// A Run Evidence invariant violation: Archive discovery is not the
    /// current Run Phase, or the counts were already recorded.
    pub fn record_skipped_archive_counts(
        &mut self,
        counts: BTreeMap<SkipReason, usize>,
    ) -> Result<(), Error> {
        if !self.storage.archive_discovery_current() {
            return Err(Error::EvidenceInvariant(
                "Skipped Archive counts must be recorded during Archive discovery",
            ));
        }
        if self.storage.skipped_archive_counts_recorded {
            return Err(Error::EvidenceInvariant(
                "Skipped Archive counts can only be recorded once",
            ));
        }
        self.storage.skipped_archive_counts_recorded = true;
        self.storage
            .archive_discovery
            .get_or_insert_default()
            .skipped_archive_counts = counts;
        Ok(())
    }

    /// Retains the distinct Archives that appeared only after extraction.
    ///
    /// Recorded only after a complete definitive pass: a partial tree never
    /// reports nested Archives, so the public count then stays zero.
    ///
    /// # Errors
    /// A Run Evidence invariant violation: Archive discovery is not the
    /// current Run Phase, or the count was already recorded.
    pub fn record_nested_archive_count(&mut self, count: usize) -> Result<(), Error> {
        if !self.storage.archive_discovery_current() {
            return Err(Error::EvidenceInvariant(
                "The nested Archive count must be recorded during Archive discovery",
            ));
        }
        if self.storage.nested_archive_count_recorded {
            return Err(Error::EvidenceInvariant(
                "The nested Archive count can only be recorded once",
            ));
        }
        self.storage.nested_archive_count_recorded = true;
        self.storage
            .archive_discovery
            .get_or_insert_default()
            .nested_archive_count = count;
        Ok(())
    }

    /// Retains definitive routing after a successful Effective Asset Tree
    /// discovery and opens Processing Assets against its routed total, as
    /// determinate progress starting at zero.
    ///
    /// The total is derived from `ledger`'s Routed Assets, so it cannot
    /// disagree with the routing. The returned handle is the only way to
    /// record the phase's attempts, and it holds this evidence until its
    /// borrow ends, which closes the phase.
    ///
    /// # Errors
    /// A Run Evidence invariant violation, retaining nothing, when Building
    /// Effective Asset Tree is not current, its Archive discovery facts are
    /// missing, a Run Failure was recorded, or routing was already retained.
    pub fn open_processing_assets(
        &mut self,
        ledger: RoutingLedger,
    ) -> Result<AttemptPhaseHandle<'_, 'a, RoutedAssetAttempt>, Error> {
        if self.storage.current_phase() != Some(RunPhase::BuildingEffectiveAssetTree)
            || !self.storage.archive_discovery_complete()
            || !self.storage.failures.is_empty()
        {
            return Err(Error::EvidenceInvariant(
                "Routing Ledger requires completed, successful Effective Asset Tree discovery",
            ));
        }
        // Opening also leaves Building Effective Asset Tree, so the check
        // above already refuses a second opening; this one keeps routing
        // irreplaceable whatever that check becomes.
        if self.storage.routing_ledger.is_some() {
            return Err(Error::EvidenceInvariant(
                "Definitive routing can only be recorded once",
            ));
        }
        let total = ledger.routed_assets().len();
        self.storage.routing_ledger = Some(ledger);
        // The checks above already keep Processing Assets from opening
        // twice; it shares the openers' once-only entry so no attempt-bearing
        // phase depends on its own preconditions for that.
        self.open_attempt_phase(RunPhaseRecord::executed(
            RunPhase::ProcessingAssets,
            Some(RunProgress::determinate(total, 0, 0)),
        ))?;
        Ok(AttemptPhaseHandle::new(self))
    }

    /// Retains one Archive extraction a Dry Run planned instead of
    /// attempting, after those already recorded.
    ///
    /// # Errors
    /// A Run Evidence invariant violation, retaining nothing, when
    /// Extracting Archives is not the current, Planned Run Phase.
    pub(crate) fn record_planned_extraction(
        &mut self,
        extraction: PlannedArchiveExtraction,
    ) -> Result<(), Error> {
        if !self.storage.planning(RunPhase::ExtractingArchives) {
            return Err(Error::EvidenceInvariant(
                "Planned Archive extractions must be recorded during a Planned Extracting Archives",
            ));
        }
        self.storage
            .planned_archive_work
            .push_extraction(extraction);
        Ok(())
    }

    /// Retains what a Dry Run's output Archives are planned from, once
    /// Archive Finalization planned Archive creation and before it records
    /// any planned output, so every planned output is marked with it.
    ///
    /// # Errors
    /// A Run Evidence invariant violation, retaining nothing, when Archive
    /// Finalization is not the current, Planned Run Phase, or when a basis
    /// was already recorded.
    pub(crate) fn record_output_plan_basis(&mut self, basis: OutputPlanBasis) -> Result<(), Error> {
        if !self.storage.planning(RunPhase::ArchiveFinalization) {
            return Err(Error::EvidenceInvariant(
                "An output plan basis must be recorded during a Planned Archive Finalization",
            ));
        }
        if self.storage.planned_archive_work.output_basis().is_some() {
            return Err(Error::EvidenceInvariant(
                "An output plan basis can only be recorded once",
            ));
        }
        self.storage.planned_archive_work.set_output_basis(basis);
        Ok(())
    }

    /// Retains one output Archive a Dry Run planned instead of creating,
    /// after those already recorded.
    ///
    /// # Errors
    /// A Run Evidence invariant violation, retaining nothing, when Archive
    /// Finalization is not the current, Planned Run Phase, or when no
    /// output plan basis was recorded first
    /// ([`Self::record_output_plan_basis`]).
    pub(crate) fn record_planned_output(
        &mut self,
        output: PlannedOutputArchive,
    ) -> Result<(), Error> {
        if !self.storage.planning(RunPhase::ArchiveFinalization) {
            return Err(Error::EvidenceInvariant(
                "Planned output Archives must be recorded during a Planned Archive Finalization",
            ));
        }
        if self.storage.planned_archive_work.output_basis().is_none() {
            return Err(Error::EvidenceInvariant(
                "Planned output Archives must follow the basis they were planned from",
            ));
        }
        self.storage.planned_archive_work.push_output(output);
        Ok(())
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
    // Only the evidence suite reads a phase back from the crate-private
    // store (#561).
    #[cfg(test)]
    pub fn phase(&self, phase: RunPhase) -> Option<&RunPhaseRecord> {
        self.storage.phase(phase)
    }

    /// The current lifecycle position, or `None` before traversal starts.
    pub fn current_phase(&self) -> Option<&RunPhaseRecord> {
        self.storage.phases.last()
    }

    /// Definitive routing, once it has completed.
    // Only the evidence suite reads routing back from the crate-private
    // store: the work phases iterate the copy they open Processing Assets
    // with.
    #[cfg(test)]
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
        storage.mutation_summaries = derive_mutation_summaries(storage);
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
///
/// Attempts come from [`Storage::completed_attempts`], the same view Run
/// Outcome classification reads; each counts once. The finalization effects
/// that are not output attempts (pruned directories, Dummy Plugin changes)
/// keep their own record type and add their recorded counts.
fn derive_mutation_summaries(storage: &Storage) -> Vec<MutationSummary> {
    let mut grouped: BTreeMap<(PathBuf, MutationKind), MutationSummary> = BTreeMap::new();
    let finalization_mutations = storage
        .archive_finalization
        .as_ref()
        .map_or(&[][..], |result| &result.mutations);
    let effects = storage
        .completed_attempts()
        .map(|attempt| (attempt.mod_root, attempt.kind, attempt.facts.mutation(), 1))
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
