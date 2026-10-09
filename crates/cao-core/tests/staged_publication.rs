//! Apply-mode staged publication through the whole run (#491): every output
//! is staged and published atomically, its mutation fact reaches Run
//! Evidence, Safety Cleanup always runs, Quarantine renames Textures that
//! fail to load (and only Textures), and Dry Run never touches staging.
//! Apply Preparing also recovers a crashed run's leftover staging (#492).
//!
//! These run the production shape of the work service ([`BackendWork`]) over
//! real temporary directories; the fake backend's file-name markers choose
//! what each Asset does.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cao_core::execution::{AssetExecutionFailure, MutationState};
use cao_core::routing::{ExecutionMode, RequestedWork};
use cao_core::run::{
    InlineRunScheduler, MutationKind, OptimizationRunResult, OptimizationRunService,
    RunFailureCode, RunOutcome, RunPhase, RunRequest, RunWorkService, TemporaryArtifactRegistry,
    create_run_id,
};
use cao_winfs::OwnerLock;
use common::{BackendWork, canonical, scratch_dir, snapshot_tree, test_configuration, write_tree};

/// Runs `request` to completion on the calling thread.
fn run(work: Arc<dyn RunWorkService>, request: RunRequest) -> Arc<OptimizationRunResult> {
    let _serial = common::serial();
    let service = OptimizationRunService::with_scheduler(
        Arc::new(InlineRunScheduler),
        Some(test_configuration()),
        Some(work),
    );
    service.start(request, None).expect("the run starts").wait()
}

/// A fresh, canonical Mod Root holding `files`.
fn mod_root(name: &str, files: &[&str]) -> PathBuf {
    let root = canonical(&scratch_dir(&format!("staged-publication/{name}")));
    write_tree(&root, files);
    root
}

fn apply(root: &Path, work: &[RequestedWork]) -> RunRequest {
    common::request(ExecutionMode::Apply, root, work)
}

fn read(path: &Path) -> String {
    String::from_utf8(std::fs::read(path).unwrap()).unwrap()
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

/// Spec (#491): a changed Texture is staged beside its destination,
/// published over it, and recorded as a Committed Mutation; Safety Cleanup
/// leaves only the staging control files behind.
#[test]
fn an_apply_change_is_published_and_recorded_as_committed() {
    let root = mod_root(
        "texture-commit",
        &["textures/a_changes.dds", "textures/b.dds"],
    );

    let result = run(
        Arc::new(BackendWork::new()),
        apply(&root, &[RequestedWork::NativeTextureOptimization]),
    );

    assert_eq!(
        result.outcome(),
        RunOutcome::Succeeded,
        "{:?}",
        result.failures()
    );
    let attempts = result.asset_attempts();
    assert_eq!(attempts.len(), 2);
    assert_eq!(
        attempts[0].result.mutation_state(),
        MutationState::Committed
    );
    assert_eq!(attempts[1].result.mutation_state(), MutationState::None);
    let summaries = result.mutation_summaries();
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].mod_root, root);
    assert_eq!(summaries[0].kind, MutationKind::AssetProcessing);
    assert_eq!(summaries[0].committed, 1);
    assert_eq!(summaries[0].partial_or_unknown, 0);

    assert_eq!(
        read(&root.join("textures/a_changes.dds")),
        "optimized a_changes.dds"
    );
    assert_eq!(read(&root.join("textures/b.dds")), "textures/b.dds");
    assert_eq!(names(&root.join("textures")), ["a_changes.dds", "b.dds"]);
    let staging = root.join(".cao-staging");
    assert_eq!(names(&staging), ["owner.lock", "ownership.manifest"]);
    // The released sibling left the manifest; only the run child is recorded.
    let manifest = read(&staging.join("ownership.manifest"));
    assert_eq!(manifest.lines().nth(3), Some("1"), "{manifest}");
    assert!(manifest.contains(&format!("\"run-{}-", result.run_id())));
}

/// A run with nothing to persist never creates staging.
#[test]
fn an_apply_run_without_changes_creates_no_staging() {
    let root = mod_root("texture-unchanged", &["textures/b.dds"]);

    let result = run(
        Arc::new(BackendWork::new()),
        apply(&root, &[RequestedWork::NativeTextureOptimization]),
    );

    assert_eq!(result.outcome(), RunOutcome::Succeeded);
    assert!(!root.join(".cao-staging").exists());
}

