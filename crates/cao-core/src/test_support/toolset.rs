//! The scripted Run Toolset, the configuration provider that returns it, and
//! the call that runs a request through the real Run Executor with them.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::Error;
use crate::execution::{AssetExecutionResult, MutationState};
use crate::routing::RoutedAsset;
use crate::run::{
    ArchiveSeams, ArchiveWorkSettings, AssetAttemptAdapter, AssetAttemptPort, AssetBackendFactory,
    AssetInitializationCancelled, CancellationToken, OptimizationRunResult,
    RunConfigurationProvider, RunDiagnostic, RunExecutor, RunObservationSink, RunRequest,
    RunServices, RunToolset, SafetyCleanupService, SelectedProfile, TemporaryArtifactRegistry,
    create_run_id,
};
use crate::test_support::{
    FakeArchivePacker, FakeArchiveReader, FakeBackendFactory, FakeVolumeProbe, sse_profile,
};

/// What a scripted Asset attempt returns for one Routed Asset and its frozen
/// Mod Root. It may stage through the lent registry, or panic.
pub type AssetAttemptScript = Box<
    dyn Fn(
            &RoutedAsset,
            &Path,
            &mut TemporaryArtifactRegistry,
        ) -> Result<AssetExecutionResult, AssetInitializationCancelled>
        + Send
        + Sync,
>;

/// Makes a fresh Asset attempt port for each Toolset: the production
/// [`AssetAttemptAdapter`] over a scenario's own backend factory.
pub type AssetAdapterMaker = Box<dyn Fn() -> Box<dyn AssetAttemptPort> + Send + Sync>;

/// How a [`ScriptedToolset`]'s Asset attempts are produced.
#[derive(Default)]
pub enum AssetAttempts {
    /// Every attempt succeeds with no mutation.
    #[default]
    Succeeding,
    /// Every attempt returns what this script returns.
    Scripted(AssetAttemptScript),
    /// Attempts go through the production [`AssetAttemptAdapter`] over a
    /// clone of this fake backend factory, which each run's Toolset creates
    /// its backend from afresh.
    FakeBackend(FakeBackendFactory),
    /// Attempts go through the production [`AssetAttemptAdapter`] this makes
    /// afresh for each run's Toolset, over a scenario's own backend factory.
    Adapter(AssetAdapterMaker),
}

/// A Run Toolset scripted per scenario, shared between the scenario and the
/// provider that hands a fresh [`RunToolset`] over it to each run.
///
/// Asset attempts are produced as `assets` says. Archive work is never
/// scripted: `reader`, `volume_probe` and `packer` are the Toolset's
/// Archive seams, and `archive_work` its Archive work settings, and every
/// Toolset is built through the production [`RunToolset::new`] over them.
/// Every run therefore does core's real Archive extraction and Archive
/// Finalization over the fakes, and a scenario observes that work through
/// the run's Run Evidence and, only for what Run Evidence does not hold, the
/// fakes' own records.
#[derive(Default)]
pub struct ScriptedToolset {
    /// How each Asset attempt is produced; by default it succeeds with no
    /// mutation.
    pub assets: AssetAttempts,
    /// Cancels the run's token as this Asset attempt (counting from 1)
    /// starts, as a user clicking Cancel mid-attempt would; the attempt
    /// itself still runs and completes.
    pub cancel_at_attempt: Option<usize>,
    pub reader: Arc<FakeArchiveReader>,
    pub volume_probe: Arc<FakeVolumeProbe>,
    /// The Archive packer core's Archive Finalization packs through; keep a
    /// clone to read its writes.
    pub packer: Arc<FakeArchivePacker>,
    /// The source cleanup and Archive Finalization settings core's Archive
    /// work runs with. By default, extracted sources are backed up, as in
    /// production, and finalization uses its default settings.
    pub archive_work: ArchiveWorkSettings,
    /// Each Asset attempt's execution path and the Mod Root it was handed.
    pub attempts: Mutex<Vec<(PathBuf, PathBuf)>>,
}

