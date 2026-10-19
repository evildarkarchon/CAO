//! The Run Toolset: the work ports one Optimization Run uses, built by
//! Preparing from the same profile read as its Routing Policy.
//!
//! The configuration provider returns a [`RunToolset`] beside the
//! [`RunConfiguration`](crate::run::RunConfiguration), and the Run Executor
//! runs every work phase through it: Archive discovery and the Capacity Check
//! through the Archive reader and the two probes, then one
//! [`ArchiveExtractionPort`] call per planned Archive, one [`AssetAttemptPort`]
//! call per Routed Asset, and one [`ArchiveFinalizationPort`] call in Apply.
//! Observation goes only through the Run Observation Sink and cancellation
//! only through the run's token.
//!
//! Every port has a production adapter here, and a scripted adapter in
//! `test_support`. The executor contains each port's panics at that port: an
//! Asset attempt's becomes an unsafe Operation Failure with unknown mutation,
//! an extraction's becomes unknown mutation, and Archive Finalization's
//! becomes an Unexpected Exception that forbids continuation.

use std::path::Path;
use std::sync::Arc;

use crate::Error;
use crate::execution::AssetExecutionResult;
use crate::routing::RoutedAsset;
use crate::run::{
    ArchiveExtractionPlan, ArchiveExtractionResult, ArchiveExtractor, ArchiveFinalization,
    ArchiveFinalizationRecorder, ArchiveFinalizationSettings, ArchivePacker, ArchiveReader,
    AssetAttemptAdapter, AssetBackendFactory, CancellationToken, CapacityProbe, RunDiagnostic,
    RunPreparation, SourceCleanup, TemporaryArtifactRegistry, VolumeIdentityProbe,
};

/// Cancellation observed while an Asset backend initialized, before any attempt existed.
///
/// Initialization is read-only, so the run records cancellation without
/// inventing an attempt or a mutation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AssetInitializationCancelled;

/// Executes one Routed Asset against its frozen Mod Root.
///
/// The production adapter is [`AssetAttemptAdapter`]; a panic is contained
/// by the executor as an unsafe Operation Failure with unknown mutation.
pub trait AssetAttemptPort {
    /// Attempts `asset` against its frozen canonical `mod_root`, staging any
    /// output through `artifacts`, the run's Temporary Ownership registry,
    /// which is lent per call. Run Diagnostics handed to `retain` are kept
    /// and published with the run's other deferred diagnostics.
    ///
    /// # Errors
    /// [`AssetInitializationCancelled`] when cancellation was observed while
    /// the backend initialized, before any attempt existed.
    fn attempt(
        &mut self,
        asset: &RoutedAsset,
        mod_root: &Path,
        artifacts: &mut TemporaryArtifactRegistry,
        stop: &CancellationToken,
        retain: &mut dyn FnMut(RunDiagnostic),
    ) -> Result<AssetExecutionResult, AssetInitializationCancelled>;
}

impl<F: AssetBackendFactory> AssetAttemptPort for AssetAttemptAdapter<F> {
    fn attempt(
        &mut self,
        asset: &RoutedAsset,
        mod_root: &Path,
        artifacts: &mut TemporaryArtifactRegistry,
        stop: &CancellationToken,
        retain: &mut dyn FnMut(RunDiagnostic),
    ) -> Result<AssetExecutionResult, AssetInitializationCancelled> {
        AssetAttemptAdapter::attempt(self, asset, mod_root, artifacts, stop, retain)
    }
}

/// Extracts one Archive by its frozen preflight plan.
///
/// The production adapter is [`ArchiveExtractionAdapter`]. The result carries
/// its own mutation fact; a panic is contained by the executor as unknown
/// mutation, unsafe to continue.
pub trait ArchiveExtractionPort {
    /// Extracts the Archive `plan` names, staging every entry through
    /// `artifacts`, the run's Temporary Ownership registry, lent per call.
    fn extract(
        &mut self,
        plan: &ArchiveExtractionPlan,
        artifacts: &mut TemporaryArtifactRegistry,
    ) -> ArchiveExtractionResult;
}

/// Runs the Apply-only Archive Finalization phase.
///
/// The production adapter is [`ArchiveFinalizationAdapter`]. A panic is
/// contained by the executor as an Unexpected Exception that forbids
/// continuation.
pub trait ArchiveFinalizationPort {
    /// Runs the phase over the preparation's Mod Roots, writing its facts
    /// through `recorder` and staging through `artifacts`, which is lent for
    /// the call. `stop` is polled between atomic outputs.
    ///
    /// # Errors
    /// A Run Evidence invariant violation, which the executor raises after
    /// Safety Cleanup. Any other error escapes the run's work and fails the
    /// run at Archive Finalization.
    fn finalize(
        &mut self,
        preparation: &RunPreparation,
        recorder: &ArchiveFinalizationRecorder<'_, '_>,
        artifacts: &mut TemporaryArtifactRegistry,
        stop: &CancellationToken,
    ) -> Result<(), Error>;
}

