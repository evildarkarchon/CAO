//! Fakes of `cao-core`'s seams shared by the run scenarios.
//!
//! The scenarios port the C++ suites' intent, not their fixtures: each fake
//! here stands in for one ported seam trait, and fixture trees are real
//! directories under the target directory.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use cao_core::Error;
use cao_core::execution::{
    AssetExecutionBackend, AssetExecutionResult, AssetExecutor, MutationState, OperationResult,
    quarantine_failed_load,
};
use cao_core::routing::{
    AssetOperations, ExecutionMode, MeshVariant, RequestedWork, RoutedAsset, TextureVariant,
};
use cao_core::run::{
    ArchiveAdapters, ArchiveCollision, ArchiveEntry, ArchiveExtractionPlan,
    ArchiveExtractionResult, ArchiveExtractor, ArchiveReader, AssetInitializationCancelled,
    AssetRunAdapters, AssetRunProgress, CancellationToken, CapacityProbe, ModSelection,
    RunConfiguration, RunConfigurationProvider, RunDiagnostic, RunEvent, RunEventDispatcher,
    RunEventPayload, RunFailure, RunHandle, RunObservationSink, RunPhase, RunPhaseRecord,
    RunPreparation, RunRequest, RunScheduler, RunWork, RunWorkEvidence, RunWorkMilestones,
    RunWorkService, SafetyCleanupService, ScheduledRunWorker, SelectedProfileFacts,
    StandardRunScheduler, TemporaryArtifactRegistry, VolumeIdentityProbe, execute_asset_run,
};

/// Serializes scenarios that start runs: one active run is allowed per
/// process, and the test harness runs a file's tests on parallel threads.
pub fn serial() -> MutexGuard<'static, ()> {
    static SERIAL: Mutex<()> = Mutex::new(());
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A fresh, empty directory under the target directory for one scenario.
pub fn scratch_dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("cao-core")
        .join(name);
    // A missing directory is the expected case; anything else surfaces in create_dir_all.
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Writes each relative file under `root` with its own name as content.
pub fn write_tree(root: &Path, files: &[&str]) {
    for file in files {
        let path = root.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, file.as_bytes()).unwrap();
    }
}

/// Every file under `root` with its bytes, for proving a tree was not touched.
pub fn snapshot_tree(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                files.insert(
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    std::fs::read(&path).unwrap(),
                );
            }
        }
    }
    files
}

/// The canonical form of a directory, as Preparing resolves it.
pub fn canonical(path: &Path) -> PathBuf {
    cao_winfs::msvc_canonical(path).unwrap()
}

/// Creates the directory junction `link` pointing at `target`.
pub fn junction(link: &Path, target: &Path) {
    let created = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .expect("cmd runs");
    assert!(
        created.status.success(),
        "mklink /J failed: {}",
        String::from_utf8_lossy(&created.stderr)
    );
}

/// An SSE-like profile that supports every kind of work, with `.bsa` Archives.
pub fn sse_profile() -> SelectedProfileFacts {
    SelectedProfileFacts {
        archive_extension: Some(".bsa".to_owned()),
        supports_native_texture_optimization: true,
        supports_texture_conversion: true,
        supports_standard_mesh_optimization: true,
        supports_terrain_mesh_optimization: true,
        supports_animation_optimization: true,
        supports_archive_extraction: true,
        supports_mesh_reference_maintenance: true,
        supports_archive_creation: true,
    }
}

/// A provider returning one fixed configuration and counting its loads.
pub struct FixedConfiguration {
    pub configuration: RunConfiguration,
    pub loads: AtomicUsize,
}

impl RunConfigurationProvider for FixedConfiguration {
    fn load(&self, _profile_identity: &str) -> Result<RunConfiguration, Error> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        Ok(self.configuration.clone())
    }
}

/// The provider most scenarios use: the SSE-like profile.
pub fn test_configuration() -> Arc<FixedConfiguration> {
    Arc::new(FixedConfiguration {
        configuration: RunConfiguration {
            profile: sse_profile(),
            ..RunConfiguration::default()
        },
        loads: AtomicUsize::new(0),
    })
}

/// A closure standing in for a profile load.
pub type Loader = Box<dyn Fn(&str) -> Result<RunConfiguration, Error> + Send + Sync>;

/// A provider delegating to a closure, to observe thread identity or fail.
pub struct CallbackConfiguration(pub Loader);

