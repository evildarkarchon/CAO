//! The scripted Run Toolset and the configuration provider that returns it.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::Error;
use crate::execution::{AssetExecutionResult, MutationState};
use crate::routing::RoutedAsset;
use crate::run::{
    ArchiveExtractionPlan, ArchiveExtractionPort, ArchiveExtractionResult, ArchiveExtractor,
    ArchiveFinalizationPort, ArchiveFinalizationRecorder, ArchiveFinalizationResult,
    AssetAttemptAdapter, AssetAttemptPort, AssetBackendFactory, AssetInitializationCancelled,
    CancellationToken, RunConfiguration, RunConfigurationProvider, RunDiagnostic, RunPreparation,
    RunToolset, TemporaryArtifactRegistry,
};
use crate::test_support::{FakeArchiveReader, FakeBackendFactory, FakeCapacity, FakeVolumes};

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
            &ArchiveFinalizationRecorder<'_, '_>,
            &mut TemporaryArtifactRegistry,
        ) -> Result<(), Error>
        + Send
        + Sync,
>;

/// Makes a fresh Asset attempt port for each Toolset: the production
/// [`AssetAttemptAdapter`] over a scenario's own backend factory.
pub type AssetAdapterMaker = Box<dyn Fn() -> Box<dyn AssetAttemptPort> + Send + Sync>;

/// A Run Toolset scripted per scenario, shared between the scenario and the
/// provider that hands a fresh [`RunToolset`] over it to each run.
///
/// Each script left `None` keeps its default:
/// - an Asset attempt succeeds with no mutation, unless `backend` or
///   `backend_factory` is set, when the production [`AssetAttemptAdapter`]
///   drives a backend that each run's Toolset creates afresh;
/// - extraction is the real [`ArchiveExtractor`] over `reader` and
///   `capacity`, without source cleanup;
/// - Archive Finalization records an output total of zero and an empty
///   result, as the production phase does with nothing to pack.
///
/// `reader`, `capacity` and `volumes` are the Toolset's Archive reader and
/// probes. Every port call is logged, whatever its script.
#[derive(Default)]
pub struct ScriptedToolset {
    pub assets: Option<AssetAttemptScript>,
    /// When set, Asset attempts go through the production adapter over this
    /// fake backend factory instead of `assets`.
    pub backend: Option<FakeBackendFactory>,
    /// When set, Asset attempts go through the production adapter over the
    /// backend factory this makes, instead of `assets` or `backend`.
    pub backend_factory: Option<AssetAdapterMaker>,
    /// Cancels the run's token as this Asset attempt (counting from 1)
    /// starts, as a user clicking Cancel mid-attempt would; the attempt
    /// itself still runs and completes.
    pub cancel_at_attempt: Option<usize>,
    pub extraction: Option<ExtractionScript>,
    pub finalization: Option<FinalizationScript>,
    pub reader: Arc<FakeArchiveReader>,
    pub capacity: Arc<FakeCapacity>,
    pub volumes: Arc<FakeVolumes>,
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
            assets: Some(Box::new(script)),
            ..self
        }
    }

    /// Drives Asset attempts through the production Asset attempt adapter
    /// over `factory`'s fake backend; keep clones of its shared logs to
    /// observe it.
    pub fn with_backend(self, factory: FakeBackendFactory) -> Self {
        Self {
            backend: Some(factory),
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
            backend_factory: Some(Box::new(move || Box::new(AssetAttemptAdapter::new(make())))),
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
            &ArchiveFinalizationRecorder<'_, '_>,
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

    /// Archive Finalization returns an error that is not a Run Evidence
    /// invariant violation, so it escapes the run's work and fails the run.
    pub fn with_finalization_error(self, detail: impl Into<String>) -> Self {
        let detail = detail.into();
        self.with_finalization(move |_, _, _| Err(Error::WorkService(detail.clone())))
    }

    /// A fresh Run Toolset over this script, as a provider returns for one
    /// run. A scripted fake backend, or a scripted backend factory, is
    /// created afresh for each Toolset.
    pub fn toolset(self: &Arc<Self>) -> RunToolset {
        let backend: Option<Box<dyn AssetAttemptPort>> = match &self.backend_factory {
            Some(make) => Some(make()),
            None => self.backend.clone().map(|factory| {
                Box::new(AssetAttemptAdapter::new(factory)) as Box<dyn AssetAttemptPort>
            }),
        };
        RunToolset {
            asset_attempt: Box::new(ScriptedAssetAttempt {
                script: Arc::clone(self),
                backend,
            }),
            archive_extraction: Box::new(ScriptedExtraction(Arc::clone(self))),
            archive_finalization: Box::new(ScriptedFinalization(Arc::clone(self))),
            archive_reader: self.reader.clone(),
            capacity: self.capacity.clone(),
            volume_identity: self.volumes.clone(),
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
    /// The production adapter over a fake backend or a scripted backend
    /// factory, when one is scripted.
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
            Some(script) => script(asset, mod_root, artifacts),
            None => Ok(AssetExecutionResult::success(MutationState::None)),
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
        recorder: &ArchiveFinalizationRecorder<'_, '_>,
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
    recorder: &ArchiveFinalizationRecorder<'_, '_>,
    result: ArchiveFinalizationResult,
    total: Option<usize>,
) -> Result<(), Error> {
    recorder.record_output_total(total.unwrap_or(result.attempts.len()))?;
    for attempt in &result.attempts {
        recorder.record_attempt(attempt.clone())?;
    }
    recorder.record_result(result)
}

/// A provider returning one fixed configuration and, if scripted, a fresh
/// Run Toolset over `toolset` for each run. It counts its loads.
///
/// Without a Toolset, requested work has no means to run, so Preparing fails
/// with `RequestedWorkUnavailable` unless a Run Work Service stands in.
pub struct ToolsetConfiguration {
    pub configuration: RunConfiguration,
    pub toolset: Option<Arc<ScriptedToolset>>,
    pub loads: AtomicUsize,
}

impl ToolsetConfiguration {
    /// A provider of `configuration` and of `toolset`'s Toolsets, if any.
    pub fn new(configuration: RunConfiguration, toolset: Option<Arc<ScriptedToolset>>) -> Self {
        Self {
            configuration,
            toolset,
            loads: AtomicUsize::new(0),
        }
    }
}

impl RunConfigurationProvider for ToolsetConfiguration {
    fn load(&self, _profile_identity: &str) -> Result<RunConfiguration, Error> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        Ok(self.configuration.clone())
    }

    fn load_with_toolset(
        &self,
        profile_identity: &str,
    ) -> Result<(RunConfiguration, Option<RunToolset>), Error> {
        let configuration = self.load(profile_identity)?;
        Ok((
            configuration,
            self.toolset.as_ref().map(ScriptedToolset::toolset),
        ))
    }
}