/// A failed save publishes nothing: the original survives, cleanup removes
/// the staged sibling, and the run completes with failures.
#[test]
fn a_failed_save_publishes_nothing() {
    let root = mod_root(
        "texture-save-failure",
        &["textures/a_changes_unsaveable.dds"],
    );

    let result = run(
        Arc::new(BackendWork::new()),
        apply(&root, &[RequestedWork::NativeTextureOptimization]),
    );

    assert_eq!(result.outcome(), RunOutcome::CompletedWithFailures);
    let attempt = &result.asset_attempts()[0].result;
    assert_eq!(attempt.failure(), Some(AssetExecutionFailure::SaveFailed));
    assert_eq!(attempt.mutation_state(), MutationState::None);
    assert_eq!(
        read(&root.join("textures/a_changes_unsaveable.dds")),
        "textures/a_changes_unsaveable.dds"
    );
    assert_eq!(names(&root.join("textures")), ["a_changes_unsaveable.dds"]);
    assert!(result.cleanup_failures().is_empty());
}

/// A converted TGA is published as a DDS beside it, and the verified source
/// is removed only after that publication.
#[test]
fn a_texture_conversion_publishes_the_dds_and_removes_its_source() {
    let root = mod_root("texture-conversion", &["textures/c_changes.tga"]);

    let result = run(
        Arc::new(BackendWork::new()),
        apply(
            &root,
            &[
                RequestedWork::NativeTextureOptimization,
                RequestedWork::ConvertibleTextureConversion,
            ],
        ),
    );

    assert_eq!(
        result.outcome(),
        RunOutcome::Succeeded,
        "{:?}",
        result.failures()
    );
    assert_eq!(
        result.asset_attempts()[0].result.mutation_state(),
        MutationState::Committed
    );
    assert_eq!(names(&root.join("textures")), ["c_changes.dds"]);
    assert_eq!(
        read(&root.join("textures/c_changes.dds")),
        "optimized c_changes.tga"
    );
}

/// Mesh and Animation output go through the same staged publication.
#[test]
fn mesh_and_animation_output_is_published() {
    let root = mod_root(
        "mesh-animation",
        &[
            "meshes/m_changes.nif",
            "animations/w_changes.hkx",
            "animations/still.hkx",
        ],
    );

    let result = run(
        Arc::new(BackendWork::new()),
        apply(
            &root,
            &[
                RequestedWork::StandardMeshOptimization,
                RequestedWork::AnimationOptimization,
            ],
        ),
    );

    assert_eq!(
        result.outcome(),
        RunOutcome::Succeeded,
        "{:?}",
        result.failures()
    );
    assert_eq!(
        read(&root.join("meshes/m_changes.nif")),
        "optimized m_changes.nif"
    );
    assert_eq!(
        read(&root.join("animations/w_changes.hkx")),
        "converted w_changes.hkx"
    );
    // An unchanged Animation's empty staged output is cleaned up.
    assert_eq!(
        read(&root.join("animations/still.hkx")),
        "animations/still.hkx"
    );
    assert_eq!(
        names(&root.join("animations")),
        ["still.hkx", "w_changes.hkx"]
    );
    assert_eq!(result.mutation_summaries()[0].committed, 2);
}

