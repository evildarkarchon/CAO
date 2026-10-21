//! The request builder that drives Archive work through a whole Optimization
//! Run (#596).

use std::path::PathBuf;
use std::sync::Arc;

use crate::routing::{ExecutionMode, RequestedWork};
use crate::run::{ArchiveSeams, ArchiveWorkSettings, ModSelection, RunRequest, SelectedProfile};
use crate::test_support::{
    ScriptedToolset, SeamsConfiguration, ToolsetConfiguration, sse_capabilities,
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
pub struct ArchiveRunRequest {
    /// The one Mod Root the run selects.
    mod_root: PathBuf,
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
        Self {
            mod_root: mod_root.into(),
            packing_exclusions: Vec::new(),
            archive_work: ArchiveWorkSettings::default(),
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

    /// The Apply request over the Mod Root.
    pub fn request(&self) -> RunRequest {
        RunRequest::new(
            "SkyrimSE",
            ExecutionMode::Apply,
            ModSelection::SingleModRoot(self.mod_root.clone()),
            vec![RequestedWork::ArchiveCreation],
        )
    }

    /// The Selected Profile the run's configuration supplies: SSE-like, with
    /// `.bsa` Archives and the chosen Packing Exclusions.
    pub fn profile(&self) -> SelectedProfile {
        SelectedProfile::new(sse_capabilities(".bsa"))
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
}
