//! The Run Executor driven through a Run Toolset: the configuration provider
//! returns the run's work ports beside its Run Configuration, and the
//! executor runs every work phase itself (#550).
//!
//! These scenarios prove the Toolset path at the executor's interface.

mod common;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use cao_core::execution::{AssetExecutionFailure, AssetExecutionResult, MutationState};
use cao_core::routing::{ExecutionMode, RequestedWork};
use cao_core::run::{
    ArchiveFinalizationAttempt, ArchiveFinalizationFailure, CancellationToken,
    OptimizationRunResult, PhaseSkipReason, RunConfigurationProvider, RunDiagnostic,
    RunDiagnosticCode, RunFailureCode, RunOutcome, RunPhase, RunPhaseStatus, RunRequest,
};
use cao_core::test_support::{
    CallbackConfiguration, CountingCleanup, FakeBackendFactory, FixedConfiguration, Observed,
    RecordingSink, ScriptedToolset, ToolsetConfiguration, entry, execute_run, sse_profile,
    write_archive,
};
use common::{canonical, request, scratch_dir, write_tree};

/// Executes `request` through `configuration`, observing every fact through
/// `sink`.
fn execute(
    request: &RunRequest,
    configuration: &ToolsetConfiguration,
    sink: &RecordingSink,
    cleanup: &mut CountingCleanup,
    stop: &CancellationToken,
) -> OptimizationRunResult {
    execute_run(request, cleanup, Some(sink), Some(configuration), stop)
}

#[test]
fn an_apply_run_through_a_toolset_traverses_every_phase_and_succeeds() {
    let root = scratch_dir("toolset-apply-success");
    write_tree(&root, &["textures/a.dds", "textures/b.dds"]);
    let toolset = Arc::new(
        ScriptedToolset::default()
            .with_assets(|_, _, _| Ok(AssetExecutionResult::success(MutationState::Committed))),
    );
    let configuration = ToolsetConfiguration::sse(Some(Arc::clone(&toolset)));
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
    assert_eq!(sink.phase_order(), RunPhase::SEQUENCE);
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

/// Every Optimization Run has a Run Toolset: a test provider given no script
/// returns a default scripted one, as production always returns its own, so
/// requested work runs and every Asset attempt succeeds.
#[test]
fn an_unscripted_provider_runs_requested_work_through_a_default_toolset() {
    let providers: [(&str, Box<dyn RunConfigurationProvider>); 3] = [
        ("toolset", Box::new(ToolsetConfiguration::sse(None))),
        (
            "fixed",
            Box::new(FixedConfiguration {
                profile: sse_profile(),
                loads: AtomicUsize::new(0),
            }),
        ),
        (
            "callback",
            Box::new(CallbackConfiguration(Box::new(|_| Ok(sse_profile())))),
        ),
    ];
    for (name, provider) in providers {
        let root = scratch_dir(&format!("toolset-default-{name}"));
        write_tree(&root, &["textures/a.dds"]);
        let mut cleanup = CountingCleanup::default();

        let result = execute_run(
            &request(
                ExecutionMode::Apply,
                &root,
                &[RequestedWork::NativeTextureOptimization],
            ),
            &mut cleanup,
            None,
            Some(&*provider),
            &CancellationToken::new(),
        );

        assert_eq!(
            result.outcome(),
            RunOutcome::Succeeded,
            "{name}: {:?}",
            result.failures()
        );
        assert!(result.preparation().is_some(), "{name}");
        assert_eq!(result.asset_attempts().len(), 1, "{name}");
        assert_eq!(cleanup.passes, 1, "{name}");
    }
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
        &ToolsetConfiguration::sse(Some(Arc::clone(&toolset))),
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
        &ToolsetConfiguration::sse(Some(Arc::clone(&toolset))),
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
        &ToolsetConfiguration::sse(Some(Arc::clone(&toolset))),
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
        &ToolsetConfiguration::sse(Some(Arc::clone(&toolset))),
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
        &ToolsetConfiguration::sse(Some(Arc::clone(&toolset))),
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

/// The executor's own containment: a finalization port that breaks its
/// contract by returning an error other than a Run Evidence invariant
/// violation simulates the run's work escaping. The run fails with Run Work
/// Escaped at Archive Finalization and keeps every earlier fact.
#[test]
fn a_finalization_port_error_escapes_the_work_and_fails_the_run() {
    let root = scratch_dir("toolset-finalization-error");
    write_tree(&root, &["textures/a.dds"]);
    let toolset =
        Arc::new(ScriptedToolset::default().with_escaping_finalization_work("the packer is gone"));
    let mut cleanup = CountingCleanup::default();

    let result = execute(
        &request(
            ExecutionMode::Apply,
            &root,
            &[RequestedWork::NativeTextureOptimization],
        ),
        &ToolsetConfiguration::sse(Some(Arc::clone(&toolset))),
        &RecordingSink::default(),
        &mut cleanup,
        &CancellationToken::new(),
    );

    assert_eq!(result.outcome(), RunOutcome::Failed);
    let [failure] = result.failures() else {
        panic!("one Run Failure: {:?}", result.failures());
    };
    assert_eq!(failure.code, RunFailureCode::RunWorkEscaped);
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
        &ToolsetConfiguration::sse(Some(Arc::clone(&toolset))),
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
        &ToolsetConfiguration::sse(Some(Arc::clone(&toolset))),
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