impl RunConfigurationProvider for CallbackConfiguration {
    fn load(&self, profile_identity: &str) -> Result<RunConfiguration, Error> {
        (self.0)(profile_identity)
    }
}

/// The shared Mod Root of scenarios that need one to exist but not to hold Assets.
pub fn test_mod_root() -> PathBuf {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| scratch_dir("empty-mod-root")).clone()
}

/// A no-work Apply request whose Mod Root passes Preparing.
pub fn no_work_request() -> RunRequest {
    RunRequest::new(
        "SkyrimSE",
        ExecutionMode::Apply,
        ModSelection::SingleModRoot(test_mod_root()),
        Vec::new(),
    )
}

/// A request for one kind of work over one Mod Root.
pub fn request(mode: ExecutionMode, root: &Path, work: &[RequestedWork]) -> RunRequest {
    RunRequest::new(
        "SkyrimSE",
        mode,
        ModSelection::SingleModRoot(root.to_path_buf()),
        work.to_vec(),
    )
}

/// The events one run dispatched, shared with its dispatcher.
#[derive(Clone, Default)]
pub struct EventLog(pub Arc<Mutex<Vec<RunEvent>>>);

impl EventLog {
    /// A dispatcher appending every event here.
    pub fn dispatcher(&self) -> RunEventDispatcher {
        let events = Arc::clone(&self.0);
        Box::new(move |event| events.lock().unwrap().push(event))
    }

    pub fn events(&self) -> Vec<RunEvent> {
        self.0.lock().unwrap().clone()
    }

    /// The phases of every phase event, in dispatch order.
    pub fn phases(&self) -> Vec<RunPhase> {
        self.events()
            .into_iter()
            .filter_map(|event| match event.payload {
                RunEventPayload::Phase(record) => Some(record.phase()),
                _ => None,
            })
            .collect()
    }
}

/// A slot a dispatcher can read the run's own handle from once `start` returns.
pub type HandleSlot = Arc<OnceLock<RunHandle>>;

/// Takes the handle back out of its slot on the test thread, so it is never
/// dropped (and joined) on the run's own worker.
pub fn reclaim(slot: HandleSlot) -> RunHandle {
    Arc::into_inner(slot)
        .expect("the dispatcher released its slot")
        .into_inner()
        .expect("the handle was stored")
}

/// Runs each worker inline, counting how many workers it was asked to start.
#[derive(Default)]
pub struct CountingInlineScheduler {
    pub scheduled: AtomicUsize,
}

struct CompletedWorker;

impl ScheduledRunWorker for CompletedWorker {
    // The work ran inline before `schedule` returned; nothing is left to join.
    fn join(&self) {}
    fn is_current_thread(&self) -> bool {
        false
    }
}

impl RunScheduler for CountingInlineScheduler {
    fn schedule(&self, work: RunWork) -> Result<Box<dyn ScheduledRunWorker>, Error> {
        self.scheduled.fetch_add(1, Ordering::SeqCst);
        work();
        Ok(Box::new(CompletedWorker))
    }
}

/// Holds each worker's work until it is joined, and never runs it otherwise.
///
/// A real worker would join itself as a last resort, which would hide a
/// missing join in the Run Handle; this one makes an abandoned run visible as
/// work that never completed.
#[derive(Default)]
pub struct DeferredScheduler {
    pub joins: Arc<AtomicUsize>,
    pub completions: Arc<AtomicUsize>,
}

struct DeferredWorker {
    work: Mutex<Option<RunWork>>,
    joins: Arc<AtomicUsize>,
    completions: Arc<AtomicUsize>,
}

impl ScheduledRunWorker for DeferredWorker {
    fn join(&self) {
        self.joins.fetch_add(1, Ordering::SeqCst);
        let work = self.work.lock().unwrap().take();
        if let Some(work) = work {
            work();
            self.completions.fetch_add(1, Ordering::SeqCst);
        }
    }

    // No execution thread exists until this deterministic worker is joined.
    fn is_current_thread(&self) -> bool {
        false
    }
}

impl RunScheduler for DeferredScheduler {
    fn schedule(&self, work: RunWork) -> Result<Box<dyn ScheduledRunWorker>, Error> {
        Ok(Box::new(DeferredWorker {
            work: Mutex::new(Some(work)),
            joins: Arc::clone(&self.joins),
            completions: Arc::clone(&self.completions),
        }))
    }
}

/// Fails every scheduling attempt, as a thread-backed scheduler out of resources would.
pub struct ExhaustedScheduler;

