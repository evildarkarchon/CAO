//! The Archive Finalization phase over a fake packer and reader, fake probes
//! and real Mod Root directories, as the C++ suite drove
//! `ArchiveFinalization::run`.
//!
//! Each scenario names its C++ origin in `tests/ArchiveFinalizationTests.cpp`.
//! The fake packer stands in for one game's rules and writer, so these pin
//! the phase's own logic: capacity, naming, Loading Plugins, publication,
//! source cleanup, cancellation and evidence.
//!
//! Ported in `cao-optimizers` against real Archives, each naming its origin:
//! `finalizationCapacityEstimateCoversPublishedOutput`,
//! `existingArchivePluginNamesFollowProfile`,
//! `existingArchivesShareLoadingPlugin`, `plannedOutputNamesUseExactDummyBytes`
//! and `plannedOutputRechecksLoadingPluginNames`, whose rows are per-game rules.
//!
//! Not ported, with reasons:
//! - `volumeQueriesAcceptLongModRoots` and
//!   `volumeMountPointGrowsForLongMountedFolders`: native volume queries are
//!   `cao-winfs`'s, whose `volumes.rs` covers long paths and mount points.
//! - The SSE and FO4 rows of `finalizationRemovesOnlyExactDummyPlugins`: the
//!   canonical bytes are per game, so the fake game's single row stands for
//!   both; `cao-archive` pins each game's bytes.
//! - The non-Windows branches of the linked Loading Plugin scenarios: the
//!   port is Windows only.

mod common;

use std::cell::{Cell, RefCell};
use std::fs::{File, OpenOptions};
use std::os::windows::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use cao_core::Error;
use cao_core::execution::MutationState;
use cao_core::routing::{ExecutionMode, RequestedWork, RoutingPolicyRequest};
use cao_core::run::{
    ArchiveFinalization, ArchiveFinalizationFailure, ArchiveFinalizationMutationKind,
    ArchiveFinalizationResult, ArchiveFinalizationSettings, ArchivePrecedence, ArchiveReader,
    CancellationToken, MutableRunEvidence, MutationKind, PackedArchiveKind, RunConfiguration,
    RunDiagnostic, RunEvidence, RunFailure, RunObservationSink, RunPhase, RunPhaseRecord,
    RunPreparation, RunProgress, RunWorkEvidence, TemporaryArtifactRegistry, create_run_id,
};
use common::{
    FakeArchivePacker, FakeArchiveReader, FakeCapacity, FakeVolumes, archives_in, canonical,
    fake_dummy_plugin, junction, scratch_dir, sse_profile,
};

/// More bytes than any scenario needs.
const UNLIMITED: u64 = u64::MAX;

/// `FILE_SHARE_READ`, for handles that deny writing and deleting.
const SHARE_READ: u32 = 0x1;
/// `FILE_SHARE_WRITE | FILE_SHARE_DELETE`, for a handle that denies reading.
const SHARE_WRITE_DELETE: u32 = 0x2 | 0x4;

/// Writes `contents` to `path`, creating its parents.
fn write_file(path: &Path, contents: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// A fresh, canonical scratch directory for one scenario.
fn scratch(name: &str) -> PathBuf {
    canonical(&scratch_dir(&format!("finalization-{name}")))
}

/// A fresh Temporary Ownership scope.
fn registry() -> TemporaryArtifactRegistry {
    TemporaryArtifactRegistry::new(create_run_id())
}

/// An Apply run over `roots` whose Routing Policy requests Archive creation,
/// or only Texture work when `create_archives` is false.
fn preparation_for(roots: &[PathBuf], create_archives: bool) -> RunPreparation {
    let work = if create_archives {
        RequestedWork::ArchiveCreation
    } else {
        RequestedWork::NativeTextureOptimization
    };
    let configuration = RunConfiguration {
        profile: sse_profile(),
        ignored_mods: Vec::new(),
        separator_suffixes: Vec::new(),
    };
    let policy = configuration
        .profile
        .compile_policy(RoutingPolicyRequest::for_work(
            ExecutionMode::Apply,
            &[work],
        ))
        .unwrap();
    RunPreparation::new(
        roots.to_vec(),
        configuration,
        policy,
        ArchivePrecedence::DeterministicDiscovery,
    )
}

/// Run Evidence that has reached the executed Archive Finalization phase,
/// with the work view the phase records into (C++
/// `ArchiveFinalizationEvidenceFixture`).
struct Evidence<'s> {
    owner: RefCell<MutableRunEvidence<'s>>,
}

impl<'s> Evidence<'s> {
    fn new() -> Self {
        Self::observed(None)
    }

    /// Evidence publishing every accepted fact to `sink`.
    fn observed(sink: Option<&'s dyn RunObservationSink>) -> Self {
        let mut evidence = MutableRunEvidence::new(sink);
        for phase in [RunPhase::Preparing, RunPhase::ArchiveFinalization] {
            evidence
                .record_phase(RunPhaseRecord::executed(phase, None))
                .unwrap();
        }
        Self {
            owner: RefCell::new(evidence),
        }
    }

    fn work(&self) -> RunWorkEvidence<'_, 's> {
        RunWorkEvidence::new(&self.owner)
    }

    /// The retained result, or `None` before the phase recorded anything.
    fn recorded(&self) -> Option<ArchiveFinalizationResult> {
        self.owner.borrow().archive_finalization().cloned()
    }

    /// The retained result.
    fn finalization(&self) -> ArchiveFinalizationResult {
        self.recorded().expect("the phase recorded a result")
    }

    /// The phase's latest progress, or `None` before an output total exists.
    fn progress(&self) -> Option<RunProgress> {
        self.owner
            .borrow()
            .phase(RunPhase::ArchiveFinalization)
            .and_then(RunPhaseRecord::progress)
    }

    /// Moves the evidence on to Safety Cleanup, out of the phase.
    fn leave_phase(&self) {
        self.owner
            .borrow_mut()
            .record_phase(RunPhaseRecord::executed(RunPhase::SafetyCleanup, None))
            .unwrap();
    }

    /// Performs the executor's mandatory Safety Cleanup and seals the facts.
    fn seal(self) -> RunEvidence {
        self.leave_phase();
        self.owner.into_inner().consume().unwrap()
    }
}

/// A sink retaining every Archive Finalization progress account, as
/// `completed/total ok=succeeded failed=failed`, in a log probes also write.
struct ProgressLog(Log);

impl RunObservationSink for ProgressLog {
    fn record_phase(&self, phase: &RunPhaseRecord) {
        if let (RunPhase::ArchiveFinalization, Some(progress)) = (phase.phase(), phase.progress()) {
            self.0.push(format!(
                "progress {}/{} ok={} failed={}",
                progress.completed(),
                progress.total(),
                progress.succeeded(),
                progress.failed()
            ));
        }
    }

    // Archive Finalization reports no Run Failures or diagnostics.
    fn record_failure(&self, _: &RunFailure) {}

    fn record_diagnostic(&self, _: &RunDiagnostic) {}
}

/// An ordered log shared by sinks and probes.
#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<String>>>);

impl Log {
    fn push(&self, entry: String) {
        self.0.lock().unwrap().push(entry);
    }

    fn entries(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }

    /// The progress accounts, in publication order.
    fn progress(&self) -> Vec<String> {
        self.entries()
            .into_iter()
            .filter(|entry| entry.starts_with("progress"))
            .collect()
    }
}

/// A capacity probe answering `probe` for every Mod Root.
fn capacity(probe: impl Fn(&Path) -> Option<u64> + Send + Sync + 'static) -> FakeCapacity {
    FakeCapacity(Some(Box::new(probe)))
}

/// A volume-identity probe answering `probe` for every Mod Root.
fn volumes(probe: impl Fn(&Path) -> Option<String> + Send + Sync + 'static) -> FakeVolumes {
    FakeVolumes(Some(Box::new(probe)))
}

/// One Archive Finalization configuration; scenarios override only what they
/// depend on. The settings default to the options model's.
#[derive(Default)]
struct Finalizer {
    packer: FakeArchivePacker,
    reader: FakeArchiveReader,
    capacity: FakeCapacity,
    volumes: FakeVolumes,
    settings: ArchiveFinalizationSettings,
    /// When set, the Routing Policy requests only Texture work.
    no_archive_creation: bool,
    files_to_not_pack: Vec<String>,
    stop: CancellationToken,
}

impl Finalizer {
    /// Runs the phase once over `roots`, polling the finalizer's token.
    fn run(
        &self,
        roots: &[PathBuf],
        evidence: &Evidence<'_>,
        artifacts: &mut TemporaryArtifactRegistry,
    ) -> Result<(), Error> {
        self.run_polling(roots, evidence, artifacts, &|| self.stop.is_cancelled())
    }

    /// Runs the phase once over `roots`, polling `cancelled`.
    fn run_polling(
        &self,
        roots: &[PathBuf],
        evidence: &Evidence<'_>,
        artifacts: &mut TemporaryArtifactRegistry,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), Error> {
        ArchiveFinalization::new(
            &self.packer,
            &self.reader,
            &self.capacity,
            &self.volumes,
            self.settings,
            &self.files_to_not_pack,
        )
        .run(
            &preparation_for(roots, !self.no_archive_creation),
            &evidence.work(),
            artifacts,
            cancelled,
        )
    }

    /// The staging estimate the phase reports when `root` has no capacity at
    /// all: one planned output's estimate plus its plugin reserve.
    fn reported_estimate(&self, root: &Path) -> u64 {
        let zero = capacity(|_| Some(0));
        let evidence = Evidence::new();
        let mut artifacts = registry();
        ArchiveFinalization::new(
            &self.packer,
            &self.reader,
            &zero,
            &self.volumes,
            self.settings,
            &self.files_to_not_pack,
        )
        .run(
            &preparation_for(&[root.to_path_buf()], true),
            &evidence.work(),
            &mut artifacts,
            &|| false,
        )
        .unwrap();
        assert!(artifacts.cleanup().is_empty());
        let result = evidence.finalization();
        let detail = result
            .attempts
            .first()
            .map_or(result.detail.as_str(), |attempt| attempt.detail.as_str());
        let digits: String = detail
            .split_once("estimated ")
            .expect("the detail reports an estimate")
            .1
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        digits.parse().unwrap()
    }
}

/// The Dummy Plugin of the root `root`, as the phase names it.
fn root_plugin(root: &Path) -> PathBuf {
    let name = root.file_name().unwrap().to_str().unwrap();
    root.join(format!("{name}.esp"))
}

/// Whether the fake reader can read `archive` back.
fn reads_back(archive: &Path) -> bool {
    FakeArchiveReader::default().list_entries(archive).is_ok()
}

/// Creates a file symlink, or reports that this host cannot.
fn symlink(target: &Path, link: &Path) -> bool {
    std::os::windows::fs::symlink_file(target, link).is_ok()
}

/// Opens `path` for reading, sharing only `share` with other handles.
fn hold(path: &Path, share: u32) -> File {
    OpenOptions::new()
        .read(true)
        .share_mode(share)
        .open(path)
        .unwrap()
}