/// Quarantine (Apply only): a Texture that fails to load is renamed to
/// `.caobad`, or the next free `.caobad.N`, and the rename is a Committed
/// Mutation of a contained failure.
#[test]
fn an_unloadable_texture_is_quarantined() {
    let root = mod_root(
        "quarantine",
        &[
            "textures/x_unloadable.dds",
            "textures/y_unloadable.dds",
            "textures/y_unloadable.dds.caobad",
        ],
    );

    let result = run(
        Arc::new(BackendWork::new()),
        apply(&root, &[RequestedWork::NativeTextureOptimization]),
    );

    assert_eq!(result.outcome(), RunOutcome::CompletedWithFailures);
    for attempt in result.asset_attempts() {
        assert_eq!(
            attempt.result.failure(),
            Some(AssetExecutionFailure::LoadFailed)
        );
        assert_eq!(attempt.result.mutation_state(), MutationState::Committed);
        assert!(attempt.result.safe_to_continue());
    }
    assert_eq!(
        names(&root.join("textures")),
        [
            "x_unloadable.dds.caobad",
            "y_unloadable.dds.caobad",
            "y_unloadable.dds.caobad.1"
        ]
    );
    // The earlier quarantine is never overwritten.
    assert_eq!(
        read(&root.join("textures/y_unloadable.dds.caobad")),
        "textures/y_unloadable.dds.caobad"
    );
    assert_eq!(
        read(&root.join("textures/y_unloadable.dds.caobad.1")),
        "textures/y_unloadable.dds"
    );
    assert_eq!(result.mutation_summaries()[0].committed, 2);
}

/// Rust-only, until Meshes load through `nifly-sys`: a Mesh that fails to
/// load is reported but never quarantined, because every Mesh fails to load
/// in this build and Texture conversion routes them all.
#[test]
fn an_unloadable_mesh_is_not_quarantined() {
    let root = mod_root("mesh-not-quarantined", &["meshes/m_unloadable.nif"]);
    let before = snapshot_tree(&root);

    let result = run(
        Arc::new(BackendWork::new()),
        apply(&root, &[RequestedWork::StandardMeshOptimization]),
    );

    assert_eq!(result.outcome(), RunOutcome::CompletedWithFailures);
    let attempt = &result.asset_attempts()[0].result;
    assert_eq!(attempt.failure(), Some(AssetExecutionFailure::LoadFailed));
    assert_eq!(attempt.mutation_state(), MutationState::None);
    assert_eq!(snapshot_tree(&root), before);
}

/// Dry Run only reports a load failure; nothing is renamed.
#[test]
fn a_dry_run_never_quarantines() {
    let root = mod_root("quarantine-dry-run", &["textures/x_unloadable.dds"]);
    let before = snapshot_tree(&root);

    let result = run(
        Arc::new(BackendWork::new()),
        common::request(
            ExecutionMode::DryRun,
            &root,
            &[RequestedWork::NativeTextureOptimization],
        ),
    );

    assert_eq!(result.outcome(), RunOutcome::CompletedWithFailures);
    assert_eq!(
        result.asset_attempts()[0].result.mutation_state(),
        MutationState::None
    );
    assert_eq!(snapshot_tree(&root), before);
}

/// Spec (#491, story 45): a Dry Run never inspects, recovers or cleans
/// staging, even staging another process actively owns.
#[test]
fn a_dry_run_never_touches_existing_staging() {
    let root = mod_root(
        "dry-run-staging",
        &[
            "textures/a_changes.dds",
            ".cao-staging/owner.lock",
            ".cao-staging/ownership.manifest",
            ".cao-staging/run-x-y/archive-entry-z",
        ],
    );
    let before = snapshot_tree(&root);
    let active = OwnerLock::open_existing(&root.join(".cao-staging/owner.lock")).unwrap();

    let result = run(
        Arc::new(BackendWork::new()),
        common::request(
            ExecutionMode::DryRun,
            &root,
            &[RequestedWork::NativeTextureOptimization],
        ),
    );

    assert_eq!(
        result.outcome(),
        RunOutcome::Succeeded,
        "{:?}",
        result.failures()
    );
    drop(active);
    assert_eq!(snapshot_tree(&root), before);
}

/// Apply Preparing fails with `StagingActive` while another process owns the
/// Mod Root's staging, before any Asset is attempted.
#[test]
fn apply_preparing_fails_on_active_staging() {
    let root = mod_root(
        "active-staging",
        &["textures/a_changes.dds", ".cao-staging/owner.lock"],
    );
    let lock = root.join(".cao-staging/owner.lock");
    let before = snapshot_tree(&root);
    let active = OwnerLock::open_existing(&lock).unwrap();

    let result = run(
        Arc::new(BackendWork::new()),
        apply(&root, &[RequestedWork::NativeTextureOptimization]),
    );

    assert_eq!(result.outcome(), RunOutcome::Failed);
    assert_eq!(result.final_phase(), RunPhase::Preparing);
    let failure = &result.failures()[0];
    assert_eq!(failure.code, RunFailureCode::StagingActive);
    assert_eq!(failure.phase, RunPhase::Preparing);
    assert_eq!(failure.path, lock);
    assert!(failure.detail.contains("Wait for the owning CAO run"));
    assert!(result.asset_attempts().is_empty());
    drop(active);
    assert_eq!(snapshot_tree(&root), before);
}

