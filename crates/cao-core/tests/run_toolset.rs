//! The Run Executor driven through a Run Toolset: the configuration provider
//! returns the run's work ports beside its Run Configuration, and the
//! executor runs every work phase itself, with no Run Work Service (#550).
//!
//! These scenarios prove the Toolset path at the executor's interface. The
//! existing suites still drive the work-service path until their own
//! migration tickets (#551 to #555) move them here.

mod common;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use cao_core::execution::{AssetExecutionFailure, AssetExecutionResult, MutationState};
use cao_core::routing::{ExecutionMode, RequestedWork};
use cao_core::run::{
    ArchiveFinalizationAttempt, ArchiveFinalizationFailure, CancellationToken,
    OptimizationRunResult, PhaseSkipReason, RunConfiguration, RunDiagnostic, RunDiagnosticCode,
    RunExecutor, RunFailureCode, RunOutcome, RunPhase, RunPhaseStatus, RunRequest, RunServices,
    create_run_id,
};
use cao_core::test_support::{
    CountingCleanup, FakeBackendFactory, Observed, RecordingSink, ScriptedToolset,
    ToolsetConfiguration, entry, write_archive,
};
use common::{canonical, request, scratch_dir, sse_profile, write_tree};

/// A provider of the SSE-like profile that returns `toolset`, if any.
fn provider(toolset: Option<&Arc<ScriptedToolset>>) -> ToolsetConfiguration {
    ToolsetConfiguration::new(
        RunConfiguration {
            profile: sse_profile(),
            ..RunConfiguration::default()
        },
        toolset.cloned(),
    )
}

/// Executes `request` through `configuration`, with no work service,
/// observing every fact through `sink`.
fn execute(
    request: &RunRequest,
    configuration: &ToolsetConfiguration,
    sink: &RecordingSink,
    cleanup: &mut CountingCleanup,
    stop: &CancellationToken,
) -> OptimizationRunResult {
    RunExecutor.execute(
        request,
        RunServices {
            safety_cleanup: cleanup,
            observations: Some(sink),
            configuration: Some(configuration),
            work: None,
        },
        stop,
        create_run_id(),
    )
}

/// Each phase in the order the sink first observed it.
fn observed_phase_order(sink: &RecordingSink) -> Vec<RunPhase> {
    let mut phases: Vec<RunPhase> = Vec::new();
    for observation in sink.observed() {
        if let Observed::Phase(record) = observation
            && phases.last() != Some(&record.phase())
        {
            phases.push(record.phase());
        }
    }
    phases
}

#[test]
fn an_apply_run_through_a_toolset_traverses_every_phase_and_succeeds() {
    let root = scratch_dir("toolset-apply-success");
    write_tree(&root, &["textures/a.dds", "textures/b.dds"]);
    let toolset = Arc::new(
        ScriptedToolset::default()
            .with_assets(|_, _, _| Ok(AssetExecutionResult::success(MutationState::Committed))),
    );
    let configuration = provider(Some(&toolset));
    let sink = RecordingSink::default();
    let mut cleanup = CountingCleanup::default();

    let result = execute(
        &request(
            ExecutionMode::Apply,
            &root,
            &[RequestedWork::NativeTextureOptimization],
        ),
        &configuration,
        &sink,
        &mut cleanup,
        &CancellationToken::new(),
    );

    assert_eq!(result.outcome(), RunOutcome::Succeeded);
    assert_eq!(observed_phase_order(&sink), RunPhase::SEQUENCE);
    assert_eq!(result.asset_attempts().len(), 2);
    assert_eq!(toolset.attempted().len(), 2);
    assert_eq!(
        result
            .phase(RunPhase::ArchiveFinalization)
            .map(|record| record.status()),
        Some(RunPhaseStatus::Executed)
    );
    assert_eq!(toolset.finalizations(), 1);
    assert!(result.archive_finalization().is_some());
    assert_eq!(cleanup.passes, 1);
}

#[test]
fn requested_work_without_a_toolset_or_work_service_fails_preparing() {
    let root = scratch_dir("toolset-unavailable");
    write_tree(&root, &["textures/a.dds"]);
    let configuration = provider(None);
    let sink = RecordingSink::default();
    let mut cleanup = CountingCleanup::default();

    let result = execute(
        &request(
            ExecutionMode::Apply,
            &root,
            &[RequestedWork::NativeTextureOptimization],
        ),
        &configuration,
        &sink,
        &mut cleanup,
        &CancellationToken::new(),
    );

    assert_eq!(result.outcome(), RunOutcome::Failed);
    assert_eq!(
        result.failures()[0].code,
        RunFailureCode::RequestedWorkUnavailable
    );
    assert_eq!(result.failures()[0].phase, RunPhase::Preparing);
    assert!(result.preparation().is_none());
    assert_eq!(cleanup.passes, 1);
}

