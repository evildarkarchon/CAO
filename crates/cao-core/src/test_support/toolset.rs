//! The scripted Run Toolset, the configuration provider that returns it, and
//! the call that runs a request through the real Run Executor with them.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::Error;
use crate::execution::{AssetExecutionResult, MutationState};
use crate::routing::RoutedAsset;
use crate::run::{
    ArchiveExtractionPlan, ArchiveExtractionPort, ArchiveExtractionResult, ArchiveExtractor,
    ArchiveFinalizationPort, ArchiveFinalizationRecorder, ArchiveFinalizationResult, ArchiveSeams,
    ArchiveWorkSettings, AssetAttemptAdapter, AssetAttemptPort, AssetBackendFactory,
    AssetInitializationCancelled, CancellationToken, OptimizationRunResult,
    RunConfigurationProvider, RunDiagnostic, RunExecutor, RunObservationSink, RunPreparation,
    RunRequest, RunServices, RunToolset, SafetyCleanupService, SelectedProfile,
    TemporaryArtifactRegistry, create_run_id,
};
use crate::test_support::configuration::default_toolset;
use crate::test_support::{
    FakeArchivePacker, FakeArchiveReader, FakeBackendFactory, FakeCapacity, FakeVolumes,
    sse_profile,
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

/// What a scripted extraction returns for one frozen plan. It may stage
/// through the lent registry, or panic.
pub type ExtractionScript = Box<
    dyn Fn(&ArchiveExtractionPlan, &mut TemporaryArtifactRegistry) -> ArchiveExtractionResult
        + Send
        + Sync,
>;

/// What a scripted Archive Finalization does: it writes through the
/// finalization recorder, and may return an error or panic.
pub type FinalizationScript = Box<
    dyn Fn(
            &RunPreparation,
            &mut ArchiveFinalizationRecorder<'_, '_>,
            &mut TemporaryArtifactRegistry,
        ) -> Result<(), Error>
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
/// Asset attempts are produced as `assets` says. Each other script left
/// `None` keeps its default:
/// - extraction is the real [`ArchiveExtractor`] over `reader` and
///   `capacity`, without source cleanup;
/// - Archive Finalization records an output total of zero and an empty
///   result, as the production phase does with nothing to pack.
///
/// `reader`, `capacity`, `volumes` and `packer` are the Toolset's Archive
/// seams, and `archive_work` its Archive work settings. Every Toolset is
/// built through the production [`RunToolset::new`] over them. Unless
/// [`Self::core_archive_work`] opts in, the scripted extraction and Archive
/// Finalization then replace core's own, and every port call is logged,
/// whatever its script.
#[derive(Default)]
pub struct ScriptedToolset {
    /// How each Asset attempt is produced; by default it succeeds with no
    /// mutation.
    pub assets: AssetAttempts,
    /// Cancels the run's token as this Asset attempt (counting from 1)
    /// starts, as a user clicking Cancel mid-attempt would; the attempt
    /// itself still runs and completes.
    pub cancel_at_attempt: Option<usize>,
    pub extraction: Option<ExtractionScript>,
    pub finalization: Option<FinalizationScript>,
    pub reader: Arc<FakeArchiveReader>,
    pub capacity: Arc<FakeCapacity>,
    pub volumes: Arc<FakeVolumes>,
    /// The Archive packer core's Archive Finalization packs through, under
    /// [`Self::core_archive_work`]; keep a clone to read its writes.
    pub packer: Arc<FakeArchivePacker>,
    /// The source cleanup and Archive Finalization settings core's Archive
    /// work runs with, under [`Self::core_archive_work`]. By default,
    /// extracted sources are backed up, as in production, and finalization
    /// uses its default settings.
    pub archive_work: ArchiveWorkSettings,
    /// Runs the production Run Toolset's own Archive extraction and Archive
    /// Finalization over the seams above, instead of the scripted ones. The
    /// `extraction` and `finalization` scripts are then unused, and neither
    /// [`Self::extracted`] nor [`Self::finalizations`] records anything: the
    /// run's Run Evidence and the fakes' own records are what a scenario
    /// observes.
    pub core_archive_work: bool,
    /// Each Asset attempt's execution path and the Mod Root it was handed.
    pub attempts: Mutex<Vec<(PathBuf, PathBuf)>>,
    /// Each extraction plan, in order.
    pub extractions: Mutex<Vec<ArchiveExtractionPlan>>,
    /// How many times Archive Finalization ran.
    pub finalization_calls: AtomicUsize,
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

    /// Opts in to core's real Archive work; see
    /// [`ScriptedToolset::core_archive_work`].
    pub fn with_core_archive_work(self) -> Self {
        Self {
            core_archive_work: true,
            ..self
        }
    }

    /// Scripts every extraction's result.
    pub fn with_extraction(
        self,
        script: impl Fn(
            &ArchiveExtractionPlan,
            &mut TemporaryArtifactRegistry,
        ) -> ArchiveExtractionResult
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            extraction: Some(Box::new(script)),
            ..self
        }
    }

    /// Scripts Archive Finalization through the finalization recorder.
    pub fn with_finalization(
        self,
        script: impl Fn(
            &RunPreparation,
            &mut ArchiveFinalizationRecorder<'_, '_>,
            &mut TemporaryArtifactRegistry,
        ) -> Result<(), Error>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            finalization: Some(Box::new(script)),
            ..self
        }
    }

    /// Archive Finalization records `result` as the real phase would; see
    /// [`record_finalization_result`] for `total`.
    pub fn with_finalization_result(
        self,
        result: ArchiveFinalizationResult,
        total: Option<usize>,
    ) -> Self {
        self.with_finalization(move |_, recorder, _| {
            record_finalization_result(recorder, result.clone(), total)
        })
    }

    /// Simulates the run's work escaping at Archive Finalization: the port
    /// returns an error that is not a Run Evidence invariant violation.
    ///
    /// No real port does this, since the port contract allows only invariant
    /// violations; it exercises the executor's own containment, which fails
    /// the run with Run Work Escaped. A scenario that needs a failed phase a
    /// real run can produce records it with [`Self::with_finalization_result`].
    pub fn with_escaping_finalization_work(self, detail: impl Into<String>) -> Self {
        let detail = detail.into();
        self.with_finalization(move |_, _, _| Err(Error::RunWorkEscaped(detail.clone())))
    }

    /// A fresh Run Toolset over this script, as a provider returns for one
    /// run, built through the production [`RunToolset::new`]. A scripted
    /// fake backend, or a scripted adapter, is created afresh for each
    /// Toolset. Unless [`Self::core_archive_work`] opts in, the scripted
    /// extraction and Archive Finalization ports replace core's own.
    pub fn toolset(self: &Arc<Self>) -> RunToolset {
        let backend: Option<Box<dyn AssetAttemptPort>> = match &self.assets {
            AssetAttempts::FakeBackend(factory) => {
                Some(Box::new(AssetAttemptAdapter::new(factory.clone())))
            }
            AssetAttempts::Adapter(make) => Some(make()),
            AssetAttempts::Succeeding | AssetAttempts::Scripted(_) => None,
        };
        let toolset = RunToolset::new(
            Box::new(ScriptedAssetAttempt {
                script: Arc::clone(self),
                backend,
            }),
            ArchiveSeams {
                reader: self.reader.clone(),
                capacity: self.capacity.clone(),
                volume_identity: self.volumes.clone(),
                packer: self.packer.clone(),
            },
            self.archive_work,
        );
        if self.core_archive_work {
            return toolset;
        }
        // The crate-visible fields let the scripted ports stand in for core's
        // until the scripted runs migrate onto the seams (#581).
        RunToolset {
            archive_extraction: Box::new(ScriptedExtraction(Arc::clone(self))),
            archive_finalization: Box::new(ScriptedFinalization(Arc::clone(self))),
            ..toolset
        }
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

    /// Every extraction plan so far, in order.
    pub fn extracted(&self) -> Vec<ArchiveExtractionPlan> {
        self.extractions.lock().unwrap().clone()
    }

    /// How many times Archive Finalization ran.
    pub fn finalizations(&self) -> usize {
        self.finalization_calls.load(Ordering::SeqCst)
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

/// The scripted Archive extraction port.
struct ScriptedExtraction(Arc<ScriptedToolset>);

impl ArchiveExtractionPort for ScriptedExtraction {
    fn extract(
        &mut self,
        plan: &ArchiveExtractionPlan,
        artifacts: &mut TemporaryArtifactRegistry,
    ) -> ArchiveExtractionResult {
        let script = &self.0;
        script.extractions.lock().unwrap().push(plan.clone());
        match &script.extraction {
            Some(extract) => extract(plan, artifacts),
            None => {
                ArchiveExtractor::new(&*script.reader, &*script.capacity).extract(plan, artifacts)
            }
        }
    }
}

/// The scripted Archive Finalization port.
struct ScriptedFinalization(Arc<ScriptedToolset>);

impl ArchiveFinalizationPort for ScriptedFinalization {
    fn finalize(
        &mut self,
        preparation: &RunPreparation,
        recorder: &mut ArchiveFinalizationRecorder<'_, '_>,
        artifacts: &mut TemporaryArtifactRegistry,
        _stop: &CancellationToken,
    ) -> Result<(), Error> {
        let script = &self.0;
        script.finalization_calls.fetch_add(1, Ordering::SeqCst);
        match &script.finalization {
            Some(finalize) => finalize(preparation, recorder, artifacts),
            None => {
                record_finalization_result(recorder, ArchiveFinalizationResult::default(), None)
            }
        }
    }
}

/// Records a scripted finalizer's result the way Archive Finalization does:
/// the output total, each attempt in order, then the complete result (C++
/// `recordArchiveFinalizationResult`). The total defaults to the attempt
/// count; pass a larger one for a result that stopped before every planned
/// output.
///
/// # Errors
/// A Run Evidence invariant violation the recorder reports.
pub fn record_finalization_result(
    recorder: &mut ArchiveFinalizationRecorder<'_, '_>,
    result: ArchiveFinalizationResult,
    total: Option<usize>,
) -> Result<(), Error> {
    recorder.record_output_total(total.unwrap_or(result.attempts.len()))?;
    for attempt in &result.attempts {
        recorder.record_attempt(attempt.clone())?;
    }
    recorder.record_result(result)
}

/// A provider returning one fixed Selected Profile and a fresh Run Toolset
/// for each run: over `toolset` if scripted, or over a default
/// [`ScriptedToolset`] otherwise. It counts its loads.
pub struct ToolsetConfiguration {
    pub profile: SelectedProfile,
    pub toolset: Option<Arc<ScriptedToolset>>,
    pub loads: AtomicUsize,
}

impl ToolsetConfiguration {
    /// A provider of `profile` and of `toolset`'s Toolsets, or default
    /// scripted ones when `toolset` is `None`.
    pub fn new(profile: SelectedProfile, toolset: Option<Arc<ScriptedToolset>>) -> Self {
        Self {
            profile,
            toolset,
            loads: AtomicUsize::new(0),
        }
    }

    /// A provider of the SSE-like [`sse_profile`] and of `toolset`'s
    /// Toolsets, or default scripted ones when `toolset` is `None`.
    pub fn sse(toolset: Option<Arc<ScriptedToolset>>) -> Self {
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
        let toolset = self
            .toolset
            .as_ref()
            .map_or_else(default_toolset, ScriptedToolset::toolset);
        Ok((self.profile.clone(), toolset))
    }
}
