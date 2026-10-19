//! Fresh Run Evidence for driving Archive Finalization at its own interface.

use std::cell::RefCell;

use crate::run::{
    ArchiveFinalizationRecorder, ArchiveFinalizationResult, MutableRunEvidence, RunEvidence,
    RunObservationSink, RunPhase, RunPhaseRecord, RunProgress,
};

/// Run Evidence that has already reached the executed Archive Finalization
/// phase, over which a scenario opens the phase's finalization recorder (C++
/// `ArchiveFinalizationEvidenceFixture`).
///
/// It records Preparing and Archive Finalization itself, so a scenario never
/// restates the Run Phase order to reach the phase.
pub struct ArchiveFinalizationEvidence<'s> {
    owner: RefCell<MutableRunEvidence<'s>>,
}

impl Default for ArchiveFinalizationEvidence<'_> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'s> ArchiveFinalizationEvidence<'s> {
    /// Evidence in the executed Archive Finalization phase, publishing nothing.
    pub fn new() -> Self {
        Self::open(None)
    }

    /// Evidence in the executed Archive Finalization phase that publishes
    /// every fact it accepts to `sink`, as a run's evidence does.
    pub fn observed(sink: &'s dyn RunObservationSink) -> Self {
        Self::open(Some(sink))
    }

    /// Records the phases leading into Archive Finalization.
    fn open(sink: Option<&'s dyn RunObservationSink>) -> Self {
        let mut evidence = MutableRunEvidence::new(sink);
        for phase in [RunPhase::Preparing, RunPhase::ArchiveFinalization] {
            evidence
                .record_phase(RunPhaseRecord::executed(phase, None))
                .expect("Preparing then Archive Finalization is a canonical traversal");
        }
        Self {
            owner: RefCell::new(evidence),
        }
    }

    /// The finalization recorder over this evidence. Recorders opened more
    /// than once share the same facts.
    pub fn recorder(&self) -> ArchiveFinalizationRecorder<'_, 's> {
        ArchiveFinalizationRecorder::new(&self.owner)
    }

    /// The retained attempts and result, or `None` before the phase recorded
    /// an output total or a result.
    pub fn recorded(&self) -> Option<ArchiveFinalizationResult> {
        self.owner.borrow().archive_finalization().cloned()
    }

    /// The retained result.
    ///
    /// # Panics
    /// If the phase has recorded nothing yet.
    pub fn finalization(&self) -> ArchiveFinalizationResult {
        self.recorded().expect("the phase recorded a result")
    }

    /// The phase's latest progress, or `None` before an output total exists.
    pub fn progress(&self) -> Option<RunProgress> {
        self.owner
            .borrow()
            .phase(RunPhase::ArchiveFinalization)
            .and_then(RunPhaseRecord::progress)
    }

    /// Moves the evidence on to Safety Cleanup, out of the phase, so any
    /// later recorder operation is a Run Evidence invariant violation.
    pub fn leave_phase(&self) {
        self.owner
            .borrow_mut()
            .record_phase(RunPhaseRecord::executed(RunPhase::SafetyCleanup, None))
            .expect("Safety Cleanup follows Archive Finalization");
    }

    /// Performs the executor's mandatory Safety Cleanup and seals the facts.
    pub fn seal(self) -> RunEvidence {
        self.leave_phase();
        self.owner
            .into_inner()
            .consume()
            .expect("the evidence seals after Safety Cleanup")
    }
}