#[test]
fn a_dry_run_through_a_toolset_never_extracts_or_finalizes() {
    let root = canonical(&scratch_dir("toolset-dry-run"));
    write_archive(
        &root.join("content.bsa"),
        &[entry("textures/archived.dds", b"archived")],
    );
    write_tree(&root, &["textures/loose.dds"]);
    let toolset = Arc::new(ScriptedToolset::default());
    let sink = RecordingSink::default();

    let result = execute(
        &archives_and_textures(ExecutionMode::DryRun, &root),
        &provider(Some(&toolset)),
        &sink,
        &mut CountingCleanup::default(),
        &CancellationToken::new(),
    );

    assert_eq!(result.outcome(), RunOutcome::Succeeded);
    assert!(toolset.extracted().is_empty());
    assert!(toolset.reader.listed.lock().unwrap().is_empty());
    assert_eq!(toolset.finalizations(), 0);
    assert_eq!(
        toolset.attempted(),
        [root.join("textures").join("loose.dds")]
    );
    for phase in [RunPhase::ExtractingArchives, RunPhase::ArchiveFinalization] {
        assert_eq!(
            result.phase(phase).unwrap().skip_reason(),
            Some(PhaseSkipReason::DryRun),
            "{phase:?}"
        );
    }
}

#[test]
fn an_apply_run_extracts_archives_through_the_toolset_before_routing() {
    let root = canonical(&scratch_dir("toolset-extracts"));
    write_archive(
        &root.join("content.bsa"),
        &[entry("textures/extracted.dds", b"extracted")],
    );
    write_tree(&root, &["textures/loose.dds"]);
    let toolset = Arc::new(ScriptedToolset::default());

    let result = execute(
        &archives_and_textures(ExecutionMode::Apply, &root),
        &provider(Some(&toolset)),
        &RecordingSink::default(),
        &mut CountingCleanup::default(),
        &CancellationToken::new(),
    );

    assert_eq!(
        result.outcome(),
        RunOutcome::Succeeded,
        "{:?}",
        result.failures()
    );
    let plans = toolset.extracted();
    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0].archive_path, root.join("content.bsa"));
    assert_eq!(result.archive_extraction_attempts().len(), 1);
    let mut attempted = toolset.attempted();
    attempted.sort();
    assert_eq!(
        attempted,
        [
            root.join("textures").join("extracted.dds"),
            root.join("textures").join("loose.dds"),
        ]
    );
}

#[test]
fn an_extraction_port_panic_is_unknown_mutation_that_stops_the_run() {
    let root = canonical(&scratch_dir("toolset-extraction-panic"));
    write_archive(
        &root.join("content.bsa"),
        &[entry("textures/extracted.dds", b"extracted")],
    );
    write_tree(&root, &["textures/loose.dds"]);
    let toolset = Arc::new(
        ScriptedToolset::default().with_extraction(|_, _| panic!("the extractor panicked")),
    );

    let result = execute(
        &archives_and_textures(ExecutionMode::Apply, &root),
        &provider(Some(&toolset)),
        &RecordingSink::default(),
        &mut CountingCleanup::default(),
        &CancellationToken::new(),
    );

    assert_eq!(result.outcome(), RunOutcome::Failed);
    let attempt = &result.archive_extraction_attempts()[0];
    assert_eq!(attempt.mutation, MutationState::PartialOrUnknown);
    assert!(!attempt.safe_to_continue);
    assert_eq!(attempt.detail, "the extractor panicked");
    assert!(result.failures().is_empty(), "contained at the port");
    assert!(toolset.attempted().is_empty(), "no Asset is attempted");
    assert_eq!(toolset.finalizations(), 0);
}

#[test]
fn an_asset_attempt_port_panic_is_an_unsafe_operation_failure() {
    let root = scratch_dir("toolset-asset-panic");
    write_tree(&root, &["textures/a.dds", "textures/b.dds"]);
    let toolset =
        Arc::new(ScriptedToolset::default().with_assets(|_, _, _| panic!("the backend panicked")));

    let result = execute(
        &request(
            ExecutionMode::Apply,
            &root,
            &[RequestedWork::NativeTextureOptimization],
        ),
        &provider(Some(&toolset)),
        &RecordingSink::default(),
        &mut CountingCleanup::default(),
        &CancellationToken::new(),
    );

    assert_eq!(result.outcome(), RunOutcome::Failed);
    let [attempt] = result.asset_attempts() else {
        panic!("an unsafe attempt stops every later one");
    };
    assert_eq!(
        attempt.result.failure(),
        Some(AssetExecutionFailure::BackendException)
    );
    assert_eq!(
        attempt.result.mutation_state(),
        MutationState::PartialOrUnknown
    );
    assert!(!attempt.result.safe_to_continue());
    assert!(result.failures().is_empty(), "contained at the port");
    assert_eq!(toolset.finalizations(), 0);
}

