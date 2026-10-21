//! The Run Executor driven through a Run Toolset: the configuration provider
//! returns the run's Asset attempt port and Archive seams beside its Run
//! Configuration, and the executor runs every work phase itself (#550), with
//! core's own Archive extraction and Archive Finalization over the seams
//! (#581).
//!
//! These scenarios prove the Toolset path at the executor's interface,
//! including that each panic is contained once, by the module that owns its
//! rule.

mod common;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use cao_core::execution::{AssetExecutionFailure, AssetExecutionResult, MutationState};
use cao_core::routing::{ExecutionMode, RequestedWork};
use cao_core::run::{
    ArchiveExtractionFailure, ArchiveFinalizationFailure, ArchiveFinalizationMutationKind,
    CancellationToken, ModSelection, OptimizationRunResult, PhaseSkipReason,
    RunConfigurationProvider, RunDiagnostic, RunDiagnosticCode, RunOutcome, RunPhase,
    RunPhaseStatus, RunRequest,
};
use cao_core::test_support::{
    CallbackConfiguration, CountingCleanup, FakeArchivePacker, FakeArchiveReader,
    FakeBackendFactory, FixedConfiguration, Observed, RecordingSink, ScriptedToolset,
    ToolsetConfiguration, entry, execute_run, sse_profile, write_archive,
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
    let configuration = ToolsetConfiguration::sse(Arc::clone(&toolset));
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
    // Without Archive creation, the real phase only prunes, and this tree
    // has no empty directory to prune.
    let finalization = result.archive_finalization().unwrap();
    assert!(finalization.attempts.is_empty(), "{finalization:?}");
    assert!(finalization.mutations.is_empty(), "{finalization:?}");
    assert!(toolset.packer.writes().is_empty());
    assert_eq!(cleanup.passes, 1);
}

