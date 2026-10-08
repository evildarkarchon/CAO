//! Several Mods scenarios: how Preparing resolves a mods directory into Mod
//! Roots, reports Mod Exclusions, and rejects overlapping or escaping children.
//!
//! Each scenario names its C++ origin in `tests/OptimizationRunServiceTests.cpp`
//! or `tests/RunExecutorTests.cpp`. Scenarios that pin a recorded deviation say
//! so; the parity corpus never reaches them.

mod common;

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex, OnceLock};

use cao_core::routing::{ExecutionMode, RequestedWork};
use cao_core::run::{
    InlineRunScheduler, ModSelection, OptimizationRunResult, OptimizationRunService,
    RunConfiguration, RunDiagnosticCode, RunEventPayload, RunFailureCode, RunHandle, RunOutcome,
    RunPhase, RunRequest, RunSnapshot, RunWorkService,
};
use common::{
    BackendWork, EventLog, FixedConfiguration, GatedScheduler, HandleSlot, canonical, junction,
    reclaim, scratch_dir, serial, sse_profile, write_tree,
};

/// A provider for the SSE-like profile with the given Mod Exclusion lists.
fn exclusions(ignored_mods: &[&str], separator_suffixes: &[&str]) -> Arc<FixedConfiguration> {
    Arc::new(FixedConfiguration {
        configuration: RunConfiguration {
            profile: sse_profile(),
            ignored_mods: ignored_mods.iter().map(|name| (*name).to_owned()).collect(),
            separator_suffixes: separator_suffixes
                .iter()
                .map(|suffix| (*suffix).to_owned())
                .collect(),
        },
        loads: AtomicUsize::new(0),
    })
}

/// A Several Mods request over `mods` asking for no work.
fn several(mode: ExecutionMode, mods: &Path) -> RunRequest {
    RunRequest::new(
        "SkyrimSE",
        mode,
        ModSelection::ChildModRoots(mods.to_path_buf()),
        Vec::new(),
    )
}

/// Runs `request` inline with `configuration` and `work`, collecting its events.
fn run(
    configuration: Arc<FixedConfiguration>,
    work: Option<Arc<dyn RunWorkService>>,
    request: RunRequest,
) -> (Arc<OptimizationRunResult>, RunHandle, EventLog) {
    let service = OptimizationRunService::with_scheduler(
        Arc::new(InlineRunScheduler),
        Some(configuration),
        work,
    );
    let events = EventLog::default();
    let handle = service
        .start(request, Some(events.dispatcher()))
        .expect("the run starts");
    (handle.wait(), handle, events)
}

/// Creates each named child directory of `mods`.
fn children(mods: &Path, names: &[&str]) {
    for name in names {
        std::fs::create_dir_all(mods.join(name)).unwrap();
    }
}

/// The codes and paths of a result's diagnostics, in recording order.
fn diagnosed(result: &OptimizationRunResult) -> Vec<(RunDiagnosticCode, PathBuf)> {
    result
        .diagnostics()
        .iter()
        .map(|diagnostic| (diagnostic.code, diagnostic.path.clone()))
        .collect()
}

/// Origin: OptimizationRunServiceTests::severalModsRecordsOrderedImmediateRoots.
/// Only immediate child directories become Mod Roots, ordered by folded name
/// with the original name breaking ties.
#[test]
fn several_mods_resolves_each_immediate_child_directory_in_folded_name_order() {
    let _serial = serial();
    let mods = scratch_dir("several-ordered");
    children(
        &mods,
        &["Zebra/nested", "alpha", "Bravo", "Stra\u{df}e", "STRASSE"],
    );
    write_tree(&mods, &["not-a-mod.txt"]);

    let (result, _handle, _events) = run(
        exclusions(&[], &[]),
        None,
        several(ExecutionMode::DryRun, &mods.join(".")),
    );

    assert_eq!(result.outcome(), RunOutcome::Succeeded);
    let root = canonical(&mods);
    let expected: Vec<_> = ["alpha", "Bravo", "STRASSE", "Stra\u{df}e", "Zebra"]
        .iter()
        .map(|name| root.join(name))
        .collect();
    assert_eq!(result.mod_roots(), expected);
    assert!(
        mods.join("Zebra/nested").is_dir(),
        "Preparing never mutates"
    );
    assert!(mods.join("not-a-mod.txt").is_file());
}