#[test]
fn a_fake_backend_runs_behind_the_production_asset_attempt_adapter() {
    let root = scratch_dir("toolset-fake-backend");
    write_tree(&root, &["textures/a.dds", "textures/unloadable.dds"]);
    let diagnostic = RunDiagnostic::new(
        RunDiagnosticCode::PluginUnreadable,
        RunPhase::ProcessingAssets,
        "an unreadable plugin",
    );
    let factory = FakeBackendFactory {
        diagnostics: vec![diagnostic.clone()],
        ..FakeBackendFactory::default()
    };
    let creations = Arc::clone(&factory.creations);
    let toolset = Arc::new(ScriptedToolset::default().with_backend(factory));
    let sink = RecordingSink::default();

    let result = execute(
        &request(
            ExecutionMode::Apply,
            &root,
            &[RequestedWork::NativeTextureOptimization],
        ),
        &provider(Some(&toolset)),
        &sink,
        &mut CountingCleanup::default(),
        &CancellationToken::new(),
    );

    assert_eq!(result.outcome(), RunOutcome::CompletedWithFailures);
    assert_eq!(creations.load(Ordering::SeqCst), 1, "one backend per run");
    assert_eq!(result.diagnostics(), std::slice::from_ref(&diagnostic));
    assert!(sink.observed().contains(&Observed::Diagnostic(diagnostic)));
    assert!(
        root.join("textures/unloadable.dds.caobad").exists(),
        "a failed load is quarantined"
    );
}

#[test]
fn a_finalization_port_error_escapes_the_work_and_fails_the_run() {
    let root = scratch_dir("toolset-finalization-error");
    write_tree(&root, &["textures/a.dds"]);
    let toolset =
        Arc::new(ScriptedToolset::default().with_finalization_error("the packer is gone"));
    let mut cleanup = CountingCleanup::default();

    let result = execute(
        &request(
            ExecutionMode::Apply,
            &root,
            &[RequestedWork::NativeTextureOptimization],
        ),
        &provider(Some(&toolset)),
        &RecordingSink::default(),
        &mut cleanup,
        &CancellationToken::new(),
    );

    assert_eq!(result.outcome(), RunOutcome::Failed);
    let [failure] = result.failures() else {
        panic!("one Run Failure: {:?}", result.failures());
    };
    assert_eq!(failure.code, RunFailureCode::WorkServiceFailed);
    assert_eq!(failure.phase, RunPhase::ArchiveFinalization);
    assert!(failure.detail.contains("the packer is gone"), "{failure:?}");
    assert_eq!(result.asset_attempts().len(), 1, "earlier facts are kept");
    assert!(result.preparation().is_some());
    assert_eq!(cleanup.passes, 1);
}

#[test]
fn a_finalization_port_panic_is_an_unexpected_exception_keeping_its_attempts() {
    let root = scratch_dir("toolset-finalization-panic");
    write_tree(&root, &["textures/a.dds"]);
    let output = ArchiveFinalizationAttempt::new(root.join("out.bsa"), root.clone());
    let recorded = output.clone();
    let toolset = Arc::new(
        ScriptedToolset::default().with_finalization(move |_, recorder, _| {
            recorder.record_output_total(2)?;
            recorder.record_attempt(recorded.clone())?;
            panic!("the packer panicked")
        }),
    );

    let result = execute(
        &request(
            ExecutionMode::Apply,
            &root,
            &[RequestedWork::NativeTextureOptimization],
        ),
        &provider(Some(&toolset)),
        &RecordingSink::default(),
        &mut CountingCleanup::default(),
        &CancellationToken::new(),
    );

    assert_eq!(result.outcome(), RunOutcome::Failed);
    assert!(result.failures().is_empty(), "contained at the port");
    let finalization = result.archive_finalization().unwrap();
    assert_eq!(
        finalization.failure,
        Some(ArchiveFinalizationFailure::UnexpectedException)
    );
    assert!(!finalization.safe_to_continue);
    assert_eq!(finalization.detail, "the packer panicked");
    assert_eq!(finalization.attempts, [output]);
}

#[test]
fn a_sink_observer_can_cancel_once_the_extraction_phase_is_recorded() {
    let root = canonical(&scratch_dir("toolset-cancel-at-extraction"));
    write_archive(
        &root.join("content.bsa"),
        &[entry("textures/extracted.dds", b"extracted")],
    );
    let toolset = Arc::new(ScriptedToolset::default());
    let stop = CancellationToken::new();
    let token = stop.clone();
    let sink = RecordingSink::with_hook(move |observation| {
        if let Observed::Phase(record) = observation
            && record.phase() == RunPhase::ExtractingArchives
        {
            token.cancel();
        }
    });

    let result = execute(
        &archives_and_textures(ExecutionMode::Apply, &root),
        &provider(Some(&toolset)),
        &sink,
        &mut CountingCleanup::default(),
        &stop,
    );

    assert_eq!(result.outcome(), RunOutcome::Cancelled);
    assert!(result.archive_extraction_attempts().is_empty());
    assert!(toolset.extracted().is_empty(), "nothing is extracted");
    assert_eq!(toolset.finalizations(), 0);
}

/// The Apply or Dry Run request over native Textures and Archive extraction
/// for `root`.
fn archives_and_textures(mode: ExecutionMode, root: &Path) -> RunRequest {
    request(
        mode,
        root,
        &[
            RequestedWork::NativeTextureOptimization,
            RequestedWork::ArchiveExtraction,
        ],
    )
}
