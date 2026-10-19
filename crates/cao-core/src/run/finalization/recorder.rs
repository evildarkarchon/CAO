//! The finalization recorder: the only way Archive Finalization writes Run
//! Evidence.

use std::cell::RefCell;

use crate::Error;
use crate::run::{
    ArchiveFinalizationAttempt, ArchiveFinalizationFailure, ArchiveFinalizationResult,
    MutableRunEvidence,
};

/// Records Archive Finalization's facts into the run's evidence: its output
/// total, each completed attempt, and its one complete result.
///
/// It reaches no other part of the run's evidence. Each fact is retained
/// before it is published, and the output total and every attempt publish
/// the phase's determinate progress through the run's observation sink. The
/// run's evidence must already be in the executed Archive Finalization phase;
/// anything else is a Run Evidence invariant violation.
pub struct ArchiveFinalizationRecorder<'e, 'a> {
    evidence: &'e RefCell<MutableRunEvidence<'a>>,
}

impl<'e, 'a> ArchiveFinalizationRecorder<'e, 'a> {
    /// A recorder over the run's evidence owner. Crate-private, so only the
    /// run and `test_support` can open one.
    pub(crate) fn new(evidence: &'e RefCell<MutableRunEvidence<'a>>) -> Self {
        Self { evidence }
    }

    /// Records the frozen output total, once, before any attempt, and starts
    /// the phase's determinate progress at zero of `total`.
    ///
    /// # Errors
    /// A Run Evidence invariant violation: the phase is not the current,
    /// executed one, or its total is already recorded.
    pub fn record_output_total(&self, total: usize) -> Result<(), Error> {
        self.evidence
            .borrow_mut()
            .record_archive_finalization_plan(total)
    }

    /// Retains one completed output attempt before the next output starts,
    /// and advances progress against the recorded output total.
    ///
    /// # Errors
    /// A Run Evidence invariant violation: no output total is recorded, the
    /// result is already recorded, or the attempts would exceed the total.
    pub fn record_attempt(&self, attempt: ArchiveFinalizationAttempt) -> Result<(), Error> {
        let mut evidence = self.evidence.borrow_mut();
        // Evidence rejects an attempt without a total; zero only gives that
        // rejection something to compare against.
        let total = evidence.archive_finalization_total().unwrap_or(0);
        evidence.record_archive_finalization_attempt(attempt, total)
    }

    /// Retains the phase's one complete result. Its attempts must begin with
    /// every attempt already recorded, in order; any it adds are recorded as
    /// attempts first. A result without an output total, because planning
    /// failed or was cancelled, must have no attempts.
    ///
    /// # Errors
    /// A Run Evidence invariant violation: the phase is not the current,
    /// executed one, a result is already recorded, or the attempts disagree
    /// with those recorded.
    pub fn record_result(&self, result: ArchiveFinalizationResult) -> Result<(), Error> {
        self.evidence
            .borrow_mut()
            .record_archive_finalization(result)
    }

    /// Records the phase's Unexpected Exception after its port panicked with
    /// `detail`: a result keeping every attempt already recorded, which
    /// forbids continuation.
    ///
    /// # Errors
    /// [`Error::RunWorkEscaped`] when the phase had already recorded its result,
    /// so the panic has no result left to fail and escapes the run's work
    /// instead; or a Run Evidence invariant violation.
    pub(crate) fn record_unexpected_exception(&self, detail: String) -> Result<(), Error> {
        let mut evidence = self.evidence.borrow_mut();
        if evidence.archive_finalization_completed() {
            return Err(Error::RunWorkEscaped(format!(
                "Archive Finalization panicked after recording its result: {detail}"
            )));
        }
        let attempts = evidence
            .archive_finalization()
            .map(|recorded| recorded.attempts.clone())
            .unwrap_or_default();
        evidence.record_archive_finalization(ArchiveFinalizationResult {
            attempts,
            failure: Some(ArchiveFinalizationFailure::UnexpectedException),
            safe_to_continue: false,
            detail,
            ..ArchiveFinalizationResult::default()
        })
    }
}