/// Origin: OptimizationRunServiceTests::severalModsDiagnosesIgnoredChildrenOnce.
/// An ignored mod matches its whole name, case-folded, and is diagnosed once
/// however many list entries name it.
#[test]
fn an_ignored_mod_is_excluded_by_its_whole_folded_name_and_diagnosed_once() {
    let _serial = serial();
    let mods = scratch_dir("several-ignored");
    children(&mods, &["Nemesis", "\u{100}ssets", "Nemesis Extended"]);

    let (result, handle, events) = run(
        exclusions(&["nEmEsIs", "NEMESIS", "\u{101}ssets"], &[]),
        None,
        several(ExecutionMode::Apply, &mods),
    );

    assert_eq!(result.outcome(), RunOutcome::Succeeded);
    let root = canonical(&mods);
    assert_eq!(result.mod_roots(), [root.join("Nemesis Extended")]);
    assert_eq!(handle.diagnostics().len(), 2);
    assert_eq!(handle.snapshot().diagnostic_count, 2);
    let published: Vec<_> = events
        .events()
        .into_iter()
        .filter_map(|event| match event.payload {
            RunEventPayload::Diagnostic(diagnostic) => Some(diagnostic),
            _ => None,
        })
        .collect();
    assert_eq!(published.len(), 2);
    for diagnostic in &published {
        assert_eq!(diagnostic.code, RunDiagnosticCode::IgnoredModExcluded);
        assert_eq!(diagnostic.phase, RunPhase::Preparing);
        assert!(!diagnostic.detail.is_empty());
    }
    // Folded names order by their UTF-8 bytes, as C++'s `std::string` did, so
    // the non-ASCII name comes last.
    assert_eq!(
        diagnosed(&result),
        [
            (RunDiagnosticCode::IgnoredModExcluded, root.join("Nemesis")),
            (
                RunDiagnosticCode::IgnoredModExcluded,
                root.join("\u{100}ssets")
            ),
        ]
    );
}

/// Deviation 20 (origin: OptimizationRunServiceTests::severalModsUsesConfiguredSeparatorExclusions).
/// A separator is a child whose name ends in a configured suffix, matched
/// case-sensitively. "separator" elsewhere in a name, or in the mods
/// directory's own name, excludes nothing.
#[test]
fn a_separator_is_a_child_name_ending_in_the_separator_suffix() {
    let _serial = serial();
    let mods = scratch_dir("several-separators").join("parent_separator");
    children(
        &mods,
        &[
            "a_separator",
            "b separator mod",
            "c_SEPARATOR",
            "d_separator_old",
            "keep",
            "z_separator",
        ],
    );

    // An empty suffix would match every name, so it never excludes anything.
    // `z_separator` is also ignored, and owes only its separator exclusion.
    let (result, handle, _events) = run(
        exclusions(&["z_separator"], &["", "_separator"]),
        None,
        several(ExecutionMode::DryRun, &mods),
    );

    assert_eq!(result.outcome(), RunOutcome::Succeeded);
    let root = canonical(&mods);
    let processed: Vec<_> = ["b separator mod", "c_SEPARATOR", "d_separator_old", "keep"]
        .iter()
        .map(|name| root.join(name))
        .collect();
    assert_eq!(result.mod_roots(), processed);
    assert_eq!(
        diagnosed(&result),
        [
            (
                RunDiagnosticCode::SeparatorModExcluded,
                root.join("a_separator")
            ),
            (
                RunDiagnosticCode::SeparatorModExcluded,
                root.join("z_separator")
            ),
        ]
    );
    assert!(
        result
            .diagnostics()
            .iter()
            .all(|diagnostic| diagnostic.phase == RunPhase::Preparing)
    );
    drop(handle);

    // A Mod Exclusion applies only to the children of a mods directory.
    let (single, _handle, _events) = run(
        exclusions(&["z_separator"], &["_separator"]),
        None,
        RunRequest::new(
            "SkyrimSE",
            ExecutionMode::Apply,
            ModSelection::SingleModRoot(mods.join("a_separator")),
            Vec::new(),
        ),
    );
    assert_eq!(single.mod_roots(), [root.join("a_separator")]);
    assert!(single.diagnostics().is_empty());

    // With no suffix configured, every child is a Mod Root.
    let (unfiltered, _handle, _events) = run(
        exclusions(&[], &[]),
        None,
        several(ExecutionMode::Apply, &mods),
    );
    assert_eq!(unfiltered.mod_roots().len(), 6);
}

