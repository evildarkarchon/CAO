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
//! The Run Executor runs every work phase over it: Archive discovery and the
//! Capacity Check through the Archive reader and the two probes, then, in
//! Apply, the core [`ArchiveExtractor`](crate::run::ArchiveExtractor) over the
//! reader and capacity probe for each planned Archive, one
//! [`AssetAttemptPort`] call per Routed Asset, and, in Apply,
//! [`ArchiveFinalization`](crate::run::ArchiveFinalization) over the packer,
//! reader and probes. Observation goes only through the Run Observation Sink
//! and cancellation only through the run's token.
//!
//! Each panic is contained once, by the module that owns its rule: the
//! executor turns an Asset attempt's into an unsafe Operation Failure with
//! unknown mutation, the Archive extractor classifies its own by how far the
//! attempt got, and Archive Finalization turns its own into an Unexpected
//! Exception that forbids continuation. The Capacity Check owns the probes'
//! panics wherever it runs, in the preflight, the extraction recheck and
//! Archive Finalization: a probe that cannot answer, even by panicking, gives
//! an unknown answer, which never fails the check, so a probe panic is never
//! Run Work Escaped, an extraction failure or an Unexpected Exception.

use std::path::Path;
use std::sync::Arc;

use crate::Error;
use crate::execution::AssetExecutionResult;
use crate::routing::RoutedAsset;
use crate::run::archives::{Preflight, RootArchives, preflight};
use crate::run::{
    ArchiveExtractionPlan, ArchiveExtractionResult, ArchiveExtractor, ArchiveFinalization,
    ArchiveFinalizationRecorder, ArchiveFinalizationSettings, ArchivePacker, ArchivePrecedence,
    ArchiveReader, CancellationToken, CapacityProbe, RunDiagnostic, RunPreparation, SourceCleanup,
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

/// The Archive seams one Optimization Run's Archive work goes through: the
/// lower seams core assembles extraction and Archive Finalization over.
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
/// Worker, so its ports need not be `Send`. Dry Run never touches the packer,
/// extraction or Archive Finalization.
///
/// Its fields are private: every Toolset is built by [`RunToolset::new`],
/// so its single reader serves Archive discovery, extraction and Archive
/// Finalization by construction.
pub struct RunToolset {
    /// Executes each Routed Asset.
    asset_attempt: Box<dyn AssetAttemptPort>,
    /// The seams core assembles Archive discovery, extraction and Archive
    /// Finalization over.
    seams: ArchiveSeams,
    /// The run's source cleanup and Archive Finalization settings.
    settings: ArchiveWorkSettings,
}

impl RunToolset {
    /// A Run Toolset whose Asset attempts go through `asset_attempt` and
    /// whose Archive work core assembles over `seams` with `settings`.
    ///
    /// The Run Executor extracts each planned Archive with the core
    /// [`ArchiveExtractor`](crate::run::ArchiveExtractor) over the seams'
    /// reader and capacity probe with `settings.source_cleanup`, logging each
    /// success to the Application Log, and runs
    /// [`ArchiveFinalization`](crate::run::ArchiveFinalization) over the
    /// packer, reader and probes with `settings.finalization`. The seams'
    /// single reader therefore serves Archive discovery, extraction and
    /// finalization alike.
    pub fn new(
        asset_attempt: Box<dyn AssetAttemptPort>,
        seams: ArchiveSeams,
        settings: ArchiveWorkSettings,
    ) -> Self {
        Self {
            asset_attempt,
            seams,
            settings,
        }
    }

    /// The port each Routed Asset is attempted through.
    pub(crate) fn asset_attempt(&mut self) -> &mut dyn AssetAttemptPort {
        &mut *self.asset_attempt
    }

    /// Runs Archive discovery's all-or-nothing preflight over `roots` through
    /// the seams' reader, capacity probe and volume identity probe.
    ///
    /// Nothing on disk changes; see [`preflight`] for what is checked. A
    /// probe that panics gives the Capacity Check an unknown answer rather
    /// than escaping the run's work.
    pub(crate) fn preflight_archives(
        &self,
        roots: Vec<RootArchives>,
        precedence: &ArchivePrecedence,
        stop: &CancellationToken,
    ) -> Preflight {
        preflight(
            roots,
            precedence,
            &*self.seams.reader,
            &*self.seams.capacity,
            &*self.seams.volume_identity,
            stop,
        )
    }

    /// Extracts one planned Archive with the core [`ArchiveExtractor`] over
    /// the seams' reader and capacity probe, then cleans up its source with
    /// the run's [`SourceCleanup`], logging a success to the Application Log.
    ///
    /// Never fails outright and never panics: the extractor contains its own
    /// panics and classifies every failure by how far the attempt got.
    pub(crate) fn extract_archive(
        &self,
        plan: &ArchiveExtractionPlan,
        artifacts: &mut TemporaryArtifactRegistry,
    ) -> ArchiveExtractionResult {
        let attempt = ArchiveExtractor::new(&*self.seams.reader, &*self.seams.capacity)
            .extract_with_source_cleanup(plan, self.settings.source_cleanup, artifacts);
        if attempt.succeeded() {
            // The Application Log line C++ `BSAOptimizer::extract` wrote. It
            // is logged from this module so its module path, which the debug
            // Application Log layout prints, stays `cao_core::run::toolset`.
            log::info!(
                "BSA successfully extracted: {}",
                plan.archive_path.display()
            );
        }
        attempt
    }

    /// Runs [`ArchiveFinalization`] over the seams' packer, reader and probes
    /// with the run's finalization settings, recording through `recorder`.
    ///
    /// Archive Finalization contains its own panics as an Unexpected
    /// Exception, so callers need no second layer.
    ///
    /// # Errors
    /// As [`ArchiveFinalization::run`]: a Run Evidence invariant violation.
    pub(crate) fn finalize_archives(
        &self,
        preparation: &RunPreparation,
        recorder: &mut ArchiveFinalizationRecorder<'_, '_>,
        artifacts: &mut TemporaryArtifactRegistry,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), Error> {
        ArchiveFinalization::new(
            &*self.seams.packer,
            &*self.seams.reader,
            &*self.seams.capacity,
            &*self.seams.volume_identity,
            self.settings.finalization,
        )
        .run(preparation, recorder, artifacts, cancelled)
    }
}