/// Every Optimization Run has a Run Toolset: a test provider given no script
/// returns a default scripted one, as production always returns its own, so
/// requested work runs and every Asset attempt succeeds. Its Archive work is
/// core's own, so Apply runs the real Archive Finalization.
#[test]
fn an_unscripted_provider_runs_requested_work_through_a_default_toolset() {
    let providers: [(&str, Box<dyn RunConfigurationProvider>); 3] = [
        (
            "toolset",
            Box::new(ToolsetConfiguration::sse(Arc::new(
                ScriptedToolset::default(),
            ))),
        ),
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
        std::fs::create_dir(root.join("empty")).unwrap();
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
        let finalization = result.archive_finalization().unwrap();
        assert_eq!(
            finalization
                .mutations
                .iter()
                .map(|mutation| (mutation.kind, mutation.count))
                .collect::<Vec<_>>(),
            [(ArchiveFinalizationMutationKind::EmptyDirectoryPruning, 1)],
            "{name}"
        );
        assert!(!root.join("empty").exists(), "{name}: pruned");
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
    std::fs::create_dir(root.join("empty")).unwrap();
    let toolset = Arc::new(ScriptedToolset::default());
    let sink = RecordingSink::default();

    let result = execute(
        &archives_and_textures(ExecutionMode::DryRun, &root),
        &ToolsetConfiguration::sse(Arc::clone(&toolset)),
        &sink,
        &mut CountingCleanup::default(),
        &CancellationToken::new(),
    );

    assert_eq!(result.outcome(), RunOutcome::Succeeded);
    assert!(result.archive_extraction_attempts().is_empty());
    assert!(toolset.reader.listed.lock().unwrap().is_empty());
    assert!(result.archive_finalization().is_none());
    assert!(
        toolset.packer.writes().is_empty(),
        "the packer is untouched"
    );
    assert!(root.join("content.bsa").exists(), "nothing is extracted");
    assert!(root.join("empty").exists(), "nothing is pruned");
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
        &ToolsetConfiguration::sse(Arc::clone(&toolset)),
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
    let [extraction] = result.archive_extraction_attempts() else {
        panic!("one extraction attempt");
    };
    assert_eq!(extraction.archive_path, root.join("content.bsa"));
    assert_eq!(extraction.mod_root, root);
    assert_eq!(extraction.mutation, MutationState::Committed);
    assert!(
        root.join("content.bsa.bak").exists(),
        "the source is backed up"
    );
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

/// Archive extraction is the only layer that contains a panic of the
/// Archive reader: one while staging, before anything is published, is a
/// failed extraction attempt with no mutation that is safe to continue. The
/// source is kept, the run goes on to the Loose Assets and Archive
/// Finalization, and no Run Failure is recorded.
#[test]
fn a_reader_panicking_while_staging_is_a_safe_failed_extraction() {
    let root = canonical(&scratch_dir("toolset-extraction-panic"));
    let source = root.join("content.bsa");
    write_archive(&source, &[entry("textures/extracted.dds", b"extracted")]);
    write_tree(&root, &["textures/loose.dds"]);
    let toolset = Arc::new(ScriptedToolset {
        reader: Arc::new(FakeArchiveReader {
            on_extract: Some(Box::new(|_, _| panic!("the archive library panicked"))),
            ..FakeArchiveReader::default()
        }),
        ..ScriptedToolset::default()
    });

    let result = execute(
        &archives_and_textures(ExecutionMode::Apply, &root),
        &ToolsetConfiguration::sse(Arc::clone(&toolset)),
        &RecordingSink::default(),
        &mut CountingCleanup::default(),
        &CancellationToken::new(),
    );

    assert_eq!(result.outcome(), RunOutcome::CompletedWithFailures);
    assert!(
        result.failures().is_empty(),
        "contained by Archive extraction"
    );
    let [attempt] = result.archive_extraction_attempts() else {
        panic!("one extraction attempt");
    };
    assert_eq!(
        attempt.failure,
        Some(ArchiveExtractionFailure::ExtractionFailed)
    );
    assert_eq!(attempt.mutation, MutationState::None);
    assert!(attempt.safe_to_continue);
    assert!(attempt.detail.contains("the archive library panicked"));
    assert!(source.exists(), "a failed extraction keeps its source");
    assert!(!root.join("textures/extracted.dds").exists());
    assert_eq!(
        toolset.attempted(),
        [root.join("textures").join("loose.dds")]
    );
    assert!(result.archive_finalization().is_some());
}

/// Core's real extraction leaves unknown mutation when its merge fails after
/// the first entry was published: a file takes the place of a folder an
/// entry needs once preflight has passed. The run is Failed before any Asset
/// is attempted or Archive Finalization runs, and the source is kept.
#[test]
fn an_extraction_left_partial_is_unknown_mutation_that_stops_the_run() {
    let root = canonical(&scratch_dir("toolset-extraction-partial"));
    let source = root.join("content.bsa");
    write_archive(
        &source,
        &[entry("a.dds", b"published"), entry("z/b.dds", b"blocked")],
    );
    write_tree(&root, &["textures/loose.dds"]);
    let obstruction = root.join("z");
    let reader = FakeArchiveReader {
        on_extract: Some(Box::new({
            let obstruction = obstruction.clone();
            move |_, _| {
                if !obstruction.exists() {
                    std::fs::write(&obstruction, "obstruction").unwrap();
                }
            }
        })),
        ..FakeArchiveReader::default()
    };
    let toolset = Arc::new(ScriptedToolset {
        reader: Arc::new(reader),
        ..ScriptedToolset::default()
    });

    let result = execute(
        &archives_and_textures(ExecutionMode::Apply, &root),
        &ToolsetConfiguration::sse(Arc::clone(&toolset)),
        &RecordingSink::default(),
        &mut CountingCleanup::default(),
        &CancellationToken::new(),
    );

    assert_eq!(result.outcome(), RunOutcome::Failed);
    let [attempt] = result.archive_extraction_attempts() else {
        panic!("one extraction attempt");
    };
    assert_eq!(attempt.failure, Some(ArchiveExtractionFailure::MergeFailed));
    assert_eq!(attempt.mutation, MutationState::PartialOrUnknown);
    assert!(!attempt.safe_to_continue);
    assert!(
        result.failures().is_empty(),
        "contained by Archive extraction"
    );
    assert!(toolset.attempted().is_empty(), "no Asset is attempted");
    assert!(result.archive_finalization().is_none());
    assert_eq!(std::fs::read(root.join("a.dds")).unwrap(), b"published");
    assert!(source.exists(), "a failed merge keeps its source");
    assert!(!root.join("content.bsa.bak").exists());
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
        &ToolsetConfiguration::sse(Arc::clone(&toolset)),
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
    assert!(
        result.archive_finalization().is_none(),
        "unsafe work never reaches Archive Finalization"
    );
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
        &ToolsetConfiguration::sse(Arc::clone(&toolset)),
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

/// Archive Finalization is the only layer that contains a panic of the
/// packer: here it panics after writing the second of two outputs. That
/// output attempt is an Unexpected Exception that forbids continuation,
/// while the first, already committed, is kept. The run is Failed with no
/// Run Failure, and the panicked output is never published.
#[test]
fn a_packer_panic_is_an_unexpected_exception_keeping_earlier_attempts() {
    let parent = canonical(&scratch_dir("toolset-finalization-panic"));
    let (first, second) = (parent.join("mod-a"), parent.join("mod-b"));
    write_tree(&first, &["textures/a.dds"]);
    write_tree(&second, &["textures/b.dds"]);
    let toolset = Arc::new(ScriptedToolset {
        packer: Arc::new(FakeArchivePacker::panicking_after_write_beyond(1)),
        ..ScriptedToolset::default()
    });

    let result = execute_run(
        &RunRequest::new(
            "SkyrimSE",
            ExecutionMode::Apply,
            ModSelection::ChildModRoots(parent.clone()),
            vec![
                RequestedWork::NativeTextureOptimization,
                RequestedWork::ArchiveCreation,
            ],
        ),
        &mut CountingCleanup::default(),
        None,
        Some(&ToolsetConfiguration::sse(Arc::clone(&toolset))),
        &CancellationToken::new(),
    );

    assert_eq!(result.outcome(), RunOutcome::Failed);
    assert!(result.failures().is_empty(), "contained by the phase");
    let finalization = result.archive_finalization().unwrap();
    let [kept, panicked] = finalization.attempts.as_slice() else {
        panic!("two output attempts: {finalization:?}");
    };
    assert!(kept.succeeded(), "{}", kept.detail);
    assert_eq!(kept.mutation, MutationState::Committed);
    assert_eq!(
        panicked.failure,
        Some(ArchiveFinalizationFailure::UnexpectedException)
    );
    assert_eq!(panicked.detail, "the archive writer panicked");
    assert!(!panicked.safe_to_continue);
    assert_eq!(panicked.mutation, MutationState::None);
    assert!(!finalization.safe_to_continue);
    assert!(!panicked.archive_path.exists(), "nothing is published");
    assert!(second.join("textures/b.dds").exists());
}

/// Core's real Archive Finalization contains a failure of its own work
/// outside any output attempt: here the packer's name listing panics in
/// Loading Plugin maintenance, once planning is done. It is one phase-level
/// Unexpected Exception that forbids continuation, so the run is Failed with
/// no Run Failure, and nothing is packed. The Texture is a Packing Exclusion,
/// so it is still attempted but plans no output that could commit before
/// maintenance. A panicking volume probe is no such failure: it is an
/// unknown volume (#592).
#[test]
fn an_archive_finalization_failure_outside_any_attempt_is_an_unexpected_exception() {
    let root = canonical(&scratch_dir("toolset-finalization-unexpected"));
    write_tree(&root, &["textures/asset.dds"]);
    let toolset = Arc::new(ScriptedToolset {
        // Planning lists the root's Archives and then its Plugins;
        // maintenance's first listing panics.
        packer: Arc::new(FakeArchivePacker::panicking_list_names_beyond(2)),
        ..ScriptedToolset::default()
    });
    let configuration = ToolsetConfiguration::new(
        sse_profile().with_packing_exclusions(vec!["textures/".to_owned()]),
        Arc::clone(&toolset),
    );

    let result = execute(
        &request(
            ExecutionMode::Apply,
            &root,
            &[
                RequestedWork::NativeTextureOptimization,
                RequestedWork::ArchiveCreation,
            ],
        ),
        &configuration,
        &RecordingSink::default(),
        &mut CountingCleanup::default(),
        &CancellationToken::new(),
    );

    assert_eq!(result.outcome(), RunOutcome::Failed);
    assert!(result.failures().is_empty(), "contained by the phase");
    assert_eq!(result.asset_attempts().len(), 1, "earlier facts are kept");
    let finalization = result.archive_finalization().unwrap();
    assert_eq!(
        finalization.failure,
        Some(ArchiveFinalizationFailure::UnexpectedException)
    );
    assert!(!finalization.safe_to_continue);
    assert_eq!(finalization.detail, "the archive name listing panicked");
    assert!(finalization.attempts.is_empty());
    assert_eq!(
        result
            .phase(RunPhase::ArchiveFinalization)
            .and_then(|record| record.progress())
            .map(|progress| progress.total()),
        Some(0),
        "planning finished, with nothing to pack"
    );
    assert!(toolset.packer.writes().is_empty());
    assert!(root.join("textures/asset.dds").exists());
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
        &ToolsetConfiguration::sse(Arc::clone(&toolset)),
        &sink,
        &mut CountingCleanup::default(),
        &stop,
    );

    assert_eq!(result.outcome(), RunOutcome::Cancelled);
    assert!(result.archive_extraction_attempts().is_empty());
    assert!(root.join("content.bsa").exists(), "nothing is extracted");
    assert!(!root.join("textures").exists(), "no entry is published");
    assert!(result.archive_finalization().is_none());
    assert!(toolset.packer.writes().is_empty());
}

/// A scripted run's Toolset is built through the production constructor:
/// extraction backs its source up, as the production default cleanup does,
/// and the real Archive Finalization packs the extracted and Loose Assets
/// through the scripted fake packer.
#[test]
fn an_apply_run_extracts_and_finalizes_for_real() {
    let root = canonical(&scratch_dir("toolset-core-archive-work"));
    write_archive(
        &root.join("content.bsa"),
        &[entry("meshes/extracted.nif", b"extracted")],
    );
    write_tree(&root, &["textures/loose.dds"]);
    let toolset = Arc::new(ScriptedToolset::default());

    let result = execute(
        &request(
            ExecutionMode::Apply,
            &root,
            &[
                RequestedWork::NativeTextureOptimization,
                RequestedWork::ArchiveExtraction,
                RequestedWork::ArchiveCreation,
            ],
        ),
        &ToolsetConfiguration::sse(Arc::clone(&toolset)),
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
    let [extraction] = result.archive_extraction_attempts() else {
        panic!("one extraction attempt");
    };
    assert_eq!(extraction.mutation, MutationState::Committed);
    assert!(
        root.join("content.bsa.bak").exists(),
        "the source is backed up, as the production default cleanup does"
    );
    let finalization = result.archive_finalization().unwrap();
    assert_eq!(finalization.failure, None, "{}", finalization.detail);
    assert!(!finalization.attempts.is_empty(), "{finalization:?}");
    assert!(
        finalization
            .attempts
            .iter()
            .all(|attempt| attempt.succeeded())
    );
    let mut packed: Vec<_> = toolset
        .packer
        .writes()
        .into_iter()
        .flat_map(|write| write.names)
        .collect();
    packed.sort();
    assert!(
        packed.contains(&"meshes/extracted.nif".to_owned()),
        "{packed:?}"
    );
    assert!(
        packed.contains(&"textures/loose.dds".to_owned()),
        "{packed:?}"
    );
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