impl ScriptedToolset {
    /// Scripts every Asset attempt's result.
    pub fn with_assets(
        self,
        script: impl Fn(
            &RoutedAsset,
            &Path,
            &mut TemporaryArtifactRegistry,
        ) -> Result<AssetExecutionResult, AssetInitializationCancelled>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            assets: AssetAttempts::Scripted(Box::new(script)),
            ..self
        }
    }

    /// Drives Asset attempts through the production Asset attempt adapter
    /// over `factory`'s fake backend; keep clones of its shared logs to
    /// observe it.
    pub fn with_backend(self, factory: FakeBackendFactory) -> Self {
        Self {
            assets: AssetAttempts::FakeBackend(factory),
            ..self
        }
    }

    /// Drives Asset attempts through the production Asset attempt adapter
    /// over a backend factory `make` creates afresh for each Toolset, for a
    /// scenario whose backend the shared fake backend cannot stand in for.
    pub fn with_backend_factory<F>(self, make: impl Fn() -> F + Send + Sync + 'static) -> Self
    where
        F: AssetBackendFactory + 'static,
    {
        Self {
            assets: AssetAttempts::Adapter(Box::new(move || {
                Box::new(AssetAttemptAdapter::new(make()))
            })),
            ..self
        }
    }

    /// Cancels the run's token as the `attempt`-th Asset attempt (counting
    /// from 1) starts; see [`ScriptedToolset::cancel_at_attempt`].
    pub fn cancelling_at_attempt(self, attempt: usize) -> Self {
        Self {
            cancel_at_attempt: Some(attempt),
            ..self
        }
    }

    /// A fresh Run Toolset over this script, as a provider returns for one
    /// run, built through the production [`RunToolset::new`] over the fake
    /// Archive seams. A scripted fake backend, or a scripted adapter, is
    /// created afresh for each Toolset.
    pub fn toolset(self: &Arc<Self>) -> RunToolset {
        let backend: Option<Box<dyn AssetAttemptPort>> = match &self.assets {
            AssetAttempts::FakeBackend(factory) => {
                Some(Box::new(AssetAttemptAdapter::new(factory.clone())))
            }
            AssetAttempts::Adapter(make) => Some(make()),
            AssetAttempts::Succeeding | AssetAttempts::Scripted(_) => None,
        };
        RunToolset::new(
            Box::new(ScriptedAssetAttempt {
                script: Arc::clone(self),
                backend,
            }),
            ArchiveSeams {
                reader: self.reader.clone(),
                volume_probe: self.volume_probe.clone(),
                packer: self.packer.clone(),
            },
            self.archive_work,
        )
    }

    /// The execution path of every Asset attempt so far, in order.
    pub fn attempted(&self) -> Vec<PathBuf> {
        let attempts = self.attempts.lock().unwrap();
        attempts.iter().map(|(path, _)| path.clone()).collect()
    }

    /// The Mod Root handed to every Asset attempt so far, in order.
    pub fn attempted_roots(&self) -> Vec<PathBuf> {
        let attempts = self.attempts.lock().unwrap();
        attempts.iter().map(|(_, root)| root.clone()).collect()
    }
}

/// The scripted Asset attempt port.
struct ScriptedAssetAttempt {
    script: Arc<ScriptedToolset>,
    /// The production adapter over a fake backend or a scenario's own
    /// backend factory, when [`ScriptedToolset::assets`] scripts one.
    backend: Option<Box<dyn AssetAttemptPort>>,
}

impl AssetAttemptPort for ScriptedAssetAttempt {
    fn attempt(
        &mut self,
        asset: &RoutedAsset,
        mod_root: &Path,
        artifacts: &mut TemporaryArtifactRegistry,
        stop: &CancellationToken,
        retain: &mut dyn FnMut(RunDiagnostic),
    ) -> Result<AssetExecutionResult, AssetInitializationCancelled> {
        let started = {
            let mut attempts = self.script.attempts.lock().unwrap();
            attempts.push((asset.execution_path().to_path_buf(), mod_root.to_path_buf()));
            attempts.len()
        };
        if self.script.cancel_at_attempt == Some(started) {
            stop.cancel();
        }
        if let Some(backend) = &mut self.backend {
            return backend.attempt(asset, mod_root, artifacts, stop, retain);
        }
        match &self.script.assets {
            AssetAttempts::Scripted(script) => script(asset, mod_root, artifacts),
            _ => Ok(AssetExecutionResult::success(MutationState::None)),
        }
    }
}

/// A provider returning one fixed Selected Profile and a fresh Run Toolset
/// over `toolset` for each run. It counts its loads.
///
/// A scenario that scripts nothing passes a default [`ScriptedToolset`].
pub struct ToolsetConfiguration {
    pub profile: SelectedProfile,
    pub toolset: Arc<ScriptedToolset>,
    pub loads: AtomicUsize,
}

impl ToolsetConfiguration {
    /// A provider of `profile` and of `toolset`'s Toolsets.
    pub fn new(profile: SelectedProfile, toolset: Arc<ScriptedToolset>) -> Self {
        Self {
            profile,
            toolset,
            loads: AtomicUsize::new(0),
        }
    }

    /// A provider of the SSE-like [`sse_profile`] and of `toolset`'s Toolsets.
    pub fn sse(toolset: Arc<ScriptedToolset>) -> Self {
        Self::new(sse_profile(), toolset)
    }
}

/// Executes `request` through the real Run Executor with the services the
/// scenario supplies and a fresh Run ID. Requested work runs only through the
/// Run Toolset `configuration` returns; without a provider, Preparing fails.
pub fn execute_run(
    request: &RunRequest,
    cleanup: &mut dyn SafetyCleanupService,
    observations: Option<&dyn RunObservationSink>,
    configuration: Option<&dyn RunConfigurationProvider>,
    stop: &CancellationToken,
) -> OptimizationRunResult {
    RunExecutor.execute(
        request,
        RunServices {
            safety_cleanup: cleanup,
            observations,
            configuration,
        },
        stop,
        create_run_id(),
    )
}

impl RunConfigurationProvider for ToolsetConfiguration {
    fn load(&self, _profile_identity: &str) -> Result<(SelectedProfile, RunToolset), Error> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        Ok((self.profile.clone(), self.toolset.toolset()))
    }
}
