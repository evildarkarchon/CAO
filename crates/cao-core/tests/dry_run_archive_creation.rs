//! A Dry Run plans its Archive creation and reports the plan, mutating
//! nothing (#643).
//!
//! Each scenario runs a whole Dry Run through the real Run Executor over the
//! fake packer and real Mod Root directories, as `archive_finalization.rs`
//! does for Apply, and asserts what Run Evidence records, what the packer was
//! asked to do, and that the Mod Roots are byte-for-byte unchanged.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cao_core::routing::{ExecutionMode, RequestedWork};
use cao_core::run::{
    ArchiveFinalizationChoices, ArchiveNameKind, CancellationToken, OptimizationRunResult,
    OutputPlanBasis, PackedArchiveKind, PlannedOutputArchive, RunFailure, RunFailureCode,
    RunOutcome, RunPhase, RunPhaseRecord,
};
use cao_core::test_support::{
    ArchiveRunRequest, FakeArchivePacker, ScriptedToolset, fake_dummy_plugin,
};
use common::{canonical, scratch_dir, snapshot_tree};

/// Writes `contents` to `path`, creating its parents.
fn write_file(path: &Path, contents: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// A fresh Mod Root named `mod` in a canonical scratch directory for one
/// scenario.
fn mod_root(name: &str) -> PathBuf {
    let root = canonical(&scratch_dir(&format!("dry-run-creation-{name}"))).join("mod");
    std::fs::create_dir(&root).unwrap();
    root
}

/// Runs `run` as a Dry Run over `packer`, returning the result and the fakes
/// it ran over.
fn dry_run(
    run: ArchiveRunRequest,
    packer: FakeArchivePacker,
    stop: &CancellationToken,
) -> (OptimizationRunResult, Arc<ScriptedToolset>) {
    let run = run.with_execution_mode(ExecutionMode::DryRun);
    let configuration = run.configuration(ScriptedToolset {
        packer: Arc::new(packer),
        ..ScriptedToolset::default()
    });
    let result = run.execute(&configuration, None, stop);
    (result, configuration.toolset)
}

/// Spec (#643): a Dry Run with Archive creation plans the outputs an Apply
/// run would create, a split included, each with the Dummy Plugin it would
/// need, and reaches only the packer's planning seams. It packs nothing,
/// creates no Loading Plugin for its outputs or for an existing Archive, and
/// prunes no directory, so the Mod Root stays byte-for-byte unchanged.
#[test]
fn a_dry_run_plans_its_output_archives_without_creating_any() {
    let root = mod_root("split");
    for name in ["c", "a", "b"] {
        write_file(&root.join(format!("meshes/{name}.nif")), b"twelve bytes");
    }
    // An Apply run would create a Dummy Plugin for this existing Archive
    // while maintaining Loading Plugins, and would prune the empty directory.
    write_file(&root.join("old.bsa"), b"an existing archive");
    std::fs::create_dir_all(root.join("empty/nested")).unwrap();
    let before = snapshot_tree(&root);
    // Room for one source per Archive.
    let packer = FakeArchivePacker {
        archive_limit: 20,
        ..FakeArchivePacker::default()
    };
    let choices = ArchiveFinalizationChoices {
        merge_incompressible: false,
        ..ArchiveFinalizationChoices::default()
    };

    let (result, toolset) = dry_run(
        ArchiveRunRequest::new(&root).with_archive_finalization(choices),
        packer,
        &CancellationToken::new(),
    );

    assert!(result.failures().is_empty(), "{:?}", result.failures());
    assert_eq!(
        result.phase(RunPhase::ArchiveFinalization),
        Some(&RunPhaseRecord::planned(RunPhase::ArchiveFinalization))
    );
    assert!(result.archive_finalization().is_none());
    // One twelve-byte mesh per Archive, each Archive with its own Dummy
    // Plugin, as `split_outputs_take_counter_names_and_their_own_plugins`
    // creates them under Apply.
    let planned = |archive: &str, plugin: &str| PlannedOutputArchive {
        archive_path: root.join(archive),
        mod_root: root.clone(),
        kind: PackedArchiveKind::Standard,
        file_count: 1,
        total_bytes: 12,
        dummy_plugin_path: Some(root.join(plugin)),
        deletes_packed_sources: true,
    };
    let work = result.planned_archive_work();
    assert_eq!(
        work.outputs(),
        [
            planned("mod.bsa", "mod.esp"),
            planned("mod0.bsa", "mod0.esp"),
            planned("mod1.bsa", "mod1.esp"),
        ]
    );
    assert_eq!(
        work.output_basis(),
        Some(OutputPlanBasis::EstimateFromModRootsAsTheyAre)
    );
    assert!(work.extractions().is_empty());
    assert!(result.mutation_summaries().is_empty());

    let packer = &toolset.packer;
    assert_eq!(
        packer.listed_names(),
        [
            (root.clone(), ArchiveNameKind::Archive),
            (root.clone(), ArchiveNameKind::Plugin)
        ]
    );
    assert_eq!(packer.partitioned(), std::slice::from_ref(&root));
    assert!(packer.writes().is_empty(), "nothing is packed");
    assert_eq!(snapshot_tree(&root), before, "the Mod Root is unchanged");
    assert!(root.join("empty/nested").is_dir(), "nothing is pruned");
    assert!(!root.join(".cao-staging").exists());
}

/// Spec (#643): without Dummy Plugins an Apply run removes each exact Dummy
/// Plugin and keeps its packed sources when asked to; a Dry Run plans its
/// output with no Dummy Plugin and no source deletion, and removes nothing.
#[test]
fn a_dry_run_without_dummy_plugins_plans_none_and_removes_none() {
    let root = mod_root("no-dummies");
    write_file(&root.join("meshes/a.nif"), b"mesh");
    write_file(&root.join("old.bsa"), b"an existing archive");
    write_file(&root.join("old.esp"), &fake_dummy_plugin());
    let before = snapshot_tree(&root);
    let choices = ArchiveFinalizationChoices {
        create_dummy_plugins: false,
        delete_sources: false,
        ..ArchiveFinalizationChoices::default()
    };

    let (result, toolset) = dry_run(
        ArchiveRunRequest::new(&root).with_archive_finalization(choices),
        FakeArchivePacker::default(),
        &CancellationToken::new(),
    );

    assert!(result.failures().is_empty(), "{:?}", result.failures());
    let [output] = result.planned_archive_work().outputs() else {
        panic!("one output: {:?}", result.planned_archive_work());
    };
    assert_eq!(output.archive_path, root.join("mod.bsa"));
    assert_eq!((output.file_count, output.total_bytes), (1, 4));
    assert_eq!(output.dummy_plugin_path, None);
    assert!(!output.deletes_packed_sources);
    // Without Dummy Plugins, planning lists only the Loading Plugins.
    assert_eq!(
        toolset.packer.listed_names(),
        [(root.clone(), ArchiveNameKind::Plugin)]
    );
    assert!(toolset.packer.writes().is_empty());
    assert_eq!(snapshot_tree(&root), before, "the Dummy Plugin stays");
}

/// Spec (#643): a Dry Run that requests work but no Archive creation still
/// reports Archive Finalization as Planned, with nothing planned and the
/// packer never reached.
#[test]
fn a_dry_run_without_archive_creation_plans_no_output() {
    let root = mod_root("no-creation");
    write_file(&root.join("textures/a.dds"), b"texture");
    std::fs::create_dir(root.join("empty")).unwrap();

    let (result, toolset) = dry_run(
        ArchiveRunRequest::new(&root).with_work([RequestedWork::NativeTextureOptimization]),
        FakeArchivePacker::default(),
        &CancellationToken::new(),
    );

    assert!(result.failures().is_empty(), "{:?}", result.failures());
    assert_eq!(
        result.phase(RunPhase::ArchiveFinalization),
        Some(&RunPhaseRecord::planned(RunPhase::ArchiveFinalization))
    );
    let work = result.planned_archive_work();
    assert!(work.outputs().is_empty());
    assert_eq!(work.output_basis(), None, "no Archive creation was planned");
    assert!(toolset.packer.listed_names().is_empty());
    assert!(toolset.packer.partitioned().is_empty());
    assert!(root.join("empty").is_dir(), "nothing is pruned");
}

/// Spec (#643): a planning failure is a Run Failure, as under Apply, where
/// it is Archive Finalization's Unexpected Exception
/// (`an_unexpected_failure_is_recorded_once` in `archive_finalization.rs`).
/// A panic of the packer while planning is contained, and nothing is
/// planned.
#[test]
fn a_planning_failure_fails_the_dry_run() {
    let root = mod_root("planning-fails");
    write_file(&root.join("meshes/a.nif"), b"mesh");
    let before = snapshot_tree(&root);

    let (result, _) = dry_run(
        ArchiveRunRequest::new(&root),
        FakeArchivePacker::panicking_list_names_beyond(0),
        &CancellationToken::new(),
    );

    assert_eq!(result.outcome(), RunOutcome::Failed);
    assert_eq!(result.final_phase(), RunPhase::ArchiveFinalization);
    assert_eq!(
        result.failures(),
        [RunFailure::new(
            RunFailureCode::ArchiveCreationPlanningFailed,
            RunPhase::ArchiveFinalization,
            "the archive name listing panicked",
        )]
    );
    assert_eq!(
        result.phase(RunPhase::ArchiveFinalization),
        Some(&RunPhaseRecord::planned(RunPhase::ArchiveFinalization))
    );
    assert!(result.planned_archive_work().outputs().is_empty());
    assert_eq!(snapshot_tree(&root), before);
}

/// Spec (#643): cancellation observed while planning stops the Dry Run with
/// nothing planned, as an Apply run's cancelled planning records no output
/// total (`cancelled_planning_records_no_output_total`).
#[test]
fn cancelled_planning_plans_nothing() {
    let root = mod_root("planning-cancelled");
    write_file(&root.join("meshes/a.nif"), b"mesh");
    let stop = CancellationToken::new();
    let cancel = stop.clone();
    let packer = FakeArchivePacker {
        before_list: Some(Box::new(move |_, _| cancel.cancel())),
        ..FakeArchivePacker::default()
    };

    let (result, toolset) = dry_run(ArchiveRunRequest::new(&root), packer, &stop);

    assert_eq!(result.outcome(), RunOutcome::Cancelled);
    assert!(result.cancellation_observed());
    assert!(result.failures().is_empty(), "{:?}", result.failures());
    assert_eq!(
        result.phase(RunPhase::ArchiveFinalization),
        Some(&RunPhaseRecord::planned(RunPhase::ArchiveFinalization))
    );
    let work = result.planned_archive_work();
    assert!(work.outputs().is_empty());
    assert_eq!(work.output_basis(), None);
    assert!(toolset.packer.partitioned().is_empty());
}
