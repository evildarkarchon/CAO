//! The Run Toolset: the Asset attempt port and Archive seams one Optimization
//! Run's work goes through, built by Preparing from the same profile read as
//! its Routing Policy.
//!
//! The configuration provider returns a [`RunToolset`] beside the
//! [`SelectedProfile`](crate::run::SelectedProfile), built through
//! [`RunToolset::new`] from the Asset attempt port, the run's
//! [`ArchiveSeams`] and its [`ArchiveWorkSettings`]. Core assembles Archive
//! extraction and Archive Finalization over those seams, so one reader serves
//! discovery, extraction and finalization by construction.
//!
//! The Run Executor runs every work phase through it: Archive discovery and
//! the Capacity Check through the Archive reader and the two probes, then one
//! [`ArchiveExtractionPort`] call per planned Archive, one [`AssetAttemptPort`]
//! call per Routed Asset, and one [`ArchiveFinalizationPort`] call in Apply.
//! Observation goes only through the Run Observation Sink and cancellation
//! only through the run's token.
//!
//! The extraction and finalization ports are crate-private: the constructor
//! fills them with core's own adapters, and only `test_support` still
//! installs scripted ones until the scripted runs migrate onto the seams
//! (#581). The executor contains each port's panics at that port: an Asset
//! attempt's becomes an unsafe Operation Failure with unknown mutation, an
//! extraction's becomes unknown mutation, and Archive Finalization's becomes
//! an Unexpected Exception that forbids continuation.

use std::path::Path;
use std::sync::Arc;

use crate::Error;
use crate::execution::AssetExecutionResult;
use crate::routing::RoutedAsset;
use crate::run::{
    ArchiveExtractionPlan, ArchiveExtractionResult, ArchiveExtractor, ArchiveFinalization,
    ArchiveFinalizationRecorder, ArchiveFinalizationSettings, ArchivePacker, ArchiveReader,
    CancellationToken, CapacityProbe, RunDiagnostic, RunPreparation, SourceCleanup,
    TemporaryArtifactRegistry, VolumeIdentityProbe,
};

/// Cancellation observed while an Asset backend initialized, before any attempt existed.
///
/// Initialization is read-only, so the run records cancellation without
/// inventing an attempt or a mutation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AssetInitializationCancelled;

/// Executes one Routed Asset against its frozen Mod Root.
///
/// The production adapter is
/// [`AssetAttemptAdapter`](crate::run::AssetAttemptAdapter); a panic is contained
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

/// Extracts one Archive by its frozen preflight plan.
///
/// The production adapter is [`ArchiveExtractionAdapter`]. The result carries
/// its own mutation fact; a panic is contained by the executor as unknown
/// mutation, unsafe to continue.
pub(crate) trait ArchiveExtractionPort {
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
pub(crate) trait ArchiveFinalizationPort {
    /// Runs the phase over the preparation's Mod Roots, writing its facts
    /// through `recorder` and staging through `artifacts`, which is lent for
    /// the call. `stop` is polled between atomic outputs.
    ///
    /// # Errors
    /// Only a Run Evidence invariant violation, which the executor raises
    /// after Safety Cleanup. Every other failure of the phase is recorded
    /// through `recorder` as the phase's result.
    fn finalize(
        &mut self,
        preparation: &RunPreparation,
        recorder: &mut ArchiveFinalizationRecorder<'_, '_>,
        artifacts: &mut TemporaryArtifactRegistry,
        stop: &CancellationToken,
    ) -> Result<(), Error>;
}

/// The Archive seams one Optimization Run's Archive work goes through: the
/// lower adapters core assembles extraction and Archive Finalization over.
///
/// The one `reader` serves Archive discovery, extraction and Archive
/// Finalization for the whole run.
pub struct ArchiveSeams {
    /// Reads Archive manifests and entries, and verifies packed Archives.
    pub reader: Arc<dyn ArchiveReader>,
    /// Samples free space for the Capacity Check and Archive Finalization.
    pub capacity: Arc<dyn CapacityProbe>,
    /// Groups Mod Roots by volume for the Capacity Check and Archive
    /// Finalization.
    pub volume_identity: Arc<dyn VolumeIdentityProbe>,
    /// Packs Loose Assets into Archives during Archive Finalization.
    pub packer: Arc<dyn ArchivePacker>,
}

/// The run's Archive work choices, captured from its options before
/// scheduling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArchiveWorkSettings {
    /// What becomes of each source Archive once it is extracted.
    pub source_cleanup: SourceCleanup,
    /// How Archive Finalization packs and maintains Loading Plugins.
    pub finalization: ArchiveFinalizationSettings,
}

impl Default for ArchiveWorkSettings {
    /// The options model's defaults: extracted sources are backed up, and
    /// Archive Finalization uses its default settings.
    fn default() -> Self {
        Self {
            source_cleanup: SourceCleanup::Backup,
            finalization: ArchiveFinalizationSettings::default(),
        }
    }
}

