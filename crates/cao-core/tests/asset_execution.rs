//! Asset Execution scenarios, ported from `tests/AssetExecutionTests.cpp`.
//!
//! Each scenario names its C++ origin. The fixtures are real Mod Roots under
//! the target directory and a [`HookBackend`] standing in for the Asset
//! Execution Backend seam. A contained failure is asserted on the
//! [`AssetExecutor`]'s result and the run's [`TemporaryArtifactRegistry`],
//! whose Safety Cleanup must leave no staging behind.
//!
//! A backend that panics is asserted through a whole run instead, because the
//! panic is contained at the Asset Run's backend boundary, not inside the
//! executor. There the spec (#476, #468) replaces C++: a backend exception was
//! `BackendException` with no mutation, while a panic is `BackendException`
//! with `PartialOrUnknown` mutation, unsafe to continue, because nothing can
//! establish what the interrupted call wrote.
//!
//! Deviations from the C++ shape, not from its contract:
//! - C++ `execute(asset)` cleaned an internal registry before returning. The
//!   Rust executor always stages under the run's registry, so these scenarios
//!   run its Safety Cleanup themselves.
//! - `textureCleanupFailurePreservesPrimaryFailure`: C++ cleaned staging per
//!   attempt and reported a failure there as an attempt-local cleanup
//!   failure. Rust leaves staging to Safety Cleanup, which reports it.
//!
//! Not ported, with reasons:
//! - The `(asset)` and `(asset, root)` overloads: the Rust executor has one
//!   entry point, which always takes the run's registry and the frozen Mod Root.

mod common;

use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use cao_core::Error;
use cao_core::execution::{
    AssetExecutionBackend, AssetExecutionFailure, AssetExecutionResult, AssetExecutor,
    ExecutionFailureCategory, MutationState, OperationResult,
};
use cao_core::routing::{
    AssetOperation, AssetOperations, AssetRouter, ExecutionMode, MeshVariant, OptimizerTarget,
    ProfileCapabilities, ProfileCapability, RequestedWork, RoutedAsset, RoutingDecision,
    RoutingPolicy, RoutingPolicyRequest, TextureVariant,
};
use cao_core::run::{
    AssetRunAdapters, CancellationToken, OptimizationRunResult, RunExecutor, RunFailure,
    RunFailureCode, RunOutcome, RunPhase, RunPreparation, RunServices, RunWorkEvidence,
    RunWorkMilestones, RunWorkService, TemporaryArtifactRegistry, create_run_id, execute_asset_run,
    is_staging_name,
};
use common::{CountingCleanup, canonical, scratch_dir, test_configuration};

/// Every capability a routed scenario needs.
const CAPABILITIES: &[ProfileCapability] = &[
    ProfileCapability::NativeTextureOptimization,
    ProfileCapability::ConvertibleTextureConversion,
    ProfileCapability::StandardMeshOptimization,
    ProfileCapability::TerrainMeshOptimization,
    ProfileCapability::AnimationOptimization,
    ProfileCapability::ArchiveExtraction,
    ProfileCapability::MeshReferenceMaintenance,
];

/// Routes `path` under a policy compiled for `work`, as Asset Routing would.
fn route(mode: ExecutionMode, work: &[RequestedWork], path: &Path) -> RoutedAsset {
    let policy = RoutingPolicy::compile(
        RoutingPolicyRequest::for_work(mode, work),
        ProfileCapabilities::define(".bsa", CAPABILITIES),
    )
    .expect("the scenario's policy is valid");
    match AssetRouter::new(policy).route(path) {
        RoutingDecision::Routed(asset) => asset,
        other => panic!("expected a Routed Asset, got {other:?}"),
    }
}

/// A fresh canonical Mod Root.
fn mod_root(name: &str) -> PathBuf {
    canonical(&scratch_dir(&format!("asset-execution/{name}")))
}

