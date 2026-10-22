//! The request builder that drives Archive work through a whole Optimization
//! Run (#596).

use std::path::PathBuf;
use std::sync::Arc;

use crate::routing::{ExecutionMode, RequestedWork};
use crate::run::{
    ArchiveFinalizationChoices, CancellationToken, ModSelection, OptimizationRunResult,
    RunConfigurationProvider, RunObservationSink, RunRequest, SelectedProfile, SourceCleanup,
    WorkChoices,
};
use crate::test_support::{
    CountingCleanup, ScriptedToolset, SeamsBuilder, SeamsConfiguration, ToolsetConfiguration,
    execute_run, sse_capabilities,
};

/// An Apply run that requests Archive creation, by default over one Mod Root,
/// and the configuration its Preparing loads.
///
/// The Archive creation request and the Packing Exclusions reach Archive
/// Finalization as they do in production: the request asks for the work, the
/// profile the configuration supplies carries the exclusions, and Preparing
/// makes the run's preparation from both. No scenario hand-builds a
/// preparation.
///
/// The profile is SSE-like and supports every kind of work, so any work a
/// scenario requests passes Preparing; its Archive extension is `.bsa`.
///
/// A scenario may instead select every child Mod Root of a mods directory
/// ([`Self::over_mods`]), request other work ([`Self::with_work`]), or give
/// the profile another Archive extension ([`Self::with_archive_extension`]),
/// such as FO4's `.ba2` (#597). It may also set the Run Request's Archive
/// extraction ([`Self::with_archive_extraction`]) and Archive Finalization
/// ([`Self::with_archive_finalization`]) choices, which the run's Archive
/// work reads from the request, as production's does.
pub struct ArchiveRunRequest {
    /// The Mod Selection the run resolves: one Mod Root by default.
    selection: ModSelection,
    /// The work the request asks for: only Archive creation by default.
    work: Vec<RequestedWork>,
    /// The Selected Profile's raw Archive extension definition, such as
    /// `.bsa`.
    archive_extension: String,
    /// The Selected Profile's Packing Exclusion rules, as `FilesToNotPack.txt`
    /// spells them.
    packing_exclusions: Vec<String>,
    /// The Work Choices the Run Request carries, of which only the Archive
    /// extraction and Archive Finalization choices are ever changed here.
    /// The Run Request is their one source, as in production: no provider
    /// this builder makes carries any choice.
    work_choices: WorkChoices,
}

impl ArchiveRunRequest {
    /// An Apply run over `mod_root` that requests only Archive creation, with
    /// no Packing Exclusions and the default Work Choices.
    pub fn new(mod_root: impl Into<PathBuf>) -> Self {
        Self::selecting(ModSelection::SingleModRoot(mod_root.into()))
    }

    /// Like [`Self::new`], but the run selects every child Mod Root of
    /// `mods_directory`, which Preparing resolves in name order, as a Several
    /// Mods run does.
    pub fn over_mods(mods_directory: impl Into<PathBuf>) -> Self {
        Self::selecting(ModSelection::ChildModRoots(mods_directory.into()))
    }

    /// The default request over `selection`.
    fn selecting(selection: ModSelection) -> Self {
        Self {
            selection,
            work: vec![RequestedWork::ArchiveCreation],
            archive_extension: ".bsa".to_owned(),
            packing_exclusions: Vec::new(),
            work_choices: WorkChoices::default(),
        }
    }

    /// Replaces the requested work with `work`, for a run that extracts
    /// Archives, or one that does not request Archive creation at all.
    pub fn with_work(self, work: impl IntoIterator<Item = RequestedWork>) -> Self {
        Self {
            work: work.into_iter().collect(),
            ..self
        }
    }

    /// Replaces the profile's raw Archive extension definition, such as
    /// `.ba2` for a run over FO4's packer; discovery recognizes Archives by
    /// it.
    pub fn with_archive_extension(self, extension: impl Into<String>) -> Self {
        Self {
            archive_extension: extension.into(),
            ..self
        }
    }

    /// Replaces the profile's Packing Exclusions with `exclusions`, spelled
    /// as `FilesToNotPack.txt` lines; Archive Finalization normalizes them.
    pub fn with_packing_exclusions(
        self,
        exclusions: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            packing_exclusions: exclusions.into_iter().map(Into::into).collect(),
            ..self
        }
    }

    /// Replaces the Run Request's Archive extraction choice: what becomes of
    /// each source Archive once it is extracted. By default it is backed up.
    pub fn with_archive_extraction(mut self, cleanup: SourceCleanup) -> Self {
        self.work_choices.archive_extraction = cleanup;
        self
    }

    /// Replaces the Run Request's Archive Finalization choices. By default
    /// they are [`ArchiveFinalizationChoices::default`].
    pub fn with_archive_finalization(mut self, choices: ArchiveFinalizationChoices) -> Self {
        self.work_choices.archive_finalization = choices;
        self
    }

    /// The Apply request over the Mod Selection, carrying the chosen Work
    /// Choices.
    pub fn request(&self) -> RunRequest {
        RunRequest::new(
            "SkyrimSE",
            ExecutionMode::Apply,
            self.selection.clone(),
            self.work.clone(),
        )
        .with_work_choices(self.work_choices)
    }

    /// The Selected Profile the run's configuration supplies: SSE-like, with
    /// the chosen Archive extension (`.bsa` by default) and Packing
    /// Exclusions.
    pub fn profile(&self) -> SelectedProfile {
        SelectedProfile::new(sse_capabilities(&self.archive_extension))
            .with_packing_exclusions(self.packing_exclusions.clone())
    }

    /// A provider of [`Self::profile`] and of Run Toolsets over `toolset`'s
    /// typed fakes, for runs over the fakes.
    ///
    /// Neither the provider nor `toolset` carries any Archive choice: the
    /// run's Archive work takes them from [`Self::request`]. Read the fakes'
    /// records through the provider's `toolset`.
    pub fn configuration(&self, toolset: ScriptedToolset) -> ToolsetConfiguration {
        ToolsetConfiguration::new(self.profile(), Arc::new(toolset))
    }

    /// A provider of [`Self::profile`] and, for each run, of a Run Toolset
    /// over the Archive seams `seams` builds, such as `cao-optimizers`'
    /// production adapters. As with [`Self::configuration`], the run's
    /// Archive work takes its choices from [`Self::request`]. Its Asset
    /// attempts succeed with no mutation until
    /// [`SeamsConfiguration::with_assets`] scripts them.
    pub fn seams_configuration(&self, seams: SeamsBuilder) -> SeamsConfiguration {
        SeamsConfiguration::new(self.profile(), seams)
    }

    /// Executes [`Self::request`] through the real Run Executor with
    /// `configuration`, such as [`Self::configuration`] or
    /// [`Self::seams_configuration`] returns, publishing live facts to
    /// `observations` and observing cancellation through `stop`.
    ///
    /// Safety Cleanup is a counting service with nothing to fail.
    ///
    /// # Panics
    ///
    /// When the run did not make exactly one Safety Cleanup pass, which every
    /// run makes on every terminal path.
    pub fn execute(
        &self,
        configuration: &dyn RunConfigurationProvider,
        observations: Option<&dyn RunObservationSink>,
        stop: &CancellationToken,
    ) -> OptimizationRunResult {
        let mut cleanup = CountingCleanup::default();
        let result = execute_run(
            &self.request(),
            &mut cleanup,
            observations,
            Some(configuration),
            stop,
        );
        assert_eq!(cleanup.passes, 1, "Safety Cleanup always runs once");
        result
    }
}