/// Leaves the staging of a run that crashed while staging `destination`: a
/// partial sibling that the manifest still owns. Returns the sibling.
fn crashed_run_leftover(root: &Path, destination: &str) -> PathBuf {
    let mut crashed = TemporaryArtifactRegistry::new(create_run_id());
    let sibling = crashed.stage_file(root, &root.join(destination)).unwrap();
    std::fs::write(&sibling.path, "partial output").unwrap();
    // Dropping the registry without Safety Cleanup is what a crash leaves.
    sibling.path
}

/// Spec (#492): Apply Preparing recovers a crashed run's leftover staging
/// before any work, then stages this run's output in the same area.
#[test]
fn apply_recovers_leftover_staging_before_any_work() {
    let root = mod_root("leftover-recovered", &["textures/a_changes.dds"]);
    let abandoned = crashed_run_leftover(&root, "textures/a_changes.dds");

    let result = run(
        Arc::new(BackendWork::new()),
        apply(&root, &[RequestedWork::NativeTextureOptimization]),
    );

    assert_eq!(
        result.outcome(),
        RunOutcome::Succeeded,
        "{:?}",
        result.failures()
    );
    assert!(!abandoned.exists());
    assert_eq!(result.asset_attempts().len(), 1);
    assert_eq!(names(&root.join("textures")), ["a_changes.dds"]);
    assert_eq!(
        read(&root.join("textures/a_changes.dds")),
        "optimized a_changes.dds"
    );
    let staging = root.join(".cao-staging");
    assert_eq!(names(&staging), ["owner.lock", "ownership.manifest"]);
    let manifest = read(&staging.join("ownership.manifest"));
    assert!(manifest.contains(&format!("\"run-{}-", result.run_id())));
}

/// Spec (#492, story 45): a Dry Run never recovers leftover staging, even
/// staging an Apply run would recover.
#[test]
fn a_dry_run_never_recovers_leftover_staging() {
    let root = mod_root("dry-run-leftover", &["textures/a_changes.dds"]);
    let abandoned = crashed_run_leftover(&root, "textures/a_changes.dds");
    let before = snapshot_tree(&root);

    let result = run(
        Arc::new(BackendWork::new()),
        common::request(
            ExecutionMode::DryRun,
            &root,
            &[RequestedWork::NativeTextureOptimization],
        ),
    );

    assert_eq!(
        result.outcome(),
        RunOutcome::Succeeded,
        "{:?}",
        result.failures()
    );
    assert_eq!(read(&abandoned), "partial output");
    assert_eq!(snapshot_tree(&root), before);
}

/// Spec (#492): Apply Preparing fails closed on leftover staging whose
/// manifest it cannot verify, naming the manifest, saying what to do, and
/// leaving everything untouched.
#[test]
fn apply_preparing_fails_closed_on_unverifiable_staging() {
    let root = mod_root(
        "leftover-staging",
        &[
            "textures/a_changes.dds",
            ".cao-staging/owner.lock",
            ".cao-staging/ownership.manifest",
        ],
    );
    let before = snapshot_tree(&root);

    let result = run(
        Arc::new(BackendWork::new()),
        apply(&root, &[RequestedWork::NativeTextureOptimization]),
    );

    assert_eq!(result.outcome(), RunOutcome::Failed);
    let failure = &result.failures()[0];
    assert_eq!(failure.code, RunFailureCode::StagingOwnershipUnverified);
    assert_eq!(failure.phase, RunPhase::Preparing);
    assert_eq!(failure.path, root.join(".cao-staging/ownership.manifest"));
    assert!(
        failure.detail.contains("Inspect ownership.manifest"),
        "{}",
        failure.detail
    );
    assert!(result.asset_attempts().is_empty());
    assert_eq!(snapshot_tree(&root), before);
}
