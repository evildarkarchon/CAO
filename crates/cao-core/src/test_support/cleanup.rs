//! Safety Cleanup Service fakes.

use crate::Error;
use crate::run::{CancellationToken, RunFailure, SafetyCleanupService};

/// A Safety Cleanup Service that counts its passes and returns scripted failures.
#[derive(Default)]
pub struct CountingCleanup {
    pub passes: usize,
    pub failures: Vec<RunFailure>,
    pub service_error: Option<String>,
}

impl SafetyCleanupService for CountingCleanup {
    fn perform_safety_cleanup(&mut self) -> Result<Vec<RunFailure>, Error> {
        self.passes += 1;
        match &self.service_error {
            Some(error) => Err(Error::SafetyCleanup(error.clone())),
            None => Ok(self.failures.clone()),
        }
    }
}

/// A Safety Cleanup Service that may cancel the run during its pass, then
/// returns scripted failures or panics.
#[derive(Default)]
pub struct ScriptedCleanup {
    /// Cancelled during the pass, as a user clicking Cancel during cleanup would.
    pub cancels: Option<CancellationToken>,
    pub failures: Vec<RunFailure>,
    pub panics: bool,
    pub passes: usize,
}

impl SafetyCleanupService for ScriptedCleanup {
    fn perform_safety_cleanup(&mut self) -> Result<Vec<RunFailure>, Error> {
        self.passes += 1;
        if let Some(token) = &self.cancels {
            token.cancel();
        }
        if self.panics {
            panic!("the cleanup service panicked");
        }
        Ok(self.failures.clone())
    }
}