impl RunScheduler for ExhaustedScheduler {
    fn schedule(&self, _work: RunWork) -> Result<Box<dyn ScheduledRunWorker>, Error> {
        Err(Error::Scheduling(
            "No Run Worker could be started".to_owned(),
        ))
    }
}

/// A hook a gated worker runs on its own thread.
pub type WorkerHook = Arc<Mutex<Option<Box<dyn FnMut() + Send>>>>;

/// Runs the production worker, holding it until `release` so a test can
/// inspect or cancel a run it knows is active.
pub struct GatedScheduler {
    release: Mutex<Option<mpsc::Sender<()>>>,
    gate: Mutex<Option<mpsc::Receiver<()>>>,
    pub before_work: WorkerHook,
    pub after_work: WorkerHook,
}

impl Default for GatedScheduler {
    fn default() -> Self {
        let (release, gate) = mpsc::channel();
        Self {
            release: Mutex::new(Some(release)),
            gate: Mutex::new(Some(gate)),
            before_work: Arc::default(),
            after_work: Arc::default(),
        }
    }
}

impl GatedScheduler {
    /// Lets the held worker run.
    pub fn release(&self) {
        if let Some(release) = self.release.lock().unwrap().take() {
            // The worker may already be gone (a scheduling failure never
            // started it); then there is nothing to release.
            let _ = release.send(());
        }
    }
}

impl RunScheduler for GatedScheduler {
    fn schedule(&self, work: RunWork) -> Result<Box<dyn ScheduledRunWorker>, Error> {
        let gate = self
            .gate
            .lock()
            .unwrap()
            .take()
            .expect("a gated scheduler starts one run");
        let before = Arc::clone(&self.before_work);
        let after = Arc::clone(&self.after_work);
        StandardRunScheduler.schedule(Box::new(move || {
            // A dropped scheduler disconnects the gate, which releases the
            // worker just as `release` would.
            let _ = gate.recv();
            if let Some(hook) = before.lock().unwrap().as_mut() {
                hook();
            }
            work();
            if let Some(hook) = after.lock().unwrap().as_mut() {
                hook();
            }
        }))
    }
}

/// A Safety Cleanup Service that counts its passes and returns scripted failures.
#[derive(Default)]
pub struct CountingCleanup {
    pub passes: usize,
    pub failures: Vec<RunFailure>,
    pub service_error: Option<String>,
}

impl SafetyCleanupService for CountingCleanup {
    fn perform_safety_cleanup(&mut self) -> Result<Vec<RunFailure>, Error> {
        self.passes += 1;
        match &self.service_error {
            Some(error) => Err(Error::SafetyCleanup(error.clone())),
            None => Ok(self.failures.clone()),
        }
    }
}

/// How the fake backend treats one Asset, chosen by a marker in its file name.
///
/// `unloadable` fails to load, `panics` panics while optimizing, `changes`
/// reports a change to persist; anything else is evaluated as unchanged. In
/// Apply a change is saved as `optimized <name>` (an Animation's output as
/// `converted <name>`), unless the name also contains `unsaveable`.
pub struct FakeBackend {
    pub calls: Arc<Mutex<Vec<String>>>,
    loaded: Option<PathBuf>,
}

impl FakeBackend {
    pub fn new(calls: Arc<Mutex<Vec<String>>>) -> Self {
        Self {
            calls,
            loaded: None,
        }
    }

    fn record(&self, call: &str, path: &Path) {
        let name = path.file_name().unwrap().to_string_lossy();
        self.calls.lock().unwrap().push(format!("{call} {name}"));
    }

    fn evaluate(&self) -> OperationResult {
        let name = self
            .loaded
            .as_ref()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        if name.contains("panics") {
            panic!("the backend panicked while optimizing {name}");
        }
        if name.contains("changes") {
            OperationResult::changed()
        } else {
            OperationResult::unchanged()
        }
    }

    fn load(&mut self, call: &str, path: &Path) -> bool {
        self.record(call, path);
        self.loaded = Some(path.to_path_buf());
        !path.to_string_lossy().contains("unloadable")
    }

    /// The loaded Asset's file name.
    fn loaded_name(&self) -> String {
        let loaded = self.loaded.as_ref().unwrap();
        loaded.file_name().unwrap().to_string_lossy().into_owned()
    }

