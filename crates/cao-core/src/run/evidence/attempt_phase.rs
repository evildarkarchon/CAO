//! The attempt-phase handle: the only way an attempt-bearing Run Phase
//! records its attempts and their determinate Run Progress.
//!
//! [`MutableRunEvidence`] opens a phase by handing out an
//! [`AttemptPhaseHandle`] that borrows it exclusively, so nothing else can
//! write the evidence while the phase is open, and the phase closes when
//! that borrow ends. Each attempt type implements the sealed
//! [`PhaseAttempt`] trait, so handing a phase the wrong kind of attempt is a
//! type error rather than a runtime invariant.

use std::marker::PhantomData;

use super::{MutableRunEvidence, RoutedAssetAttempt, Storage};
use crate::Error;
use crate::run::{
    ArchiveExtractionResult, ArchiveFinalizationAccount, ArchiveFinalizationAttempt,
    ArchiveFinalizationResult, RunDiagnostic, RunPhase,
};

mod sealed {
    use super::Storage;

    /// Seals [`super::PhaseAttempt`] to the attempt types this module
    /// implements it for, and carries the retention step, which needs the
    /// evidence's private storage.
    pub trait Sealed {
        /// Retains the attempt in its kind's own list, once the shared
        /// progress step has accepted it.
        fn retain(self, storage: &mut Storage);
    }
}

/// One kind of attempt recorded during its own executed Run Phase against
/// the determinate total recorded when that phase was planned.
///
/// Sealed and crate-private: it supplies the Run Phase, the success test,
/// the retention and the invariant messages the shared progress step needs,
/// so the step lives in one place for every kind.
pub(crate) trait PhaseAttempt: sealed::Sealed {
    /// The Run Phase this kind of attempt is recorded during.
    const PHASE: RunPhase;
    /// The invariant an attempt breaks when its Run Phase is not the current,
    /// executed one.
    const OUTSIDE_PHASE: &'static str;
    /// The invariant an attempt breaks when its Run Phase has no planned
    /// total yet.
    const BEFORE_PLAN: &'static str;
    /// The invariant an attempt breaks when the planned total is already
    /// complete.
    const BEYOND_TOTAL: &'static str;

    /// Whether the attempt counts as succeeded rather than failed in its
    /// phase's progress.
    fn succeeded(&self) -> bool;
}

/// The attempt-bearing Run Phases, one per [`PhaseAttempt`] implementor,
/// read from their own [`PhaseAttempt::PHASE`]s so the list cannot drift
/// from the sealed attempt types. They execute only through their handles.
pub(super) const ATTEMPT_BEARING_PHASES: [RunPhase; 3] = [
    ArchiveExtractionResult::PHASE,
    RoutedAssetAttempt::PHASE,
    ArchiveFinalizationAttempt::PHASE,
];

/// The Run Phases a Dry Run can record as Planned: the two whose Archive
/// work it plans instead of attempting. Read from their attempt types'
/// [`PhaseAttempt::PHASE`]s, so a planned phase is always one whose executed
/// account only its attempt-phase handle can record.
pub(super) const PLANNABLE_PHASES: [RunPhase; 2] = [
    ArchiveExtractionResult::PHASE,
    ArchiveFinalizationAttempt::PHASE,
];

impl sealed::Sealed for ArchiveExtractionResult {
    fn retain(self, storage: &mut Storage) {
        storage.archive_extraction_attempts.push(self);
    }
}

impl PhaseAttempt for ArchiveExtractionResult {
    const PHASE: RunPhase = RunPhase::ExtractingArchives;
    const OUTSIDE_PHASE: &'static str =
        "Archive extraction attempts must be recorded during Extracting Archives";
    const BEFORE_PLAN: &'static str = "Archive extraction attempts require a planned total";
    const BEYOND_TOTAL: &'static str =
        "Archive extraction attempts cannot exceed the planned total";

    fn succeeded(&self) -> bool {
        ArchiveExtractionResult::succeeded(self)
    }
}

impl sealed::Sealed for RoutedAssetAttempt {
    fn retain(self, storage: &mut Storage) {
        storage.asset_attempts.push(self);
    }
}

impl PhaseAttempt for RoutedAssetAttempt {
    const PHASE: RunPhase = RunPhase::ProcessingAssets;
    const OUTSIDE_PHASE: &'static str = "Asset attempts must be recorded during Processing Assets";
    const BEFORE_PLAN: &'static str = "Asset attempts require a planned routed-work total";
    const BEYOND_TOTAL: &'static str = "Asset attempts cannot exceed the routed-work total";

    fn succeeded(&self) -> bool {
        self.result.succeeded()
    }
}

