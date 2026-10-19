//! Asset attempt scenarios: the production [`AssetAttemptAdapter`] owning one
//! Routed Asset's whole attempt (#549).
//!
//! The adapter creates its backend through a backend factory on the first
//! Routed Asset, executes the Asset, tells the backend the attempt finished,
//! then performs Quarantine of a failed load. Each scenario drives the adapter
//! at its interface with the fake backend and its factory from
//! `cao_core::test_support`, on real Mod Roots under the target directory;
//! the one run-level scenario drives it through the shared
//! `backend_toolset`, a scripted Run Toolset over the same fake backend.

mod common;

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use cao_core::execution::{AssetExecutionFailure, AssetExecutionResult, MutationState};
use cao_core::routing::{
    AssetRouter, ExecutionMode, ProfileCapabilities, ProfileCapability, RequestedWork, RoutedAsset,
    RoutingDecision, RoutingPolicy, RoutingPolicyRequest,
};
use cao_core::run::{
    AssetAttemptAdapter, AssetAttemptPort, AssetInitializationCancelled, CancellationToken,
    RunDiagnostic, RunDiagnosticCode, RunExecutor, RunOutcome, RunPhase, RunServices,
    TemporaryArtifactRegistry, create_run_id,
};
use cao_core::test_support::{CountingCleanup, FakeBackendFactory};
use common::{backend_toolset, canonical, request, scratch_dir, toolset_configuration};

/// Every capability a routed scenario needs.
const CAPABILITIES: &[ProfileCapability] = &[
    ProfileCapability::NativeTextureOptimization,
    ProfileCapability::StandardMeshOptimization,
];

/// Routes `path` for native Texture optimization, as Asset Routing would.
fn route(mode: ExecutionMode, path: &Path) -> RoutedAsset {
    let policy = RoutingPolicy::compile(
        RoutingPolicyRequest::for_work(mode, &[RequestedWork::NativeTextureOptimization]),
        ProfileCapabilities::define(".bsa", CAPABILITIES),
    )
    .expect("the scenario's policy is valid");
    match AssetRouter::new(policy).route(path) {
        RoutingDecision::Routed(asset) => asset,
        other => panic!("expected a Routed Asset, got {other:?}"),
    }
}

/// A fresh canonical Mod Root holding a Texture at each of `names`.
fn mod_root(name: &str, names: &[&str]) -> PathBuf {
    let root = canonical(&scratch_dir(&format!("asset-attempt/{name}")));
    for name in names {
        let path = root.join("textures").join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "original").unwrap();
    }
    root
}

/// One attempt at the Texture `name` under `root`, returning its outcome and
/// the diagnostics the adapter handed back to retain.
fn attempt(
    adapter: &mut AssetAttemptAdapter<FakeBackendFactory>,
    mode: ExecutionMode,
    root: &Path,
    name: &str,
    stop: &CancellationToken,
) -> (
    Result<AssetExecutionResult, AssetInitializationCancelled>,
    Vec<RunDiagnostic>,
) {
    let asset = route(mode, &root.join("textures").join(name));
    let mut artifacts = TemporaryArtifactRegistry::new(create_run_id());
    let mut retained = Vec::new();
    let outcome = adapter.attempt(&asset, root, &mut artifacts, stop, &mut |diagnostic| {
        retained.push(diagnostic)
    });
    let failures = artifacts.cleanup();
    assert!(failures.is_empty(), "{failures:?}");
    (outcome, retained)
}

