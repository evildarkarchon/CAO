//! The scripted Run Toolset, the configuration provider that returns it, and
//! the call that runs a request through the real Run Executor with them.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::CompletedMutation;
use crate::Error;
use crate::execution::AssetExecutionResult;
use crate::routing::{ExecutionMode, RoutedAsset};
use crate::run::{
    ArchiveSeams, AssetAttemptAdapter, AssetAttemptPort, AssetBackendFactory,
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
/// Archive seams, and every Toolset is built through the production
/// [`RunToolset::new`] over them. The Archive choices are not part of the
/// script, nor of any provider: the run's Archive work takes them from its
/// Run Request, as in production. Every run therefore does core's real
/// Archive extraction and Archive Finalization over the fakes, and a
/// scenario observes that work through the run's Run Evidence and, only for
/// what Run Evidence does not hold, the fakes' own records.
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
    /// run in `mode`, built through the production [`RunToolset::new`] over
    /// the fake Archive seams. A scripted fake backend, or a scripted
    /// adapter, is created afresh for each Toolset.
    pub fn toolset(self: &Arc<Self>, mode: ExecutionMode) -> RunToolset {
        self.toolset_over(self.fake_seams(), mode)
    }

    /// The Archive seams over this script's shared fakes: `reader`,
    /// `volume_probe` and `packer`, so every Toolset built over them writes
    /// to the records a scenario reads.
    pub fn fake_seams(&self) -> ArchiveSeams {
        ArchiveSeams {
            reader: self.reader.clone(),
            volume_probe: self.volume_probe.clone(),
            packer: self.packer.clone(),
        }
    }

    /// A fresh Run Toolset whose Asset attempts follow this script and whose
    /// Archive work goes over `seams`, which may be this script's
    /// [`Self::fake_seams`] or any other reader, packer and volume probe, for
    /// a run in `mode`. It is built through the production
    /// [`RunToolset::new`], as [`Self::toolset`] is.
    pub fn toolset_over(self: &Arc<Self>, seams: ArchiveSeams, mode: ExecutionMode) -> RunToolset {
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
            seams,
            mode,
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
            _ => Ok(AssetExecutionResult::success(CompletedMutation::None)),
        }
    }
}

/// What every load of a scripted provider returns besides its Toolset's
/// Archive seams, and the count of those loads.
///
/// [`ToolsetConfiguration`] and [`SeamsConfiguration`] share it; they differ
/// only in the Archive seams each load's Run Toolset goes over. It carries no
/// Archive choice, as the production provider carries none: the run's
/// Archive work takes them from its Run Request.
pub struct ProviderLoads {
    /// The Selected Profile every load returns.
    pub profile: SelectedProfile,
    /// How many times Preparing has loaded the provider so far.
    pub count: AtomicUsize,
}

impl ProviderLoads {
    /// Loads of `profile`, none made yet.
    fn new(profile: SelectedProfile) -> Self {
        Self {
            profile,
            count: AtomicUsize::new(0),
        }
    }

    /// Counts one load and returns the profile with a fresh Run Toolset whose
    /// Asset attempts follow `assets` and whose Archive work goes over
    /// `seams`, built with `request`'s Execution Mode as the production
    /// provider builds it.
    fn load(
        &self,
        assets: &Arc<ScriptedToolset>,
        seams: ArchiveSeams,
        request: &RunRequest,
    ) -> Result<(SelectedProfile, RunToolset), Error> {
        self.count.fetch_add(1, Ordering::SeqCst);
        Ok((
            self.profile.clone(),
            assets.toolset_over(seams, request.execution_mode()),
        ))
    }
}

/// A provider returning one fixed Selected Profile and a fresh Run Toolset
/// over `toolset` for each run. It counts its loads.
///
/// A scenario that scripts nothing passes a default [`ScriptedToolset`].
pub struct ToolsetConfiguration {
    /// The profile and load count of every load.
    pub loads: ProviderLoads,
    /// The Asset attempt script and the fake Archive seams every load's
    /// Toolset goes over; keep a clone, or read it here, to observe them.
    pub toolset: Arc<ScriptedToolset>,
}

impl ToolsetConfiguration {
    /// A provider of `profile` and of `toolset`'s Toolsets.
    pub fn new(profile: SelectedProfile, toolset: Arc<ScriptedToolset>) -> Self {
        Self {
            loads: ProviderLoads::new(profile),
            toolset,
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
    fn load(&self, request: &RunRequest) -> Result<(SelectedProfile, RunToolset), Error> {
        self.loads
            .load(&self.toolset, self.toolset.fake_seams(), request)
    }
}

/// Builds the Archive seams of one run's Toolset.
pub type SeamsBuilder = Box<dyn Fn() -> ArchiveSeams + Send + Sync>;

/// A provider returning one fixed Selected Profile and, for each run, a
/// fresh Run Toolset over the Archive seams its builder makes. It counts its
/// loads.
///
/// Unlike [`ToolsetConfiguration`], its Archive seams are not the typed
/// fakes, so a scenario can drive a run over any reader, packer and volume
/// probe, such as `cao-optimizers`' production adapters with its own probes.
/// The Toolset is built through the production [`RunToolset::new`], so the
/// run does core's real Archive work over the seams, with the choices its
/// Run Request carries. Asset attempts follow `assets` as [`ScriptedToolset`]
/// scripts them; only its Asset attempt script is used, never its fake
/// Archive seams.
pub struct SeamsConfiguration {
    /// The profile and load count of every load.
    pub loads: ProviderLoads,
    /// Called once per run, as Preparing loads the configuration, so each
    /// run's Toolset has seams of its own.
    pub seams: SeamsBuilder,
    /// The Asset attempt script; keep a clone to read its attempts.
    pub assets: Arc<ScriptedToolset>,
}

impl SeamsConfiguration {
    /// A provider of `profile` and of Toolsets over the seams `seams` builds.
    /// Every Asset attempt succeeds with no mutation.
    pub fn new(profile: SelectedProfile, seams: SeamsBuilder) -> Self {
        Self {
            loads: ProviderLoads::new(profile),
            seams,
            assets: Arc::new(ScriptedToolset::default()),
        }
    }

    /// Scripts each run's Asset attempts with `assets`; see
    /// [`SeamsConfiguration::assets`].
    pub fn with_assets(self, assets: Arc<ScriptedToolset>) -> Self {
        Self { assets, ..self }
    }
}

impl RunConfigurationProvider for SeamsConfiguration {
    fn load(&self, request: &RunRequest) -> Result<(SelectedProfile, RunToolset), Error> {
        self.loads.load(&self.assets, (self.seams)(), request)
    }
}