    /// Writes `optimized <name>` to the staged `path`, unless the loaded
    /// Asset's name contains `unsaveable`.
    fn save(&mut self, call: &str, path: &Path) -> bool {
        self.record(call, path);
        let name = self.loaded_name();
        if name.contains("unsaveable") {
            return false;
        }
        std::fs::write(path, format!("optimized {name}")).unwrap();
        true
    }
}

impl AssetExecutionBackend for FakeBackend {
    fn load_texture(&mut self, path: &Path, _variant: TextureVariant) -> bool {
        self.load("load_texture", path)
    }

    fn optimize_texture(
        &mut self,
        _operations: AssetOperations,
        _mode: ExecutionMode,
    ) -> OperationResult {
        self.record("optimize_texture", &self.loaded.clone().unwrap());
        self.evaluate()
    }

    fn save_texture(&mut self, path: &Path) -> bool {
        self.save("save_texture", path)
    }

    fn remove_texture(&mut self, path: &Path, remove_verified: &mut dyn FnMut() -> bool) -> bool {
        self.record("remove_texture", path);
        remove_verified()
    }

    fn load_mesh(&mut self, path: &Path, _variant: MeshVariant) -> bool {
        self.load("load_mesh", path)
    }

    fn optimize_mesh(&mut self, path: &Path, _mode: ExecutionMode) -> OperationResult {
        self.record("optimize_mesh", path);
        self.evaluate()
    }

    fn maintain_mesh_references(&mut self, _mode: ExecutionMode) -> OperationResult {
        self.record("maintain_mesh_references", &self.loaded.clone().unwrap());
        OperationResult::unchanged()
    }

    fn save_mesh(&mut self, path: &Path) -> bool {
        self.save("save_mesh", path)
    }

    fn optimize_animation(
        &mut self,
        path: &Path,
        output_path: Option<&Path>,
        mode: ExecutionMode,
    ) -> OperationResult {
        assert_eq!(
            output_path.is_some(),
            mode == ExecutionMode::Apply,
            "only Apply gets an Animation output path"
        );
        self.loaded = Some(path.to_path_buf());
        self.record("optimize_animation", path);
        let result = self.evaluate();
        if let Some(output) = output_path
            && result.would_change()
        {
            std::fs::write(output, format!("converted {}", self.loaded_name())).unwrap();
        }
        result
    }
}

/// The production shape of a Run Work Service: one backend per run, driven
/// through the Asset Executor inside the Asset Run.
pub struct BackendWork {
    pub calls: Arc<Mutex<Vec<String>>>,
    /// Cancels this token after the named number of attempts have started.
    pub cancel_after: Option<(usize, CancellationToken)>,
}

impl BackendWork {
    pub fn new() -> Self {
        Self {
            calls: Arc::default(),
            cancel_after: None,
        }
    }

