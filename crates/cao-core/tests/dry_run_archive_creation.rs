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

use cao_core::execution::MutationState;
use cao_core::routing::{ExecutionMode, RequestedWork};
use cao_core::run::{
    ArchiveFinalizationChoices, ArchiveFinalizationFailure, ArchiveNameKind, CancellationToken,
    OptimizationRunResult, OutputPlanBasis, PackedArchiveKind, PlannedOutputArchive, RunDiagnostic,
    RunDiagnosticCode, RunFailure, RunFailureCode, RunOutcome, RunPhase, RunPhaseRecord,
};
use cao_core::test_support::{
    ArchiveRunRequest, FakeArchivePacker, FakeVolumeProbe, ScriptedToolset, fake_dummy_plugin,
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
    run_over(
        run.with_execution_mode(ExecutionMode::DryRun),
        packer,
        FakeVolumeProbe::default(),
        stop,
    )
}

/// Runs `run`, in whichever execution mode it carries, over `packer` and
/// `probe`, returning the result and the fakes it ran over.
fn run_over(
    run: ArchiveRunRequest,
    packer: FakeArchivePacker,
    probe: FakeVolumeProbe,
    stop: &CancellationToken,
) -> (OptimizationRunResult, Arc<ScriptedToolset>) {
    let configuration = run.configuration(ScriptedToolset {
        packer: Arc::new(packer),
        volume_probe: Arc::new(probe),
        ..ScriptedToolset::default()
    });
    let result = run.execute(&configuration, None, stop);
    (result, configuration.toolset)
}

/// A volume probe answering `available` free bytes for every Mod Root, all
/// of which sit on one volume named `shared-volume`.
fn shared_volume_with(available: u64) -> FakeVolumeProbe {
    FakeVolumeProbe::default()
        .with_capacity(move |_| Some(available))
        .with_volumes(|_| Some("shared-volume".to_owned()))
}

/// The run's capacity shortage warnings.
fn capacity_warnings(result: &OptimizationRunResult) -> Vec<&RunDiagnostic> {
    result
        .diagnostics()
        .iter()
        .filter(|diagnostic| diagnostic.code == RunDiagnosticCode::CapacityShortage)
        .collect()
}

/// The estimated bytes a capacity detail names, an Apply run's failure or a
/// Dry Run's warning alike: the number after "estimated ".
fn estimate_in(detail: &str) -> u64 {
    let digits: String = detail
        .split_once("estimated ")
        .unwrap_or_else(|| panic!("no estimate in {detail:?}"))
        .1
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().unwrap()
}

/// The Capacity Check failure an Apply run's Archive Finalization reported,
/// as its rejected output attempt's detail, or `None` when no output was
/// rejected for capacity.
fn creation_capacity_failure(result: &OptimizationRunResult) -> Option<String> {
    let finalization = result
        .archive_finalization()
        .unwrap_or_else(|| panic!("no Archive Finalization: {:?}", result.failures()));
    assert_eq!(finalization.failure, None);
    finalization.attempts.iter().find_map(|attempt| {
        (attempt.outcome.failure_category()
            == Some(ArchiveFinalizationFailure::InsufficientCapacity))
        .then(|| attempt.outcome.failure().unwrap().detail().to_owned())
    })
}