/// Writes `contents` at `path`, creating its parents.
fn write(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

/// The names directly inside `directory`, sorted.
fn names(directory: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// Asserts `staged` was a staging sibling of `destination`: in its folder and
/// in the reserved namespace.
fn assert_sibling(staged: &Path, destination: &Path) {
    assert_eq!(
        staged.parent(),
        destination.parent(),
        "{}",
        staged.display()
    );
    assert!(
        is_staging_name(staged.file_name().unwrap()),
        "{}",
        staged.display()
    );
    assert_ne!(staged, destination);
}

/// Occupies the staging manifest's fixed scratch name, so the next ownership
/// release fails after its destination has already been published.
fn block_release(root: &Path) -> bool {
    std::fs::write(
        root.join(".cao-staging").join("ownership.manifest.next"),
        "occupied scratch",
    )
    .is_ok()
}

type LoadHook = Box<dyn FnMut(&Path) -> bool + Send>;
type SaveHook = Box<dyn FnMut(&Path) -> bool + Send>;
type RemoveHook = Box<dyn FnMut(&Path, &mut dyn FnMut() -> bool) -> bool + Send>;
type AnimationHook = Box<dyn FnMut(Option<&Path>) -> OperationResult + Send>;

/// An Asset Execution Backend whose every call can be scripted.
///
/// Unscripted, it loads anything, reports `operation` from every optimize or
/// maintenance call, saves `saved <name>` into the staged path, removes a
/// converted source through the verified removal, and writes `converted`
/// to an Apply Animation output when the operation reports a change.
/// `panic_at` names one call that panics instead.
struct HookBackend {
    operation: OperationResult,
    panic_at: Option<&'static str>,
    load: Option<LoadHook>,
    save: Option<SaveHook>,
    remove: Option<RemoveHook>,
    animation: Option<AnimationHook>,
    calls: Vec<String>,
    texture_variant: Option<TextureVariant>,
    texture_operations: Option<AssetOperations>,
    mesh_variant: Option<MeshVariant>,
    modes: Vec<ExecutionMode>,
    loaded: Option<PathBuf>,
    saved: Option<PathBuf>,
}

impl Default for HookBackend {
    fn default() -> Self {
        Self {
            operation: OperationResult::changed(),
            panic_at: None,
            load: None,
            save: None,
            remove: None,
            animation: None,
            calls: Vec::new(),
            texture_variant: None,
            texture_operations: None,
            mesh_variant: None,
            modes: Vec::new(),
            loaded: None,
            saved: None,
        }
    }
}

impl HookBackend {
    /// Records one call and panics if it is the scripted panic.
    fn enter(&mut self, call: &'static str) {
        self.calls.push(call.to_owned());
        if self.panic_at == Some(call) {
            panic!("{call} backend panicked");
        }
    }

    fn count(&self, call: &str) -> usize {
        self.calls.iter().filter(|made| *made == call).count()
    }

    fn load(&mut self, call: &'static str, path: &Path) -> bool {
        self.enter(call);
        self.loaded = Some(path.to_path_buf());
        match self.load.as_mut() {
            Some(hook) => hook(path),
            None => true,
        }
    }

    fn save(&mut self, call: &'static str, path: &Path) -> bool {
        self.saved = Some(path.to_path_buf());
        self.enter(call);
        if let Some(hook) = self.save.as_mut() {
            return hook(path);
        }
        let name = self.loaded.as_ref().unwrap().file_name().unwrap();
        std::fs::write(path, format!("saved {}", name.to_string_lossy())).unwrap();
        true
    }
}

impl AssetExecutionBackend for HookBackend {
    fn load_texture(&mut self, path: &Path, variant: TextureVariant) -> bool {
        self.texture_variant = Some(variant);
        self.load("load_texture", path)
    }

    fn optimize_texture(
        &mut self,
        operations: AssetOperations,
        mode: ExecutionMode,
    ) -> OperationResult {
        self.texture_operations = Some(operations);
        self.modes.push(mode);
        self.enter("optimize_texture");
        self.operation.clone()
    }

    fn save_texture(&mut self, path: &Path) -> bool {
        self.save("save_texture", path)
    }

    fn remove_texture(&mut self, path: &Path, remove_verified: &mut dyn FnMut() -> bool) -> bool {
        self.enter("remove_texture");
        match self.remove.as_mut() {
            Some(hook) => hook(path, remove_verified),
            None => remove_verified(),
        }
    }

    fn texture_failure_detail(&self) -> String {
        "texture service detail".to_owned()
    }

    fn load_mesh(&mut self, path: &Path, variant: MeshVariant) -> bool {
        self.mesh_variant = Some(variant);
        self.load("load_mesh", path)
    }

    fn optimize_mesh(&mut self, _path: &Path, mode: ExecutionMode) -> OperationResult {
        self.modes.push(mode);
        self.enter("optimize_mesh");
        self.operation.clone()
    }

    fn maintain_mesh_references(&mut self, mode: ExecutionMode) -> OperationResult {
        self.modes.push(mode);
        self.enter("maintain_mesh_references");
        self.operation.clone()
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
        self.modes.push(mode);
        self.loaded = Some(path.to_path_buf());
        self.saved = output_path.map(Path::to_path_buf);
        self.enter("optimize_animation");
        if let Some(hook) = self.animation.as_mut() {
            return hook(output_path);
        }
        let result = self.operation.clone();
        if let Some(output) = output_path
            && result.would_change()
        {
            std::fs::write(output, "converted").unwrap();
        }
        result
    }
}

/// Executes one Routed Asset against `root` under a fresh run registry,
/// returning the result and the registry so the scenario can clean it up.
fn execute(
    asset: &RoutedAsset,
    backend: &mut HookBackend,
    root: &Path,
) -> (AssetExecutionResult, TemporaryArtifactRegistry) {
    let mut artifacts = TemporaryArtifactRegistry::new(create_run_id());
    let result = AssetExecutor::new(backend).execute(asset, &mut artifacts, root);
    (result, artifacts)
}

/// Executes one Routed Asset and performs the run's Safety Cleanup, which
/// must succeed without failures.
fn execute_and_clean(
    asset: &RoutedAsset,
    backend: &mut HookBackend,
    root: &Path,
) -> AssetExecutionResult {
    let (result, mut artifacts) = execute(asset, backend, root);
    let failures = artifacts.cleanup();
    assert!(failures.is_empty(), "{failures:?}");
    result
}

/// A Run Work Service driving every Routed Asset through one shared backend.
struct BackendRun(Mutex<HookBackend>);

impl BackendRun {
    fn backend(&self) -> MutexGuard<'_, HookBackend> {
        // A contained backend panic poisons the lock; its state is still whole.
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl RunWorkService for BackendRun {
    fn execute(
        &self,
        preparation: &RunPreparation,
        evidence: &RunWorkEvidence<'_, '_>,
        artifacts: &mut TemporaryArtifactRegistry,
        milestones: &dyn RunWorkMilestones,
        stop: &CancellationToken,
    ) -> Result<(), Error> {
        let mut adapters = AssetRunAdapters::new(Box::new(|asset, mod_root, artifacts| {
            Ok(AssetExecutor::new(&mut *self.backend()).execute(asset, artifacts, mod_root))
        }));
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

/// Runs `work` over `root` through the Run Executor, Safety Cleanup included.
fn run(
    root: &Path,
    mode: ExecutionMode,
    requested: &[RequestedWork],
    work: &BackendRun,
) -> OptimizationRunResult {
    let mut cleanup = CountingCleanup::default();
    RunExecutor.execute(
        &common::request(mode, root, requested),
        RunServices {
            safety_cleanup: &mut cleanup,
            observations: None,
            configuration: Some(&*test_configuration()),
            work: Some(work),
        },
        &CancellationToken::new(),
        create_run_id(),
    )
}

/// Asserts the one attempt of `result` is the spec's contained panic:
/// `BackendException`, unknown mutation, unsafe, and a Failed run.
fn assert_contained_panic(result: &OptimizationRunResult, boundary: &str, path: &Path) {
    assert_eq!(result.outcome(), RunOutcome::Failed, "{boundary}");
    assert_eq!(result.asset_attempts().len(), 1, "{boundary}");
    let attempt = &result.asset_attempts()[0].result;
    assert_eq!(
        attempt.failure(),
        Some(AssetExecutionFailure::BackendException),
        "{boundary}"
    );
    assert_eq!(
        attempt.failure_category(),
        Some(ExecutionFailureCategory::Contract),
        "{boundary}"
    );
    assert_eq!(attempt.phase(), RunPhase::ProcessingAssets, "{boundary}");
    assert_eq!(
        attempt.mutation_state(),
        MutationState::PartialOrUnknown,
        "{boundary}"
    );
    assert!(!attempt.safe_to_continue(), "{boundary}");
    assert_eq!(attempt.affected_path(), path, "{boundary}");
    assert!(
        attempt
            .message()
            .contains(&format!("{boundary} backend panicked")),
        "{boundary}: {}",
        attempt.message()
    );
    assert!(result.cleanup_failures().is_empty(), "{boundary}");
}

/// Origin: AssetExecutionTests::failedTextureSavePreservesOriginal.
#[test]
fn a_failed_texture_save_preserves_the_original_and_its_staging_is_removed() {
    let root = mod_root("texture-save-failure");
    let source = root.join("native.dds");
    write(&source, "original");
    let mut backend = HookBackend {
        save: Some(Box::new(|path| {
            std::fs::write(path, "partial output").unwrap();
            false
        })),
        ..HookBackend::default()
    };
    let asset = route(
        ExecutionMode::Apply,
        &[RequestedWork::NativeTextureOptimization],
        &source,
    );

    let result = execute_and_clean(&asset, &mut backend, &root);

    assert_eq!(result.failure(), Some(AssetExecutionFailure::SaveFailed));
    assert_eq!(result.mutation_state(), MutationState::None);
    assert!(result.safe_to_continue());
    assert_eq!(result.affected_path(), source);
    assert_eq!(read(&source), "original");
    assert_eq!(backend.count("remove_texture"), 0);
    assert!(!backend.saved.unwrap().exists());
}

/// Origin: AssetExecutionTests::textureSourceRemovalFailure (the five
/// non-throwing rows). A removal that fails after the converted output was
/// committed keeps the mutation Committed only while both files are intact.
/// The pinned source denies writers, so the truncation and overwrite rows
/// leave it intact.
#[test]
fn a_failed_source_removal_after_commit_reports_what_survived() {
    // (row, damage, usable)
    let rows = [
        ("both usable", 0, true),
        ("source missing", 1, false),
        ("output missing", 2, false),
        ("source truncation attempt", 3, true),
        ("source overwrite attempt", 4, true),
    ];
    for (row, damage, usable) in rows {
        let root = mod_root(&format!("texture-removal-{damage}"));
        let source = root.join("source.tga");
        let output = root.join("source.dds");
        write(&source, "original");
        let denied = Arc::new(Mutex::new(None));
        let mut backend = HookBackend {
            save: Some(Box::new(|path| {
                std::fs::write(path, "converted").unwrap();
                true
            })),
            remove: Some(Box::new({
                let (source, output, denied) = (source.clone(), output.clone(), denied.clone());
                move |_, _| {
                    // The output was committed before removal was requested.
                    assert_eq!(read(&output), "converted");
                    match damage {
                        1 => std::fs::remove_file(&source).unwrap(),
                        2 => std::fs::remove_file(&output).unwrap(),
                        3 => {
                            let opened =
                                OpenOptions::new().write(true).truncate(true).open(&source);
                            *denied.lock().unwrap() = Some(opened.is_err());
                        }
                        4 => {
                            let written = OpenOptions::new()
                                .write(true)
                                .open(&source)
                                .and_then(|mut file| file.write_all(b"damaged!"));
                            *denied.lock().unwrap() = Some(written.is_err());
                        }
                        _ => {}
                    }
                    false
                }
            })),
            ..HookBackend::default()
        };
        let asset = route(
            ExecutionMode::Apply,
            &[RequestedWork::ConvertibleTextureConversion],
            &source,
        );

        let result = execute_and_clean(&asset, &mut backend, &root);

        assert_eq!(
            result.failure(),
            Some(AssetExecutionFailure::SourceRemovalFailed),
            "{row}"
        );
        if damage >= 3 {
            assert_eq!(
                *denied.lock().unwrap(),
                Some(true),
                "{row}: the pin denies writers"
            );
        }
        let expected = if usable {
            MutationState::Committed
        } else {
            MutationState::PartialOrUnknown
        };
        assert_eq!(result.mutation_state(), expected, "{row}");
        assert_eq!(result.safe_to_continue(), usable, "{row}");
        assert_eq!(result.affected_path(), source, "{row}");
        assert_eq!(result.operation(), "remove_texture_source", "{row}");
        if damage != 2 {
            assert_eq!(read(&output), "converted", "{row}");
        }
        if usable {
            assert_eq!(read(&source), "original", "{row}");
        }
    }
}

/// Origin: AssetExecutionTests::textureSourceRemovalFailure (the two throwing
/// rows), under the spec's panic rule: a removal that panics after the
/// commit is an unsafe attempt with unknown mutation. The committed output is
/// kept, and so is a source the panic left behind.
#[test]
fn a_panicking_source_removal_is_unsafe_and_keeps_the_committed_output() {
    for (row, removes_source) in [("after commit", false), ("after source loss", true)] {
        let root = mod_root(&format!("texture-removal-panic-{removes_source}"));
        let source = root.join("source.tga");
        let output = root.join("source.dds");
        write(&source, "original");
        let work = BackendRun(Mutex::new(HookBackend {
            remove: Some(Box::new(move |_, remove_verified| {
                if removes_source {
                    assert!(remove_verified());
                }
                panic!("remove_texture backend panicked")
            })),
            ..HookBackend::default()
        }));

        let result = run(
            &root,
            ExecutionMode::Apply,
            &[RequestedWork::ConvertibleTextureConversion],
            &work,
        );

        assert_contained_panic(&result, "remove_texture", &source);
        assert_eq!(read(&output), "saved source.tga", "{row}");
        assert_eq!(source.exists(), !removes_source, "{row}");
        let expected: &[&str] = if removes_source {
            &[".cao-staging", "source.dds"]
        } else {
            &[".cao-staging", "source.dds", "source.tga"]
        };
        assert_eq!(names(&root), expected, "{row}: no staging sibling survives");
    }
}

/// Origin: AssetExecutionTests::convertibleSourceReplacementPreservesNewcomer.
/// A source swapped during load still yields the output made from the bytes
/// that were loaded, but removal never deletes the newcomer.
#[test]
fn a_source_replaced_during_load_is_never_removed() {
    let root = mod_root("texture-source-newcomer");
    let source = root.join("source.tga");
    let old = root.join("old-source.tga");
    write(&source, "original");
    let loaded = Arc::new(Mutex::new(String::new()));
    let mut backend = HookBackend {
        load: Some(Box::new({
            let (source, old, loaded) = (source.clone(), old.clone(), loaded.clone());
            move |_| {
                *loaded.lock().unwrap() = read(&source);
                std::fs::rename(&source, &old).unwrap();
                std::fs::write(&source, "newcomer").unwrap();
                true
            }
        })),
        save: Some(Box::new({
            let loaded = loaded.clone();
            move |path| {
                std::fs::write(path, format!("{} converted", loaded.lock().unwrap())).unwrap();
                true
            }
        })),
        ..HookBackend::default()
    };
    let asset = route(
        ExecutionMode::Apply,
        &[RequestedWork::ConvertibleTextureConversion],
        &source,
    );

    let result = execute_and_clean(&asset, &mut backend, &root);

    assert_eq!(
        result.failure(),
        Some(AssetExecutionFailure::SourceRemovalFailed)
    );
    assert_eq!(result.mutation_state(), MutationState::Committed);
    assert_eq!(read(&root.join("source.dds")), "original converted");
    assert_eq!(read(&source), "newcomer");
    assert_eq!(read(&old), "original");
}

/// Origin: AssetExecutionTests::nativeDestinationReplacementDuringLoadIsRejected.
#[test]
fn a_native_texture_replaced_during_load_is_never_published_over() {
    let root = mod_root("texture-destination-newcomer");
    let native = root.join("native.dds");
    let old = root.join("old-native.dds");
    write(&native, "original");
    let loaded = Arc::new(Mutex::new(String::new()));
    let mut backend = HookBackend {
        load: Some(Box::new({
            let (native, old, loaded) = (native.clone(), old.clone(), loaded.clone());
            move |_| {
                *loaded.lock().unwrap() = read(&native);
                std::fs::rename(&native, &old).unwrap();
                std::fs::write(&native, "newcomer").unwrap();
                true
            }
        })),
        save: Some(Box::new({
            let loaded = loaded.clone();
            move |path| {
                std::fs::write(path, format!("{} optimized", loaded.lock().unwrap())).unwrap();
                true
            }
        })),
        ..HookBackend::default()
    };
    let asset = route(
        ExecutionMode::Apply,
        &[RequestedWork::NativeTextureOptimization],
        &native,
    );

    let result = execute_and_clean(&asset, &mut backend, &root);

    assert!(!result.succeeded());
    assert_eq!(result.mutation_state(), MutationState::None);
    assert_eq!(read(&native), "newcomer");
    assert_eq!(read(&old), "original");
}

/// Origin: AssetExecutionTests::textureSaveException, under the spec's panic
/// rule. The original is untouched and Safety Cleanup removes the partial
/// staged output.
#[test]
fn a_panicking_texture_save_is_unsafe_and_leaves_the_original_whole() {
    let root = mod_root("texture-save-panic");
    let source = root.join("native.dds");
    write(&source, "original");
    let work = BackendRun(Mutex::new(HookBackend {
        save: Some(Box::new(|path| {
            std::fs::write(path, "partial output").unwrap();
            panic!("save_texture backend panicked")
        })),
        ..HookBackend::default()
    }));

    let result = run(
        &root,
        ExecutionMode::Apply,
        &[RequestedWork::NativeTextureOptimization],
        &work,
    );

    assert_contained_panic(&result, "save_texture", &source);
    assert_eq!(read(&source), "original");
    let staged = work.backend().saved.clone().unwrap();
    assert_sibling(&staged, &source);
    assert!(
        !staged.exists(),
        "Safety Cleanup removed the partial output"
    );
}

/// Origin: AssetExecutionTests::textureOperationFailureDetails.
#[test]
fn a_texture_operation_failure_keeps_its_message_apart_from_the_service_detail() {
    let mut backend = HookBackend {
        operation: OperationResult::failed("synthetic Texture service error"),
        ..HookBackend::default()
    };
    let asset = route(
        ExecutionMode::DryRun,
        &[RequestedWork::NativeTextureOptimization],
        Path::new("fixture.dds"),
    );

    let result = execute_and_clean(&asset, &mut backend, Path::new(""));

    assert_eq!(
        result.failure(),
        Some(AssetExecutionFailure::OperationFailed)
    );
    assert_eq!(result.message(), "Failed to optimize Texture.");
    assert_eq!(result.service_detail(), "synthetic Texture service error");
    assert_eq!(result.operation(), "optimize_texture");
}

/// Origin: AssetExecutionTests::textureStagingIsRegisteredBeforeSave.
#[test]
fn the_texture_writer_receives_registered_staging_that_safety_cleanup_removes() {
    let root = mod_root("texture-staging-registered");
    let source = root.join("native.dds");
    write(&source, "original");
    let reserved = Arc::new(Mutex::new(false));
    let mut backend = HookBackend {
        save: Some(Box::new({
            let reserved = reserved.clone();
            move |path| {
                *reserved.lock().unwrap() = path.is_file();
                std::fs::write(path, "partial output").unwrap();
                false
            }
        })),
        ..HookBackend::default()
    };
    let asset = route(
        ExecutionMode::Apply,
        &[RequestedWork::NativeTextureOptimization],
        &source,
    );

    let (result, mut artifacts) = execute(&asset, &mut backend, &root);

    assert!(
        *reserved.lock().unwrap(),
        "the staged file existed before the save"
    );
    assert_eq!(result.failure(), Some(AssetExecutionFailure::SaveFailed));
    let staged = backend.saved.clone().unwrap();
    assert!(
        staged.exists(),
        "the run's Safety Cleanup owns the staged file"
    );
    assert!(artifacts.cleanup().is_empty());
    assert!(!staged.exists());
    assert_eq!(read(&source), "original");
}

/// Origin: AssetExecutionTests::textureCommitFailure, meshCommitFailure and
/// animationCommitFailure. A destination obstructed by an unowned directory
/// fails without mutation, safe to continue, and the obstruction is kept.
#[test]
fn a_destination_obstructed_by_a_directory_is_refused_without_mutation() {
    // (row, requested work, source, destination, failing boundary)
    let rows = [
        (
            "texture conversion",
            RequestedWork::ConvertibleTextureConversion,
            "source.tga",
            "source.dds",
            "commit_texture",
        ),
        (
            "mesh",
            RequestedWork::StandardMeshOptimization,
            "actor.nif",
            "actor.nif",
            "commit_mesh",
        ),
        (
            "animation",
            RequestedWork::AnimationOptimization,
            "Walk.hkx",
            "Walk.hkx",
            "commit_animation",
        ),
    ];
    for (row, requested, source, destination, operation) in rows {
        let root = mod_root(&format!("obstructed-{destination}"));
        let source = root.join(source);
        let destination = root.join(destination);
        write(&destination.join("unowned"), "keep");
        if source != destination {
            write(&source, "original");
        }
        let mut backend = HookBackend::default();
        let asset = route(ExecutionMode::Apply, &[requested], &source);

        let result = execute_and_clean(&asset, &mut backend, &root);

        assert_eq!(
            result.failure(),
            Some(AssetExecutionFailure::CommitFailed),
            "{row}"
        );
        assert_eq!(result.operation(), operation, "{row}");
        assert_eq!(result.affected_path(), destination, "{row}");
        assert_eq!(result.mutation_state(), MutationState::None, "{row}");
        assert!(result.safe_to_continue(), "{row}: {result:?}");
        assert_eq!(read(&destination.join("unowned")), "keep", "{row}");
        assert_eq!(backend.count("remove_texture"), 0, "{row}");
        if source != destination {
            assert_eq!(read(&source), "original", "{row}");
        }
        assert!(
            names(&root)
                .iter()
                .all(|name| !is_staging_name(name.as_ref()) || name == ".cao-staging"),
            "{row}: {:?}",
            names(&root)
        );
    }
}

/// Origin: AssetExecutionTests::texturePublicationReleaseFailure,
/// meshPublicationReleaseFailure and animationPublicationReleaseFailure. The
/// destination is replaced, but releasing its temporary name fails: the
/// mutation is Committed and unsafe, the bytes survive Safety Cleanup, and a
/// converted source is never removed.
#[test]
fn a_failed_release_after_publication_keeps_the_committed_output() {
    // (row, requested work, source, destination, expected bytes, operation)
    let rows = [
        (
            "native Texture",
            vec![RequestedWork::NativeTextureOptimization],
            "native.dds",
            "native.dds",
            "saved native.dds",
            "commit_texture",
        ),
        (
            "convertible Texture",
            vec![RequestedWork::ConvertibleTextureConversion],
            "source.tga",
            "source.dds",
            "saved source.tga",
            "commit_texture",
        ),
        (
            "Mesh optimization",
            vec![RequestedWork::StandardMeshOptimization],
            "actor.nif",
            "actor.nif",
            "saved actor.nif",
            "commit_mesh",
        ),
        (
            "Mesh Reference Maintenance",
            vec![RequestedWork::ConvertibleTextureConversion],
            "actor.nif",
            "actor.nif",
            "saved actor.nif",
            "commit_mesh",
        ),
        (
            "Animation",
            vec![RequestedWork::AnimationOptimization],
            "Walk.hkx",
            "Walk.hkx",
            "converted",
            "commit_animation",
        ),
    ];
    for (index, (row, requested, source, destination, expected, operation)) in
        rows.into_iter().enumerate()
    {
        let root = mod_root(&format!("release-failure-{index}"));
        let source = root.join(source);
        let destination = root.join(destination);
        write(&source, "original");
        if source != destination {
            write(&destination, "old destination");
        }
        // Writes the staged bytes, then blocks the release that follows publication.
        let saving = {
            let root = root.clone();
            move |path: &Path| {
                std::fs::write(path, expected).unwrap();
                block_release(&root)
            }
        };
        let mut backend = HookBackend::default();
        if operation == "commit_animation" {
            backend.animation = Some(Box::new(move |output| {
                if saving(output.unwrap()) {
                    OperationResult::changed()
                } else {
                    OperationResult::failed("scratch setup failed")
                }
            }));
        } else {
            backend.save = Some(Box::new(saving));
        }
        let asset = route(ExecutionMode::Apply, &requested, &source);

        let (result, mut artifacts) = execute(&asset, &mut backend, &root);

        assert_eq!(
            result.failure(),
            Some(AssetExecutionFailure::CommitFailed),
            "{row}: {result:?}"
        );
        assert_eq!(result.mutation_state(), MutationState::Committed, "{row}");
        assert!(!result.safe_to_continue(), "{row}");
        assert_eq!(result.operation(), operation, "{row}");
        assert!(!result.service_detail().is_empty(), "{row}");
        assert_eq!(read(&destination), expected, "{row}");
        assert_eq!(backend.count("remove_texture"), 0, "{row}");
        if source != destination {
            assert_eq!(read(&source), "original", "{row}");
        }
        assert!(!backend.saved.clone().unwrap().exists(), "{row}");
        assert!(artifacts.cleanup().is_empty(), "{row}");
        assert_eq!(read(&destination), expected, "{row}");
    }
}

/// Origin: AssetExecutionTests::textureCleanupFailurePreservesPrimaryFailure.
/// The primary failure stays the save failure; the cleanup failure belongs to
/// Safety Cleanup, which keeps the unregistered content it found.
#[test]
fn a_staging_cleanup_failure_keeps_the_primary_failure_and_unregistered_content() {
    let root = mod_root("texture-cleanup-failure");
    let source = root.join("native.dds");
    write(&source, "original");
    let mut backend = HookBackend {
        save: Some(Box::new(|path| {
            std::fs::remove_file(path).unwrap();
            write(&path.join("unregistered"), "keep");
            false
        })),
        ..HookBackend::default()
    };
    let asset = route(
        ExecutionMode::Apply,
        &[RequestedWork::NativeTextureOptimization],
        &source,
    );

    let (result, mut artifacts) = execute(&asset, &mut backend, &root);

    assert_eq!(result.failure(), Some(AssetExecutionFailure::SaveFailed));
    let failures: Vec<RunFailure> = artifacts.cleanup();
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert_eq!(
        failures[0].code,
        RunFailureCode::TemporaryArtifactCleanupFailed
    );
    let staged = backend.saved.clone().unwrap();
    assert_eq!(read(&staged.join("unregistered")), "keep");
    assert_eq!(read(&source), "original");
}

/// Origin: AssetExecutionTests::textureReadOnlyException (both rows), under
/// the spec's panic rule. A panic while loading or optimizing saves nothing.
#[test]
fn a_panicking_texture_load_or_optimization_saves_nothing() {
    for boundary in ["load_texture", "optimize_texture"] {
        let root = mod_root(&format!("texture-{boundary}-panic"));
        let source = root.join("native.dds");
        write(&source, "original");
        let work = BackendRun(Mutex::new(HookBackend {
            panic_at: Some(boundary),
            ..HookBackend::default()
        }));

        let result = run(
            &root,
            ExecutionMode::Apply,
            &[RequestedWork::NativeTextureOptimization],
            &work,
        );

        assert_contained_panic(&result, boundary, &source);
        assert_eq!(work.backend().count("save_texture"), 0, "{boundary}");
        assert_eq!(read(&source), "original", "{boundary}");
    }
}

/// Origin: AssetExecutionTests::nativeTextureCommit.
#[test]
fn a_native_texture_is_saved_to_staging_and_then_replaces_the_source() {
    let root = mod_root("texture-native-commit");
    let source = root.join("native.dds");
    write(&source, "original");
    let mut backend = HookBackend {
        save: Some(Box::new({
            let source = source.clone();
            move |path| {
                // The source is only replaced once the staged save completes.
                assert_ne!(path, source);
                assert_eq!(read(&source), "original");
                std::fs::write(path, "optimized").unwrap();
                true
            }
        })),
        ..HookBackend::default()
    };
    let asset = route(
        ExecutionMode::Apply,
        &[RequestedWork::NativeTextureOptimization],
        &source,
    );

    let result = execute_and_clean(&asset, &mut backend, &root);

    assert!(result.succeeded(), "{result:?}");
    assert_eq!(result.mutation_state(), MutationState::Committed);
    assert_eq!(read(&source), "optimized");
    assert_eq!(backend.count("remove_texture"), 0);
    let staged = backend.saved.unwrap();
    assert_sibling(&staged, &source);
    assert!(!staged.exists());
}

/// Origin: AssetExecutionTests::conversionOnlyTextureExecution (both rows).
/// Conversion loads the Texture as Convertible and requests conversion only;
/// Apply commits the destination before removing the source, and Dry Run
/// neither saves, removes nor stages.
#[test]
fn conversion_only_work_converts_in_apply_and_only_evaluates_in_dry_run() {
    for (mode, saves) in [(ExecutionMode::Apply, 1), (ExecutionMode::DryRun, 0)] {
        let root = mod_root(&format!("texture-conversion-only-{mode:?}"));
        let source = root.join("Source.Name.TgA");
        let destination = root.join("Source.Name.dds");
        write(&source, "original");
        write(&destination, "old destination");
        let mut backend = HookBackend {
            save: Some(Box::new({
                let destination = destination.clone();
                move |path| {
                    assert_sibling(path, &destination);
                    assert_eq!(read(&destination), "old destination");
                    std::fs::write(path, "converted").unwrap();
                    true
                }
            })),
            remove: Some(Box::new({
                let destination = destination.clone();
                move |_, remove_verified| {
                    assert_eq!(read(&destination), "converted");
                    remove_verified()
                }
            })),
            ..HookBackend::default()
        };
        let asset = route(
            mode,
            &[RequestedWork::ConvertibleTextureConversion],
            &source,
        );

        let result = execute_and_clean(&asset, &mut backend, &root);

        assert!(result.succeeded(), "{mode:?}: {result:?}");
        assert_eq!(backend.count("load_texture"), 1);
        assert_eq!(backend.count("optimize_texture"), 1);
        assert_eq!(backend.texture_variant, Some(TextureVariant::Convertible));
        let operations = backend.texture_operations.unwrap();
        assert!(!operations.contains(AssetOperation::Optimization));
        assert!(operations.contains(AssetOperation::Conversion));
        assert_eq!(backend.modes, [mode]);
        assert_eq!(backend.count("save_texture"), saves);
        assert_eq!(backend.count("remove_texture"), saves);
        if mode == ExecutionMode::Apply {
            assert_eq!(result.mutation_state(), MutationState::Committed);
            assert_eq!(read(&destination), "converted");
            assert!(!source.exists());
            assert!(!backend.saved.unwrap().exists());
        } else {
            assert_eq!(result.mutation_state(), MutationState::None);
            assert_eq!(read(&source), "original");
            assert_eq!(read(&destination), "old destination");
            assert!(!root.join(".cao-staging").exists());
        }
    }
}

/// Origin: AssetExecutionTests::failedMeshSavePreservesOriginal.
#[test]
fn a_failed_mesh_save_preserves_the_original() {
    let root = mod_root("mesh-save-failure");
    let source = root.join("actor.nif");
    write(&source, "original");
    let mut backend = HookBackend {
        save: Some(Box::new(|path| {
            std::fs::write(path, "partial output").unwrap();
            false
        })),
        ..HookBackend::default()
    };
    let asset = route(
        ExecutionMode::Apply,
        &[RequestedWork::StandardMeshOptimization],
        &source,
    );

    let result = execute_and_clean(&asset, &mut backend, &root);

    assert_eq!(result.failure(), Some(AssetExecutionFailure::SaveFailed));
    assert_eq!(result.mutation_state(), MutationState::None);
    assert!(result.safe_to_continue());
    assert_eq!(result.affected_path(), source);
    assert_eq!(result.operation(), "save_mesh");
    assert_eq!(read(&source), "original");
    assert!(!backend.saved.unwrap().exists());
}

/// Origin: AssetExecutionTests::meshBackendException (all four rows), under
/// the spec's panic rule. Nothing is saved before the panicking call, and the
/// original survives.
#[test]
fn a_panicking_mesh_call_is_unsafe_and_preserves_the_original() {
    for boundary in [
        "load_mesh",
        "optimize_mesh",
        "maintain_mesh_references",
        "save_mesh",
    ] {
        let root = mod_root(&format!("mesh-{boundary}-panic"));
        let source = root.join("actor.nif");
        write(&source, "original");
        let mut backend = HookBackend {
            panic_at: Some(boundary),
            ..HookBackend::default()
        };
        if boundary == "save_mesh" {
            backend.panic_at = None;
            backend.save = Some(Box::new(|path| {
                std::fs::write(path, "partial output").unwrap();
                panic!("save_mesh backend panicked")
            }));
        }
        let work = BackendRun(Mutex::new(backend));

        let result = run(
            &root,
            ExecutionMode::Apply,
            &[
                RequestedWork::StandardMeshOptimization,
                RequestedWork::ConvertibleTextureConversion,
            ],
            &work,
        );

        assert_contained_panic(&result, boundary, &source);
        assert_eq!(read(&source), "original", "{boundary}");
        let backend = work.backend();
        if boundary == "save_mesh" {
            assert!(!backend.saved.clone().unwrap().exists(), "{boundary}");
        } else {
            assert_eq!(backend.count("save_mesh"), 0, "{boundary}");
        }
    }
}

/// Origin: AssetExecutionTests::meshOperationFailure (all three rows).
#[test]
fn an_ordinary_mesh_failure_names_its_operation_and_is_safe_to_continue() {
    for boundary in ["load_mesh", "optimize_mesh", "maintain_mesh_references"] {
        let root = mod_root(&format!("mesh-{boundary}-failure"));
        let source = root.join("actor.nif");
        write(&source, "original");
        let mut backend = HookBackend {
            operation: OperationResult::failed("synthetic Mesh service error"),
            load: (boundary == "load_mesh").then(|| Box::new(|_: &Path| false) as LoadHook),
            ..HookBackend::default()
        };
        let requested = if boundary == "maintain_mesh_references" {
            RequestedWork::ConvertibleTextureConversion
        } else {
            RequestedWork::StandardMeshOptimization
        };
        let asset = route(ExecutionMode::Apply, &[requested], &source);

        let result = execute_and_clean(&asset, &mut backend, &root);

        let expected = if boundary == "load_mesh" {
            AssetExecutionFailure::LoadFailed
        } else {
            AssetExecutionFailure::OperationFailed
        };
        assert_eq!(result.failure(), Some(expected), "{boundary}");
        assert_eq!(result.operation(), boundary);
        assert_eq!(result.mutation_state(), MutationState::None, "{boundary}");
        assert!(result.safe_to_continue(), "{boundary}");
        assert_eq!(result.affected_path(), source, "{boundary}");
        if boundary != "load_mesh" {
            assert_eq!(result.service_detail(), "synthetic Mesh service error");
        }
        assert_eq!(backend.count("save_mesh"), 0, "{boundary}");
        assert_eq!(read(&source), "original", "{boundary}");
    }
}

/// Origin: AssetExecutionTests::meshWithoutChanges (both rows).
#[test]
fn an_unchanged_apply_mesh_or_a_dry_run_mesh_is_never_staged() {
    for (mode, changed) in [(ExecutionMode::Apply, false), (ExecutionMode::DryRun, true)] {
        let root = mod_root(&format!("mesh-without-changes-{mode:?}"));
        let source = root.join("actor.nif");
        write(&source, "original");
        let mut backend = HookBackend {
            operation: if changed {
                OperationResult::changed()
            } else {
                OperationResult::unchanged()
            },
            ..HookBackend::default()
        };
        let asset = route(
            mode,
            &[
                RequestedWork::StandardMeshOptimization,
                RequestedWork::ConvertibleTextureConversion,
            ],
            &source,
        );

        let result = execute_and_clean(&asset, &mut backend, &root);

        assert!(result.succeeded(), "{mode:?}");
        assert_eq!(result.mutation_state(), MutationState::None);
        assert_eq!(backend.count("optimize_mesh"), 1);
        assert_eq!(backend.count("maintain_mesh_references"), 1);
        assert_eq!(backend.count("save_mesh"), 0);
        assert_eq!(read(&source), "original");
        assert_eq!(names(&root), ["actor.nif"], "{mode:?}");
    }
}

/// Origin: AssetExecutionTests::meshStagingFailure.
#[test]
fn obstructed_mesh_staging_is_unsafe_and_touches_nothing() {
    let root = mod_root("mesh-staging-failure");
    let source = root.join("actor.nif");
    let obstruction = root.join(".cao-staging");
    write(&source, "original");
    write(&obstruction, "unowned");
    let mut backend = HookBackend::default();
    let asset = route(
        ExecutionMode::Apply,
        &[RequestedWork::StandardMeshOptimization],
        &source,
    );

    let result = execute_and_clean(&asset, &mut backend, &root);

    assert_eq!(result.failure(), Some(AssetExecutionFailure::StagingFailed));
    assert_eq!(result.mutation_state(), MutationState::None);
    assert!(!result.safe_to_continue());
    assert_eq!(result.operation(), "stage_mesh");
    assert_eq!(backend.count("save_mesh"), 0);
    assert_eq!(read(&source), "original");
    assert_eq!(read(&obstruction), "unowned");
}

/// Origin: AssetExecutionTests::meshVariantSelectsLoadMode (all three rows).
#[test]
fn a_mesh_loads_once_with_its_routed_path_and_variant() {
    let rows = [
        (
            "Meshes/Actor.NIF",
            MeshVariant::Standard,
            RequestedWork::StandardMeshOptimization,
        ),
        (
            "Meshes/Landscape.BTR",
            MeshVariant::Terrain,
            RequestedWork::TerrainMeshOptimization,
        ),
        (
            "Meshes/Landscape.BTO",
            MeshVariant::Terrain,
            RequestedWork::TerrainMeshOptimization,
        ),
    ];
    for (index, (relative, variant, requested)) in rows.into_iter().enumerate() {
        let root = mod_root(&format!("mesh-variant-{index}"));
        let source = root.join(relative);
        write(&source, "original");
        let mut backend = HookBackend::default();
        let asset = route(ExecutionMode::Apply, &[requested], &source);

        let result = execute_and_clean(&asset, &mut backend, &root);

        assert!(result.succeeded(), "{relative}: {result:?}");
        assert_eq!(backend.count("load_mesh"), 1, "{relative}");
        assert_eq!(backend.loaded.as_deref(), Some(asset.execution_path()));
        assert_eq!(backend.mesh_variant, Some(variant), "{relative}");
    }
}

/// Origin: AssetExecutionTests::meshOperationsShareOneTransaction (all three
/// rows). Any combination of optimization and Mesh Reference Maintenance is
/// one load and one staged save, published once.
#[test]
fn mesh_operations_share_one_load_and_one_staged_save() {
    // (row, optimizes, maintains, requested work)
    let rows = [
        (
            "optimization only",
            1,
            0,
            vec![RequestedWork::StandardMeshOptimization],
        ),
        (
            "maintenance only",
            0,
            1,
            vec![RequestedWork::ConvertibleTextureConversion],
        ),
        (
            "optimization and maintenance",
            1,
            1,
            vec![
                RequestedWork::StandardMeshOptimization,
                RequestedWork::ConvertibleTextureConversion,
            ],
        ),
    ];
    for (index, (row, optimizes, maintains, requested)) in rows.into_iter().enumerate() {
        let root = mod_root(&format!("mesh-transaction-{index}"));
        let source = root.join("Meshes/Actor.nif");
        write(&source, "original");
        let reserved = Arc::new(Mutex::new(false));
        let mut backend = HookBackend {
            save: Some(Box::new({
                let (source, reserved) = (source.clone(), reserved.clone());
                move |path| {
                    *reserved.lock().unwrap() = path.is_file();
                    assert_sibling(path, &source);
                    assert_eq!(read(&source), "original");
                    std::fs::write(path, "saved mesh").unwrap();
                    true
                }
            })),
            ..HookBackend::default()
        };
        let asset = route(ExecutionMode::Apply, &requested, &source);

        let result = execute_and_clean(&asset, &mut backend, &root);

        assert!(result.succeeded(), "{row}: {result:?}");
        assert_eq!(backend.count("load_mesh"), 1, "{row}");
        assert_eq!(backend.count("optimize_mesh"), optimizes, "{row}");
        assert_eq!(
            backend.count("maintain_mesh_references"),
            maintains,
            "{row}"
        );
        assert_eq!(backend.count("save_mesh"), 1, "{row}");
        assert!(*reserved.lock().unwrap(), "{row}");
        assert_eq!(result.mutation_state(), MutationState::Committed, "{row}");
        assert_eq!(read(&source), "saved mesh", "{row}");
        assert!(!backend.saved.unwrap().exists(), "{row}");
    }
}

/// Origin: AssetExecutionTests::dryRunMeshMaintenanceDoesNotMutate.
#[test]
fn dry_run_mesh_reference_maintenance_is_evaluated_without_saving() {
    let mut backend = HookBackend::default();
    let asset = route(
        ExecutionMode::DryRun,
        &[RequestedWork::ConvertibleTextureConversion],
        Path::new("Meshes/Actor.nif"),
    );

    let result = execute_and_clean(&asset, &mut backend, Path::new(""));

    assert!(result.succeeded(), "{result:?}");
    assert_eq!(result.mutation_state(), MutationState::None);
    assert_eq!(backend.count("load_mesh"), 1);
    assert_eq!(backend.count("maintain_mesh_references"), 1);
    assert_eq!(backend.modes, [ExecutionMode::DryRun]);
    assert_eq!(backend.count("save_mesh"), 0);
}

/// Origin: AssetExecutionTests::animationExecution (both rows).
#[test]
fn an_animation_is_converted_through_staging_in_apply_and_only_evaluated_in_dry_run() {
    for mode in [ExecutionMode::Apply, ExecutionMode::DryRun] {
        let root = mod_root(&format!("animation-{mode:?}"));
        let source = root.join("Walk.hkx");
        write(&source, "original");
        let mut backend = HookBackend::default();
        let asset = route(mode, &[RequestedWork::AnimationOptimization], &source);

        let result = execute_and_clean(&asset, &mut backend, &root);

        assert!(result.succeeded(), "{mode:?}: {result:?}");
        assert_eq!(backend.count("optimize_animation"), 1);
        assert_eq!(backend.loaded.as_deref(), Some(asset.execution_path()));
        assert_eq!(backend.modes, [mode]);
        if mode == ExecutionMode::Apply {
            assert_eq!(result.mutation_state(), MutationState::Committed);
            assert_eq!(read(&source), "converted");
            let staged = backend.saved.unwrap();
            assert_sibling(&staged, &source);
            assert!(!staged.exists());
        } else {
            assert_eq!(result.mutation_state(), MutationState::None);
            assert_eq!(read(&source), "original");
            assert!(backend.saved.is_none(), "Dry Run gets no output path");
            assert!(!root.join(".cao-staging").exists());
        }
    }
}

/// Origin: AssetExecutionTests::animationFailureIsReported.
#[test]
fn an_animation_failure_is_reported_with_its_service_detail() {
    let root = mod_root("animation-failure");
    let source = root.join("Walk.hkx");
    write(&source, "original");
    let mut backend = HookBackend {
        operation: OperationResult::failed("synthetic animation failure"),
        ..HookBackend::default()
    };
    let asset = route(
        ExecutionMode::Apply,
        &[RequestedWork::AnimationOptimization],
        &source,
    );

    let result = execute_and_clean(&asset, &mut backend, &root);

    assert_eq!(
        result.failure(),
        Some(AssetExecutionFailure::OperationFailed)
    );
    assert_eq!(result.service_detail(), "synthetic animation failure");
    assert_eq!(result.mutation_state(), MutationState::None);
    assert!(result.safe_to_continue());
    assert_eq!(read(&source), "original");
}

/// Origin: AssetExecutionTests::animationStagedOutcomes (the partial output,
/// empty output and unchanged rows). Every converter outcome preserves the
/// original, and the staged output never survives Safety Cleanup.
#[test]
fn every_contained_animation_outcome_preserves_the_original() {
    // (row, failure, category, operation)
    let rows = [
        (
            "partial output failure",
            Some(AssetExecutionFailure::OperationFailed),
            Some(ExecutionFailureCategory::Backend),
            "optimize_animation",
        ),
        (
            "empty output success",
            Some(AssetExecutionFailure::SaveFailed),
            Some(ExecutionFailureCategory::Filesystem),
            "save_animation",
        ),
        ("unchanged output", None, None, ""),
    ];
    for (index, (row, failure, category, operation)) in rows.into_iter().enumerate() {
        let root = mod_root(&format!("animation-outcome-{index}"));
        let source = root.join("Walk.HKX");
        write(&source, "original");
        let mut backend = HookBackend {
            animation: Some(Box::new(move |output| {
                let output = output.unwrap();
                match index {
                    1 => OperationResult::changed(),
                    _ => {
                        std::fs::write(output, "partial").unwrap();
                        if index == 0 {
                            OperationResult::failed("converter failed")
                        } else {
                            OperationResult::unchanged()
                        }
                    }
                }
            })),
            ..HookBackend::default()
        };
        let asset = route(
            ExecutionMode::Apply,
            &[RequestedWork::AnimationOptimization],
            &source,
        );

        let result = execute_and_clean(&asset, &mut backend, &root);

        assert_eq!(read(&source), "original", "{row}");
        assert_eq!(result.mutation_state(), MutationState::None, "{row}");
        assert!(result.safe_to_continue(), "{row}");
        assert!(result.cleanup_failures().is_empty(), "{row}");
        let staged = backend.saved.unwrap();
        assert_sibling(&staged, &source);
        assert!(!staged.exists(), "{row}");
        assert_eq!(result.failure(), failure, "{row}");
        if failure.is_some() {
            assert_eq!(result.failure_category(), category, "{row}");
            assert_eq!(result.operation(), operation, "{row}");
            assert_eq!(result.affected_path(), source, "{row}");
            assert_eq!(result.phase(), RunPhase::ProcessingAssets, "{row}");
        }
    }
}

/// Origin: AssetExecutionTests::animationStagedOutcomes (the two exception
/// rows), under the spec's panic rule. The original survives and Safety
/// Cleanup removes the partial output.
#[test]
fn a_panicking_animation_converter_is_unsafe_and_preserves_the_original() {
    let root = mod_root("animation-panic");
    let source = root.join("Walk.HKX");
    write(&source, "original");
    let work = BackendRun(Mutex::new(HookBackend {
        animation: Some(Box::new(|output| {
            std::fs::write(output.unwrap(), "partial").unwrap();
            panic!("optimize_animation backend panicked")
        })),
        ..HookBackend::default()
    }));

    let result = run(
        &root,
        ExecutionMode::Apply,
        &[RequestedWork::AnimationOptimization],
        &work,
    );

    assert_contained_panic(&result, "optimize_animation", &source);
    assert_eq!(read(&source), "original");
    let staged = work.backend().saved.clone().unwrap();
    assert_sibling(&staged, &source);
    assert!(!staged.exists());
}

/// Origin: AssetExecutionTests::executionFailurePreservesRoutedDecision.
#[test]
fn an_execution_failure_never_rewrites_the_routing_decision() {
    let root = mod_root("routed-decision");
    let source = root.join("Meshes/Actor.nif");
    write(&source, "original");
    let mut backend = HookBackend {
        operation: OperationResult::failed("synthetic optimizer failure"),
        ..HookBackend::default()
    };
    let asset = route(
        ExecutionMode::Apply,
        &[RequestedWork::StandardMeshOptimization],
        &source,
    );
    let before = asset.clone();

    let result = execute_and_clean(&asset, &mut backend, &root);

    assert_eq!(
        result.failure(),
        Some(AssetExecutionFailure::OperationFailed)
    );
    assert_eq!(asset, before);
    assert_eq!(asset.execution_path(), source);
    assert_eq!(asset.target(), OptimizerTarget::Mesh);
    assert_eq!(asset.execution_mode(), ExecutionMode::Apply);
    assert!(asset.operations().contains(AssetOperation::Optimization));
}

/// Origin: AssetExecutionTests::archiveIsNotOwnedByAssetExecutor.
#[test]
fn an_archive_is_never_executed_as_a_loose_asset() {
    let mut backend = HookBackend::default();
    let asset = route(
        ExecutionMode::Apply,
        &[RequestedWork::ArchiveExtraction],
        Path::new("Archives/Assets.bsa"),
    );

    let result = execute_and_clean(&asset, &mut backend, Path::new(""));

    assert_eq!(
        result.failure(),
        Some(AssetExecutionFailure::UnsupportedTarget)
    );
    assert!(backend.calls.is_empty());
}

/// The child-process converter of [`a_conversion_killed_at_any_boundary_recovers_consistently`]:
/// it converts `textures/source.tga` and, at the boundary `CAO_CONVERSION_CHILD`
/// names, reports and waits to be killed.
#[test]
fn conversion_crash_child() {
    let (Ok(boundary), Ok(root), Ok(report)) = (
        std::env::var("CAO_CONVERSION_CHILD"),
        std::env::var("CAO_CONVERSION_ROOT"),
        std::env::var("CAO_CONVERSION_REPORT"),
    ) else {
        return;
    };
    let root = PathBuf::from(root);
    let report = PathBuf::from(report);
    // Reports through a file renamed into place, then waits, bounded, for the kill.
    let pause = Arc::new(move || -> ! {
        let pending = report.with_extension("pending");
        std::fs::write(&pending, "paused").unwrap();
        std::fs::rename(&pending, &report).unwrap();
        std::thread::sleep(Duration::from_secs(60));
        panic!("the converter was not killed");
    });
    let during_save = boundary == "during-save";
    let before_removal = boundary == "before-source-removal";
    let pause_saving = Arc::clone(&pause);
    let mut backend = HookBackend {
        save: Some(Box::new(move |path| {
            let pause = &pause_saving;
            std::fs::write(path, if during_save { "partial" } else { "converted" }).unwrap();
            if during_save {
                pause();
            }
            true
        })),
        remove: Some(Box::new(move |_, remove_verified| {
            if before_removal {
                pause();
            }
            assert!(remove_verified());
            pause()
        })),
        ..HookBackend::default()
    };
    let source = root.join("textures/source.tga");
    let asset = route(
        ExecutionMode::Apply,
        &[RequestedWork::ConvertibleTextureConversion],
        &source,
    );
    // The registry, and with it the ownership lock, lives until the kill.
    let mut artifacts = TemporaryArtifactRegistry::new(create_run_id());
    // The converter is killed inside this call; returning at all is the failure.
    let _ = AssetExecutor::new(&mut backend).execute(&asset, &mut artifacts, &root);
    panic!("the converter passed its boundary without pausing");
}

/// A converter process, killed when dropped so a failing parent never leaks it.
struct ConverterProcess(Child);

impl Drop for ConverterProcess {
    fn drop(&mut self) {
        // It may already have exited; either way it is gone afterwards.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Origin: AssetExecutionTests::interruptedTextureRecovery (all three rows).
/// A converter killed at any filesystem boundary leaves a tree the next run's
/// recovery makes consistent: the old destination before the commit, the
/// converted output after it, and the source unless its removal had happened.
#[test]
fn a_conversion_killed_at_any_boundary_recovers_consistently() {
    // (boundary, destination after recovery, source survives)
    let rows = [
        ("during-save", "old destination", true),
        ("before-source-removal", "converted", true),
        ("after-source-removal", "converted", false),
    ];
    for (boundary, destination_bytes, source_survives) in rows {
        let root = mod_root(&format!("conversion-crash-{boundary}"));
        let source = root.join("textures/source.tga");
        let destination = root.join("textures/source.dds");
        write(&source, "original");
        write(&destination, "old destination");
        let report = root.with_extension("report");
        let output = root.with_extension("child-output");
        // Leftovers from an earlier run would satisfy the wait below; absence is fine.
        let _ = std::fs::remove_file(&report);
        let log = std::fs::File::create(&output).unwrap();
        let mut child = ConverterProcess(
            Command::new(std::env::current_exe().unwrap())
                .args(["conversion_crash_child", "--exact", "--test-threads=1"])
                .env("CAO_CONVERSION_CHILD", boundary)
                .env("CAO_CONVERSION_ROOT", &root)
                .env("CAO_CONVERSION_REPORT", &report)
                .stdin(Stdio::null())
                .stdout(log.try_clone().unwrap())
                .stderr(log)
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(30);
        while !report.exists() {
            let exited = child.0.try_wait().unwrap();
            assert!(
                exited.is_none() && Instant::now() < deadline,
                "{boundary}: the converter did not pause ({exited:?}):\n{}",
                std::fs::read_to_string(&output).unwrap_or_default()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        drop(child);

        let mut recovery = TemporaryArtifactRegistry::new(create_run_id());
        let failure = recovery
            .prepare_root(&root, &CancellationToken::new())
            .unwrap();
        assert!(failure.is_none(), "{boundary}: {failure:?}");
        drop(recovery);

        assert_eq!(read(&destination), destination_bytes, "{boundary}");
        assert_eq!(source.exists(), source_survives, "{boundary}");
        if source_survives {
            assert_eq!(read(&source), "original", "{boundary}");
        }
        let expected: &[&str] = if source_survives {
            &["source.dds", "source.tga"]
        } else {
            &["source.dds"]
        };
        assert_eq!(names(&root.join("textures")), expected, "{boundary}");
        assert!(
            names(&root.join(".cao-staging"))
                .iter()
                .all(|name| name == "owner.lock" || name == "ownership.manifest"),
            "{boundary}: {:?}",
            names(&root.join(".cao-staging"))
        );
    }
}
