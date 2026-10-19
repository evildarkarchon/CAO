//! Scenario helpers and the scripted work shared by `cao-core`'s run scenarios.
//!
//! The fakes of `cao-core`'s seams live in `cao_core::test_support` (#547);
//! this fixture keeps what is not a fake. Fixture trees are real directories
//! under the target directory.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use cao_core::Error;
use cao_core::execution::{AssetExecutionResult, MutationState};
use cao_core::routing::{ExecutionMode, RequestedWork, RoutedAsset};
use cao_core::run::{
    ArchiveAdapters, ArchiveCollision, ArchiveExtractionPlan, ArchiveExtractionResult,
    ArchiveExtractor, ArchiveFinalizationResult, AssetInitializationCancelled, AssetRunAdapters,
    AssetRunProgress, CancellationToken, ModSelection, RunConfiguration, RunHandle, RunPhaseRecord,
    RunPreparation, RunRequest, RunWorkEvidence, RunWorkMilestones, RunWorkService,
    SelectedProfileFacts, TemporaryArtifactRegistry, execute_asset_run,
};
use cao_core::test_support::{
    FakeArchiveReader, FakeBackendFactory, FakeCapacity, FakeVolumes, FixedConfiguration,
    ScriptedToolset, ToolsetConfiguration,
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

/// A provider of the SSE-like profile that returns a fresh Run Toolset over
/// `toolset` to each run, or no Toolset at all when it is `None`.
pub fn toolset_configuration(toolset: Option<Arc<ScriptedToolset>>) -> Arc<ToolsetConfiguration> {
    Arc::new(ToolsetConfiguration::new(
        RunConfiguration {
            profile: sse_profile(),
            ..RunConfiguration::default()
        },
        toolset,
    ))
}

/// The production shape of a run's Asset attempts: a scripted Toolset whose
/// attempts drive `factory`'s fake backend behind the production Asset
/// attempt adapter, one backend per run. Keep clones of the factory's shared
/// logs to observe it.
pub fn backend_toolset(factory: FakeBackendFactory) -> Arc<ScriptedToolset> {
    Arc::new(ScriptedToolset::default().with_backend(factory))
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
    /// A scripted Archive Finalization result, recorded as the real phase
    /// records one; it takes the place of `finalize`, and its calls are
    /// counted too.
    pub finalization: Option<Hook<ArchiveFinalizationResult>>,
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
            adapters.finalize_archive_lifecycle = Some(Box::new(move |_, _| {
                self.finalizations.fetch_add(1, Ordering::SeqCst);
                finalize()
            }));
        }
        if let Some(finalization) = &self.finalization {
            adapters.finalize_archive_lifecycle = Some(Box::new(move |recorder, _| {
                self.finalizations.fetch_add(1, Ordering::SeqCst);
                record_finalization_result(recorder, finalization(), None)
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

// Moved into `test_support`, where the scripted Run Toolset records its
// scripted results the same way.
pub use cao_core::test_support::record_finalization_result;

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

/// The Archives directly in `root`, by the fake game's two extensions.
pub fn archives_in(root: &Path) -> Vec<PathBuf> {
    let mut archives: Vec<PathBuf> = std::fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .is_some_and(|extension| extension == "bsa" || extension == "ba2")
        })
        .collect();
    archives.sort();
    archives
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
