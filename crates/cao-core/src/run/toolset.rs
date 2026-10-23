//! The Run Toolset: the Asset attempt port and Archive seams one Optimization
//! Run's work goes through, built by Preparing from the same profile read as
//! its Routing Policy.
//!
//! The configuration provider returns a [`RunToolset`] beside the
//! [`SelectedProfile`](crate::run::SelectedProfile), built through
//! [`RunToolset::new`] from the Asset attempt port and the run's
//! [`ArchiveSeams`]. Core runs Archive extraction and Archive Finalization
//! over those seams, so one reader serves discovery, extraction and
//! finalization by construction. The Toolset carries no Work Choices: the
//! Run Executor hands extraction and Archive Finalization their choices from
//! the run's Run Request.
//!
//! The Toolset is the only way to the Archive work. The Run Executor reaches
//! it through crate-private steps, in phase order: Archive discovery's
//! preflight ([`RunToolset::preflight_archives`]), extracting one planned
//! Archive ([`RunToolset::extract_archive`]) and Archive Finalization
//! ([`RunToolset::finalize_archives`]), or, in a Dry Run, its planning-only
//! step ([`RunToolset::plan_archive_creation`]). Each calls a crate-private
//! work function over the seams and the run's Capacity Check. The executor
//! keeps every Run Evidence write except Archive Finalization's, which writes
//! only its own phase, through the phase handle the executor opens and hands
//! it; the planning-only step writes none.
//!
//! The Run Executor runs every work phase over it: Archive discovery and the
//! Capacity Check through the Archive reader and the volume probe, then, in
//! Apply, Archive extraction over the reader and the Capacity Check for each
//! planned Archive, one [`AssetAttemptPort`] call per Routed Asset, and, in
//! Apply, Archive Finalization over the packer, reader and the Capacity Check,
//! or, in Dry Run, the planning of its outputs over the packer's planning
//! seams.
//! Observation goes only through the Run Observation Sink and cancellation
//! only through the run's token.
//!
//! The Toolset holds the run's one Capacity Check state: each Mod Root's
//! volume identity is looked up the first time any of those checks asks for
//! it and frozen for the rest of the run, so preflight, extraction and
//! Archive Finalization agree on volume grouping.
//!
//! Each panic is contained once, by the module that owns its rule: the
//! executor turns an Asset attempt's into an unsafe Operation Failure with
//! unknown mutation, Archive extraction classifies its own by how far the
//! attempt got, and Archive Finalization turns its own into an Unexpected
//! Exception that forbids continuation. The Capacity Check owns the volume
//! probe's panics wherever it runs, in the preflight, the extraction recheck and
//! Archive Finalization: a probe that cannot answer, even by panicking, gives
//! an unknown answer, which never fails the check, so a probe panic is never
//! Run Work Escaped, an extraction failure or an Unexpected Exception.

use std::path::Path;
use std::sync::Arc;

use crate::Error;
use crate::execution::AssetExecutionResult;
use crate::routing::RoutedAsset;
use crate::run::archives::{self, Preflight, RootArchives, preflight};
use crate::run::capacity::{CapacityCheck, FrozenVolumes};
use crate::run::finalization::{self, CreationPlanning};
use crate::run::{
    ArchiveExtractionPlan, ArchiveExtractionResult, ArchiveFinalizationChoices,
    ArchiveFinalizationHandle, ArchivePacker, ArchivePrecedence, ArchiveReader, CancellationToken,
    RunDiagnostic, RunPreparation, SourceCleanup, TemporaryArtifactRegistry, VolumeProbe,
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
    /// Samples free space and groups Mod Roots by volume for the Capacity
    /// Check, in the preflight, extraction and Archive Finalization.
    ///
    /// The run's Capacity Check is its only caller in core: no Archive work
    /// samples it directly, so every check groups Mod Roots by the volume
    /// identities the run froze.
    pub volume_probe: Arc<dyn VolumeProbe>,
    /// Packs Loose Assets into Archives during Archive Finalization.
    pub packer: Arc<dyn ArchivePacker>,
}

/// The Asset attempt port and Archive seams one Optimization Run's work goes
/// through, built by Preparing from the same profile read as its Routing
/// Policy.
///
/// It is owned by the run for its whole traversal and used only on the Run
/// Worker, so its ports need not be `Send`. Dry Run never touches extraction,
/// and reaches the packer only through its planning seams (`rules`,
/// `list_names` and `partition`), never `write` or a commit.
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
    /// The volume identities every Capacity Check of the run shares, so each
    /// Mod Root's volume is looked up at most once per run.
    volumes: FrozenVolumes,
}

