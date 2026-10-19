//! The Run Executor's own containment, which no Run Toolset port can reach.
//!
//! Each port contains its own panics, so work that escapes the executor's
//! traversal can only be injected below its interface. These scenarios pass
//! the traversal a scripted work closure through the crate-private
//! [`RunExecutor::execute_with_work`] instead of the Toolset's work phases.

use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::{RunEvidenceInvariantPanic, RunExecutor, RunServices};
use crate::Error;
use crate::routing::{ExecutionMode, RequestedWork};
use crate::run::{
    CancellationToken, ModSelection, MutableRunEvidence, OptimizationRunResult, RunConfiguration,
    RunFailureCode, RunOutcome, RunPhase, RunPreparation, RunRequest, RunToolset,
    SelectedProfileFacts, TemporaryArtifactRegistry, create_run_id,
};
use crate::test_support::{CountingCleanup, ScriptedToolset, ToolsetConfiguration};

/// A fresh, empty directory for one scenario under the system temporary
/// directory: `CARGO_TARGET_TMPDIR` exists only for integration tests.
fn scratch_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("cao-core-unit")
        .join(format!("{name}-{}", std::process::id()));
    // A missing directory is the expected case; anything else surfaces in create_dir_all.
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// An Apply request for Native Texture Optimization over a Mod Root holding
/// one Texture, `textures/a.dds`, whose content is its own name.
fn texture_request(name: &str) -> (RunRequest, PathBuf) {
    let root = scratch_dir(name);
    let texture = root.join("textures").join("a.dds");
    std::fs::create_dir_all(texture.parent().unwrap()).unwrap();
    std::fs::write(&texture, "textures/a.dds").unwrap();
    let request = RunRequest::new(
        "SkyrimSE",
        ExecutionMode::Apply,
        ModSelection::SingleModRoot(root.clone()),
        vec![RequestedWork::NativeTextureOptimization],
    );
    (request, root)
}

/// A provider of an SSE-like profile with a default scripted Run Toolset, so
/// the traversal reaches its work phases.
fn provider() -> ToolsetConfiguration {
    let profile = SelectedProfileFacts {
        archive_extension: Some(".bsa".to_owned()),
        supports_native_texture_optimization: true,
        supports_texture_conversion: true,
        supports_standard_mesh_optimization: true,
        supports_terrain_mesh_optimization: true,
        supports_animation_optimization: true,
        supports_archive_extraction: true,
        supports_mesh_reference_maintenance: true,
        supports_archive_creation: true,
    };
    ToolsetConfiguration::new(
        RunConfiguration {
            profile,
            ..RunConfiguration::default()
        },
        Some(Arc::new(ScriptedToolset::default())),
    )
}

/// Runs `request` with `work` in place of the Toolset's work phases.
fn execute(
    request: &RunRequest,
    cleanup: &mut CountingCleanup,
    work: impl FnOnce(
        &RunPreparation,
        RunToolset,
        &RefCell<MutableRunEvidence<'_>>,
        &mut TemporaryArtifactRegistry,
        &CancellationToken,
    ) -> Result<(), Error>,
) -> OptimizationRunResult {
    let provider = provider();
    RunExecutor.execute_with_work(
        request,
        RunServices {
            safety_cleanup: cleanup,
            observations: None,
            configuration: Some(&provider),
            work: None,
        },
        &CancellationToken::new(),
        create_run_id(),
        work,
    )
}

/// Enters Archive discovery, then stages a durable sibling of the Mod Root's
/// Texture under the run's Temporary Ownership and half-writes it, returning
/// the staged path.
fn discover_and_stage(
    preparation: &RunPreparation,
    evidence: &RefCell<MutableRunEvidence<'_>>,
    artifacts: &mut TemporaryArtifactRegistry,
) -> Result<PathBuf, Error> {
    evidence.borrow_mut().record_archive_discovery_started()?;
    let root = &preparation.mod_roots()[0];
    let receipt = artifacts
        .capture_and_stage_file(root, &root.join("textures").join("a.dds"))
        .expect("the staging sibling is created");
    let staged = receipt.path().unwrap().to_path_buf();
    std::fs::write(&staged, "half-written output").unwrap();
    Ok(staged)
}

/// Asserts that Safety Cleanup removed `staged` and left the original
/// Texture under `root` untouched.
fn assert_staging_removed(name: &str, staged: &Path, root: &Path) {
    assert!(
        !staged.exists(),
        "{name}: Safety Cleanup removed {}",
        staged.display()
    );
    assert_eq!(
        std::fs::read_to_string(root.join("textures").join("a.dds")).unwrap(),
        "textures/a.dds",
        "{name}: the original is untouched"
    );
}

/// A panic payload whose own drop panics again, which a containment boundary
/// that merely dropped it would let escape.
struct PanicsAgainOnDrop;