    /// The backend calls made so far.
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl RunWorkService for BackendWork {
    fn execute(
        &self,
        preparation: &RunPreparation,
        evidence: &RunWorkEvidence<'_, '_>,
        artifacts: &mut TemporaryArtifactRegistry,
        milestones: &dyn RunWorkMilestones,
        stop: &CancellationToken,
    ) -> Result<(), Error> {
        let mut backend = FakeBackend::new(Arc::clone(&self.calls));
        let mut attempts = 0;
        let cancel_after = self.cancel_after.clone();
        let mut adapters = AssetRunAdapters::new(Box::new(
            move |asset: &RoutedAsset, mod_root: &Path, artifacts| {
                attempts += 1;
                if let Some((after, token)) = &cancel_after
                    && attempts == *after
                {
                    token.cancel();
                }
                let result = AssetExecutor::new(&mut backend).execute(asset, artifacts, mod_root);
                Ok(quarantine_failed_load(asset, result))
            },
        ));
        if let Some((_, token)) = self.cancel_after.clone() {
            adapters.is_cancelled = Some(Box::new(move || token.is_cancelled()));
        }
        execute_asset_run(
            preparation,
            evidence,
            artifacts,
            milestones,
            stop,
            &mut adapters,
        )
    }
}

/// What a controlled attempt does, given the Routed Asset and its frozen Mod Root.
pub type AssetScript = Box<
    dyn Fn(&RoutedAsset, &Path) -> Result<AssetExecutionResult, AssetInitializationCancelled>
        + Send
        + Sync,
>;

/// A hook a controlled run calls without arguments.
pub type Hook<T> = Box<dyn Fn() -> T + Send + Sync>;

/// A hook observing one Asset Run lifecycle boundary.
pub type PhaseHook = Box<dyn Fn(&RunPhaseRecord) + Send + Sync>;

/// A hook observing one Asset Run progress update.
pub type ProgressHook = Box<dyn Fn(AssetRunProgress) + Send + Sync>;

/// A hook observing the Archive Collision plan.
pub type CollisionHook = Box<dyn Fn(&[ArchiveCollision]) + Send + Sync>;

/// A hook a [`RecordingSink`] runs on each observation.
pub type ObservationHook = Box<dyn Fn(&Observed)>;

/// The production `execute_asset_run` driven by scripted adapters, as C++
/// `ControlledAssetWork` drove `executeAssetRun`.
///
/// Discovery, routing, phase publication and Run Evidence are all real. Each
/// field left `None` keeps the adapter's default: attempts succeed without
/// mutation, nothing extra cancels, and no Archive Finalization adapter
/// exists. In Apply, `stage_temporary` stages `temporary.dds` beside the
/// first Mod Root under the run's Temporary Ownership before any Asset runs,
/// so a scenario can prove Safety Cleanup removed it.
#[derive(Default)]
pub struct ControlledWork {
    pub execute: Option<AssetScript>,
    /// Polled wherever the Asset Run checks cancellation; it may panic to
    /// stand in for a later orchestration failure.
    pub is_cancelled: Option<Hook<bool>>,
    pub report_phase: Option<PhaseHook>,
    pub report_progress: Option<ProgressHook>,
    /// The Archive Finalization adapter; its calls are counted.
    pub finalize: Option<Hook<Result<(), Error>>>,
    /// Work-specific configuration loaded during Preparing.
    pub prepare: Option<Hook<Result<(), Error>>>,
    pub stage_temporary: bool,
    /// The Archive seams; without them an Apply run selecting an Archive fails.
    pub archives: Option<ArchiveFakes>,
    /// Observes the collision plan; it may panic or cancel.
    pub report_collisions: Option<CollisionHook>,
    /// Each extraction plan handed to the extraction adapter, in order.
    pub extracted: Mutex<Vec<ArchiveExtractionPlan>>,
    /// Each attempt's execution path and the Mod Root it was handed.
    pub attempts: Mutex<Vec<(PathBuf, PathBuf)>>,
    pub executions: AtomicUsize,
    pub finalizations: AtomicUsize,
    pub staged: Mutex<Option<PathBuf>>,
}

impl ControlledWork {
    /// The Mod Roots handed to each attempt, in attempt order.
    pub fn attempted_roots(&self) -> Vec<PathBuf> {
        let attempts = self.attempts.lock().unwrap();
        attempts.iter().map(|(_, root)| root.clone()).collect()
    }

    /// The temporary file staged in Apply, which must not survive the run.
    pub fn staged_temporary(&self) -> Option<PathBuf> {
        self.staged.lock().unwrap().clone()
    }
}

impl RunWorkService for ControlledWork {
    fn prepare(&self) -> Result<(), Error> {
        self.prepare.as_ref().map_or(Ok(()), |prepare| prepare())
    }