/// Origin: finalizationCapacityChecks (all four rows). Each output fits by
/// itself, but the phase checks its shared volume as a whole, and each
/// attempt rechecks its own estimate, grown sources included. A rejection
/// keeps its sources, publishes no Dummy Plugin and stops pruning; unknown
/// capacity proceeds.
#[test]
fn capacity_shortages_stop_before_mutation() {
    for (scenario, name) in [
        (0, "late-root-shortage"),
        (1, "unknown-capacity"),
        (2, "capacity-disappears"),
        (3, "source-grows-after-planning"),
    ] {
        let parent = scratch(&format!("capacity-{name}"));
        let roots = [parent.join("mod-a"), parent.join("mod-b")];
        for root in &roots {
            write_file(&root.join("meshes/asset.nif"), &[b'x'; 8192]);
            std::fs::create_dir(root.join("empty")).unwrap();
        }
        let mut finalizer = Finalizer {
            // Both roots share one volume, so the phase sums both outputs.
            volumes: volumes(|_| Some("shared-volume".to_owned())),
            ..Finalizer::default()
        };
        // Identical roots plan identical outputs, so one estimate stands for both.
        let estimate = finalizer.reported_estimate(&roots[0]);
        let grown = AtomicBool::new(false);
        let (first, second) = (roots[0].clone(), roots[1].clone());
        finalizer.capacity = capacity(move |root| {
            if scenario == 1 {
                return None;
            }
            // Each output fits alone, but not the shared phase at the later root.
            if scenario == 0 && root == second {
                return Some(estimate);
            }
            // The first probe follows planning, which froze the later estimate.
            if scenario == 3 && !grown.swap(true, Ordering::SeqCst) {
                write_file(&second.join("meshes/asset.nif"), &vec![b'y'; 1 << 20]);
            }
            if !archives_in(&first).is_empty() {
                return Some(if scenario == 3 { estimate } else { 0 });
            }
            Some(UNLIMITED)
        });
        let evidence = Evidence::new();
        let mut artifacts = registry();
        finalizer.run(&roots, &evidence, &mut artifacts).unwrap();

        let result = evidence.finalization();
        assert!(result.safe_to_continue, "{name}");
        assert!(!result.cancelled, "{name}");
        let progress = evidence.progress().unwrap();
        assert_eq!(progress.total(), 2, "{name}");
        assert_eq!(progress.completed(), result.attempts.len(), "{name}");
        let committed = match scenario {
            0 => 0,
            1 => 2,
            _ => 1,
        };
        for (index, root) in roots.iter().enumerate() {
            let done = index < committed;
            assert_eq!(archives_in(root).len(), usize::from(done), "{name} {index}");
            assert_eq!(root.join("meshes/asset.nif").exists(), !done, "{name}");
            assert_eq!(root.join("empty").exists(), scenario != 1, "{name}");
            // The Dummy Plugin is published with its output, never before a
            // rejected one.
            assert_eq!(root_plugin(root).exists(), done, "{name}");
            if !done {
                assert!(!root.join(".cao-staging").exists(), "{name}");
            }
        }
        if scenario != 1 {
            let rejected = result.attempts.last().unwrap();
            assert_eq!(
                rejected.failure,
                Some(ArchiveFinalizationFailure::InsufficientCapacity),
                "{name}"
            );
            assert_eq!(rejected.mutation, MutationState::None);
            // Every rejection is at the later root: its preflight or attempt.
            assert_eq!(rejected.mod_root, roots[1]);
            assert!(!rejected.detail.is_empty());
        }
        assert!(artifacts.cleanup().is_empty());
    }
}

/// Origin: finalizationCapacityIsGroupedByVolume. Roots on different volumes
/// each need only their own output's allowance.
#[test]
fn capacity_is_grouped_by_volume() {
    let parent = scratch("capacity-by-volume");
    let roots = [parent.join("mod-a"), parent.join("mod-b")];
    for root in &roots {
        write_file(&root.join("textures/asset.dds"), b"source bytes");
    }
    let mut finalizer = Finalizer {
        settings: ArchiveFinalizationSettings {
            create_dummy_plugins: false,
            compress: false,
            delete_sources: false,
            ..ArchiveFinalizationSettings::default()
        },
        ..Finalizer::default()
    };
    let estimate = finalizer.reported_estimate(&roots[0]);
    finalizer.capacity = capacity(move |_| Some(estimate));
    let first = roots[0].clone();
    finalizer.volumes = volumes(move |root| {
        Some(
            if root == first {
                "first-volume"
            } else {
                "second-volume"
            }
            .to_owned(),
        )
    });
    let evidence = Evidence::new();
    let mut artifacts = registry();
    finalizer.run(&roots, &evidence, &mut artifacts).unwrap();

    let result = evidence.finalization();
    assert_eq!(result.attempts.len(), 2);
    assert!(result.safe_to_continue);
    assert_eq!(result.failure, None);
    for attempt in &result.attempts {
        assert!(attempt.succeeded(), "{}", attempt.detail);
        assert!(attempt.archive_path.exists());
    }
    assert!(artifacts.cleanup().is_empty());
}

/// Origin: idleUnknownVolumeDoesNotInflateCapacity. A root with no planned
/// writes and an unknown volume cannot share another volume's work.
#[test]
fn an_idle_root_of_unknown_volume_does_not_inflate_capacity() {
    let parent = scratch("idle-unknown-volume");
    let roots = [
        parent.join("mod-a"),
        parent.join("mod-b"),
        parent.join("mod-idle"),
    ];
    for root in &roots[..2] {
        write_file(&root.join("textures/asset.dds"), b"source bytes");
    }
    std::fs::create_dir(&roots[2]).unwrap();
    let mut finalizer = Finalizer {
        settings: ArchiveFinalizationSettings {
            create_dummy_plugins: false,
            compress: false,
            ..ArchiveFinalizationSettings::default()
        },
        ..Finalizer::default()
    };
    let estimate = finalizer.reported_estimate(&roots[0]);
    let (first, idle) = (roots[0].clone(), roots[2].clone());
    let idle_root = idle.clone();
    finalizer.capacity = capacity(move |root| Some(if root == idle { 0 } else { estimate }));
    finalizer.volumes = volumes(move |root| {
        (root != idle_root).then(|| {
            if root == first {
                "first-volume"
            } else {
                "second-volume"
            }
            .to_owned()
        })
    });
    let evidence = Evidence::new();
    let mut artifacts = registry();
    finalizer.run(&roots, &evidence, &mut artifacts).unwrap();

    let result = evidence.finalization();
    assert_eq!(result.attempts.len(), 2);
    assert_eq!(result.failure, None);
    assert!(result.attempts.iter().all(|attempt| attempt.succeeded()));
    assert!(artifacts.cleanup().is_empty());
}

/// Origin: dummyCapacityDecreasesAfterEachRoot. Plugin maintenance rechecks
/// only the allowance the remaining roots still need, and each root's volume
/// is queried once.
#[test]
fn the_plugin_allowance_shrinks_after_each_root() {
    let parent = scratch("dummy-capacity");
    let roots = [parent.join("mod-a"), parent.join("mod-b")];
    for root in &roots {
        write_file(&root.join("existing.bsa"), b"retained archive");
    }
    let first_plugin = roots[0].join("existing.esp");
    let probed_plugin = first_plugin.clone();
    let queries = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&queries);
    let finalizer = Finalizer {
        capacity: capacity(move |_| {
            Some(std::fs::metadata(&probed_plugin).map_or(UNLIMITED, |metadata| metadata.len()))
        }),
        volumes: volumes(move |_| {
            counted.fetch_add(1, Ordering::SeqCst);
            Some("shared-volume".to_owned())
        }),
        ..Finalizer::default()
    };
    let evidence = Evidence::new();
    let mut artifacts = registry();
    finalizer.run(&roots, &evidence, &mut artifacts).unwrap();

    let result = evidence.finalization();
    assert_eq!(result.failure, None, "{}", result.detail);
    assert!(result.attempts.is_empty());
    assert_eq!(evidence.progress().unwrap().total(), 0);
    assert_eq!(queries.load(Ordering::SeqCst), roots.len());
    for root in &roots {
        assert!(root.join("existing.esp").exists());
    }
    assert!(artifacts.cleanup().is_empty());
}

/// Origin: finalizationCapacityWithoutOutputs. A plugin-only shortage is a
/// phase-level failure with no invented output progress, and no pruning.
#[test]
fn a_plugin_only_shortage_fails_the_phase_without_progress() {
    let root = scratch("capacity-without-outputs").join("mod");
    write_file(&root.join("existing.bsa"), b"retained archive");
    std::fs::create_dir(root.join("empty")).unwrap();
    let finalizer = Finalizer {
        capacity: capacity(|_| Some(0)),
        ..Finalizer::default()
    };
    let evidence = Evidence::new();
    let mut artifacts = registry();
    finalizer
        .run(std::slice::from_ref(&root), &evidence, &mut artifacts)
        .unwrap();

    let result = evidence.finalization();
    assert_eq!(
        result.failure,
        Some(ArchiveFinalizationFailure::InsufficientCapacity)
    );
    assert!(result.attempts.is_empty());
    assert!(result.safe_to_continue);
    assert!(!result.detail.is_empty());
    let progress = evidence.progress().unwrap();
    assert_eq!((progress.completed(), progress.total()), (0, 0));
    assert!(root.join("empty").exists());
    assert!(!root.join("existing.esp").exists());
    assert!(!root.join(".cao-staging").exists());
    assert!(artifacts.cleanup().is_empty());
}

/// Origin: disappearingLoadingPluginCapacityIsReserved (both rows). A
/// Loading Plugin seen while planning can vanish, so both the phase
/// preflight and the attempt reserve room for the fallback Dummy Plugin.
#[test]
fn a_disappearing_loading_plugin_keeps_its_fallback_reserved() {
    for shortage_at_preflight in [true, false] {
        let root = scratch(&format!("disappearing-plugin-{shortage_at_preflight}")).join("mod");
        let source = root.join("textures/asset.dds");
        write_file(&source, b"source bytes");
        let earlier_plugin = root.join("mod.esm");
        write_file(&earlier_plugin, b"existing loading plugin");
        let mut finalizer = Finalizer {
            settings: ArchiveFinalizationSettings {
                compress: false,
                create_dummy_plugins: false,
                ..ArchiveFinalizationSettings::default()
            },
            ..Finalizer::default()
        };
        // Without dummies the estimate is the Archive's alone.
        let archive_only = finalizer.reported_estimate(&root);
        finalizer.settings.create_dummy_plugins = true;
        let queries = AtomicUsize::new(0);
        let vanishing = earlier_plugin.clone();
        finalizer.capacity = capacity(move |_| {
            // Planning saw the Loading Plugin; it disappears before any mutation.
            if queries.fetch_add(1, Ordering::SeqCst) == 0 {
                let _ = std::fs::remove_file(&vanishing);
                if !shortage_at_preflight {
                    return Some(UNLIMITED);
                }
            }
            Some(archive_only)
        });
        let evidence = Evidence::new();
        let mut artifacts = registry();
        finalizer
            .run(std::slice::from_ref(&root), &evidence, &mut artifacts)
            .unwrap();

        let result = evidence.finalization();
        assert!(!earlier_plugin.exists());
        assert_eq!(result.attempts.len(), 1);
        assert_eq!(
            result.attempts[0].failure,
            Some(ArchiveFinalizationFailure::InsufficientCapacity)
        );
        assert_eq!(result.attempts[0].mutation, MutationState::None);
        assert!(result.mutations.is_empty());
        assert!(archives_in(&root).is_empty());
        assert!(!root.join("mod.esp").exists());
        assert!(source.exists());
        assert!(artifacts.cleanup().is_empty());
    }
}

