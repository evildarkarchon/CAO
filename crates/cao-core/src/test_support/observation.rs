//! Observation fakes: the Run Event log and the recording Run Observation Sink.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::run::{
    RunDiagnostic, RunEvent, RunEventDispatcher, RunEventPayload, RunFailure, RunObservationSink,
    RunPhase, RunPhaseRecord,
};

/// The events one run dispatched, shared with its dispatcher.
#[derive(Clone, Default)]
pub struct EventLog(pub Arc<Mutex<Vec<RunEvent>>>);

impl EventLog {
    /// A dispatcher appending every event here.
    pub fn dispatcher(&self) -> RunEventDispatcher {
        let events = Arc::clone(&self.0);
        Box::new(move |event| events.lock().unwrap().push(event))
    }

    /// Every event dispatched so far, in dispatch order.
    pub fn events(&self) -> Vec<RunEvent> {
        self.0.lock().unwrap().clone()
    }

    /// The phases of every phase event, in dispatch order.
    pub fn phases(&self) -> Vec<RunPhase> {
        self.events()
            .into_iter()
            .filter_map(|event| match event.payload {
                RunEventPayload::Phase(record) => Some(record.phase()),
                _ => None,
            })
            .collect()
    }
}

/// A hook a [`RecordingSink`] runs on each observation.
pub type ObservationHook = Box<dyn Fn(&Observed)>;

/// One observation a [`RecordingSink`] received.
#[derive(Debug, Clone, PartialEq)]
pub enum Observed {
    Phase(RunPhaseRecord),
    Failure(RunFailure),
    Diagnostic(RunDiagnostic),
}

impl Observed {
    /// The observation's kind: `phase`, `failure` or `diagnostic`.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Phase(_) => "phase",
            Self::Failure(_) => "failure",
            Self::Diagnostic(_) => "diagnostic",
        }
    }
}

/// A Run Observation Sink that records every observation in order and runs
/// an optional hook first, which may cancel the run or panic.
#[derive(Default)]
pub struct RecordingSink {
    pub observed: Mutex<Vec<Observed>>,
    pub hook: Option<ObservationHook>,
}

impl RecordingSink {
    /// A sink running `hook` on each observation.
    pub fn with_hook(hook: impl Fn(&Observed) + 'static) -> Self {
        Self {
            observed: Mutex::default(),
            hook: Some(Box::new(hook)),
        }
    }

    /// A sink running `hook` on the first phase record of `phase`; a phase's
    /// record fires again with every progress update, so later ones are ignored.
    pub fn on_first_record_of(phase: RunPhase, hook: impl Fn() + 'static) -> Self {
        let fired = AtomicBool::new(false);
        Self::with_hook(move |observed| {
            if let Observed::Phase(record) = observed
                && record.phase() == phase
                && !fired.swap(true, Ordering::SeqCst)
            {
                hook();
            }
        })
    }

    /// Every observation received so far, in order.
    pub fn observed(&self) -> Vec<Observed> {
        self.observed.lock().unwrap().clone()
    }

    /// Each phase in the order this sink first observed it; a phase's
    /// repeated progress records count once.
    pub fn phase_order(&self) -> Vec<RunPhase> {
        let mut phases: Vec<RunPhase> = Vec::new();
        for observation in self.observed() {
            if let Observed::Phase(record) = observation
                && phases.last() != Some(&record.phase())
            {
                phases.push(record.phase());
            }
        }
        phases
    }

    fn observe(&self, observation: Observed) {
        self.observed.lock().unwrap().push(observation.clone());
        if let Some(hook) = &self.hook {
            hook(&observation);
        }
    }
}

impl RunObservationSink for RecordingSink {
    fn record_phase(&self, phase: &RunPhaseRecord) {
        self.observe(Observed::Phase(*phase));
    }

    fn record_failure(&self, failure: &RunFailure) {
        self.observe(Observed::Failure(failure.clone()));
    }

    fn record_diagnostic(&self, diagnostic: &RunDiagnostic) {
        self.observe(Observed::Diagnostic(diagnostic.clone()));
    }
}