/// Deviation 19: CAO's reserved `.cao-staging` namespace is never a Mod Root.
/// The child is skipped silently, whatever its case or suffix, so it is not a
/// Mod Exclusion and owes no diagnostic.
#[test]
fn a_staging_named_child_is_never_a_mod_root_and_is_skipped_silently() {
    let _serial = serial();
    let mods = scratch_dir("several-staging");
    children(&mods, &[".cao-staging", ".CAO-STAGING-1234", "Mod"]);
    write_tree(
        &mods,
        &[".cao-staging-1/textures/leftover.dds", "Mod/textures/a.dds"],
    );

    let (result, _handle, events) = run(
        exclusions(&[".cao-staging"], &["staging"]),
        Some(Arc::new(BackendWork::new())),
        RunRequest::new(
            "SkyrimSE",
            ExecutionMode::DryRun,
            ModSelection::ChildModRoots(mods.clone()),
            vec![RequestedWork::NativeTextureOptimization],
        ),
    );

    assert_eq!(result.outcome(), RunOutcome::Succeeded);
    assert_eq!(result.mod_roots(), [canonical(&mods).join("Mod")]);
    assert!(result.diagnostics().is_empty());
    assert!(
        events
            .events()
            .iter()
            .all(|event| !matches!(event.payload, RunEventPayload::Diagnostic(_)))
    );
    let attempted: Vec<_> = result
        .asset_attempts()
        .iter()
        .map(|attempt| attempt.asset.execution_path().to_path_buf())
        .collect();
    assert_eq!(attempted, [canonical(&mods).join("Mod/textures/a.dds")]);
}

/// Origin: RunExecutorTests' Several Mods exclusion scenarios. Each child is
/// its own Mod Root, so every Asset is attributed to the mod it lies in, and
/// Mod Exclusions are Run Diagnostics that never change the Run Outcome.
#[test]
fn mod_exclusions_never_change_the_outcome_and_each_child_keeps_its_own_assets() {
    let _serial = serial();
    let mods = scratch_dir("several-attribution");
    write_tree(
        &mods,
        &[
            "Alpha/textures/a.dds",
            "Beta/textures/b.dds",
            "Group_separator/textures/s.dds",
            "Ignored/textures/i.dds",
        ],
    );

    let (result, _handle, events) = run(
        exclusions(&["ignored"], &["_separator"]),
        Some(Arc::new(BackendWork::new())),
        RunRequest::new(
            "SkyrimSE",
            ExecutionMode::DryRun,
            ModSelection::ChildModRoots(mods.clone()),
            vec![RequestedWork::NativeTextureOptimization],
        ),
    );

    assert_eq!(result.outcome(), RunOutcome::Succeeded);
    assert!(result.failures().is_empty());
    let root = canonical(&mods);
    let attributed: Vec<_> = result
        .asset_attempts()
        .iter()
        .map(|attempt| {
            (
                attempt.mod_root.clone(),
                attempt.asset.execution_path().to_path_buf(),
            )
        })
        .collect();
    assert_eq!(
        attributed,
        [
            (root.join("Alpha"), root.join("Alpha/textures/a.dds")),
            (root.join("Beta"), root.join("Beta/textures/b.dds")),
        ]
    );
    assert_eq!(
        diagnosed(&result),
        [
            (
                RunDiagnosticCode::SeparatorModExcluded,
                root.join("Group_separator")
            ),
            (RunDiagnosticCode::IgnoredModExcluded, root.join("Ignored")),
        ]
    );
    // Exclusions are published during Preparing, before any work phase.
    let order: Vec<_> = events
        .events()
        .into_iter()
        .filter_map(|event| match event.payload {
            RunEventPayload::Phase(record) => Some(format!("{:?}", record.phase())),
            RunEventPayload::Diagnostic(diagnostic) => Some(format!("{:?}", diagnostic.code)),
            _ => None,
        })
        .take(4)
        .collect();
    assert_eq!(
        order,
        [
            "Preparing",
            "SeparatorModExcluded",
            "IgnoredModExcluded",
            "DiscoveringArchives"
        ]
    );
}