/// Origin: finalizationPlanningObservesCancellation. Cancelled planning
/// records a cancelled result with no output total.
#[test]
fn cancelled_planning_records_no_output_total() {
    let root = scratch("planning-cancelled").join("mod");
    let source = root.join("textures/asset.dds");
    write_file(&source, b"source bytes");
    let finalizer = Finalizer::default();
    finalizer.stop.cancel();
    let evidence = Evidence::new();
    let mut artifacts = registry();
    finalizer
        .run(std::slice::from_ref(&root), &evidence, &mut artifacts)
        .unwrap();

    let result = evidence.finalization();
    assert!(result.cancelled);
    assert_eq!(result.failure, None);
    assert!(result.attempts.is_empty());
    assert_eq!(evidence.progress(), None);
    assert!(source.exists());
    assert!(!root.join(".cao-staging").exists());
    assert!(artifacts.cleanup().is_empty());
    assert!(evidence.seal().cancellation_observed());
}

/// Origin: finalizationFreezesTotalAndCancelsBetweenOutputs (all four rows).
/// The output total is recorded before any mutation, each attempt before the
/// next starts, and cancellation is observed only between outputs; pruning
/// waits for the whole plan.
#[test]
fn the_total_is_frozen_and_cancellation_falls_between_outputs() {
    for cancel_after in [Some(0), Some(1), Some(2), None] {
        let parent = scratch(&format!("cancel-between-{cancel_after:?}"));
        let roots = [parent.join("mod-a"), parent.join("mod-b")];
        for root in &roots {
            write_file(&root.join("textures/asset.dds"), b"source bytes");
        }
        let log = Log::default();
        let sink = ProgressLog(log.clone());
        let evidence = Evidence::observed(Some(&sink));
        let frozen_before_mutation = Arc::new(AtomicBool::new(false));
        let first_recorded_before_second = Arc::new(AtomicBool::new(false));
        let mut finalizer = Finalizer {
            settings: ArchiveFinalizationSettings {
                create_dummy_plugins: false,
                compress: false,
                ..ArchiveFinalizationSettings::default()
            },
            ..Finalizer::default()
        };
        let (frozen, recorded) = (
            Arc::clone(&frozen_before_mutation),
            Arc::clone(&first_recorded_before_second),
        );
        let (probe_roots, probe_log, stop) = (roots.clone(), log.clone(), finalizer.stop.clone());
        let probes = AtomicUsize::new(0);
        // One phase preflight per root, then one recheck per attempt.
        // Stopping during an attempt's recheck lets that attempt finish.
        finalizer.capacity = capacity(move |_| {
            let probe = probes.fetch_add(1, Ordering::SeqCst) + 1;
            if probe == 1 {
                frozen.store(
                    probe_log.progress() == ["progress 0/2 ok=0 failed=0"]
                        && probe_roots.iter().all(|root| {
                            archives_in(root).is_empty()
                                && !root.join(".cao-staging").exists()
                                && root.join("textures/asset.dds").exists()
                        }),
                    Ordering::SeqCst,
                );
            }
            if probe == 4 {
                recorded.store(
                    probe_log.progress().len() == 2 && archives_in(&probe_roots[0]).len() == 1,
                    Ordering::SeqCst,
                );
            }
            if cancel_after.is_some_and(|after| probe == 2 + after) {
                stop.cancel();
            }
            Some(UNLIMITED)
        });
        let mut artifacts = registry();
        finalizer.run(&roots, &evidence, &mut artifacts).unwrap();

        let label = format!("{cancel_after:?}");
        assert!(frozen_before_mutation.load(Ordering::SeqCst), "{label}");
        let attempted = cancel_after.unwrap_or(2);
        if attempted == 2 {
            assert!(
                first_recorded_before_second.load(Ordering::SeqCst),
                "{label}"
            );
        }
        let result = evidence.finalization();
        for root in &roots {
            // Pruning belongs to finalization and waits for the whole plan.
            assert_eq!(
                root.join("textures").exists(),
                cancel_after.is_some(),
                "{label}"
            );
        }
        assert_eq!(result.attempts.len(), attempted, "{label}");
        assert_eq!(result.cancelled, cancel_after.is_some(), "{label}");
        assert!(result.safe_to_continue);
        let expected: Vec<String> = (0..=attempted)
            .map(|index| format!("progress {index}/2 ok={index} failed=0"))
            .collect();
        assert_eq!(log.progress(), expected, "{label}");
        for (index, root) in roots.iter().enumerate() {
            let done = index < attempted;
            assert_eq!(archives_in(root).len(), usize::from(done), "{label}");
            assert_eq!(root.join("textures/asset.dds").exists(), !done, "{label}");
            if done {
                let attempt = &result.attempts[index];
                assert!(attempt.succeeded(), "{}", attempt.detail);
                assert_eq!(attempt.mod_root, *root);
                assert_eq!(attempt.mutation, MutationState::Committed);
                assert!(reads_back(&attempt.archive_path));
            }
        }
        assert!(artifacts.cleanup().is_empty());
    }
}

/// Origin: cancellationPreservesCommittedArchiveLoadingPlugin. A committed
/// output keeps its Loading Plugin when cancellation leaves later outputs
/// unattempted, and planning itself publishes none.
#[test]
fn cancellation_keeps_the_committed_archive_loadable() {
    let parent = scratch("cancel-keeps-plugin");
    let roots = [parent.join("mod-a"), parent.join("mod-b")];
    for root in &roots {
        write_file(&root.join("textures/asset.dds"), b"source bytes");
    }
    let no_plugin_before_attempts = Arc::new(AtomicBool::new(false));
    let mut finalizer = Finalizer {
        settings: ArchiveFinalizationSettings {
            compress: false,
            ..ArchiveFinalizationSettings::default()
        },
        ..Finalizer::default()
    };
    let (flag, probe_roots, stop) = (
        Arc::clone(&no_plugin_before_attempts),
        roots.clone(),
        finalizer.stop.clone(),
    );
    let probes = AtomicUsize::new(0);
    finalizer.capacity = capacity(move |_| {
        let probe = probes.fetch_add(1, Ordering::SeqCst) + 1;
        if probe == 1 {
            flag.store(
                probe_roots.iter().all(|root| !root_plugin(root).exists()),
                Ordering::SeqCst,
            );
        }
        // The third probe opens the first attempt, which must finish atomically.
        if probe == 3 {
            stop.cancel();
        }
        Some(UNLIMITED)
    });
    let evidence = Evidence::new();
    let mut artifacts = registry();
    finalizer.run(&roots, &evidence, &mut artifacts).unwrap();

    assert!(no_plugin_before_attempts.load(Ordering::SeqCst));
    let result = evidence.finalization();
    assert_eq!(result.attempts.len(), 1);
    assert!(result.cancelled);
    assert!(result.attempts[0].succeeded());
    assert!(reads_back(&result.attempts[0].archive_path));
    assert!(!roots[0].join("textures/asset.dds").exists());
    assert!(root_plugin(&roots[0]).exists());
    assert!(roots[1].join("textures/asset.dds").exists());
    assert!(archives_in(&roots[1]).is_empty());
    assert!(!root_plugin(&roots[1]).exists());
    assert!(artifacts.cleanup().is_empty());
}

/// Origin: finalizationCancelsPluginCleanupBetweenRoots.
#[test]
fn cancellation_stops_plugin_maintenance_between_roots() {
    let parent = scratch("cancel-plugin-maintenance");
    let roots = [parent.join("mod-a"), parent.join("mod-b")];
    for root in &roots {
        write_file(&root.join("existing.bsa"), b"retained archive");
        std::fs::create_dir_all(root.join("empty/nested")).unwrap();
    }
    let first_plugin = roots[0].join("existing.esp");
    let second_plugin = roots[1].join("existing.esp");
    let mut finalizer = Finalizer::default();
    let (second, probed_plugin, stop) = (
        roots[1].clone(),
        first_plugin.clone(),
        finalizer.stop.clone(),
    );
    finalizer.capacity = capacity(move |root| {
        if root == second && probed_plugin.exists() {
            stop.cancel();
        }
        Some(UNLIMITED)
    });
    let evidence = Evidence::new();
    let mut artifacts = registry();
    finalizer.run(&roots, &evidence, &mut artifacts).unwrap();

    let result = evidence.finalization();
    assert!(result.attempts.is_empty());
    assert!(result.cancelled);
    assert!(result.safe_to_continue);
    assert!(first_plugin.exists());
    assert!(!second_plugin.exists());
    for root in &roots {
        assert!(root.join("empty/nested").exists());
    }
    assert!(artifacts.cleanup().is_empty());
}

/// Origin: finalizationReportsGuardedPluginRemovalFailure. A Dummy Plugin
/// whose deletion another handle blocks is a contained failure after the
/// completed output.
#[test]
fn a_blocked_dummy_plugin_removal_is_a_contained_failure() {
    let root = scratch("blocked-removal").join("mod");
    write_file(&root.join("textures/asset.dds"), b"source bytes");
    let plugin = root.join("existing.esp");
    write_file(&plugin, &fake_dummy_plugin());
    let held: Arc<Mutex<Option<File>>> = Arc::default();
    let holder = Arc::clone(&held);
    let probed = plugin.clone();
    let finalizer = Finalizer {
        settings: ArchiveFinalizationSettings {
            create_dummy_plugins: false,
            compress: false,
            delete_sources: false,
            ..ArchiveFinalizationSettings::default()
        },
        // After planning, block only the deletion; reads stay shared.
        capacity: capacity(move |_| {
            holder
                .lock()
                .unwrap()
                .get_or_insert_with(|| hold(&probed, SHARE_READ));
            Some(UNLIMITED)
        }),
        ..Finalizer::default()
    };
    let evidence = Evidence::new();
    let mut artifacts = registry();
    finalizer
        .run(std::slice::from_ref(&root), &evidence, &mut artifacts)
        .unwrap();
    assert!(held.lock().unwrap().take().is_some());

    let result = evidence.finalization();
    assert_eq!(result.attempts.len(), 1);
    assert!(
        result.attempts[0].succeeded(),
        "{}",
        result.attempts[0].detail
    );
    assert!(result.safe_to_continue);
    assert_eq!(
        result.failure,
        Some(ArchiveFinalizationFailure::PluginRemovalFailed)
    );
    assert!(result.mutations.is_empty());
    assert!(!result.detail.is_empty());
    assert!(result.attempts[0].archive_path.exists());
    assert!(plugin.exists());
    assert!(artifacts.cleanup().is_empty());
}

