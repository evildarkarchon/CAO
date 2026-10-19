//! The production Asset attempt adapter: one Routed Asset's whole attempt.
//!
//! [`AssetAttemptAdapter`] owns what every caller once sequenced by hand. It
//! creates the Asset Execution Backend lazily through an
//! [`AssetBackendFactory`] on the first Routed Asset, so a run with nothing to
//! process sets up no optimizer and the factory runs after Archive extraction.
//! For each Routed Asset it then runs, in order, the [`AssetExecutor`], the
//! backend's [`attempt_finished`](AssetExecutionBackend::attempt_finished)
//! notification and [`quarantine_failed_load`], so the ordering rule between
//! the last two lives in one place.
//!
//! The adapter contains a factory error as an unsafe Operation Failure with
//! unknown mutation; it does not catch panics, which the run contains at its
//! Asset attempt boundary.

use std::path::Path;

use crate::execution::{
    AssetExecutionBackend, AssetExecutionFailure, AssetExecutionResult, AssetExecutor,
    MutationState, quarantine_failed_load,
};
use crate::routing::RoutedAsset;
use crate::run::{
    AssetInitializationCancelled, CancellationToken, RunDiagnostic, TemporaryArtifactRegistry,
};

/// What an [`AssetBackendFactory`] produced when it was not stopped by an error.
pub enum BackendCreation<B> {
    /// The backend, with the Run Diagnostics its creation observed (such as an
    /// unreadable plugin), for the caller to retain.
    Created {
        backend: B,
        diagnostics: Vec<RunDiagnostic>,
    },
    /// Cancellation was observed while the backend initialized, before any
    /// attempt existed.
    Cancelled,
}

/// Why an [`AssetBackendFactory`] could not create its backend, such as COM
/// failing to initialize. Its text becomes the attempt's failure message.
pub type BackendCreationError = Box<dyn std::error::Error>;

/// Creates the Asset Execution Backend of one run, on the Run Worker.
///
/// The adapter calls it lazily, on the first Routed Asset. The two adapters
/// are the production optimizer backend in `cao-optimizers` and the fake
/// backend of `test-support`.
pub trait AssetBackendFactory {
    /// The backend this factory creates.
    type Backend: AssetExecutionBackend;

    /// Creates the run's backend, observing `stop` during any lengthy
    /// initialization.
    ///
    /// # Errors
    /// A [`BackendCreationError`] when no backend can be created; the adapter
    /// contains it as an unsafe Operation Failure.
    fn create(
        &mut self,
        stop: &CancellationToken,
    ) -> Result<BackendCreation<Self::Backend>, BackendCreationError>;
}

/// Owns one run's Asset attempts: backend creation, execution, the attempt
/// finished notification and Quarantine.
pub struct AssetAttemptAdapter<F: AssetBackendFactory> {
    factory: F,
    /// Created on the first attempt; `None` until then.
    backend: Option<F::Backend>,
}

impl<F: AssetBackendFactory> AssetAttemptAdapter<F> {
    /// An adapter that creates its backend through `factory` on its first attempt.
    pub fn new(factory: F) -> Self {
        Self {
            factory,
            backend: None,
        }
    }

    /// Attempts one Routed Asset against its frozen canonical `mod_root`,
    /// staging any output in `artifacts`, which is lent per call.
    ///
    /// The first call creates the backend; any Run Diagnostics the factory
    /// returns are handed to `retain`, once.
    ///
    /// # Errors
    /// [`AssetInitializationCancelled`] when the factory observed
    /// cancellation; no attempt exists then.
    pub fn attempt(
        &mut self,
        asset: &RoutedAsset,
        mod_root: &Path,
        artifacts: &mut TemporaryArtifactRegistry,
        stop: &CancellationToken,
        retain: &mut dyn FnMut(RunDiagnostic),
    ) -> Result<AssetExecutionResult, AssetInitializationCancelled> {
        let backend = match &mut self.backend {
            Some(backend) => backend,
            empty => match self.factory.create(stop) {
                Ok(BackendCreation::Created {
                    backend,
                    diagnostics,
                }) => {
                    diagnostics.into_iter().for_each(&mut *retain);
                    empty.insert(backend)
                }
                Ok(BackendCreation::Cancelled) => return Err(AssetInitializationCancelled),
                // Without a backend the Asset was never loaded, so there is
                // nothing to notify or quarantine. The failure cannot say what
                // initialization touched, so it is unknown mutation that stops
                // the run: the outcome the run gave the panic that signalled
                // this failure before the factory returned errors.
                Err(error) => {
                    return Ok(AssetExecutionResult::failed(
                        AssetExecutionFailure::BackendException,
                        error.to_string(),
                    )
                    .with_mutation(MutationState::PartialOrUnknown)
                    .with_safe_to_continue(false)
                    .with_path(asset.execution_path()));
                }
            },
        };
        let result = AssetExecutor::new(backend).execute(asset, artifacts, mod_root);
        // Notified before Quarantine, as C++ `finishAttempt` recorded the
        // attempt, so a failed Texture conversion is judged by the executor's
        // own result rather than by Quarantine's.
        backend.attempt_finished(asset, &result);
        Ok(quarantine_failed_load(asset, result))
    }
}