/// Origin: OptimizationRunServiceTests::severalModsCanPrepareNoSelectedRoots.
#[test]
fn several_mods_can_prepare_no_mod_roots_but_a_missing_directory_fails() {
    let _serial = serial();
    let mods = scratch_dir("several-empty");

    for excluded in [false, true] {
        if excluded {
            children(&mods, &["Ignored"]);
        }
        let (result, handle, _events) = run(
            exclusions(&["ignored"], &[]),
            None,
            several(ExecutionMode::DryRun, &mods),
        );
        assert_eq!(result.outcome(), RunOutcome::Succeeded);
        assert!(result.preparation().is_some());
        assert!(result.mod_roots().is_empty());
        assert_eq!(handle.diagnostics().len(), usize::from(excluded));
    }

    let (missing, _handle, _events) = run(
        exclusions(&[], &[]),
        None,
        several(ExecutionMode::Apply, &mods.join("missing")),
    );
    assert_eq!(missing.outcome(), RunOutcome::Failed);
    assert_eq!(
        missing.failures()[0].code,
        RunFailureCode::ModSelectionResolutionFailed
    );
    assert!(missing.preparation().is_none());
    assert!(missing.phase(RunPhase::SafetyCleanup).is_some());
}

/// Origin: OptimizationRunServiceTests::cancellationFromAnExclusionStopsPreparing.
/// A dispatcher that cancels on the first exclusion stops Preparing before the
/// next child is examined.
#[test]
fn cancelling_from_an_exclusion_diagnostic_stops_preparing() {
    let _serial = serial();
    let mods = scratch_dir("several-cancel");
    children(&mods, &["a_ignored", "b_ignored"]);
    let scheduler = Arc::new(GatedScheduler::default());
    let service = OptimizationRunService::with_scheduler(
        scheduler.clone(),
        Some(exclusions(&["a_ignored", "b_ignored"], &[])),
        None,
    );
    let slot: HandleSlot = Arc::new(OnceLock::new());
    let snapshots: Arc<Mutex<Vec<RunSnapshot>>> = Arc::default();
    let dispatcher = {
        let slot = Arc::clone(&slot);
        let snapshots = Arc::clone(&snapshots);
        Box::new(move |event: cao_core::run::RunEvent| {
            if matches!(event.payload, RunEventPayload::Diagnostic(_)) {
                let handle = slot.get().expect("the handle is published before release");
                snapshots.lock().unwrap().push(handle.snapshot());
                handle.request_cancellation();
            }
        })
    };
    slot.set(
        service
            .start(several(ExecutionMode::Apply, &mods), Some(dispatcher))
            .unwrap(),
    )
    .unwrap();
    scheduler.release();

    let result = slot.get().unwrap().wait();
    let handle = reclaim(slot);

    assert_eq!(result.outcome(), RunOutcome::Cancelled);
    assert!(result.preparation().is_none());
    assert!(result.phase(RunPhase::DiscoveringArchives).is_none());
    assert!(result.phase(RunPhase::SafetyCleanup).is_some());
    let snapshots = snapshots.lock().unwrap();
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].diagnostic_count, 1);
    assert_eq!(handle.diagnostics().len(), 1);
    assert_eq!(
        handle.diagnostics()[0].path,
        canonical(&mods).join("a_ignored")
    );
}

/// Origin: OptimizationRunServiceTests::severalModsRejectsDuplicateLinkedRoots.
/// Two children resolving to one directory would process it twice.
#[test]
fn two_children_linking_to_one_directory_are_conflicting_mod_roots() {
    let _serial = serial();
    let mods = scratch_dir("several-duplicate-link");
    children(&mods, &["original"]);
    junction(&mods.join("alias"), &mods.join("original"));

    let (result, _handle, _events) = run(
        exclusions(&[], &[]),
        None,
        several(ExecutionMode::DryRun, &mods),
    );

    assert_eq!(result.outcome(), RunOutcome::Failed);
    assert_eq!(result.final_phase(), RunPhase::Preparing);
    assert!(result.preparation().is_none());
    assert_eq!(result.failures().len(), 1);
    assert_eq!(
        result.failures()[0].code,
        RunFailureCode::ConflictingModRoots
    );
    assert!(result.phase(RunPhase::DiscoveringArchives).is_none());
    assert!(result.phase(RunPhase::SafetyCleanup).is_some());
}

