//! Observation fakes: the Run Event log and the recording Run Observation Sink.

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

    /// Every observation received so far, in order.
    pub fn observed(&self) -> Vec<Observed> {
        self.observed.lock().unwrap().clone()
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
