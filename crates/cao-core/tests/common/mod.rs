//! Scenario helpers shared by `cao-core`'s run scenarios.
//!
//! The fakes of `cao-core`'s seams live in `cao_core::test_support` (#547);
//! this fixture keeps what is not a fake. Fixture trees are real directories
//! under the target directory.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use cao_core::routing::{ExecutionMode, RequestedWork};
use cao_core::run::{
    InlineRunScheduler, ModSelection, OptimizationRunResult, OptimizationRunService, RunHandle,
    RunRequest, TemporaryArtifactRegistry,
};
use cao_core::test_support::{
    EventLog, FakeBackendFactory, FixedConfiguration, ScriptedToolset, ToolsetConfiguration,
    sse_profile,
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

/// The provider most scenarios use: the SSE-like profile.
pub fn test_configuration() -> Arc<FixedConfiguration> {
    Arc::new(FixedConfiguration {
        profile: sse_profile(),
        loads: AtomicUsize::new(0),
    })
}

/// A provider of the SSE-like profile that returns a fresh Run Toolset over
/// `toolset` to each run, or no Toolset at all when it is `None`, shared as
/// the Optimization Run Service takes it.
pub fn toolset_configuration(toolset: Option<Arc<ScriptedToolset>>) -> Arc<ToolsetConfiguration> {
    Arc::new(ToolsetConfiguration::sse(toolset))
}

/// Like [`toolset_configuration`], but the SSE-like profile prefers
/// conversion, so a run requesting native Texture work also converts its
/// convertible Textures and maintains Mesh references.
pub fn converting_configuration(
    toolset: Option<Arc<ScriptedToolset>>,
) -> Arc<ToolsetConfiguration> {
    Arc::new(ToolsetConfiguration::new(
        sse_profile().with_conversion_preference(true),
        toolset,
    ))
}

/// Runs `request` to its terminal result on the inline scheduler, loading
/// the Selected Profile and Run Toolset from `configuration`, and returns the
/// result with the events the run dispatched.
///
/// The caller holds [`serial`]: one active run is allowed per process.
pub fn run_with(
    configuration: Arc<ToolsetConfiguration>,
    request: RunRequest,
) -> (Arc<OptimizationRunResult>, EventLog) {
    let service =
        OptimizationRunService::with_scheduler(Arc::new(InlineRunScheduler), Some(configuration));
    let events = EventLog::default();
    let handle = service
        .start(request, Some(events.dispatcher()))
        .expect("the run starts");
    (handle.wait(), events)
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