/// The work ports one Optimization Run uses, built by Preparing from the same
/// profile read as its Routing Policy.
///
/// It is owned by the run for its whole traversal and used only on the Run
/// Worker, so its ports need not be `Send`. Dry Run never uses the extraction
/// or finalization port.
pub struct RunToolset {
    /// Executes each Routed Asset.
    pub asset_attempt: Box<dyn AssetAttemptPort>,
    /// Extracts each planned Archive, in Apply.
    pub archive_extraction: Box<dyn ArchiveExtractionPort>,
    /// Runs Archive Finalization, in Apply.
    pub archive_finalization: Box<dyn ArchiveFinalizationPort>,
    /// Reads Archive manifests during Archive discovery.
    pub archive_reader: Arc<dyn ArchiveReader>,
    /// Samples free space for the Capacity Check.
    pub capacity: Arc<dyn CapacityProbe>,
    /// Groups Mod Roots by volume for the Capacity Check.
    pub volume_identity: Arc<dyn VolumeIdentityProbe>,
}

/// The production Archive extraction port: [`ArchiveExtractor`] over the
/// run's reader and capacity probe, with the configured cleanup of each
/// extracted source Archive.
pub struct ArchiveExtractionAdapter {
    reader: Arc<dyn ArchiveReader>,
    capacity: Arc<dyn CapacityProbe>,
    cleanup: SourceCleanup,
}

impl ArchiveExtractionAdapter {
    /// An adapter extracting through `reader`, rechecking capacity through
    /// `capacity`, then backing up or removing each source as `cleanup` says.
    pub fn new(
        reader: Arc<dyn ArchiveReader>,
        capacity: Arc<dyn CapacityProbe>,
        cleanup: SourceCleanup,
    ) -> Self {
        Self {
            reader,
            capacity,
            cleanup,
        }
    }
}

impl ArchiveExtractionPort for ArchiveExtractionAdapter {
    /// Extracts with source cleanup, logging each success to the Application
    /// Log as C++ `BSAOptimizer::extract` did.
    fn extract(
        &mut self,
        plan: &ArchiveExtractionPlan,
        artifacts: &mut TemporaryArtifactRegistry,
    ) -> ArchiveExtractionResult {
        let result = ArchiveExtractor::new(&*self.reader, &*self.capacity)
            .extract_with_source_cleanup(plan, self.cleanup, artifacts);
        if result.succeeded() {
            log::info!(
                "BSA successfully extracted: {}",
                plan.archive_path.display()
            );
        }
        result
    }
}

/// The production Archive Finalization port: [`ArchiveFinalization`] over
/// the run's packer, reader and probes, its finalization settings and the
/// profile's Packing Exclusions.
pub struct ArchiveFinalizationAdapter {
    packer: Box<dyn ArchivePacker>,
    reader: Arc<dyn ArchiveReader>,
    capacity: Arc<dyn CapacityProbe>,
    volume_identity: Arc<dyn VolumeIdentityProbe>,
    settings: ArchiveFinalizationSettings,
    files_to_not_pack: Vec<String>,
}

impl ArchiveFinalizationAdapter {
    /// An adapter over the phase's seams; see [`ArchiveFinalization::new`]
    /// for what each is used for and how `files_to_not_pack` is matched.
    pub fn new(
        packer: Box<dyn ArchivePacker>,
        reader: Arc<dyn ArchiveReader>,
        capacity: Arc<dyn CapacityProbe>,
        volume_identity: Arc<dyn VolumeIdentityProbe>,
        settings: ArchiveFinalizationSettings,
        files_to_not_pack: Vec<String>,
    ) -> Self {
        Self {
            packer,
            reader,
            capacity,
            volume_identity,
            settings,
            files_to_not_pack,
        }
    }
}

impl ArchiveFinalizationPort for ArchiveFinalizationAdapter {
    /// Runs [`ArchiveFinalization::run`], which contains its own work's
    /// failures and panics as an Unexpected Exception.
    fn finalize(
        &mut self,
        preparation: &RunPreparation,
        recorder: &ArchiveFinalizationRecorder<'_, '_>,
        artifacts: &mut TemporaryArtifactRegistry,
        stop: &CancellationToken,
    ) -> Result<(), Error> {
        ArchiveFinalization::new(
            &*self.packer,
            &*self.reader,
            &*self.capacity,
            &*self.volume_identity,
            self.settings,
            &self.files_to_not_pack,
        )
        .run(preparation, recorder, artifacts, &|| stop.is_cancelled())
    }
}