    fn execute(
        &self,
        preparation: &RunPreparation,
        evidence: &RunWorkEvidence<'_, '_>,
        artifacts: &mut TemporaryArtifactRegistry,
        milestones: &dyn RunWorkMilestones,
        stop: &CancellationToken,
    ) -> Result<(), Error> {
        if self.stage_temporary
            && preparation.policy().execution_mode() == ExecutionMode::Apply
            && let Some(root) = preparation.mod_roots().first()
        {
            let staged = artifacts
                .stage_file(root, &root.join("temporary.dds"))
                .expect("the scenario's temporary is staged");
            std::fs::write(&staged.path, "temporary").unwrap();
            *self.staged.lock().unwrap() = Some(staged.path);
        }
        let mut adapters =
            AssetRunAdapters::new(Box::new(|asset: &RoutedAsset, mod_root: &Path, _| {
                self.attempts
                    .lock()
                    .unwrap()
                    .push((asset.execution_path().to_path_buf(), mod_root.to_path_buf()));
                self.executions.fetch_add(1, Ordering::SeqCst);
                match &self.execute {
                    Some(execute) => execute(asset, mod_root),
                    None => Ok(AssetExecutionResult::success(MutationState::None)),
                }
            }));
        if let Some(is_cancelled) = &self.is_cancelled {
            adapters.is_cancelled = Some(Box::new(is_cancelled));
        }
        if let Some(report_phase) = &self.report_phase {
            adapters.report_phase = Some(Box::new(move |record| report_phase(record)));
        }
        if let Some(report_progress) = &self.report_progress {
            adapters.report_progress = Some(Box::new(report_progress));
        }
        if let Some(finalize) = &self.finalize {
            adapters.finalize_archive_lifecycle = Some(Box::new(move |_| {
                self.finalizations.fetch_add(1, Ordering::SeqCst);
                finalize()
            }));
        }
        if let Some(fakes) = &self.archives {
            let extractor = ArchiveExtractor::new(&fakes.reader, &fakes.capacity);
            adapters.archives = Some(ArchiveAdapters {
                reader: &fakes.reader,
                capacity: &fakes.capacity,
                volume_identity: &fakes.volumes,
                extract: Box::new(move |plan, artifacts| {
                    self.extracted.lock().unwrap().push(plan.clone());
                    match &fakes.extract {
                        Some(script) => script(plan),
                        None => extractor.extract(plan, artifacts),
                    }
                }),
            });
        }
        if let Some(report_collisions) = &self.report_collisions {
            adapters.report_archive_collisions =
                Some(Box::new(move |collisions| report_collisions(collisions)));
        }
        execute_asset_run(
            preparation,
            evidence,
            artifacts,
            milestones,
            stop,
            &mut adapters,
        )
    }
}

/// One observation a [`RecordingSink`] received.
#[derive(Debug, Clone, PartialEq)]
pub enum Observed {
    Phase(RunPhaseRecord),
    Failure(RunFailure),
    Diagnostic(RunDiagnostic),
}

impl Observed {
    /// The observation's kind: `phase`, `failure` or `diagnostic`.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Phase(_) => "phase",
            Self::Failure(_) => "failure",
            Self::Diagnostic(_) => "diagnostic",
        }
    }
}

/// A Run Observation Sink that records every observation in order and runs
/// an optional hook first, which may cancel the run or panic.
#[derive(Default)]
pub struct RecordingSink {
    pub observed: Mutex<Vec<Observed>>,
    pub hook: Option<ObservationHook>,
}

impl RecordingSink {
    /// A sink running `hook` on each observation.
    pub fn with_hook(hook: impl Fn(&Observed) + 'static) -> Self {
        Self {
            observed: Mutex::default(),
            hook: Some(Box::new(hook)),
        }
    }

    pub fn observed(&self) -> Vec<Observed> {
        self.observed.lock().unwrap().clone()
    }

    fn observe(&self, observation: Observed) {
        self.observed.lock().unwrap().push(observation.clone());
        if let Some(hook) = &self.hook {
            hook(&observation);
        }
    }
}

impl RunObservationSink for RecordingSink {
    fn record_phase(&self, phase: &RunPhaseRecord) {
        self.observe(Observed::Phase(*phase));
    }

    fn record_failure(&self, failure: &RunFailure) {
        self.observe(Observed::Failure(failure.clone()));
    }

    fn record_diagnostic(&self, diagnostic: &RunDiagnostic) {
        self.observe(Observed::Diagnostic(diagnostic.clone()));
    }
}

/// `target` relative to the working directory, without changing it: the
/// harness runs tests on parallel threads, so the process-wide working
/// directory must stay put. Both paths must be on one volume.
pub fn relative_to_working_directory(target: &Path) -> PathBuf {
    let base = canonical(&std::env::current_dir().unwrap());
    let target = canonical(target);
    let common = base
        .components()
        .zip(target.components())
        .take_while(|(left, right)| left == right)
        .count();
    assert!(
        common > 0,
        "{} shares no volume with the working directory",
        target.display()
    );
    let mut relative = PathBuf::new();
    for _ in base.components().skip(common) {
        relative.push("..");
    }
    relative.extend(target.components().skip(common));
    relative
}

/// A Safety Cleanup Service that may cancel the run during its pass, then
/// returns scripted failures or panics.
#[derive(Default)]
pub struct ScriptedCleanup {
    /// Cancelled during the pass, as a user clicking Cancel during cleanup would.
    pub cancels: Option<CancellationToken>,
    pub failures: Vec<RunFailure>,
    pub panics: bool,
    pub passes: usize,
}

impl SafetyCleanupService for ScriptedCleanup {
    fn perform_safety_cleanup(&mut self) -> Result<Vec<RunFailure>, Error> {
        self.passes += 1;
        if let Some(token) = &self.cancels {
            token.cancel();
        }
        if self.panics {
            panic!("the cleanup service panicked");
        }
        Ok(self.failures.clone())
    }
}