/// The Asset attempt port and Archive seams one Optimization Run's work goes
/// through, built by Preparing from the same profile read as its Routing
/// Policy.
///
/// It is owned by the run for its whole traversal and used only on the Run
/// Worker, so its ports need not be `Send`. Dry Run never uses the extraction
/// or finalization port.
///
/// Its fields are visible only within the crate, so that `test_support` can
/// still install scripted Archive ports while scripted runs migrate onto the
/// seams (#581); every other Toolset is built by [`RunToolset::new`].
pub struct RunToolset {
    /// Executes each Routed Asset.
    pub(crate) asset_attempt: Box<dyn AssetAttemptPort>,
    /// Extracts each planned Archive, in Apply.
    pub(crate) archive_extraction: Box<dyn ArchiveExtractionPort>,
    /// Runs Archive Finalization, in Apply.
    pub(crate) archive_finalization: Box<dyn ArchiveFinalizationPort>,
    /// Reads Archive manifests during Archive discovery.
    pub(crate) archive_reader: Arc<dyn ArchiveReader>,
    /// Samples free space for the Capacity Check.
    pub(crate) capacity: Arc<dyn CapacityProbe>,
    /// Groups Mod Roots by volume for the Capacity Check.
    pub(crate) volume_identity: Arc<dyn VolumeIdentityProbe>,
}

impl RunToolset {
    /// A Run Toolset whose Asset attempts go through `asset_attempt` and
    /// whose Archive work core assembles over `seams` with `settings`.
    ///
    /// Archive extraction is the core [`ArchiveExtractor`] over the seams'
    /// reader and capacity probe with `settings.source_cleanup`, logging each
    /// success to the Application Log. Archive Finalization is
    /// [`ArchiveFinalization`] over the packer, reader and probes with
    /// `settings.finalization`. The seams' single reader therefore serves
    /// Archive discovery, extraction and finalization alike.
    pub fn new(
        asset_attempt: Box<dyn AssetAttemptPort>,
        seams: ArchiveSeams,
        settings: ArchiveWorkSettings,
    ) -> Self {
        let ArchiveSeams {
            reader,
            capacity,
            volume_identity,
            packer,
        } = seams;
        Self {
            asset_attempt,
            archive_extraction: Box::new(ArchiveExtractionAdapter::new(
                reader.clone(),
                capacity.clone(),
                settings.source_cleanup,
            )),
            archive_finalization: Box::new(ArchiveFinalizationAdapter::new(
                packer,
                reader.clone(),
                capacity.clone(),
                volume_identity.clone(),
                settings.finalization,
            )),
            archive_reader: reader,
            capacity,
            volume_identity,
        }
    }
}

/// The production Archive extraction port: [`ArchiveExtractor`] over the
/// run's reader and capacity probe, with the configured cleanup of each
/// extracted source Archive.
pub(crate) struct ArchiveExtractionAdapter {
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
/// the run's packer, reader and probes and its finalization settings. The
/// phase reads the profile's Packing Exclusions from the Selected Profile in
/// the [`RunPreparation`] it finalizes.
pub(crate) struct ArchiveFinalizationAdapter {
    packer: Arc<dyn ArchivePacker>,
    reader: Arc<dyn ArchiveReader>,
    capacity: Arc<dyn CapacityProbe>,
    volume_identity: Arc<dyn VolumeIdentityProbe>,
    settings: ArchiveFinalizationSettings,
}

impl ArchiveFinalizationAdapter {
    /// An adapter over the phase's seams; see [`ArchiveFinalization::new`]
    /// for what each is used for.
    pub fn new(
        packer: Arc<dyn ArchivePacker>,
        reader: Arc<dyn ArchiveReader>,
        capacity: Arc<dyn CapacityProbe>,
        volume_identity: Arc<dyn VolumeIdentityProbe>,
        settings: ArchiveFinalizationSettings,
    ) -> Self {
        Self {
            packer,
            reader,
            capacity,
            volume_identity,
            settings,
        }
    }
}

impl ArchiveFinalizationPort for ArchiveFinalizationAdapter {
    /// Runs [`ArchiveFinalization::run`], which contains its own work's
    /// failures and panics as an Unexpected Exception.
    fn finalize(
        &mut self,
        preparation: &RunPreparation,
        recorder: &mut ArchiveFinalizationRecorder<'_, '_>,
        artifacts: &mut TemporaryArtifactRegistry,
        stop: &CancellationToken,
    ) -> Result<(), Error> {
        ArchiveFinalization::new(
            &*self.packer,
            &*self.reader,
            &*self.capacity,
            &*self.volume_identity,
            self.settings,
        )
        .run(preparation, recorder, artifacts, &|| stop.is_cancelled())
    }
}