/// Origin: finalizationRetainsPluginMutations (both rows). Plugin actions for
/// an existing Archive are mutation facts even with zero planned outputs.
#[test]
fn plugin_actions_for_existing_archives_are_mutation_facts() {
    for create_dummies in [true, false] {
        let root = scratch(&format!("plugin-mutations-{create_dummies}")).join("mod");
        write_file(&root.join("existing.bsa"), b"retained archive");
        let plugin = root.join("existing.esp");
        if !create_dummies {
            write_file(&plugin, &fake_dummy_plugin());
        }
        let finalizer = Finalizer {
            settings: ArchiveFinalizationSettings {
                create_dummy_plugins: create_dummies,
                ..ArchiveFinalizationSettings::default()
            },
            ..Finalizer::default()
        };
        let evidence = Evidence::new();
        let mut artifacts = registry();
        finalizer
            .run(std::slice::from_ref(&root), &evidence, &mut artifacts)
            .unwrap();

        let result = evidence.finalization();
        assert!(result.attempts.is_empty());
        assert_eq!(result.failure, None, "{}", result.detail);
        assert!(result.safe_to_continue);
        assert_eq!(result.mutations.len(), 1);
        let mutation = &result.mutations[0];
        assert_eq!(mutation.mod_root, root);
        assert_eq!(mutation.path, plugin);
        assert_eq!(
            mutation.kind,
            if create_dummies {
                ArchiveFinalizationMutationKind::PluginCreation
            } else {
                ArchiveFinalizationMutationKind::PluginRemoval
            }
        );
        assert_eq!(mutation.mutation, MutationState::Committed);
        assert_eq!(mutation.count, 1);
        assert_eq!(plugin.exists(), create_dummies);
        assert!(artifacts.cleanup().is_empty());

        let terminal = evidence.seal();
        assert!(terminal.archive_finalization().unwrap().attempts.is_empty());
        let progress = terminal
            .phase(RunPhase::ArchiveFinalization)
            .and_then(RunPhaseRecord::progress)
            .unwrap();
        assert_eq!(progress.total(), 0);
        assert_eq!(terminal.mutation_summaries().len(), 1);
        assert_eq!(terminal.mutation_summaries()[0].committed, 1);
    }
}

/// Origin: finalizationRemovesOnlyExactDummyPlugins. Only a plugin whose
/// complete bytes are canonical is removed, one written outside CAO
/// included; a same-size different plugin, a full plugin and a dummy outside
/// the Mod Root stay.
#[test]
fn only_exact_dummy_plugins_are_removed() {
    let parent = scratch("exact-dummies");
    let root = parent.join("mod");
    write_file(&root.join("existing.bsa"), b"retained archive");
    let dummy = fake_dummy_plugin();
    let mut same_size = dummy.clone();
    same_size[0] ^= 0x5a;
    let exact = root.join("external.esp");
    let different = root.join("different.esp");
    let full = root.join("full.esm");
    let unrelated = parent.join("outside.esp");
    write_file(&exact, &dummy);
    write_file(&different, &same_size);
    write_file(&full, b"full loading plugin");
    write_file(&unrelated, &dummy);
    let finalizer = Finalizer {
        settings: ArchiveFinalizationSettings {
            create_dummy_plugins: false,
            ..ArchiveFinalizationSettings::default()
        },
        ..Finalizer::default()
    };
    let evidence = Evidence::new();
    let mut artifacts = registry();
    finalizer
        .run(std::slice::from_ref(&root), &evidence, &mut artifacts)
        .unwrap();

    let result = evidence.finalization();
    assert!(result.attempts.is_empty());
    assert_eq!(result.failure, None, "{}", result.detail);
    assert!(result.safe_to_continue);
    let progress = evidence.progress().unwrap();
    assert_eq!((progress.completed(), progress.total()), (0, 0));
    assert_eq!(result.mutations.len(), 1);
    assert_eq!(result.mutations[0].mod_root, root);
    assert_eq!(result.mutations[0].path, exact);
    assert_eq!(
        result.mutations[0].kind,
        ArchiveFinalizationMutationKind::PluginRemoval
    );
    assert_eq!(result.mutations[0].mutation, MutationState::Committed);
    assert!(!exact.exists());
    assert_eq!(std::fs::read(&different).unwrap(), same_size);
    assert_eq!(std::fs::read(&full).unwrap(), b"full loading plugin");
    assert_eq!(std::fs::read(&unrelated).unwrap(), dummy);
    assert!(artifacts.cleanup().is_empty());
}

/// Origin: finalizationRejectsLinkedDummyPlugins (both rows). A hard-linked
/// or symlinked plugin with canonical bytes is never removed, nor its target.
#[test]
fn linked_dummy_plugins_are_never_removed() {
    for use_symlink in [false, true] {
        let parent = scratch(&format!("linked-dummy-{use_symlink}"));
        let root = parent.join("mod");
        write_file(&root.join("existing.bsa"), b"retained archive");
        let bytes = fake_dummy_plugin();
        let target = parent.join("outside.bin");
        let plugin = root.join("external.esp");
        write_file(&target, &bytes);
        if use_symlink {
            if !symlink(&target, &plugin) {
                eprintln!("skipped: file symlinks are unavailable on this host");
                continue;
            }
        } else {
            std::fs::hard_link(&target, &plugin).unwrap();
        }
        let finalizer = Finalizer {
            settings: ArchiveFinalizationSettings {
                create_dummy_plugins: false,
                ..ArchiveFinalizationSettings::default()
            },
            ..Finalizer::default()
        };
        let evidence = Evidence::new();
        let mut artifacts = registry();
        finalizer
            .run(std::slice::from_ref(&root), &evidence, &mut artifacts)
            .unwrap();

        let result = evidence.finalization();
        assert!(result.attempts.is_empty());
        assert_eq!(
            result.failure,
            Some(ArchiveFinalizationFailure::PluginRemovalFailed)
        );
        assert!(result.safe_to_continue);
        assert!(result.mutations.is_empty());
        assert_eq!(
            std::fs::symlink_metadata(&plugin).unwrap().is_symlink(),
            use_symlink
        );
        assert_eq!(std::fs::read(&target).unwrap(), bytes);
        assert!(artifacts.cleanup().is_empty());
    }
}

/// Origin: finalizationRejectsChangedDummyPluginParent. A Mod Root swapped
/// for a junction after planning cannot lead removal outside it.
#[test]
fn a_mod_root_swapped_for_a_junction_stops_removal() {
    let parent = scratch("swapped-root");
    let root = parent.join("mod");
    let outside = parent.join("outside");
    let retained = parent.join("retained-mod");
    let bytes = fake_dummy_plugin();
    write_file(&root.join("existing.bsa"), b"retained archive");
    write_file(&root.join("original.esp"), &bytes);
    write_file(&outside.join("external.esp"), &bytes);
    let swapped = Arc::new(AtomicBool::new(false));
    let (flag, link, target, moved) = (
        Arc::clone(&swapped),
        root.clone(),
        outside.clone(),
        retained.clone(),
    );
    let finalizer = Finalizer {
        settings: ArchiveFinalizationSettings {
            create_dummy_plugins: false,
            ..ArchiveFinalizationSettings::default()
        },
        // With no output or plugin reserve, the volume query is the phase's
        // only probe after planning froze the Mod Root.
        volumes: volumes(move |_| {
            if std::fs::rename(&link, &moved).is_ok() {
                junction(&link, &target);
                flag.store(true, Ordering::SeqCst);
            }
            Some("volume".to_owned())
        }),
        ..Finalizer::default()
    };
    let evidence = Evidence::new();
    let mut artifacts = registry();
    finalizer
        .run(std::slice::from_ref(&root), &evidence, &mut artifacts)
        .unwrap();
    std::fs::remove_dir(&root).unwrap();

    assert!(swapped.load(Ordering::SeqCst));
    let result = evidence.finalization();
    assert!(result.attempts.is_empty());
    assert_eq!(
        result.failure,
        Some(ArchiveFinalizationFailure::PluginRemovalFailed)
    );
    assert!(result.safe_to_continue);
    assert!(result.mutations.is_empty());
    assert!(retained.join("original.esp").exists());
    assert_eq!(std::fs::read(outside.join("external.esp")).unwrap(), bytes);
    assert!(artifacts.cleanup().is_empty());
}

/// Origin: finalizationPreservesReplacedDummyPlugin. A plugin replaced with
/// other bytes after planning is kept, and so is the displaced original.
#[test]
fn a_dummy_plugin_replaced_after_planning_is_kept() {
    let parent = scratch("replaced-dummy");
    let root = parent.join("mod");
    write_file(&root.join("textures/asset.dds"), b"source bytes");
    let bytes = fake_dummy_plugin();
    let mut replacement = bytes.clone();
    replacement[0] ^= 0x5a;
    let plugin = root.join("external.esp");
    let displaced = parent.join("displaced.bin");
    write_file(&plugin, &bytes);
    let replaced = Arc::new(AtomicBool::new(false));
    let (flag, probed, moved, written) = (
        Arc::clone(&replaced),
        plugin.clone(),
        displaced.clone(),
        replacement.clone(),
    );
    let finalizer = Finalizer {
        settings: ArchiveFinalizationSettings {
            create_dummy_plugins: false,
            compress: false,
            delete_sources: false,
            ..ArchiveFinalizationSettings::default()
        },
        // Planning recognized the exact dummy; replace it before cleanup.
        capacity: capacity(move |_| {
            if !flag.swap(true, Ordering::SeqCst) {
                std::fs::rename(&probed, &moved).unwrap();
                write_file(&probed, &written);
            }
            Some(UNLIMITED)
        }),
        ..Finalizer::default()
    };
    let evidence = Evidence::new();
    let mut artifacts = registry();
    finalizer
        .run(std::slice::from_ref(&root), &evidence, &mut artifacts)
        .unwrap();

    assert!(replaced.load(Ordering::SeqCst));
    let result = evidence.finalization();
    assert_eq!(result.attempts.len(), 1);
    assert!(
        result.attempts[0].succeeded(),
        "{}",
        result.attempts[0].detail
    );
    assert_eq!(result.failure, None, "{}", result.detail);
    assert!(result.mutations.is_empty());
    assert_eq!(std::fs::read(&plugin).unwrap(), replacement);
    assert_eq!(std::fs::read(&displaced).unwrap(), bytes);
    assert!(artifacts.cleanup().is_empty());
}

/// Origin: finalizationRetainsRemovalPrefixOnGuardFailure. Removals in
/// earlier roots stay recorded when a later one cannot proceed.
#[test]
fn earlier_removals_survive_a_later_guard_failure() {
    let parent = scratch("removal-prefix");
    let roots = [parent.join("mod-a"), parent.join("mod-b")];
    let bytes = fake_dummy_plugin();
    for root in &roots {
        write_file(&root.join("existing.bsa"), b"retained archive");
        write_file(&root.join("external.esp"), &bytes);
    }
    let blocked = roots[1].join("external.esp");
    // Shared reads let planning recognize the dummy; deletion stays blocked.
    let held = hold(&blocked, SHARE_READ);
    let finalizer = Finalizer {
        settings: ArchiveFinalizationSettings {
            create_dummy_plugins: false,
            ..ArchiveFinalizationSettings::default()
        },
        ..Finalizer::default()
    };
    let evidence = Evidence::new();
    let mut artifacts = registry();
    finalizer.run(&roots, &evidence, &mut artifacts).unwrap();
    drop(held);

    let result = evidence.finalization();
    assert!(result.attempts.is_empty());
    assert_eq!(
        result.failure,
        Some(ArchiveFinalizationFailure::PluginRemovalFailed)
    );
    assert!(result.safe_to_continue);
    assert_eq!(result.mutations.len(), 1);
    assert_eq!(result.mutations[0].mod_root, roots[0]);
    assert_eq!(result.mutations[0].path, roots[0].join("external.esp"));
    assert_eq!(
        result.mutations[0].kind,
        ArchiveFinalizationMutationKind::PluginRemoval
    );
    assert_eq!(result.mutations[0].mutation, MutationState::Committed);
    assert!(!roots[0].join("external.esp").exists());
    assert!(blocked.exists());
    let progress = evidence.progress().unwrap();
    assert_eq!((progress.completed(), progress.total()), (0, 0));
    assert!(artifacts.cleanup().is_empty());
}