/// Leaves the staging of a run that crashed while staging `destination`, a
/// partial sibling its manifest still owns, and returns the sibling.
pub fn crashed_run_leftover(root: &Path, destination: &str) -> PathBuf {
    let mut crashed = TemporaryArtifactRegistry::new(cao_core::run::create_run_id());
    let sibling = crashed.stage_file(root, &root.join(destination)).unwrap();
    std::fs::write(&sibling.path, "partial output").unwrap();
    // Dropping the registry without Safety Cleanup is what a crash leaves.
    sibling.path
}

/// What a scripted attempt does instead of running a backend.
pub type Script = Box<
    dyn Fn(&RoutedAsset) -> Result<AssetExecutionResult, AssetInitializationCancelled>
        + Send
        + Sync,
>;

/// A Run Work Service whose Asset attempts return scripted results, for
/// mutations and failures no Dry Run backend can produce.
pub struct ScriptedWork {
    pub script: Script,
    /// Cancels the token before the named attempt returns, as a user clicking
    /// Cancel mid-attempt would.
    pub cancel_during: Option<(usize, CancellationToken)>,
    pub attempted: Arc<Mutex<Vec<PathBuf>>>,
}

impl ScriptedWork {
    pub fn new(script: Script) -> Self {
        Self {
            script,
            cancel_during: None,
            attempted: Arc::default(),
        }
    }

    /// Scripted work that commits every attempt.
    pub fn committing() -> Self {
        Self::new(Box::new(|_| {
            Ok(AssetExecutionResult::success(MutationState::Committed))
        }))
    }
}

impl RunWorkService for ScriptedWork {
    fn execute(
        &self,
        preparation: &RunPreparation,
        evidence: &RunWorkEvidence<'_, '_>,
        artifacts: &mut TemporaryArtifactRegistry,
        milestones: &dyn RunWorkMilestones,
        stop: &CancellationToken,
    ) -> Result<(), Error> {
        let mut adapters =
            AssetRunAdapters::new(Box::new(|asset: &RoutedAsset, _mod_root: &Path, _| {
                let mut attempted = self.attempted.lock().unwrap();
                attempted.push(asset.execution_path().to_path_buf());
                if let Some((during, token)) = &self.cancel_during
                    && attempted.len() == *during
                {
                    token.cancel();
                }
                drop(attempted);
                (self.script)(asset)
            }));
        if let Some((_, token)) = self.cancel_during.clone() {
            adapters.is_cancelled = Some(Box::new(move || token.is_cancelled()));
        }
        execute_asset_run(
            preparation,
            evidence,
            artifacts,
            milestones,
            stop,
            &mut adapters,
        )
    }
}

/// The magic opening every fake Archive file.
const FAKE_ARCHIVE_MAGIC: &[u8] = b"CAO-FAKE-ARCHIVE";

/// One entry of a fake Archive: its raw manifest name, its payload, and the
/// decompressed size the manifest declares (a compressed entry declares more
/// than its stored payload).
pub struct FakeEntry<'a> {
    pub name: &'a str,
    pub payload: &'a [u8],
    pub declared_size: u64,
}

/// An entry whose declared size is its payload's length.
pub fn entry<'a>(name: &'a str, payload: &'a [u8]) -> FakeEntry<'a> {
    FakeEntry {
        name,
        payload,
        declared_size: payload.len() as u64,
    }
}