/// Two Mod Roots under one canonical scratch directory, each with a Loose
/// Asset to pack and an existing Archive whose Dummy Plugin allowance the
/// creation demand counts.
fn two_roots_to_pack(name: &str) -> (PathBuf, [PathBuf; 2]) {
    let parent = canonical(&scratch_dir(&format!("dry-run-creation-{name}")));
    let roots = [parent.join("mod-a"), parent.join("mod-b")];
    for root in &roots {
        write_file(&root.join("meshes/a.nif"), b"twelve bytes");
        write_file(&root.join("old.bsa"), b"an existing archive");
    }
    (parent, roots)
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

/// Spec (#645): a Dry Run whose planned output Archives would not fit the
/// free space warns with a warning-level capacity Run Diagnostic at Archive
/// Finalization, naming the Mod Root, its volume, the demand and the
/// available space. It still records every planned output, a split
/// included, and succeeds, mutating nothing.
#[test]
fn a_dry_run_warns_of_a_creation_capacity_shortage_and_still_plans_every_output() {
    let root = mod_root("capacity-shortage");
    for name in ["c", "a", "b"] {
        write_file(&root.join(format!("meshes/{name}.nif")), b"twelve bytes");
    }
    let before = snapshot_tree(&root);
    // Room for one source per Archive, so the plan splits into three.
    let packer = FakeArchivePacker {
        archive_limit: 20,
        ..FakeArchivePacker::default()
    };

    let (result, toolset) = run_over(
        ArchiveRunRequest::new(&root).with_execution_mode(ExecutionMode::DryRun),
        packer,
        shared_volume_with(0),
        &CancellationToken::new(),
    );

    assert_eq!(
        result.outcome(),
        RunOutcome::Succeeded,
        "{:?}",
        result.failures()
    );
    assert!(result.failures().is_empty(), "{:?}", result.failures());
    assert_eq!(
        result.phase(RunPhase::ArchiveFinalization),
        Some(&RunPhaseRecord::planned(RunPhase::ArchiveFinalization))
    );
    let outputs: Vec<_> = result
        .planned_archive_work()
        .outputs()
        .iter()
        .map(|output| output.archive_path.clone())
        .collect();
    assert_eq!(
        outputs,
        ["mod.bsa", "mod0.bsa", "mod1.bsa"].map(|name| root.join(name))
    );
    let [warning] = capacity_warnings(&result)[..] else {
        panic!("one capacity warning: {:?}", result.diagnostics());
    };
    assert_eq!(warning.phase, RunPhase::ArchiveFinalization);
    assert_eq!(warning.path, root);
    // Three outputs of twelve bytes each, plus their staging allowances.
    assert!(estimate_in(&warning.detail) > 36, "{}", warning.detail);
    for named in ["volume shared-volume", "but 0 bytes are available"] {
        assert!(
            warning.detail.contains(named),
            "{named:?} missing from {:?}",
            warning.detail
        );
    }
    assert!(toolset.packer.writes().is_empty(), "nothing is packed");
    assert_eq!(snapshot_tree(&root), before, "the Mod Root is unchanged");
}

/// Spec (#645): a Dry Run with room for its planned outputs warns of
/// nothing.
#[test]
fn a_dry_run_with_room_for_its_outputs_warns_of_no_shortage() {
    let root = mod_root("capacity-room");
    write_file(&root.join("meshes/a.nif"), b"twelve bytes");

    let (result, _) = run_over(
        ArchiveRunRequest::new(&root).with_execution_mode(ExecutionMode::DryRun),
        FakeArchivePacker::default(),
        shared_volume_with(u64::MAX),
        &CancellationToken::new(),
    );

    assert!(result.failures().is_empty(), "{:?}", result.failures());
    assert_eq!(result.planned_archive_work().outputs().len(), 1);
    assert!(
        capacity_warnings(&result).is_empty(),
        "{:?}",
        result.diagnostics()
    );
}

/// Spec (#645): an Apply run and a Dry Run compute the creation capacity
/// demand the same way, Dummy Plugin allowances and the volume grouping of
/// Several Mods included. At every free space an Apply run fails its
/// Capacity Check exactly when the Dry Run warns, and both name the same
/// Mod Root and the same estimated demand.
#[test]
fn apply_and_dry_run_agree_on_the_creation_capacity_demand() {
    let run_in = |mode: ExecutionMode, available: u64| {
        let (parent, roots) = two_roots_to_pack(&format!("agree-{mode:?}-{available}"));
        let (result, _) = run_over(
            ArchiveRunRequest::over_mods(&parent).with_execution_mode(mode),
            FakeArchivePacker::default(),
            shared_volume_with(available),
            &CancellationToken::new(),
        );
        (result, roots)
    };

    // The demand an Apply run fails on with no free space at all.
    let (applied, roots) = run_in(ExecutionMode::Apply, 0);
    let failure = creation_capacity_failure(&applied).expect("no space is a shortage");
    let demand = estimate_in(&failure);
    // Both roots' outputs and Dummy Plugin allowances share the volume.
    assert!(demand > 2 * 65_536, "{failure}");
    assert_eq!(
        applied.archive_finalization().unwrap().attempts[0].mod_root,
        roots[0]
    );

    for available in [0, demand - 1, demand] {
        let short = available < demand;
        let (applied, _) = run_in(ExecutionMode::Apply, available);
        let failure = creation_capacity_failure(&applied);
        assert_eq!(
            failure.is_some(),
            short,
            "Apply at {available}: {failure:?}"
        );
        if let Some(failure) = failure {
            assert_eq!(estimate_in(&failure), demand, "{failure}");
        }

        let (planned, roots) = run_in(ExecutionMode::DryRun, available);
        assert!(planned.failures().is_empty(), "{:?}", planned.failures());
        assert_eq!(planned.planned_archive_work().outputs().len(), 2);
        let warnings = capacity_warnings(&planned);
        assert_eq!(warnings.len(), usize::from(short), "Dry Run at {available}");
        if let [warning] = warnings[..] {
            assert_eq!(warning.path, roots[0]);
            assert_eq!(estimate_in(&warning.detail), demand, "{}", warning.detail);
            assert!(
                warning
                    .detail
                    .contains(&format!("but {available} bytes are available")),
                "{}",
                warning.detail
            );
        }
    }
}

/// Spec (#645): an Apply run with the creation shortage a Dry Run only warns
/// of behaves as before: Archive Finalization rejects the root's first
/// output for Insufficient Capacity before any mutation, nothing is
/// created or cleaned, and no capacity warning is recorded.
#[test]
fn an_apply_run_with_a_creation_capacity_shortage_still_fails_its_output() {
    let root = mod_root("apply-capacity-shortage");
    write_file(&root.join("meshes/a.nif"), b"twelve bytes");
    let before = snapshot_tree(&root);

    let (result, toolset) = run_over(
        ArchiveRunRequest::new(&root),
        FakeArchivePacker::default(),
        shared_volume_with(0),
        &CancellationToken::new(),
    );

    let finalization = result.archive_finalization().expect("Archive Finalization");
    assert_eq!(finalization.failure, None);
    assert!(finalization.safe_to_continue);
    let [rejected] = &finalization.attempts[..] else {
        panic!("one rejected output: {:?}", finalization.attempts);
    };
    assert_eq!(rejected.archive_path, root.join("mod.bsa"));
    assert_eq!(rejected.mod_root, root);
    assert_eq!(
        rejected.outcome.failure_category(),
        Some(ArchiveFinalizationFailure::InsufficientCapacity)
    );
    assert_eq!(rejected.outcome.mutation(), MutationState::None);
    let detail = rejected.outcome.failure().unwrap().detail();
    assert!(
        detail.starts_with("Insufficient Archive staging capacity: estimated "),
        "{detail}"
    );
    assert!(detail.contains("available 0 bytes"), "{detail}");
    assert!(
        capacity_warnings(&result).is_empty(),
        "{:?}",
        result.diagnostics()
    );
    assert!(result.planned_archive_work().is_empty());
    assert!(toolset.packer.writes().is_empty());
    assert_eq!(snapshot_tree(&root), before, "the Mod Root is unchanged");
}