/// Origin: existingArchivePluginCollisionFailsSafely. A non-plugin entry
/// that takes an existing Archive's plugin name is left alone.
#[test]
fn an_occupied_plugin_name_for_an_existing_archive_fails_safely() {
    let root = scratch("existing-collision").join("mod");
    write_file(&root.join("existing.bsa"), b"retained archive");
    let occupied = root.join("existing.esp");
    let probed = occupied.clone();
    let finalizer = Finalizer {
        // A non-plugin entry takes the Loading Plugin name after planning.
        capacity: capacity(move |_| {
            let _ = std::fs::create_dir(&probed);
            Some(UNLIMITED)
        }),
        ..Finalizer::default()
    };
    let evidence = Evidence::new();
    let mut artifacts = registry();
    finalizer
        .run(std::slice::from_ref(&root), &evidence, &mut artifacts)
        .unwrap();

    let result = evidence.finalization();
    assert!(result.attempts.is_empty());
    assert_eq!(
        result.failure,
        Some(ArchiveFinalizationFailure::PluginCreationFailed)
    );
    assert!(result.safe_to_continue);
    assert!(result.mutations.is_empty());
    let progress = evidence.progress().unwrap();
    assert_eq!((progress.completed(), progress.total()), (0, 0));
    assert!(occupied.is_dir());
    assert!(artifacts.cleanup().is_empty());
}

/// Origin: packingPreservesStagingFiles. Temporary bytes, ours or another
/// run's, are never packed or deleted as packed sources.
#[test]
fn staging_files_are_never_packed() {
    let root = scratch("staging-not-packed").join("mod");
    std::fs::create_dir_all(&root).unwrap();
    let mut artifacts = registry();
    let staged = artifacts.stage_archive_file(&root).unwrap().path;
    let nested = root.join("textures/.cao-staging-old/pending.dds");
    std::fs::write(&staged, b"temporary bytes").unwrap();
    write_file(&nested, b"unverified temporary bytes");
    write_file(&root.join("textures/complete.dds"), b"committed bytes");
    let finalizer = Finalizer {
        settings: ArchiveFinalizationSettings {
            create_dummy_plugins: false,
            compress: false,
            ..ArchiveFinalizationSettings::default()
        },
        ..Finalizer::default()
    };
    let evidence = Evidence::new();
    finalizer
        .run(std::slice::from_ref(&root), &evidence, &mut artifacts)
        .unwrap();

    let result = evidence.finalization();
    assert_eq!(result.attempts.len(), 1);
    assert!(
        result.attempts[0].succeeded(),
        "{}",
        result.attempts[0].detail
    );
    assert!(staged.exists());
    assert!(nested.exists());
    assert!(!root.join("textures/complete.dds").exists());
    assert_eq!(
        finalizer.packer.writes()[0].names,
        ["textures/complete.dds"]
    );
    assert!(!archives_in(&root).is_empty());
    assert!(artifacts.cleanup().is_empty());
}

/// Origin: filesToNotPackAreNeitherPackedNorDeleted. A Packing Exclusion
/// matches case-insensitively, and its file stays loose.
#[test]
fn packing_exclusions_are_neither_packed_nor_deleted() {
    let root = scratch("files-to-not-pack").join("mod");
    let packed = root.join("textures/asset.dds");
    let retained = root.join("textures/keep/asset.dds");
    write_file(&packed, b"packed bytes");
    write_file(&retained, b"retained bytes");
    let finalizer = Finalizer {
        settings: ArchiveFinalizationSettings {
            create_dummy_plugins: false,
            compress: false,
            ..ArchiveFinalizationSettings::default()
        },
        files_to_not_pack: vec!["TEXTURES/KEEP/".to_owned()],
        ..Finalizer::default()
    };
    let evidence = Evidence::new();
    let mut artifacts = registry();
    finalizer
        .run(std::slice::from_ref(&root), &evidence, &mut artifacts)
        .unwrap();

    let result = evidence.finalization();
    assert_eq!(result.attempts.len(), 1);
    assert!(
        result.attempts[0].succeeded(),
        "{}",
        result.attempts[0].detail
    );
    assert!(!packed.exists());
    assert_eq!(std::fs::read(&retained).unwrap(), b"retained bytes");
    let entries = FakeArchiveReader::default()
        .list_entries(&result.attempts[0].archive_path)
        .unwrap();
    assert_eq!(entries.len(), 1);
    assert!(artifacts.cleanup().is_empty());
}

/// **Deviation 16.** A Packing Exclusion matches the path within the Mod
/// Root, never the folders above it, and a rule spelled with backslashes
/// matches too. C++ matched the absolute path, so a mods folder named like a
/// rule (`CalienteTools/` ships in `FilesToNotPack.txt`) kept every mod
/// beneath it loose.
#[test]
fn deviation_16_packing_exclusions_match_within_the_mod_root() {
    let root = scratch("deviation-16").join("CalienteTools").join("mod");
    let packed = root.join("textures/asset.dds");
    let excluded = root.join("CalienteTools/bodyslide.nif");
    let backslashed = root.join("textures/keep/asset.dds");
    write_file(&packed, b"packed bytes");
    write_file(&excluded, b"excluded bytes");
    write_file(&backslashed, b"backslash rule bytes");
    let finalizer = Finalizer {
        settings: ArchiveFinalizationSettings {
            create_dummy_plugins: false,
            compress: false,
            ..ArchiveFinalizationSettings::default()
        },
        files_to_not_pack: vec!["CalienteTools/".to_owned(), "textures\\keep\\".to_owned()],
        ..Finalizer::default()
    };
    let evidence = Evidence::new();
    let mut artifacts = registry();
    finalizer
        .run(std::slice::from_ref(&root), &evidence, &mut artifacts)
        .unwrap();

    let result = evidence.finalization();
    assert_eq!(result.attempts.len(), 1);
    assert!(
        result.attempts[0].succeeded(),
        "{}",
        result.attempts[0].detail
    );
    let names: Vec<_> = finalizer
        .packer
        .writes()
        .into_iter()
        .flat_map(|write| write.names)
        .collect();
    assert_eq!(names, ["textures/asset.dds"]);
    assert!(!packed.exists());
    assert!(excluded.exists());
    assert!(backslashed.exists());
    assert!(artifacts.cleanup().is_empty());
}

/// Origin: failedPackingRetainsSourcesAndExistingArchives. A source that
/// cannot be read fails the output before anything is published.
#[test]
fn a_failed_write_keeps_sources_and_existing_archives() {
    let root = scratch("failed-write").join("mod");
    let available = root.join("textures/available.dds");
    let unavailable = root.join("textures/unavailable.dds");
    let existing = root.join("mod.bsa");
    write_file(&available, b"available source bytes");
    write_file(&unavailable, b"unavailable source bytes");
    write_file(&existing, b"previous archive bytes");
    // Deny reads while allowing rename and delete.
    let locked = OpenOptions::new()
        .write(true)
        .share_mode(SHARE_WRITE_DELETE)
        .open(&unavailable)
        .unwrap();
    let finalizer = Finalizer {
        settings: ArchiveFinalizationSettings {
            create_dummy_plugins: false,
            compress: false,
            ..ArchiveFinalizationSettings::default()
        },
        ..Finalizer::default()
    };
    let evidence = Evidence::new();
    let mut artifacts = registry();
    finalizer
        .run(std::slice::from_ref(&root), &evidence, &mut artifacts)
        .unwrap();
    drop(locked);

    let result = evidence.finalization();
    assert_eq!(result.attempts.len(), 1);
    assert!(!result.attempts[0].succeeded());
    assert_eq!(result.attempts[0].mutation, MutationState::None);
    assert!(available.exists());
    assert!(unavailable.exists());
    assert_eq!(std::fs::read(&existing).unwrap(), b"previous archive bytes");
    assert!(artifacts.cleanup().is_empty());
}

/// Origin: packedSourceParentJunctionIsRejected. A planned source whose
/// parent became a junction is never read through it.
#[test]
fn a_source_parent_swapped_for_a_junction_fails_the_write() {
    let base = scratch("source-junction");
    let root = base.join("mod");
    let source_parent = root.join("textures");
    let outside = base.join("outside");
    let outside_source = outside.join("armor/asset.dds");
    let retained_parent = root.join("retained-textures");
    write_file(
        &source_parent.join("armor/asset.dds"),
        b"planned source bytes",
    );
    write_file(&outside_source, b"outside source bytes");
    write_file(&outside.join("marker.txt"), b"keep target directory");
    let swapped = Arc::new(AtomicBool::new(false));
    let (flag, link, target, moved) = (
        Arc::clone(&swapped),
        source_parent.clone(),
        outside.clone(),
        retained_parent.clone(),
    );
    let finalizer = Finalizer {
        settings: ArchiveFinalizationSettings {
            create_dummy_plugins: false,
            compress: false,
            ..ArchiveFinalizationSettings::default()
        },
        // Swap the parent for a junction before the output attempt starts.
        capacity: capacity(move |_| {
            if !flag.load(Ordering::SeqCst) && std::fs::rename(&link, &moved).is_ok() {
                junction(&link, &target);
                flag.store(true, Ordering::SeqCst);
            }
            Some(UNLIMITED)
        }),
        ..Finalizer::default()
    };
    let evidence = Evidence::new();
    let mut artifacts = registry();
    finalizer
        .run(std::slice::from_ref(&root), &evidence, &mut artifacts)
        .unwrap();
    std::fs::remove_dir(&source_parent).unwrap();

    assert!(swapped.load(Ordering::SeqCst));
    let result = evidence.finalization();
    assert_eq!(result.attempts.len(), 1);
    assert_eq!(
        result.attempts[0].failure,
        Some(ArchiveFinalizationFailure::WriteFailed)
    );
    assert_eq!(result.attempts[0].mutation, MutationState::None);
    assert!(!result.attempts[0].archive_path.exists());
    assert!(retained_parent.join("armor/asset.dds").exists());
    assert_eq!(
        std::fs::read(&outside_source).unwrap(),
        b"outside source bytes"
    );
    assert!(artifacts.cleanup().is_empty());
}