#[test]
fn the_backend_is_created_once_on_the_first_routed_asset() {
    let root = mod_root("lazy", &["first.dds", "second.dds"]);
    let factory = FakeBackendFactory::default();
    let creations = factory.creations.clone();
    let calls = factory.calls.clone();
    let stop = CancellationToken::new();

    let mut adapter = AssetAttemptAdapter::new(factory);
    assert_eq!(
        creations.load(Ordering::SeqCst),
        0,
        "creating the adapter creates no backend"
    );

    let (first, _) = attempt(
        &mut adapter,
        ExecutionMode::DryRun,
        &root,
        "first.dds",
        &stop,
    );
    assert!(first.unwrap().succeeded());
    assert_eq!(creations.load(Ordering::SeqCst), 1);

    let (second, _) = attempt(
        &mut adapter,
        ExecutionMode::DryRun,
        &root,
        "second.dds",
        &stop,
    );
    assert!(second.unwrap().succeeded());
    assert_eq!(creations.load(Ordering::SeqCst), 1, "the backend is reused");
    assert_eq!(
        *calls.lock().unwrap(),
        [
            "load_texture first.dds",
            "optimize_texture first.dds",
            "load_texture second.dds",
            "optimize_texture second.dds",
        ]
    );
}

/// Spec #546, user story 19: a run that routes no Asset never asks the
/// factory for a backend.
#[test]
fn a_run_with_nothing_to_process_creates_no_backend() {
    let root = mod_root("nothing-to-process", &[]);
    std::fs::write(root.join("readme.txt"), "not an Asset").unwrap();
    let factory = FakeBackendFactory::default();
    let (creations, calls) = (factory.creations.clone(), factory.calls.clone());
    let mut cleanup = CountingCleanup::default();

    let result = RunExecutor.execute(
        &request(
            ExecutionMode::Apply,
            &root,
            &[RequestedWork::NativeTextureOptimization],
        ),
        RunServices {
            safety_cleanup: &mut cleanup,
            observations: None,
            configuration: Some(&*toolset_configuration(Some(backend_toolset(factory)))),
        },
        &CancellationToken::new(),
        create_run_id(),
    );

    assert_eq!(result.outcome(), RunOutcome::Succeeded);
    assert!(result.asset_attempts().is_empty());
    assert_eq!(creations.load(Ordering::SeqCst), 0);
    assert!(calls.lock().unwrap().is_empty());
}

#[test]
fn the_factory_diagnostics_are_handed_back_once_to_be_retained() {
    let root = mod_root("diagnostics", &["first.dds", "second.dds"]);
    let unreadable = RunDiagnostic::new(
        RunDiagnosticCode::PluginUnreadable,
        RunPhase::ProcessingAssets,
        "The plugin's headparts cannot be read",
    )
    .with_path(root.join("broken.esp"));
    let factory = FakeBackendFactory {
        diagnostics: vec![unreadable.clone()],
        ..FakeBackendFactory::default()
    };
    let stop = CancellationToken::new();
    let mut adapter = AssetAttemptAdapter::new(factory);

    let (first, first_retained) = attempt(
        &mut adapter,
        ExecutionMode::DryRun,
        &root,
        "first.dds",
        &stop,
    );
    let (second, second_retained) = attempt(
        &mut adapter,
        ExecutionMode::DryRun,
        &root,
        "second.dds",
        &stop,
    );

    assert!(first.unwrap().succeeded());
    assert!(second.unwrap().succeeded());
    assert_eq!(first_retained, [unreadable]);
    assert!(second_retained.is_empty(), "{second_retained:?}");
}

#[test]
fn cancellation_during_initialization_makes_no_attempt() {
    let root = mod_root("cancelled", &["unloadable.dds"]);
    let factory = FakeBackendFactory::default();
    let calls = factory.calls.clone();
    let creations = factory.creations.clone();
    let stop = CancellationToken::new();
    stop.cancel();
    let mut adapter = AssetAttemptAdapter::new(factory);

    let (outcome, retained) = attempt(
        &mut adapter,
        ExecutionMode::Apply,
        &root,
        "unloadable.dds",
        &stop,
    );

    assert_eq!(outcome, Err(AssetInitializationCancelled));
    assert_eq!(creations.load(Ordering::SeqCst), 1);
    assert!(retained.is_empty());
    assert!(calls.lock().unwrap().is_empty());
    assert!(root.join("textures/unloadable.dds").exists());
}