/// Writes a fake Archive [`FakeArchiveReader`] can read. Names are stored
/// raw, so a manifest can hold any unsafe or aliased name.
pub fn write_archive(path: &Path, entries: &[FakeEntry<'_>]) {
    let mut bytes = FAKE_ARCHIVE_MAGIC.to_vec();
    bytes.extend((entries.len() as u32).to_le_bytes());
    for entry in entries {
        bytes.extend((entry.name.len() as u32).to_le_bytes());
        bytes.extend(entry.name.as_bytes());
        bytes.extend(entry.declared_size.to_le_bytes());
        bytes.extend((entry.payload.len() as u32).to_le_bytes());
        bytes.extend(entry.payload);
    }
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

/// Reads a fake Archive's entries with their payloads, or `None` when the
/// file is not one.
fn read_fake_archive(path: &Path) -> Option<Vec<(ArchiveEntry, Vec<u8>)>> {
    let bytes = std::fs::read(path).ok()?;
    let mut rest = bytes.strip_prefix(FAKE_ARCHIVE_MAGIC)?;
    let mut take = |count: usize| -> Option<&[u8]> {
        let (head, tail) = rest.split_at_checked(count)?;
        rest = tail;
        Some(head)
    };
    let u32_at = |bytes: &[u8]| u32::from_le_bytes(bytes.try_into().unwrap()) as usize;
    let count = u32_at(take(4)?);
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let name_length = u32_at(take(4)?);
        let name = String::from_utf8(take(name_length)?.to_vec()).ok()?;
        let declared_size = u64::from_le_bytes(take(8)?.try_into().unwrap());
        let payload_length = u32_at(take(4)?);
        let payload = take(payload_length)?.to_vec();
        entries.push((
            ArchiveEntry {
                name,
                decompressed_size: declared_size,
            },
            payload,
        ));
    }
    Some(entries)
}

/// A hook a fake reader runs as it extracts one entry, given the Archive and
/// the raw entry name, before writing the payload.
pub type ExtractHook = Box<dyn Fn(&Path, &str) + Send + Sync>;

/// The archive reader over fake Archive files on disk.
///
/// Reading a file each time means a scenario can corrupt or change an
/// Archive between preflight and extraction just by rewriting it.
#[derive(Default)]
pub struct FakeArchiveReader {
    pub on_extract: Option<ExtractHook>,
    /// Every Archive whose manifest was listed, in order.
    pub listed: Mutex<Vec<PathBuf>>,
}

impl ArchiveReader for FakeArchiveReader {
    fn list_entries(&self, archive: &Path) -> Result<Vec<ArchiveEntry>, Error> {
        self.listed.lock().unwrap().push(archive.to_path_buf());
        read_fake_archive(archive)
            .map(|entries| entries.into_iter().map(|(entry, _)| entry).collect())
            .ok_or_else(|| Error::Archive("Unrecognized Archive format.".to_owned()))
    }

    fn extract_entry(&self, archive: &Path, entry: &str, destination: &Path) -> Result<(), Error> {
        if let Some(hook) = &self.on_extract {
            hook(archive, entry);
        }
        let entries = read_fake_archive(archive)
            .ok_or_else(|| Error::Archive("Unrecognized Archive format.".to_owned()))?;
        let (_, payload) = entries
            .into_iter()
            .find(|(listed, _)| listed.name == entry)
            .ok_or_else(|| Error::Archive(format!("The Archive has no entry {entry}.")))?;
        std::fs::write(destination, payload).map_err(|error| Error::Archive(error.to_string()))
    }
}

/// A closure answering a probe for one Mod Root.
pub type RootProbe<T> = Box<dyn Fn(&Path) -> Option<T> + Send + Sync>;

/// A capacity probe; without a closure, capacity is unknown everywhere.
#[derive(Default)]
pub struct FakeCapacity(pub Option<RootProbe<u64>>);

impl CapacityProbe for FakeCapacity {
    fn available_bytes(&self, root: &Path) -> Option<u64> {
        self.0.as_ref().and_then(|probe| probe(root))
    }
}

/// A volume-identity probe; without a closure, every volume is unknown.
#[derive(Default)]
pub struct FakeVolumes(pub Option<RootProbe<String>>);

impl VolumeIdentityProbe for FakeVolumes {
    fn volume_identity(&self, root: &Path) -> Option<String> {
        self.0.as_ref().and_then(|probe| probe(root))
    }
}

/// Replaces real extraction with a scripted result for one plan.
pub type ExtractScript =
    Box<dyn Fn(&ArchiveExtractionPlan) -> ArchiveExtractionResult + Send + Sync>;

/// The Archive seams a [`ControlledWork`] wires into its Asset Run. By
/// default extraction is the real [`ArchiveExtractor`] over these fakes.
#[derive(Default)]
pub struct ArchiveFakes {
    pub reader: FakeArchiveReader,
    pub capacity: FakeCapacity,
    pub volumes: FakeVolumes,
    pub extract: Option<ExtractScript>,
}

impl ArchiveFakes {
    /// Fakes whose capacity probe answers `capacity` for every Mod Root.
    pub fn with_capacity(capacity: impl Fn(&Path) -> Option<u64> + Send + Sync + 'static) -> Self {
        Self {
            capacity: FakeCapacity(Some(Box::new(capacity))),
            ..Self::default()
        }
    }
}