/// Origin: OptimizationRunServiceTests::severalModsRejectsNestedLinkedRoots.
/// A child linking inside another child's tree overlaps it, whichever comes first.
#[test]
fn a_child_linking_inside_another_child_is_a_conflicting_mod_root() {
    let _serial = serial();
    for descendant_first in [false, true] {
        let base = scratch_dir(&format!("several-nested-link-{descendant_first}"));
        let mods = base.join("selected");
        let target = base.join("target");
        std::fs::create_dir_all(&mods).unwrap();
        std::fs::create_dir_all(target.join("nested")).unwrap();
        let (ancestor, descendant) = if descendant_first {
            ("z-ancestor", "a-descendant")
        } else {
            ("a-ancestor", "z-descendant")
        };
        junction(&mods.join(ancestor), &target);
        junction(&mods.join(descendant), &target.join("nested"));

        let (result, _handle, _events) = run(
            exclusions(&[], &[]),
            None,
            several(ExecutionMode::Apply, &mods),
        );

        assert_eq!(result.outcome(), RunOutcome::Failed, "{descendant_first}");
        assert_eq!(result.final_phase(), RunPhase::Preparing);
        assert_eq!(result.failures().len(), 1);
        assert_eq!(
            result.failures()[0].code,
            RunFailureCode::ConflictingModRoots
        );
        assert!(result.preparation().is_none());
        assert!(result.phase(RunPhase::SafetyCleanup).is_some());
    }
}

/// Origin: RunExecutor's `resolveModRoots`. A child linking back to the mods
/// directory, or above it, would make the selection one of its own Mod Roots.
#[test]
fn a_child_linking_to_the_mods_directory_or_above_it_fails_preparing() {
    let _serial = serial();
    let base = scratch_dir("several-escaping-link");
    let mods = base.join("selected");
    std::fs::create_dir_all(mods.join("Mod")).unwrap();
    for target in [mods.clone(), base.clone()] {
        let link = mods.join("loop");
        junction(&link, &target);

        let (result, _handle, _events) = run(
            exclusions(&[], &[]),
            None,
            several(ExecutionMode::DryRun, &mods),
        );
        // Remove the link itself; its target must survive.
        std::fs::remove_dir(&link).unwrap();

        assert_eq!(result.outcome(), RunOutcome::Failed, "{}", target.display());
        assert_eq!(
            result.failures()[0].code,
            RunFailureCode::ModSelectionResolutionFailed
        );
        assert!(result.preparation().is_none());
    }
    assert!(mods.join("Mod").is_dir());
}

/// Origin: OptimizationRunServiceTests::linkedModRootsRetainCanonicalIdentities.
/// A linked child is retained as its target's canonical path, in the order of
/// the child names, not the target names.
#[test]
fn linked_mod_roots_retain_their_canonical_identities() {
    let _serial = serial();
    let base = scratch_dir("several-linked-identities");
    let mods = base.join("selected");
    let target = base.join("target");
    let sibling = base.join("target-more");
    for directory in [&mods, &target, &sibling] {
        std::fs::create_dir_all(directory).unwrap();
    }
    junction(&mods.join("z-first-target"), &target);
    junction(&mods.join("a-second-target"), &sibling);

    let (single, _handle, _events) = run(
        exclusions(&[], &[]),
        None,
        RunRequest::new(
            "SkyrimSE",
            ExecutionMode::DryRun,
            ModSelection::SingleModRoot(mods.join("z-first-target")),
            Vec::new(),
        ),
    );
    let (several_result, _handle, _events) = run(
        exclusions(&[], &[]),
        None,
        several(ExecutionMode::Apply, &mods),
    );
    // Removing the aliases cannot change identities the results already own.
    std::fs::remove_dir(mods.join("z-first-target")).unwrap();
    std::fs::remove_dir(mods.join("a-second-target")).unwrap();

    assert_eq!(single.outcome(), RunOutcome::Succeeded);
    assert_eq!(single.mod_roots(), [canonical(&target)]);
    assert_eq!(several_result.outcome(), RunOutcome::Succeeded);
    assert_eq!(
        several_result.mod_roots(),
        [canonical(&sibling), canonical(&target)]
    );
}
