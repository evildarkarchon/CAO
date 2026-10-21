//! The request builder that drives Archive work through a whole Optimization
//! Run (#596).

use std::path::PathBuf;
use std::sync::Arc;

use crate::routing::{ExecutionMode, RequestedWork};
use crate::run::{
    ArchiveSeams, ArchiveWorkSettings, CancellationToken, ModSelection, OptimizationRunResult,
    RunConfigurationProvider, RunObservationSink, RunRequest, SelectedProfile,
};
use crate::test_support::{
    CountingCleanup, ScriptedToolset, SeamsConfiguration, ToolsetConfiguration, execute_run,
    sse_capabilities,
};

/// An Apply run over one Mod Root that requests Archive creation, and the
/// configuration its Preparing loads.
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
/// such as FO4's `.ba2` (#597).
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
    /// The source cleanup and Archive Finalization settings the run's Toolset
    /// is built with.
    archive_work: ArchiveWorkSettings,
}

impl ArchiveRunRequest {
    /// An Apply run over `mod_root` that requests only Archive creation, with
    /// no Packing Exclusions and the default Archive work settings.
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
            archive_work: ArchiveWorkSettings::default(),
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

    /// Replaces the Archive work settings the run's Toolset is built with.
    pub fn with_archive_work(self, archive_work: ArchiveWorkSettings) -> Self {
        Self {
            archive_work,
            ..self
        }
    }

    /// The Apply request over the Mod Selection.
    pub fn request(&self) -> RunRequest {
        RunRequest::new(
            "SkyrimSE",
            ExecutionMode::Apply,
            self.selection.clone(),
            self.work.clone(),
        )
    }

    /// The Selected Profile the run's configuration supplies: SSE-like, with
    /// the chosen Archive extension (`.bsa` by default) and Packing
    /// Exclusions.
    pub fn profile(&self) -> SelectedProfile {
        SelectedProfile::new(sse_capabilities(&self.archive_extension))
            .with_packing_exclusions(self.packing_exclusions.clone())
    }

    /// The Archive work settings the run's Toolset is built with.
    pub fn archive_work(&self) -> ArchiveWorkSettings {
        self.archive_work
    }

    /// A provider of [`Self::profile`] and of Run Toolsets over `toolset`'s
    /// typed fakes, for runs over the fakes.
    ///
    /// The chosen Archive work settings replace `toolset`'s own
    /// [`ScriptedToolset::archive_work`], so the request alone decides them.
    /// Read the fakes' records through the provider's `toolset`.
    pub fn configuration(&self, toolset: ScriptedToolset) -> ToolsetConfiguration {
        let toolset = ScriptedToolset {
            archive_work: self.archive_work,
            ..toolset
        };
        ToolsetConfiguration::new(self.profile(), Arc::new(toolset))
    }

    /// A provider of [`Self::profile`] and, for each run, of a Run Toolset
    /// with the chosen Archive work settings over the Archive seams `seams`
    /// builds, such as `cao-optimizers`' production adapters. Its Asset
    /// attempts succeed with no mutation until
    /// [`SeamsConfiguration::with_assets`] scripts them.
    pub fn seams_configuration(
        &self,
        seams: impl Fn() -> ArchiveSeams + Send + Sync + 'static,
    ) -> SeamsConfiguration {
        SeamsConfiguration::new(self.profile(), seams, self.archive_work)
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