/// Origin: plannedNamesAreDistinctAndCommitPreservesNewDestination. Each
/// output gets its own name, and a destination taken after planning wins.
#[test]
fn planned_names_are_distinct_and_a_late_destination_wins() {
    let root = scratch("distinct-names").join("mod");
    let mesh = root.join("meshes/asset.nif");
    let sound = root.join("sound/asset.wav");
    write_file(&mesh, b"mesh bytes");
    write_file(&sound, b"sound bytes");
    let log = Log::default();
    let sink = ProgressLog(log.clone());
    let evidence = Evidence::observed(Some(&sink));
    // The Standard Archive takes the Mod Root's name; the Incompressible one
    // needs another.
    let first_destination = root.join("mod.bsa");
    let competed = AtomicBool::new(false);
    let competitor = first_destination.clone();
    let finalizer = Finalizer {
        settings: ArchiveFinalizationSettings {
            create_dummy_plugins: false,
            compress: false,
            merge_incompressible: false,
            merge_textures: false,
            ..ArchiveFinalizationSettings::default()
        },
        // A competing creator after planning, before the first attempt.
        capacity: capacity(move |_| {
            if !competed.swap(true, Ordering::SeqCst) {
                write_file(&competitor, b"competing creator bytes");
            }
            Some(UNLIMITED)
        }),
        ..Finalizer::default()
    };
    let mut artifacts = registry();
    finalizer
        .run(std::slice::from_ref(&root), &evidence, &mut artifacts)
        .unwrap();

    let result = evidence.finalization();
    assert_eq!(result.attempts.len(), 2);
    assert!(result.safe_to_continue);
    assert!(!result.cancelled);
    assert_eq!(result.attempts[0].archive_path, first_destination);
    assert_ne!(result.attempts[1].archive_path, first_destination);
    assert!(!result.attempts[0].succeeded());
    assert_eq!(result.attempts[0].mutation, MutationState::None);
    assert!(
        result.attempts[1].succeeded(),
        "{}",
        result.attempts[1].detail
    );
    assert_eq!(result.attempts[1].mutation, MutationState::Committed);
    assert_eq!(
        log.progress(),
        [
            "progress 0/2 ok=0 failed=0",
            "progress 1/2 ok=0 failed=1",
            "progress 2/2 ok=1 failed=1",
        ]
    );
    assert!(mesh.exists());
    assert!(!sound.exists());
    assert_eq!(
        std::fs::read(&first_destination).unwrap(),
        b"competing creator bytes"
    );
    assert!(reads_back(&result.attempts[1].archive_path));
    assert!(artifacts.cleanup().is_empty());
}

/// Origin: plannedPluginCollisionRetainsCommittedArchive. A competitor at
/// the planned Dummy Plugin name keeps the committed Archive and its sources,
/// and stops the run.
#[test]
fn a_competing_plugin_at_the_planned_name_keeps_archive_and_sources() {
    let root = scratch("planned-plugin-collision").join("mod");
    let source = root.join("textures/asset.dds");
    write_file(&source, b"source bytes");
    let planned_plugin = root.join("mod.esp");
    let competitor = planned_plugin.clone();
    let occupied = AtomicBool::new(false);
    let finalizer = Finalizer {
        settings: ArchiveFinalizationSettings {
            compress: false,
            ..ArchiveFinalizationSettings::default()
        },
        capacity: capacity(move |_| {
            if !occupied.swap(true, Ordering::SeqCst) {
                write_file(&competitor, b"competing plugin bytes");
            }
            Some(UNLIMITED)
        }),
        ..Finalizer::default()
    };
    let evidence = Evidence::new();
    let mut artifacts = registry();
    finalizer
        .run(std::slice::from_ref(&root), &evidence, &mut artifacts)
        .unwrap();

    let result = evidence.finalization();
    assert_eq!(result.attempts.len(), 1);
    let attempt = &result.attempts[0];
    assert_eq!(
        attempt.failure,
        Some(ArchiveFinalizationFailure::PluginCreationFailed)
    );
    assert_eq!(attempt.mutation, MutationState::Committed);
    assert!(!attempt.safe_to_continue);
    assert!(!result.safe_to_continue);
    assert!(result.mutations.is_empty());
    assert!(reads_back(&attempt.archive_path));
    assert!(source.exists());
    assert_eq!(
        std::fs::read(&planned_plugin).unwrap(),
        b"competing plugin bytes"
    );
    assert!(artifacts.cleanup().is_empty());
}

/// Sets `path`'s modification time a day back and returns it, so a later
/// rewrite would show.
fn age(path: &Path) -> SystemTime {
    let earlier = SystemTime::now() - Duration::from_secs(24 * 60 * 60);
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(earlier)
        .unwrap();
    std::fs::metadata(path).unwrap().modified().unwrap()
}

/// Origin: plannedExactDummyIsReused. An exact Dummy Plugin that appears at
/// the planned name after planning is reused, not replaced.
#[test]
fn an_exact_dummy_at_the_planned_name_is_reused() {
    let root = scratch("planned-dummy-reused").join("mod");
    let source = root.join("textures/asset.dds");
    write_file(&source, b"source bytes");
    let planned_plugin = root.join("mod.esp");
    let written: Arc<Mutex<Option<SystemTime>>> = Arc::default();
    let (recorded, plugin) = (Arc::clone(&written), planned_plugin.clone());
    let finalizer = Finalizer {
        settings: ArchiveFinalizationSettings {
            compress: false,
            ..ArchiveFinalizationSettings::default()
        },
        capacity: capacity(move |_| {
            let mut recorded = recorded.lock().unwrap();
            if recorded.is_none() {
                write_file(&plugin, &fake_dummy_plugin());
                *recorded = Some(age(&plugin));
            }
            Some(UNLIMITED)
        }),
        ..Finalizer::default()
    };
    let evidence = Evidence::new();
    let mut artifacts = registry();
    finalizer
        .run(std::slice::from_ref(&root), &evidence, &mut artifacts)
        .unwrap();

    let original = written.lock().unwrap().expect("the dummy appeared");
    let result = evidence.finalization();
    assert_eq!(result.attempts.len(), 1);
    assert!(
        result.attempts[0].succeeded(),
        "{}",
        result.attempts[0].detail
    );
    assert_eq!(result.attempts[0].mutation, MutationState::Committed);
    assert!(reads_back(&result.attempts[0].archive_path));
    assert!(!source.exists());
    assert_eq!(
        std::fs::metadata(&planned_plugin)
            .unwrap()
            .modified()
            .unwrap(),
        original
    );
    assert_eq!(std::fs::read(&planned_plugin).unwrap(), fake_dummy_plugin());
    assert!(
        result
            .mutations
            .iter()
            .all(|mutation| mutation.kind != ArchiveFinalizationMutationKind::PluginCreation)
    );
    assert!(artifacts.cleanup().is_empty());
}

/// Origin: plannedOutputsRecordSharedDummyPublication. Two outputs share one
/// Dummy Plugin, published once as an effect of its own.
#[test]
fn outputs_sharing_a_dummy_plugin_publish_it_once() {
    let root = scratch("shared-dummy").join("mod");
    write_file(&root.join("meshes/asset.nif"), b"mesh bytes");
    write_file(&root.join("textures/asset.dds"), b"texture bytes");
    let finalizer = Finalizer {
        settings: ArchiveFinalizationSettings {
            compress: false,
            delete_sources: false,
            merge_textures: false,
            ..ArchiveFinalizationSettings::default()
        },
        ..Finalizer::default()
    };
    let evidence = Evidence::new();
    let mut artifacts = registry();
    finalizer
        .run(std::slice::from_ref(&root), &evidence, &mut artifacts)
        .unwrap();

    let result = evidence.finalization();
    assert_eq!(result.attempts.len(), 2);
    for attempt in &result.attempts {
        assert!(attempt.succeeded(), "{}", attempt.detail);
        assert_eq!(attempt.mutation, MutationState::Committed);
    }
    assert_eq!(result.mutations.len(), 1);
    let mutation = &result.mutations[0];
    assert_eq!(mutation.mod_root, root);
    assert_eq!(mutation.path, root.join("mod.esp"));
    assert_eq!(
        mutation.kind,
        ArchiveFinalizationMutationKind::PluginCreation
    );
    assert_eq!(mutation.mutation, MutationState::Committed);
    assert_eq!(std::fs::read(&mutation.path).unwrap(), fake_dummy_plugin());
    assert!(artifacts.cleanup().is_empty());

    let sealed = evidence.seal();
    assert_eq!(sealed.mutation_summaries().len(), 1);
    let summary = &sealed.mutation_summaries()[0];
    assert_eq!(summary.mod_root, root);
    assert_eq!(summary.kind, MutationKind::ArchiveFinalization);
    assert_eq!((summary.committed, summary.partial_or_unknown), (3, 0));
}

/// Origin: plannedLoadingPluginDisappearsBeforeCommit. A Loading Plugin seen
/// while planning that disappears before the attempt is replaced by the
/// fallback Dummy Plugin.
#[test]
fn a_vanished_loading_plugin_gets_its_fallback() {
    let root = scratch("vanished-plugin").join("mod");
    let source = root.join("textures/asset.dds");
    write_file(&source, b"source bytes");
    let earlier_plugin = root.join("mod.esm");
    write_file(&earlier_plugin, b"existing loading plugin");
    let removed = Arc::new(AtomicBool::new(false));
    let (flag, vanishing) = (Arc::clone(&removed), earlier_plugin.clone());
    let finalizer = Finalizer {
        settings: ArchiveFinalizationSettings {
            compress: false,
            ..ArchiveFinalizationSettings::default()
        },
        capacity: capacity(move |_| {
            if !flag.load(Ordering::SeqCst) {
                flag.store(std::fs::remove_file(&vanishing).is_ok(), Ordering::SeqCst);
            }
            Some(UNLIMITED)
        }),
        ..Finalizer::default()
    };
    let evidence = Evidence::new();
    let mut artifacts = registry();
    finalizer
        .run(std::slice::from_ref(&root), &evidence, &mut artifacts)
        .unwrap();

    assert!(removed.load(Ordering::SeqCst));
    let result = evidence.finalization();
    assert_eq!(result.attempts.len(), 1);
    assert!(
        result.attempts[0].succeeded(),
        "{}",
        result.attempts[0].detail
    );
    assert_eq!(result.attempts[0].mutation, MutationState::Committed);
    assert!(!source.exists());
    let creations: Vec<_> = result
        .mutations
        .iter()
        .filter(|mutation| mutation.kind == ArchiveFinalizationMutationKind::PluginCreation)
        .collect();
    assert_eq!(creations.len(), 1);
    assert_eq!(creations[0].mod_root, root);
    assert_eq!(creations[0].path, root.join("mod.esp"));
    assert_eq!(creations[0].mutation, MutationState::Committed);
    assert!(artifacts.cleanup().is_empty());
}

/// The names an SSE-like game recognizes as loading `mod - Textures.bsa`:
/// either stem with any plugin extension.
fn recognized_texture_plugins(root: &Path) -> Vec<PathBuf> {
    [".esl", ".esm", ".esp"]
        .into_iter()
        .flat_map(|extension| {
            [
                root.join(format!("mod - Textures{extension}")),
                root.join(format!("mod{extension}")),
            ]
        })
        .collect()
}