impl RunToolset {
    /// A Run Toolset whose Asset attempts go through `asset_attempt` and
    /// whose Archive work core assembles over `seams`.
    ///
    /// The Run Executor extracts each planned Archive over the seams' reader
    /// and the run's Capacity Check with the Run Request's Archive extraction
    /// choice, logging each success to the Application Log, and runs Archive
    /// Finalization over the packer, reader and the run's Capacity Check with
    /// the Run Request's Archive Finalization choices. The seams' single
    /// reader therefore serves Archive discovery, extraction and finalization
    /// alike.
    pub fn new(asset_attempt: Box<dyn AssetAttemptPort>, seams: ArchiveSeams) -> Self {
        Self {
            asset_attempt,
            seams,
            volumes: FrozenVolumes::default(),
        }
    }

    /// The port each Routed Asset is attempted through.
    pub(crate) fn asset_attempt(&mut self) -> &mut dyn AssetAttemptPort {
        &mut *self.asset_attempt
    }

    /// The run's Capacity Check: the seams' volume probe over the volume
    /// identities the whole run shares.
    ///
    /// A check borrows its probe, and the Toolset owns the probe through the
    /// seams' `Arc`, so it cannot also hold a check borrowing from itself.
    /// It holds the frozen volumes instead and hands each phase a check over
    /// them; every such check therefore behaves as the run's one check.
    fn capacity_check(&self) -> CapacityCheck<'_> {
        CapacityCheck::sharing(&*self.seams.volume_probe, &self.volumes)
    }

    /// Runs Archive discovery's all-or-nothing preflight over `roots` through
    /// the seams' reader and the run's Capacity Check.
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
            &self.capacity_check(),
            stop,
        )
    }

    /// Extracts one planned Archive with [`archives::extract_archive`] over
    /// the seams and the run's Capacity Check, cleaning up its source with
    /// `cleanup`, the Run Request's Archive extraction choice, and logs a
    /// success to the Application Log.
    ///
    /// Never fails outright and never panics: extraction contains its own
    /// panics and classifies every failure by how far the attempt got.
    pub(crate) fn extract_archive(
        &self,
        plan: &ArchiveExtractionPlan,
        cleanup: SourceCleanup,
        artifacts: &mut TemporaryArtifactRegistry,
    ) -> ArchiveExtractionResult {
        let attempt = archives::extract_archive(
            &self.seams,
            &self.capacity_check(),
            plan,
            cleanup,
            artifacts,
        );
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

    /// Runs Archive Finalization with [`finalization::finalize_archives`]
    /// over the seams and the run's Capacity Check with `choices`, the Run
    /// Request's Archive Finalization choices, handing it `phase`, the
    /// just-opened Archive Finalization phase, which it plans and finishes.
    ///
    /// Archive Finalization contains its own panics as an Unexpected
    /// Exception, so callers need no second layer.
    ///
    /// # Errors
    /// As [`finalization::finalize_archives`]: a Run Evidence invariant
    /// violation.
    pub(crate) fn finalize_archives(
        &self,
        preparation: &RunPreparation,
        choices: &ArchiveFinalizationChoices,
        phase: ArchiveFinalizationHandle<'_, '_>,
        artifacts: &mut TemporaryArtifactRegistry,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), Error> {
        finalization::finalize_archives(
            &self.seams,
            &self.capacity_check(),
            choices,
            preparation,
            phase,
            artifacts,
            cancelled,
        )
    }

    /// Plans a Dry Run's Archive creation with
    /// [`finalization::plan_archive_creation`] over the seams' packer with
    /// `choices`, the Run Request's Archive Finalization choices: the
    /// planning-only step a Dry Run takes in place of
    /// [`Self::finalize_archives`].
    ///
    /// It reaches only the packer's `rules`, `list_names` and `partition`,
    /// never `write` or a commit, and needs no phase handle: the caller
    /// records the Planned phase and the plan. Planning panics are contained
    /// and returned as a failed plan.
    pub(crate) fn plan_archive_creation(
        &self,
        preparation: &RunPreparation,
        choices: &ArchiveFinalizationChoices,
        cancelled: &dyn Fn() -> bool,
    ) -> CreationPlanning {
        finalization::plan_archive_creation(&self.seams, choices, preparation, cancelled)
    }
}
