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
};
use cao_core::routing::{
    AssetOperations, ExecutionMode, MeshVariant, RequestedWork, RoutedAsset, TextureVariant,
};
use cao_core::run::{
    AssetInitializationCancelled, AssetRunAdapters, CancellationToken, ModSelection,
    RunConfiguration, RunConfigurationProvider, RunEvent, RunEventDispatcher, RunEventPayload,
    RunFailure, RunHandle, RunPhase, RunPreparation, RunRequest, RunScheduler, RunWork,
    RunWorkEvidence, RunWorkMilestones, RunWorkService, SafetyCleanupService, ScheduledRunWorker,
    SelectedProfileFacts, StandardRunScheduler, execute_asset_run,
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
/// reports a change to persist; anything else is evaluated as unchanged.
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
        panic!(
            "a fake backend never saves, but was asked to save {}",
            path.display()
        );
    }

    fn remove_texture(&mut self, path: &Path, _remove_verified: &mut dyn FnMut() -> bool) -> bool {
        panic!(
            "a fake backend never removes, but was asked to remove {}",
            path.display()
        );
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
        panic!(
            "a fake backend never saves, but was asked to save {}",
            path.display()
        );
    }

    fn optimize_animation(
        &mut self,
        path: &Path,
        output_path: Option<&Path>,
        _mode: ExecutionMode,
    ) -> OperationResult {
        assert!(
            output_path.is_none(),
            "a Dry Run Animation gets no output path"
        );
        self.loaded = Some(path.to_path_buf());
        self.record("optimize_animation", path);
        self.evaluate()
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
        milestones: &dyn RunWorkMilestones,
        stop: &CancellationToken,
    ) -> Result<(), Error> {
        let mut backend = FakeBackend::new(Arc::clone(&self.calls));
        let mut attempts = 0;
        let cancel_after = self.cancel_after.clone();
        let mut adapters =
            AssetRunAdapters::new(Box::new(move |asset: &RoutedAsset, mod_root: &Path| {
                attempts += 1;
                if let Some((after, token)) = &cancel_after
                    && attempts == *after
                {
                    token.cancel();
                }
                Ok(AssetExecutor::new(&mut backend).execute(asset, mod_root))
            }));
        if let Some((_, token)) = self.cancel_after.clone() {
            adapters.is_cancelled = Some(Box::new(move || token.is_cancelled()));
        }
        execute_asset_run(preparation, evidence, milestones, stop, &mut adapters)
    }
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
        milestones: &dyn RunWorkMilestones,
        stop: &CancellationToken,
    ) -> Result<(), Error> {
        let mut adapters =
            AssetRunAdapters::new(Box::new(|asset: &RoutedAsset, _mod_root: &Path| {
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
        execute_asset_run(preparation, evidence, milestones, stop, &mut adapters)
    }
}