/// Origin: linkedLoadingPluginRemainsUsable (both rows). A linked Loading
/// Plugin cannot justify deleting sources, since retargeting it could unload
/// the Archive, so an ordinary fallback is published at a recognized name.
#[test]
fn a_linked_loading_plugin_gets_an_ordinary_fallback() {
    for linked_name in ["mod.esm", "mod.esp"] {
        let parent = scratch(&format!("linked-plugin-{linked_name}"));
        let root = parent.join("mod");
        let source = root.join("textures/asset.dds");
        write_file(&source, b"source bytes");
        let target = parent.join("real-plugin.bin");
        write_file(&target, b"real loading plugin bytes");
        let link = root.join(linked_name);
        if !symlink(&target, &link) {
            eprintln!("skipped: file symlinks are unavailable on this host");
            return;
        }
        let finalizer = Finalizer {
            settings: ArchiveFinalizationSettings {
                compress: false,
                ..ArchiveFinalizationSettings::default()
            },
            ..Finalizer::default()
        };
        let evidence = Evidence::new();
        let mut artifacts = registry();
        finalizer
            .run(std::slice::from_ref(&root), &evidence, &mut artifacts)
            .unwrap();

        let result = evidence.finalization();
        assert_eq!(result.attempts.len(), 1);
        assert!(
            result.attempts[0].succeeded(),
            "{}",
            result.attempts[0].detail
        );
        assert!(reads_back(&result.attempts[0].archive_path));
        assert!(!source.exists());
        assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
        let recognized = recognized_texture_plugins(&root);
        let fallbacks: Vec<_> = result
            .mutations
            .iter()
            .filter(|mutation| mutation.kind == ArchiveFinalizationMutationKind::PluginCreation)
            .map(|mutation| mutation.path.clone())
            .collect();
        assert_eq!(fallbacks.len(), 1, "{linked_name}");
        let fallback = &fallbacks[0];
        assert_ne!(*fallback, link);
        assert!(recognized.contains(fallback), "{}", fallback.display());
        let metadata = std::fs::symlink_metadata(fallback).unwrap();
        assert!(metadata.is_file() && !metadata.is_symlink());
        assert!(artifacts.cleanup().is_empty());
        std::fs::remove_file(&link).unwrap();
        assert!(fallback.exists());
    }
}

/// Origin: linkedLoadingPluginsWithoutFallbackRetainSources. With every
/// recognized name taken by a link, sources stay and the output fails.
#[test]
fn links_at_every_plugin_name_keep_the_sources() {
    let parent = scratch("linked-plugins-everywhere");
    let root = parent.join("mod");
    let source = root.join("textures/asset.dds");
    write_file(&source, b"source bytes");
    let target = parent.join("real-plugin.bin");
    write_file(&target, b"linked loading plugin");
    let probe = parent.join("symlink-probe");
    if !symlink(&target, &probe) {
        eprintln!("skipped: file symlinks are unavailable on this host");
        return;
    }
    std::fs::remove_file(&probe).unwrap();
    let recognized = recognized_texture_plugins(&root);
    let linked = Arc::new(AtomicBool::new(false));
    let (flag, names, link_target) = (Arc::clone(&linked), recognized.clone(), target.clone());
    let finalizer = Finalizer {
        settings: ArchiveFinalizationSettings {
            compress: false,
            ..ArchiveFinalizationSettings::default()
        },
        // After planning, links occupy every recognized Loading Plugin name.
        capacity: capacity(move |_| {
            if !flag.swap(true, Ordering::SeqCst) {
                for name in &names {
                    assert!(symlink(&link_target, name));
                }
            }
            Some(UNLIMITED)
        }),
        ..Finalizer::default()
    };
    let evidence = Evidence::new();
    let mut artifacts = registry();
    finalizer
        .run(std::slice::from_ref(&root), &evidence, &mut artifacts)
        .unwrap();

    assert!(linked.load(Ordering::SeqCst));
    let result = evidence.finalization();
    assert_eq!(result.attempts.len(), 1);
    assert_eq!(
        result.attempts[0].failure,
        Some(ArchiveFinalizationFailure::PluginCreationFailed)
    );
    assert!(source.exists());
    assert!(reads_back(&result.attempts[0].archive_path));
    assert!(artifacts.cleanup().is_empty());
    for name in &recognized {
        std::fs::remove_file(name).unwrap();
    }
}

/// Origin: loadingPluginRemainsPinnedThroughSourceCleanup (both rows). The
/// Loading Plugin, found or published, cannot be deleted until every packed
/// source is gone.
#[test]
fn the_loading_plugin_stays_pinned_through_source_cleanup() {
    for published in [false, true] {
        let root = scratch(&format!("pinned-plugin-{published}")).join("mod");
        // A long cleanup makes the interval after plugin selection observable.
        let mut sources: Vec<PathBuf> = (0..1024)
            .map(|index| root.join(format!("textures/asset-{index}.dds")))
            .collect();
        for source in &sources {
            write_file(source, b"source bytes");
        }
        sources.sort();
        let plugin = root.join(if published { "mod.esp" } else { "mod.esm" });
        if !published {
            write_file(&plugin, b"existing loading plugin");
        }
        let finalizer = Finalizer {
            settings: ArchiveFinalizationSettings {
                compress: false,
                ..ArchiveFinalizationSettings::default()
            },
            ..Finalizer::default()
        };
        let evidence = Evidence::new();
        let mut artifacts = registry();
        let stop = AtomicBool::new(false);
        let (saw_cleanup_gap, removed_plugin) = std::thread::scope(|scope| {
            let remover = scope.spawn(|| {
                while !stop.load(Ordering::SeqCst) {
                    // Cleanup is in progress once exactly one end of the
                    // ordered source set is gone.
                    let (first, last) = (
                        sources[0].try_exists(),
                        sources[sources.len() - 1].try_exists(),
                    );
                    if let (Ok(first), Ok(last)) = (first, last)
                        && first != last
                    {
                        return (true, std::fs::remove_file(&plugin).is_ok());
                    }
                    std::thread::yield_now();
                }
                (false, false)
            });
            finalizer
                .run(std::slice::from_ref(&root), &evidence, &mut artifacts)
                .unwrap();
            stop.store(true, Ordering::SeqCst);
            remover.join().unwrap()
        });

        let result = evidence.finalization();
        assert_eq!(result.attempts.len(), 1);
        assert!(
            result.attempts[0].succeeded(),
            "{}",
            result.attempts[0].detail
        );
        assert!(
            saw_cleanup_gap,
            "source cleanup was never observed in progress"
        );
        assert!(!removed_plugin);
        assert!(reads_back(&result.attempts[0].archive_path));
        assert!(sources.iter().all(|source| !source.exists()));
        assert!(plugin.is_file());
        std::fs::remove_file(&plugin).unwrap();
        assert!(artifacts.cleanup().is_empty());
    }
}

/// Origin: committedArchiveRetainsLockedSource (both rows). After a commit,
/// a source that cannot be deleted lets later outputs go on only while it is
/// still readable recovery material.
#[test]
fn a_locked_source_after_commit_continues_only_while_readable() {
    for deny_reads in [false, true] {
        let parent = scratch(&format!("locked-source-{deny_reads}"));
        let root = parent.join("mod-a");
        let later_root = parent.join("mod-b");
        let source = root.join("textures/asset.dds");
        write_file(&source, b"retained source bytes");
        let later_source = later_root.join("textures/later.dds");
        write_file(&later_source, b"later source bytes");
        let empty_directory = root.join("empty/nested");
        std::fs::create_dir_all(&empty_directory).unwrap();
        // Permit packing and verification reads, but deny deletion.
        let locked = Arc::new(hold(&source, SHARE_READ));
        let mut finalizer = Finalizer {
            settings: ArchiveFinalizationSettings {
                create_dummy_plugins: false,
                compress: false,
                ..ArchiveFinalizationSettings::default()
            },
            ..Finalizer::default()
        };
        if deny_reads {
            // A whole-file byte-range lock, taken once the writer has read
            // the source, makes every other handle's reads fail: the file
            // exists but is no longer usable recovery material.
            let lock = Arc::clone(&locked);
            let taken = AtomicBool::new(false);
            finalizer.packer.after_write = Some(Box::new(move |_| {
                if !taken.swap(true, Ordering::SeqCst) {
                    lock.try_lock().unwrap();
                }
            }));
        }
        let evidence = Evidence::new();
        let mut artifacts = registry();
        finalizer
            .run(
                &[root.clone(), later_root.clone()],
                &evidence,
                &mut artifacts,
            )
            .unwrap();
        drop(finalizer);
        drop(locked);

        let result = evidence.finalization();
        assert_eq!(result.attempts.len(), if deny_reads { 1 } else { 2 });
        let first = &result.attempts[0];
        assert!(!first.succeeded());
        assert_eq!(
            first.failure,
            Some(ArchiveFinalizationFailure::SourceCleanupFailed)
        );
        assert_eq!(first.mutation, MutationState::Committed);
        assert_eq!(result.safe_to_continue, !deny_reads);
        assert!(reads_back(&first.archive_path));
        assert_eq!(later_source.exists(), deny_reads);
        assert_eq!(archives_in(&later_root).len(), usize::from(!deny_reads));
        assert_eq!(empty_directory.exists(), deny_reads);
        if !deny_reads {
            let last = result.attempts.last().unwrap();
            assert!(last.succeeded(), "{}", last.detail);
            assert!(reads_back(&last.archive_path));
        }
        assert_eq!(std::fs::read(&source).unwrap(), b"retained source bytes");
        assert!(artifacts.cleanup().is_empty());
    }
}

/// Origin: packingRequiresArchiveCreationRequest. Without an Archive creation
/// request nothing is packed and no Loading Plugin is touched, but empty
/// directories are still pruned.
#[test]
fn without_archive_creation_only_pruning_runs() {
    let root = scratch("no-archive-creation").join("mod");
    let source = root.join("textures/asset.dds");
    write_file(&source, b"source bytes");
    write_file(&root.join("existing.bsa"), b"retained archive");
    std::fs::create_dir_all(root.join("empty/nested")).unwrap();
    let probes = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&probes);
    let finalizer = Finalizer {
        no_archive_creation: true,
        capacity: capacity(move |_| {
            counted.fetch_add(1, Ordering::SeqCst);
            Some(UNLIMITED)
        }),
        ..Finalizer::default()
    };
    let evidence = Evidence::new();
    let mut artifacts = registry();
    finalizer
        .run(std::slice::from_ref(&root), &evidence, &mut artifacts)
        .unwrap();

    let result = evidence.finalization();
    assert!(result.attempts.is_empty());
    assert_eq!(result.failure, None);
    assert!(result.safe_to_continue);
    assert_eq!(evidence.progress().unwrap().total(), 0);
    // Neither packing nor plugin maintenance for the existing Archive ran.
    assert_eq!(probes.load(Ordering::SeqCst), 0);
    assert!(source.exists());
    assert_eq!(archives_in(&root), [root.join("existing.bsa")]);
    assert!(!root.join("existing.esp").exists());
    assert!(!root.join(".cao-staging").exists());
    assert!(!root.join("empty").exists());
    assert_eq!(result.mutations.len(), 1);
    assert_eq!(
        result.mutations[0].kind,
        ArchiveFinalizationMutationKind::EmptyDirectoryPruning
    );
    assert_eq!(result.mutations[0].mutation, MutationState::Committed);
    assert_eq!(result.mutations[0].count, 2);
    assert!(artifacts.cleanup().is_empty());
}