impl sealed::Sealed for ArchiveFinalizationAttempt {
    fn retain(self, storage: &mut Storage) {
        // Only the plan records the phase's progress, and it opens the
        // result the attempts stream into, so the shared step never accepts
        // an attempt without one. Opening one here regardless means an
        // accepted attempt can never be lost.
        storage
            .archive_finalization
            .get_or_insert_with(ArchiveFinalizationResult::default)
            .attempts
            .push(self);
    }
}

impl PhaseAttempt for ArchiveFinalizationAttempt {
    const PHASE: RunPhase = RunPhase::ArchiveFinalization;
    // Archive Finalization has always reported every attempt outside its
    // executed, planned and open phase as one broken plan.
    const OUTSIDE_PHASE: &'static str =
        "Archive Finalization attempts require the immutable executed plan";
    const BEFORE_PLAN: &'static str = Self::OUTSIDE_PHASE;
    const BEYOND_TOTAL: &'static str =
        "Archive Finalization attempts cannot exceed the planned total";

    fn succeeded(&self) -> bool {
        ArchiveFinalizationAttempt::succeeded(self)
    }
}

/// An open attempt-bearing Run Phase, recording attempts of type `T`.
///
/// It borrows the run's [`MutableRunEvidence`] exclusively for as long as it
/// lives, so no other producer can write the evidence while the phase is
/// open; the phase closes when the borrow ends. It deliberately has no
/// `Drop`: closing a phase records nothing and cannot fail, so an exit path
/// never has a closing step to forget or an error to swallow.
///
/// A phase with operations of its own adds them as inherent methods on its
/// instantiation, alongside the generic [`Self::record`].
pub(crate) struct AttemptPhaseHandle<'e, 'a, T: PhaseAttempt> {
    evidence: &'e mut MutableRunEvidence<'a>,
    // `fn(T)` rather than `T`: the handle only consumes attempts, handing
    // each straight to the evidence, and never owns one.
    attempts: PhantomData<fn(T)>,
}

impl<'e, 'a, T: PhaseAttempt> AttemptPhaseHandle<'e, 'a, T> {
    /// Lends `evidence` to a phase its opener has just entered, and planned
    /// unless the phase plans later through its own step.
    pub(super) fn new(evidence: &'e mut MutableRunEvidence<'a>) -> Self {
        Self {
            evidence,
            attempts: PhantomData,
        }
    }

    /// Retains one completed attempt, then publishes the phase's progress
    /// advanced by one succeeded or failed attempt.
    ///
    /// # Errors
    /// A Run Evidence invariant violation, with `T`'s own message, when the
    /// planned total is already complete. The attempt is then not retained,
    /// and the caller must stop work rather than mutate anything further.
    pub(crate) fn record(&mut self, attempt: T) -> Result<(), Error> {
        self.evidence.record_attempt_progress(attempt)
    }

    /// Retains a diagnostic the phase's work observed, leaving its
    /// publication for the run's next publication boundary, as
    /// [`MutableRunEvidence::retain_diagnostic`] does. The handle holds the
    /// evidence exclusively, so the phase's work retains through it.
    pub(crate) fn retain_diagnostic(&mut self, diagnostic: RunDiagnostic) {
        self.evidence.retain_diagnostic(diagnostic);
    }
}

/// The open Archive Finalization phase, which [`MutableRunEvidence`] opens
/// unplanned: its output total is known only once the phase has planned.
pub(crate) type ArchiveFinalizationHandle<'e, 'a> =
    AttemptPhaseHandle<'e, 'a, ArchiveFinalizationAttempt>;

impl ArchiveFinalizationHandle<'_, '_> {
    /// Records the frozen output total, once, before any attempt, and starts
    /// the phase's determinate progress at zero of `total`.
    ///
    /// Planning is a runtime-checked step on the one handle rather than a
    /// change of handle type, so the finalizer keeps the same handle across
    /// a panic that happens after planning.
    ///
    /// # Errors
    /// A Run Evidence invariant violation when the total is already
    /// recorded, or the phase already finished.
    pub(crate) fn plan(&mut self, total: usize) -> Result<(), Error> {
        self.evidence.plan_archive_finalization(total)
    }

    /// Finishes the phase with its phase-level `account`, consuming the
    /// handle. Valid with or without a plan, so cancelled or failed planning
    /// finishes without inventing an output total. The evidence puts the
    /// public Archive Finalization result together from the attempts
    /// already streamed plus `account`; a cancelled account marks
    /// cancellation observed.
    ///
    /// # Errors
    /// A Run Evidence invariant violation when the phase already finished.
    pub(crate) fn finish(self, account: ArchiveFinalizationAccount) -> Result<(), Error> {
        self.evidence.finish_archive_finalization(account)
    }
}