impl Drop for PanicsAgainOnDrop {
    fn drop(&mut self) {
        panic!("the panic payload panicked while it was dropped");
    }
}

/// Origin: OptimizationRunServiceTests::workExceptionRetainsEarlierEvidence,
/// and spec #476/#468: a Run Worker panic becomes a Failed outcome while
/// Safety Cleanup still removes what the run staged. The last row's payload
/// panics again when dropped, which must not escape the boundary.
#[test]
fn escaped_work_fails_the_run_at_its_phase_and_safety_cleanup_still_removes_its_staging() {
    type Fault = fn() -> Result<(), Error>;
    let rows: [(&str, Fault, &str); 3] = [
        (
            "error",
            || Err(Error::WorkService("the backend is unavailable".to_owned())),
            "the backend is unavailable",
        ),
        (
            "panic",
            || panic!("the work escaped"),
            "The run's work panicked: the work escaped",
        ),
        (
            "payload panics on drop",
            || std::panic::panic_any(PanicsAgainOnDrop),
            "The run's work panicked",
        ),
    ];
    for (name, fault, detail) in rows {
        let (request, root) = texture_request(&format!("escaped-work-{}", name.replace(' ', "-")));
        let mut cleanup = CountingCleanup::default();
        let mut staged = None;
        let result = catch_unwind(AssertUnwindSafe(|| {
            execute(
                &request,
                &mut cleanup,
                |preparation, _toolset, evidence, artifacts, _stop| {
                    staged = Some(discover_and_stage(preparation, evidence, artifacts)?);
                    fault()
                },
            )
        }))
        .unwrap_or_else(|_| panic!("{name}: the executor contains the escaped work"));

        assert_eq!(result.outcome(), RunOutcome::Failed, "{name}");
        assert_eq!(result.failures().len(), 1, "{name}");
        let failure = &result.failures()[0];
        assert_eq!(failure.code, RunFailureCode::WorkServiceFailed, "{name}");
        assert_eq!(failure.phase, RunPhase::DiscoveringArchives, "{name}");
        assert!(
            failure.detail.starts_with(detail),
            "{name}: {}",
            failure.detail
        );
        assert_eq!(
            result.final_phase(),
            RunPhase::DiscoveringArchives,
            "{name}"
        );
        // Preparation and the phase the work had entered are retained.
        assert!(result.preparation().is_some(), "{name}");
        let phases: Vec<_> = result
            .phases()
            .iter()
            .map(|record| record.phase())
            .collect();
        assert_eq!(
            phases,
            [
                RunPhase::Preparing,
                RunPhase::DiscoveringArchives,
                RunPhase::SafetyCleanup
            ],
            "{name}"
        );
        assert_eq!(cleanup.passes, 1, "{name}: exactly one Safety Cleanup pass");
        assert!(result.cleanup_failures().is_empty(), "{name}");
        assert_staging_removed(name, &staged.expect("the work staged"), &root);
    }
}

/// Origin: RunExecutorTests::evidenceInvariantViolationsRemainProgrammingDefects.
/// The violation is raised only after Safety Cleanup has removed the run's
/// staging, carrying a Failed sealed result that keeps the earlier facts.
#[test]
fn an_evidence_invariant_violation_panics_only_after_safety_cleanup() {
    let (request, root) = texture_request("invariant-after-cleanup");
    let mut cleanup = CountingCleanup::default();
    let mut staged = None;
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        execute(
            &request,
            &mut cleanup,
            |preparation, _toolset, evidence, artifacts, _stop| {
                staged = Some(discover_and_stage(preparation, evidence, artifacts)?);
                // Planning Processing Assets before any routing is a producer bug.
                evidence.borrow_mut().record_asset_processing_plan(3)
            },
        )
    }));

    let payload = outcome.expect_err("the violation panics");
    let sealed = payload
        .downcast::<RunEvidenceInvariantPanic>()
        .expect("the panic carries the sealed result");
    assert!(
        sealed.violation.contains("Run Evidence invariant violated"),
        "{}",
        sealed.violation
    );
    assert_eq!(cleanup.passes, 1, "Safety Cleanup ran before the panic");
    assert_staging_removed("invariant", &staged.expect("the work staged"), &root);
    let result = &sealed.result;
    assert_eq!(result.outcome(), RunOutcome::Failed);
    assert!(result.preparation().is_some());
    assert_eq!(result.final_phase(), RunPhase::DiscoveringArchives);
    assert_eq!(result.failures().len(), 1);
    assert_eq!(result.failures()[0].code, RunFailureCode::WorkServiceFailed);
    assert_eq!(result.failures()[0].detail, sealed.violation);
    assert_eq!(
        result.phases().last().unwrap().phase(),
        RunPhase::SafetyCleanup
    );
}