/// **Deviation 20 (pruning).** An empty directory is pruned even when its
/// path contains "separator": C++ skipped any such directory, so a mods
/// folder below a mod-manager separator was never pruned at all.
#[test]
fn deviation_20_pruning_has_no_separator_rule() {
    let root = scratch("deviation-20").join("Separator mods").join("mod");
    std::fs::create_dir_all(root.join("separator/empty")).unwrap();
    std::fs::create_dir_all(root.join("other")).unwrap();
    let finalizer = Finalizer {
        no_archive_creation: true,
        ..Finalizer::default()
    };
    let evidence = Evidence::new();
    let mut artifacts = registry();
    finalizer
        .run(std::slice::from_ref(&root), &evidence, &mut artifacts)
        .unwrap();

    let result = evidence.finalization();
    assert!(std::fs::read_dir(&root).unwrap().next().is_none());
    assert_eq!(result.mutations.len(), 1);
    assert_eq!(result.mutations[0].count, 3);
    assert!(artifacts.cleanup().is_empty());
}

/// Split points: sources over the size limit split into several Archives of
/// one kind, in sorted order. The first takes the plugin-derived name, the
/// rest the next free counter names, and each gets its own Loading Plugin.
/// The corpus cannot reach a real game's 2 GiB limit, so this pins the
/// finalization side; `cao-archive` pins the exact split boundaries.
#[test]
fn split_outputs_take_counter_names_and_their_own_plugins() {
    let root = scratch("split-points").join("mod");
    for name in ["c", "a", "b"] {
        write_file(&root.join(format!("meshes/{name}.nif")), b"twelve bytes");
    }
    let mut finalizer = Finalizer {
        settings: ArchiveFinalizationSettings {
            compress: false,
            ..ArchiveFinalizationSettings::default()
        },
        ..Finalizer::default()
    };
    // Room for one source per Archive.
    finalizer.packer.max_size = 20;
    let evidence = Evidence::new();
    let mut artifacts = registry();
    finalizer
        .run(std::slice::from_ref(&root), &evidence, &mut artifacts)
        .unwrap();

    let result = evidence.finalization();
    let archives: Vec<_> = result
        .attempts
        .iter()
        .map(|attempt| {
            assert!(attempt.succeeded(), "{}", attempt.detail);
            attempt.archive_path.clone()
        })
        .collect();
    assert_eq!(
        archives,
        [
            root.join("mod.bsa"),
            root.join("mod0.bsa"),
            root.join("mod1.bsa")
        ]
    );
    let names: Vec<_> = finalizer
        .packer
        .writes()
        .into_iter()
        .map(|write| write.names)
        .collect();
    assert_eq!(
        names,
        [["meshes/a.nif"], ["meshes/b.nif"], ["meshes/c.nif"]]
    );
    for plugin in ["mod.esp", "mod0.esp", "mod1.esp"] {
        assert_eq!(
            std::fs::read(root.join(plugin)).unwrap(),
            fake_dummy_plugin()
        );
    }
    assert!(!root.join("meshes").exists());
    assert!(artifacts.cleanup().is_empty());
}

/// **Deviation 21 (core side).** A game that keeps Textures separate never
/// merges them into the Main Archive, even when asked; a game that allows it
/// merges them.
#[test]
fn deviation_21_separate_texture_games_never_merge_textures() {
    for separate in [true, false] {
        let root = scratch(&format!("deviation-21-{separate}")).join("mod");
        write_file(&root.join("meshes/asset.nif"), b"mesh bytes");
        write_file(&root.join("textures/asset.dds"), b"texture bytes");
        let finalizer = Finalizer {
            packer: if separate {
                FakeArchivePacker::fo4_like()
            } else {
                FakeArchivePacker::default()
            },
            // Merging even an empty Incompressible partition would make the
            // Standard Archive Incompressible, so leave it out of the picture.
            settings: ArchiveFinalizationSettings {
                create_dummy_plugins: false,
                merge_incompressible: false,
                merge_textures: true,
                ..ArchiveFinalizationSettings::default()
            },
            ..Finalizer::default()
        };
        let evidence = Evidence::new();
        let mut artifacts = registry();
        finalizer
            .run(std::slice::from_ref(&root), &evidence, &mut artifacts)
            .unwrap();

        let result = evidence.finalization();
        assert!(result.attempts.iter().all(|attempt| attempt.succeeded()));
        let kinds: Vec<_> = finalizer
            .packer
            .writes()
            .into_iter()
            .map(|write| (write.kind, write.names))
            .collect();
        if separate {
            assert_eq!(
                kinds,
                [
                    (
                        PackedArchiveKind::Standard,
                        vec!["meshes/asset.nif".to_owned()]
                    ),
                    (
                        PackedArchiveKind::Textures,
                        vec!["textures/asset.dds".to_owned()]
                    ),
                ]
            );
            assert_eq!(
                archives_in(&root),
                [root.join("mod - Main.ba2"), root.join("mod - Textures.ba2")]
            );
        } else {
            assert_eq!(
                kinds,
                [(
                    PackedArchiveKind::Standard,
                    vec![
                        "meshes/asset.nif".to_owned(),
                        "textures/asset.dds".to_owned()
                    ]
                )]
            );
        }
        assert!(artifacts.cleanup().is_empty());
    }
}

/// Origin: unexpectedExceptionIsRecordedOnce (both rows). A failure of the
/// phase's own work, in planning or after the output total, is recorded once
/// as an unsafe phase-level failure.
#[test]
fn an_unexpected_failure_is_recorded_once() {
    for after_plan in [false, true] {
        let parent = scratch(&format!("unexpected-{after_plan}"));
        let root = parent.join("mod");
        let source = root.join("textures/asset.dds");
        write_file(&source, b"source bytes");
        let finalizer = Finalizer {
            volumes: volumes(|_| panic!("volume query failed")),
            ..Finalizer::default()
        };
        // A missing Mod Root fails planning; the panicking volume query
        // fails after the plan.
        let selected = if after_plan {
            root.clone()
        } else {
            parent.join("missing")
        };
        let evidence = Evidence::new();
        let mut artifacts = registry();
        finalizer
            .run(&[selected], &evidence, &mut artifacts)
            .unwrap();

        let result = evidence.finalization();
        assert_eq!(
            result.failure,
            Some(ArchiveFinalizationFailure::UnexpectedException)
        );
        assert!(!result.safe_to_continue);
        assert!(!result.detail.is_empty());
        if after_plan {
            assert_eq!(result.detail, "volume query failed");
            assert_eq!(evidence.progress().unwrap().total(), 1);
        } else {
            assert_eq!(evidence.progress(), None);
        }
        assert!(result.attempts.is_empty());
        assert!(source.exists());
        assert!(archives_in(&root).is_empty());
        assert!(artifacts.cleanup().is_empty());
    }
}

/// Origin: cleanupExceptionKeepsRecordedAttempts. A failure in Loading
/// Plugin maintenance keeps the committed attempt before it.
#[test]
fn a_plugin_maintenance_failure_keeps_recorded_attempts() {
    let parent = scratch("cleanup-failure");
    let packed = parent.join("mod-a");
    let existing = parent.join("mod-b");
    write_file(&packed.join("textures/asset.dds"), b"source bytes");
    write_file(&existing.join("existing.bsa"), b"retained archive");
    let moved = Arc::new(AtomicBool::new(false));
    let (flag, packed_root, existing_root, destination) = (
        Arc::clone(&moved),
        packed.clone(),
        existing.clone(),
        parent.join("moved"),
    );
    let finalizer = Finalizer {
        settings: ArchiveFinalizationSettings {
            compress: false,
            ..ArchiveFinalizationSettings::default()
        },
        // mod-b's plugin allowance is rechecked after mod-a's output; moving
        // mod-b away then makes its Loading Plugin maintenance fail.
        capacity: capacity(move |root| {
            if root == existing_root
                && !archives_in(&packed_root).is_empty()
                && !flag.load(Ordering::SeqCst)
            {
                flag.store(
                    std::fs::rename(&existing_root, &destination).is_ok(),
                    Ordering::SeqCst,
                );
            }
            Some(UNLIMITED)
        }),
        ..Finalizer::default()
    };
    let evidence = Evidence::new();
    let mut artifacts = registry();
    finalizer
        .run(&[packed, existing], &evidence, &mut artifacts)
        .unwrap();

    assert!(moved.load(Ordering::SeqCst));
    let result = evidence.finalization();
    assert_eq!(
        result.failure,
        Some(ArchiveFinalizationFailure::UnexpectedException)
    );
    assert!(!result.safe_to_continue);
    assert_eq!(result.attempts.len(), 1);
    assert!(
        result.attempts[0].succeeded(),
        "{}",
        result.attempts[0].detail
    );
    assert_eq!(result.attempts[0].mutation, MutationState::Committed);
    assert_eq!(evidence.progress().unwrap().completed(), 1);
    assert!(reads_back(&result.attempts[0].archive_path));
    assert!(artifacts.cleanup().is_empty());
}

/// Origin: evidenceFailuresPropagateUnchanged (all three rows). A Run
/// Evidence invariant violation, before the total, at a capacity rejection
/// or after a commit, reaches the caller unchanged and is never converted
/// into a finalization result.
#[test]
fn evidence_failures_reach_the_caller_unchanged() {
    for stage in 0..3 {
        let root = scratch(&format!("evidence-failure-{stage}")).join("mod");
        let source = root.join("textures/asset.dds");
        write_file(&source, b"source bytes");
        let evidence = Evidence::new();
        if stage == 0 {
            evidence.leave_phase();
        }
        let probes = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&probes);
        let finalizer = Finalizer {
            settings: ArchiveFinalizationSettings {
                create_dummy_plugins: false,
                compress: false,
                ..ArchiveFinalizationSettings::default()
            },
            // The second probe opens the output attempt, after the preflight.
            capacity: capacity(move |_| {
                let probe = counted.fetch_add(1, Ordering::SeqCst) + 1;
                Some(if probe == 2 && stage == 1 {
                    0
                } else {
                    UNLIMITED
                })
            }),
            ..Finalizer::default()
        };
        // Leave the phase between the preflight and the attempt, so the
        // attempt's record is the next evidence call. Cancellation is polled
        // exactly there and never requested.
        let left = Cell::new(false);
        let cancelled = || {
            if stage != 0 && probes.load(Ordering::SeqCst) == 1 && !left.replace(true) {
                evidence.leave_phase();
            }
            false
        };
        let mut artifacts = registry();
        let outcome = finalizer.run_polling(
            std::slice::from_ref(&root),
            &evidence,
            &mut artifacts,
            &cancelled,
        );

        assert!(
            matches!(outcome, Err(Error::EvidenceInvariant(_))),
            "{stage}: {outcome:?}"
        );
        assert!(
            evidence
                .recorded()
                .is_none_or(|result| result.failure.is_none())
        );
        assert_eq!(source.exists(), stage != 2, "{stage}");
        assert_eq!(archives_in(&root).len(), usize::from(stage == 2), "{stage}");
        assert!(artifacts.cleanup().is_empty());
    }
}