/// Spec #546, user story 23: the backend hears that an attempt finished with
/// the executor's own result, while the failed load is still in place, and
/// only then does Quarantine rename it.
#[test]
fn attempt_finished_is_delivered_before_quarantine() {
    let root = mod_root("finished-before-quarantine", &["unloadable.dds"]);
    let source = root.join("textures/unloadable.dds");
    let factory = FakeBackendFactory::default();
    let finished = factory.finished.clone();
    let stop = CancellationToken::new();
    let mut adapter = AssetAttemptAdapter::new(factory);

    let (outcome, _) = attempt(
        &mut adapter,
        ExecutionMode::Apply,
        &root,
        "unloadable.dds",
        &stop,
    );

    let finished = finished.lock().unwrap();
    assert_eq!(finished.len(), 1, "{finished:?}");
    let notified = &finished[0];
    assert_eq!(notified.asset.execution_path(), source);
    assert_eq!(
        notified.result.failure(),
        Some(AssetExecutionFailure::LoadFailed)
    );
    assert_eq!(
        notified.result.mutation_state(),
        MutationState::None,
        "the notification carries the executor's result, not Quarantine's"
    );
    assert!(
        notified.source_present,
        "notified before Quarantine renamed it"
    );
    let result = outcome.unwrap();
    assert_eq!(result.failure(), Some(AssetExecutionFailure::LoadFailed));
    assert_eq!(result.mutation_state(), MutationState::Committed);
    assert!(!source.exists());
    assert!(root.join("textures/unloadable.dds.caobad").exists());
}

/// Spec #546, user story 22: a factory failure, such as COM failing to
/// initialize, fails only the attempt that needed the backend, as an unsafe
/// Operation Failure with unknown mutation, and nothing panics.
#[test]
fn a_factory_error_is_contained_as_an_unsafe_operation_failure() {
    let root = mod_root("factory-error", &["unloadable.dds"]);
    let factory = FakeBackendFactory {
        failure: Some("Failed to initialize COM".to_owned()),
        ..FakeBackendFactory::default()
    };
    let calls = factory.calls.clone();
    let stop = CancellationToken::new();
    let mut adapter = AssetAttemptAdapter::new(factory);

    let (outcome, retained) = attempt(
        &mut adapter,
        ExecutionMode::Apply,
        &root,
        "unloadable.dds",
        &stop,
    );

    let result = outcome.expect("a factory error is an attempt, not a cancellation");
    assert_eq!(
        result.failure(),
        Some(AssetExecutionFailure::BackendException)
    );
    assert_eq!(result.mutation_state(), MutationState::PartialOrUnknown);
    assert!(!result.safe_to_continue());
    assert_eq!(result.affected_path(), root.join("textures/unloadable.dds"));
    assert!(
        result.message().contains("Failed to initialize COM"),
        "{}",
        result.message()
    );
    assert!(retained.is_empty());
    assert!(calls.lock().unwrap().is_empty(), "no backend existed");
    assert!(
        root.join("textures/unloadable.dds").exists(),
        "no backend loaded the Asset, so nothing is quarantined"
    );
}

/// Spec #546 review: the Run Diagnostics a factory observed before it failed,
/// such as a plugin its Headpart Mesh scan could not read, are still handed
/// back to be retained alongside the contained failure.
#[test]
fn a_factory_error_still_hands_back_its_diagnostics() {
    let root = mod_root("factory-error-diagnostics", &["first.dds"]);
    let unreadable = RunDiagnostic::new(
        RunDiagnosticCode::PluginUnreadable,
        RunPhase::ProcessingAssets,
        "The plugin's headparts cannot be read",
    )
    .with_path(root.join("broken.esp"));
    let factory = FakeBackendFactory {
        diagnostics: vec![unreadable.clone()],
        failure: Some("Failed to initialize COM".to_owned()),
        ..FakeBackendFactory::default()
    };
    let stop = CancellationToken::new();
    let mut adapter = AssetAttemptAdapter::new(factory);

    let (outcome, retained) = attempt(
        &mut adapter,
        ExecutionMode::Apply,
        &root,
        "first.dds",
        &stop,
    );

    let result = outcome.expect("a factory error is an attempt, not a cancellation");
    assert_eq!(
        result.failure(),
        Some(AssetExecutionFailure::BackendException)
    );
    assert_eq!(retained, [unreadable]);
}
