//! The Archive Finalization phase through whole Optimization Runs over a fake
//! packer and reader, fake probes and real Mod Root directories, as the C++
//! suite drove `ArchiveFinalization::run`.
//!
//! Each scenario names its C++ origin in `tests/ArchiveFinalizationTests.cpp`.
//! The fake packer stands in for one game's rules and writer, so these pin
//! the phase's own logic: capacity, naming, Loading Plugins, publication,
//! source cleanup, cancellation and evidence.
//!
//! Every scenario is an Apply run over Mod Roots holding only Loose Assets
//! and existing Archives, and requests no Archive extraction, so the
//! preflight and extraction have nothing to do and never sample free space:
//! every volume probe call comes from Archive Finalization (#597). The
//! request asks for Archive creation, or only Texture work, and the Packing
//! Exclusions come from the run's Selected Profile, so Preparing makes the
//! phase's preparation as production does. A scenario acts mid-phase from a
//! fake the phase calls, such as the volume probe or the packer, and asserts
//! on the run's Run Evidence and the filesystem.
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
//! - `evidenceFailuresPropagateUnchanged`: every row ran the phase over Run
//!   Evidence no run leaves it in (the phase already left, or its result or
//!   output total already recorded). The evidence invariants it relied on are
//!   pinned in `cao-core`'s own `run/evidence/tests.rs` (#597).
//! - `finalizationRejectsChangedDummyPluginParent`: unreachable, because
//!   Apply Preparing pins every Mod Root for the whole run, so a run's Mod
//!   Root cannot be swapped for a junction (the rename fails with a sharing
//!   violation). The guard in Dummy Plugin removal stays as defense in depth
//!   (#595, #598).

mod common;

use std::fs::{File, OpenOptions};
use std::os::windows::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use cao_core::execution::{AssetExecutionResult, MutationState};
use cao_core::routing::RequestedWork;
use cao_core::run::{
    ArchiveFinalizationFailure, ArchiveFinalizationMutationKind, ArchiveFinalizationResult,
    ArchiveFinalizationSettings, ArchiveNameKind, ArchiveReader, ArchiveWorkSettings,
    CancellationToken, MutationKind, MutationSummary, OptimizationRunResult, PackedArchiveKind,
    RunDiagnostic, RunFailure, RunObservationSink, RunOutcome, RunPhase, RunPhaseRecord,
    RunProgress,
};
use cao_core::test_support::{
    ArchiveRunRequest, FakeArchivePacker, FakeArchiveReader, FakeVolumeProbe, Observed,
    RecordingSink, ScriptedToolset, fake_dummy_plugin,
};
use common::{archives_in, canonical, junction, scratch_dir};

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

    // Archive Finalization reports no Run Failures or diagnostics; the run's
    // other phases' stay in its Run Evidence.
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

/// A volume probe answering `probe` for every Mod Root's free space; its
/// volumes are unknown.
fn capacity(probe: impl Fn(&Path) -> Option<u64> + Send + Sync + 'static) -> FakeVolumeProbe {
    FakeVolumeProbe::default().with_capacity(probe)
}

/// One Archive Finalization run's configuration; scenarios override only what
/// they depend on. The settings default to the options model's.
#[derive(Default)]
struct Finalizer {
    packer: FakeArchivePacker,
    reader: FakeArchiveReader,
    /// Answers free space and volume identities; scenarios set the knobs
    /// they depend on through [`capacity`] and
    /// [`FakeVolumeProbe::with_volumes`].
    probe: FakeVolumeProbe,
    settings: ArchiveFinalizationSettings,
    /// When set, the run requests only Texture work.
    no_archive_creation: bool,
    /// The Selected Profile's Packing Exclusions, as the profile writes them.
    files_to_not_pack: Vec<String>,
    /// Asset attempts, for a scenario that requests Texture work and acts
    /// from them; by default they succeed with no mutation.
    assets: ScriptedToolset,
    /// The run's cancellation token; a fake the phase calls may cancel a
    /// clone of it mid-phase.
    stop: CancellationToken,
}

/// One finished run and the fakes it ran over.
struct Finalized {
    result: OptimizationRunResult,
    /// The run's fakes, whose records outlive it.
    toolset: Arc<ScriptedToolset>,
}

impl Finalized {
    /// The run's Archive Finalization result.
    ///
    /// # Panics
    ///
    /// When the run never reached Archive Finalization.
    fn finalization(&self) -> &ArchiveFinalizationResult {
        self.result
            .archive_finalization()
            .unwrap_or_else(|| panic!("no Archive Finalization: {:?}", self.result.failures()))
    }

    /// The Archive Finalization phase's progress account, or `None` when it
    /// recorded no output total.
    fn progress(&self) -> Option<RunProgress> {
        self.result
            .phase(RunPhase::ArchiveFinalization)
            .and_then(RunPhaseRecord::progress)
    }

    /// Whether the run's Safety Cleanup, and every attempt's, failed nothing.
    fn cleaned_up(&self) -> bool {
        self.result.cleanup_failures().is_empty()
    }

    /// Drops the run's fakes, and with them whatever their hooks hold, such
    /// as a scenario's file handle; the run's result stays readable.
    fn release_fakes(&mut self) {
        self.toolset = Arc::default();
    }

    /// The run's Mutation Summaries of Archive Finalization's own effects.
    fn finalization_summaries(&self) -> Vec<&MutationSummary> {
        self.result
            .mutation_summaries()
            .iter()
            .filter(|summary| summary.kind == MutationKind::ArchiveFinalization)
            .collect()
    }
}

impl Finalizer {
    /// Runs one Apply run over `roots`, which must be one Mod Root or every
    /// child Mod Root of one mods directory.
    fn run(self, roots: &[PathBuf]) -> Finalized {
        self.run_observed(roots, None)
    }

    /// [`Self::run`], publishing the run's live facts to `observations`.
    fn run_observed(
        self,
        roots: &[PathBuf],
        observations: Option<&dyn RunObservationSink>,
    ) -> Finalized {
        let run = self.request(roots);
        let configuration = run.configuration(ScriptedToolset {
            reader: Arc::new(self.reader),
            volume_probe: Arc::new(self.probe),
            packer: Arc::new(self.packer),
            ..self.assets
        });
        let result = run.execute(&configuration, observations, &self.stop);
        // Preparing resolved exactly the scenario's roots, in its order, so
        // every root-indexed assertion means what it says.
        assert_eq!(result.mod_roots(), roots, "{:?}", result.failures());
        Finalized {
            result,
            toolset: configuration.toolset,
        }
    }

    /// The run over `roots` with this configuration's work, Packing
    /// Exclusions and settings.
    fn request(&self, roots: &[PathBuf]) -> ArchiveRunRequest {
        let run = match roots {
            [root] => ArchiveRunRequest::new(root),
            _ => {
                let parent = roots[0].parent().unwrap();
                assert!(roots.iter().all(|root| root.parent() == Some(parent)));
                ArchiveRunRequest::over_mods(parent)
            }
        };
        let mut work = vec![RequestedWork::NativeTextureOptimization];
        if !self.no_archive_creation {
            work.push(RequestedWork::ArchiveCreation);
        }
        run.with_work(work)
            .with_packing_exclusions(self.files_to_not_pack.clone())
            .with_archive_work(ArchiveWorkSettings {
                finalization: self.settings,
                ..ArchiveWorkSettings::default()
            })
    }

    /// The staging estimate the phase reports when `root` has no capacity at
    /// all: one planned output's estimate plus its plugin reserve.
    ///
    /// It runs a separate run over `root` with this configuration's settings,
    /// exclusions and packer rules, which a shortage leaves unmutated.
    fn reported_estimate(&self, root: &Path) -> u64 {
        // A lone Mod Root carries the whole demand, so its volume is never
        // looked up and the finalizer's own volume answers are not needed.
        let estimating = Finalizer {
            packer: FakeArchivePacker {
                rules: self.packer.rules.clone(),
                max_size: self.packer.max_size,
                ..FakeArchivePacker::default()
            },
            probe: capacity(|_| Some(0)),
            settings: self.settings,
            files_to_not_pack: self.files_to_not_pack.clone(),
            ..Finalizer::default()
        };
        let run = estimating.run(&[root.to_path_buf()]);
        assert!(run.cleaned_up());
        let result = run.finalization();
        let detail = result
            .attempts
            .first()
            .map_or(result.detail.as_str(), |attempt| {
                attempt
                    .outcome
                    .failure()
                    .map_or("", |failure| failure.detail())
            });
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
        let mut finalizer = Finalizer::default();
        // Identical roots plan identical outputs, so one estimate stands for both.
        let estimate = finalizer.reported_estimate(&roots[0]);
        let grown = AtomicBool::new(false);
        let (first, second) = (roots[0].clone(), roots[1].clone());
        let probe = capacity(move |root| {
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
        // Both roots share one volume, so the phase sums both outputs.
        finalizer.probe = probe.with_volumes(|_| Some("shared-volume".to_owned()));
        let run = finalizer.run(&roots);

        let result = run.finalization();
        assert!(result.safe_to_continue, "{name}");
        assert!(!result.cancelled, "{name}");
        let progress = run.progress().unwrap();
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
                rejected.outcome.failure().map(|failure| failure.category()),
                Some(ArchiveFinalizationFailure::InsufficientCapacity),
                "{name}"
            );
            assert_eq!(rejected.outcome.mutation(), MutationState::None);
            // Every rejection is at the later root: its preflight or attempt.
            assert_eq!(rejected.mod_root, roots[1]);
            assert!(
                rejected
                    .outcome
                    .failure()
                    .is_some_and(|failure| !failure.detail().is_empty())
            );
        }
        assert!(run.cleaned_up());
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
    let first = roots[0].clone();
    finalizer.probe = capacity(move |_| Some(estimate)).with_volumes(move |root| {
        Some(
            if root == first {
                "first-volume"
            } else {
                "second-volume"
            }
            .to_owned(),
        )
    });
    let run = finalizer.run(&roots);

    let result = run.finalization();
    assert_eq!(result.attempts.len(), 2);
    assert!(result.safe_to_continue);
    assert_eq!(result.failure, None);
    for attempt in &result.attempts {
        assert!(attempt.succeeded(), "{:?}", attempt.outcome);
        assert!(attempt.archive_path.exists());
    }
    assert!(run.cleaned_up());
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
    finalizer.probe = capacity(move |root| Some(if root == idle { 0 } else { estimate }))
        .with_volumes(move |root| {
            (root != idle_root).then(|| {
                if root == first {
                    "first-volume"
                } else {
                    "second-volume"
                }
                .to_owned()
            })
        });
    let run = finalizer.run(&roots);

    let result = run.finalization();
    assert_eq!(result.attempts.len(), 2);
    assert_eq!(result.failure, None);
    assert!(result.attempts.iter().all(|attempt| attempt.succeeded()));
    assert!(run.cleaned_up());
}

/// Rust-only, for #592: a volume probe that panics is an unknown volume, not
/// an Unexpected Exception, so the phase proceeds. Its root is checked against
/// the whole phase's estimate: with room for one output each root would fit
/// on its own volume, as in `capacity_is_grouped_by_volume`, but the root on
/// the unknown volume is rejected before mutation.
#[test]
fn a_panicking_volume_probe_is_an_unknown_volume() {
    for shortage in [false, true] {
        let parent = scratch(&format!("volume-panic-{shortage}"));
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
        let available = if shortage { estimate } else { UNLIMITED };
        let first = roots[0].clone();
        finalizer.probe = capacity(move |_| Some(available)).with_volumes(move |root| {
            assert!(*root != first, "the volume query failed");
            Some("second-volume".to_owned())
        });
        let run = finalizer.run(&roots);

        let result = run.finalization();
        assert_eq!(result.failure, None, "{shortage}: {}", result.detail);
        assert!(result.safe_to_continue, "{shortage}");
        if shortage {
            assert_eq!(result.attempts.len(), 1, "{shortage}");
            let rejected = &result.attempts[0];
            assert_eq!(
                rejected.outcome.failure().map(|failure| failure.category()),
                Some(ArchiveFinalizationFailure::InsufficientCapacity)
            );
            assert_eq!(rejected.mod_root, roots[0]);
            assert_eq!(rejected.outcome.mutation(), MutationState::None);
            assert!(roots.iter().all(|root| archives_in(root).is_empty()));
        } else {
            assert_eq!(result.attempts.len(), 2);
            for attempt in &result.attempts {
                assert!(attempt.succeeded(), "{:?}", attempt.outcome);
                assert!(attempt.archive_path.exists());
            }
        }
        assert!(run.cleaned_up());
    }
}

/// Rust-only, for #592: a free-space probe that panics is unknown capacity,
/// which never fails the phase's check or an attempt's recheck.
#[test]
fn a_panicking_capacity_probe_is_unknown_capacity() {
    let parent = scratch("capacity-panic");
    let root = parent.join("mod");
    write_file(&root.join("textures/asset.dds"), b"source bytes");
    let finalizer = Finalizer {
        probe: capacity(|_| panic!("the free-space query failed")),
        ..Finalizer::default()
    };
    let run = finalizer.run(std::slice::from_ref(&root));

    let result = run.finalization();
    assert_eq!(result.failure, None, "{}", result.detail);
    assert!(result.safe_to_continue);
    assert_eq!(result.attempts.len(), 1);
    assert!(
        result.attempts[0].succeeded(),
        "{:?}",
        result.attempts[0].outcome
    );
    assert_eq!(archives_in(&root).len(), 1);
    assert!(root_plugin(&root).exists());
    assert!(run.cleaned_up());
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
        probe: capacity(move |_| {
            Some(std::fs::metadata(&probed_plugin).map_or(UNLIMITED, |metadata| metadata.len()))
        })
        .with_volumes(move |_| {
            counted.fetch_add(1, Ordering::SeqCst);
            Some("shared-volume".to_owned())
        }),
        ..Finalizer::default()
    };
    let run = finalizer.run(&roots);

    let result = run.finalization();
    assert_eq!(result.failure, None, "{}", result.detail);
    assert!(result.attempts.is_empty());
    assert_eq!(run.progress().unwrap().total(), 0);
    assert_eq!(queries.load(Ordering::SeqCst), roots.len());
    for root in &roots {
        assert!(root.join("existing.esp").exists());
    }
    assert!(run.cleaned_up());
}

/// Origin: finalizationCapacityWithoutOutputs. A plugin-only shortage is a
/// phase-level failure with no invented output progress, and no pruning.
#[test]
fn a_plugin_only_shortage_fails_the_phase_without_progress() {
    let root = scratch("capacity-without-outputs").join("mod");
    write_file(&root.join("existing.bsa"), b"retained archive");
    std::fs::create_dir(root.join("empty")).unwrap();
    let finalizer = Finalizer {
        probe: capacity(|_| Some(0)),
        ..Finalizer::default()
    };
    let run = finalizer.run(std::slice::from_ref(&root));

    let result = run.finalization();
    assert_eq!(
        result.failure,
        Some(ArchiveFinalizationFailure::InsufficientCapacity)
    );
    assert!(result.attempts.is_empty());
    assert!(result.safe_to_continue);
    assert!(!result.detail.is_empty());
    let progress = run.progress().unwrap();
    assert_eq!((progress.completed(), progress.total()), (0, 0));
    assert!(root.join("empty").exists());
    assert!(!root.join("existing.esp").exists());
    assert!(!root.join(".cao-staging").exists());
    assert!(run.cleaned_up());
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
        finalizer.probe = capacity(move |_| {
            // Planning saw the Loading Plugin; it disappears before any mutation.
            if queries.fetch_add(1, Ordering::SeqCst) == 0 {
                let _ = std::fs::remove_file(&vanishing);
                if !shortage_at_preflight {
                    return Some(UNLIMITED);
                }
            }
            Some(archive_only)
        });
        let run = finalizer.run(std::slice::from_ref(&root));

        let result = run.finalization();
        assert!(!earlier_plugin.exists());
        assert_eq!(result.attempts.len(), 1);
        assert_eq!(
            result.attempts[0]
                .outcome
                .failure()
                .map(|failure| failure.category()),
            Some(ArchiveFinalizationFailure::InsufficientCapacity)
        );
        assert_eq!(result.attempts[0].outcome.mutation(), MutationState::None);
        assert!(result.mutations.is_empty());
        assert!(archives_in(&root).is_empty());
        assert!(!root.join("mod.esp").exists());
        assert!(source.exists());
        assert!(run.cleaned_up());
    }
}

/// Origin: finalizationPlanningObservesCancellation. Cancelled planning
/// records a cancelled result with no output total.
///
/// A run cancelled before Archive Finalization never enters it, so the
/// cancellation comes from the packer as planning lists the root's names.
#[test]
fn cancelled_planning_records_no_output_total() {
    let root = scratch("planning-cancelled").join("mod");
    let source = root.join("textures/asset.dds");
    write_file(&source, b"source bytes");
    let mut finalizer = Finalizer::default();
    let stop = finalizer.stop.clone();
    finalizer.packer.before_list = Some(Box::new(move |_, _| stop.cancel()));
    let run = finalizer.run(std::slice::from_ref(&root));

    let result = run.finalization();
    assert!(result.cancelled);
    assert_eq!(result.failure, None);
    assert!(result.attempts.is_empty());
    assert_eq!(run.progress(), None);
    assert!(source.exists());
    assert!(!root.join(".cao-staging").exists());
    assert!(run.cleaned_up());
    assert!(run.result.cancellation_observed());
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
        finalizer.probe = capacity(move |_| {
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
        let run = finalizer.run_observed(&roots, Some(&sink));

        let label = format!("{cancel_after:?}");
        assert!(frozen_before_mutation.load(Ordering::SeqCst), "{label}");
        let attempted = cancel_after.unwrap_or(2);
        if attempted == 2 {
            assert!(
                first_recorded_before_second.load(Ordering::SeqCst),
                "{label}"
            );
        }
        let result = run.finalization();
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
                assert!(attempt.succeeded(), "{:?}", attempt.outcome);
                assert_eq!(attempt.mod_root, *root);
                assert_eq!(attempt.outcome.mutation(), MutationState::Committed);
                assert!(reads_back(&attempt.archive_path));
            }
        }
        assert!(run.cleaned_up());
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
    finalizer.probe = capacity(move |_| {
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
    let run = finalizer.run(&roots);

    assert!(no_plugin_before_attempts.load(Ordering::SeqCst));
    let result = run.finalization();
    assert_eq!(result.attempts.len(), 1);
    assert!(result.cancelled);
    assert!(result.attempts[0].succeeded());
    assert!(reads_back(&result.attempts[0].archive_path));
    assert!(!roots[0].join("textures/asset.dds").exists());
    assert!(root_plugin(&roots[0]).exists());
    assert!(roots[1].join("textures/asset.dds").exists());
    assert!(archives_in(&roots[1]).is_empty());
    assert!(!root_plugin(&roots[1]).exists());
    assert!(run.cleaned_up());
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
    finalizer.probe = capacity(move |root| {
        if root == second && probed_plugin.exists() {
            stop.cancel();
        }
        Some(UNLIMITED)
    });
    let run = finalizer.run(&roots);

    let result = run.finalization();
    assert!(result.attempts.is_empty());
    assert!(result.cancelled);
    assert!(result.safe_to_continue);
    assert!(first_plugin.exists());
    assert!(!second_plugin.exists());
    for root in &roots {
        assert!(root.join("empty/nested").exists());
    }
    assert!(run.cleaned_up());
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
        probe: capacity(move |_| {
            holder
                .lock()
                .unwrap()
                .get_or_insert_with(|| hold(&probed, SHARE_READ));
            Some(UNLIMITED)
        }),
        ..Finalizer::default()
    };
    let run = finalizer.run(std::slice::from_ref(&root));
    assert!(held.lock().unwrap().take().is_some());

    let result = run.finalization();
    assert_eq!(result.attempts.len(), 1);
    assert!(
        result.attempts[0].succeeded(),
        "{:?}",
        result.attempts[0].outcome
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
    assert!(run.cleaned_up());
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
        let run = finalizer.run(std::slice::from_ref(&root));

        let result = run.finalization();
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
        assert!(run.cleaned_up());

        let progress = run.progress().unwrap();
        assert_eq!(progress.total(), 0);
        let summaries = run.finalization_summaries();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].committed, 1);
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
    let run = finalizer.run(std::slice::from_ref(&root));

    let result = run.finalization();
    assert!(result.attempts.is_empty());
    assert_eq!(result.failure, None, "{}", result.detail);
    assert!(result.safe_to_continue);
    let progress = run.progress().unwrap();
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
    assert!(run.cleaned_up());
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
        let run = finalizer.run(std::slice::from_ref(&root));

        let result = run.finalization();
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
        assert!(run.cleaned_up());
    }
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
        probe: capacity(move |_| {
            if !flag.swap(true, Ordering::SeqCst) {
                std::fs::rename(&probed, &moved).unwrap();
                write_file(&probed, &written);
            }
            Some(UNLIMITED)
        }),
        ..Finalizer::default()
    };
    let run = finalizer.run(std::slice::from_ref(&root));

    assert!(replaced.load(Ordering::SeqCst));
    let result = run.finalization();
    assert_eq!(result.attempts.len(), 1);
    assert!(
        result.attempts[0].succeeded(),
        "{:?}",
        result.attempts[0].outcome
    );
    assert_eq!(result.failure, None, "{}", result.detail);
    assert!(result.mutations.is_empty());
    assert_eq!(std::fs::read(&plugin).unwrap(), replacement);
    assert_eq!(std::fs::read(&displaced).unwrap(), bytes);
    assert!(run.cleaned_up());
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
    let run = finalizer.run(&roots);
    drop(held);

    let result = run.finalization();
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
    let progress = run.progress().unwrap();
    assert_eq!((progress.completed(), progress.total()), (0, 0));
    assert!(run.cleaned_up());
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
        probe: capacity(move |_| {
            let _ = std::fs::create_dir(&probed);
            Some(UNLIMITED)
        }),
        ..Finalizer::default()
    };
    let run = finalizer.run(std::slice::from_ref(&root));

    let result = run.finalization();
    assert!(result.attempts.is_empty());
    assert_eq!(
        result.failure,
        Some(ArchiveFinalizationFailure::PluginCreationFailed)
    );
    assert!(result.safe_to_continue);
    assert!(result.mutations.is_empty());
    let progress = run.progress().unwrap();
    assert_eq!((progress.completed(), progress.total()), (0, 0));
    assert!(occupied.is_dir());
    assert!(run.cleaned_up());
}

/// Origin: packingPreservesStagingFiles. Temporary bytes, ours or another
/// run's, are never packed or deleted as packed sources.
#[test]
fn staging_files_are_never_packed() {
    let root = scratch("staging-not-packed").join("mod");
    let nested = root.join("textures/.cao-staging-old/pending.dds");
    write_file(&nested, b"unverified temporary bytes");
    write_file(&root.join("textures/complete.dds"), b"committed bytes");
    // Ours: the run's own Asset attempt stages a temporary file through the
    // run's registry, which only Safety Cleanup removes, after the phase.
    let staged: Arc<Mutex<Option<PathBuf>>> = Arc::default();
    let staging = Arc::clone(&staged);
    let assets = ScriptedToolset::default().with_assets(move |_, mod_root, artifacts| {
        let mut staged = staging.lock().unwrap();
        if staged.is_none() {
            let path = artifacts.stage_archive_file(mod_root).unwrap().path;
            std::fs::write(&path, b"temporary bytes").unwrap();
            *staged = Some(path);
        }
        Ok(AssetExecutionResult::success(MutationState::None))
    });
    // Once the output's attempt, source deletion included, is recorded, both
    // temporaries must still be there.
    let survived = Arc::new(AtomicBool::new(false));
    let (flag, temporary, foreign) = (Arc::clone(&survived), Arc::clone(&staged), nested.clone());
    let sink = RecordingSink::with_hook(move |observed| {
        if let Observed::Phase(record) = observed
            && record.phase() == RunPhase::ArchiveFinalization
            && record
                .progress()
                .is_some_and(|progress| progress.completed() == 1)
        {
            let ours = temporary.lock().unwrap().clone().expect("ours was staged");
            flag.store(ours.exists() && foreign.exists(), Ordering::SeqCst);
        }
    });
    let finalizer = Finalizer {
        settings: ArchiveFinalizationSettings {
            create_dummy_plugins: false,
            compress: false,
            ..ArchiveFinalizationSettings::default()
        },
        assets,
        ..Finalizer::default()
    };
    let run = finalizer.run_observed(std::slice::from_ref(&root), Some(&sink));

    let result = run.finalization();
    assert_eq!(result.attempts.len(), 1);
    assert!(
        result.attempts[0].succeeded(),
        "{:?}",
        result.attempts[0].outcome
    );
    assert!(survived.load(Ordering::SeqCst));
    assert!(nested.exists());
    assert!(!root.join("textures/complete.dds").exists());
    assert_eq!(
        run.toolset.packer.writes()[0].names,
        ["textures/complete.dds"]
    );
    assert!(!archives_in(&root).is_empty());
    assert!(run.cleaned_up());
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
    let run = finalizer.run(std::slice::from_ref(&root));

    let result = run.finalization();
    assert_eq!(result.attempts.len(), 1);
    assert!(
        result.attempts[0].succeeded(),
        "{:?}",
        result.attempts[0].outcome
    );
    assert!(!packed.exists());
    assert_eq!(std::fs::read(&retained).unwrap(), b"retained bytes");
    let entries = FakeArchiveReader::default()
        .list_entries(&result.attempts[0].archive_path)
        .unwrap();
    assert_eq!(entries.len(), 1);
    assert!(run.cleaned_up());
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
    let run = finalizer.run(std::slice::from_ref(&root));

    let result = run.finalization();
    assert_eq!(result.attempts.len(), 1);
    assert!(
        result.attempts[0].succeeded(),
        "{:?}",
        result.attempts[0].outcome
    );
    let names: Vec<_> = run
        .toolset
        .packer
        .writes()
        .into_iter()
        .flat_map(|write| write.names)
        .collect();
    assert_eq!(names, ["textures/asset.dds"]);
    assert!(!packed.exists());
    assert!(excluded.exists());
    assert!(backslashed.exists());
    assert!(run.cleaned_up());
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
    let run = finalizer.run(std::slice::from_ref(&root));
    drop(locked);

    let result = run.finalization();
    assert_eq!(result.attempts.len(), 1);
    assert!(!result.attempts[0].succeeded());
    assert_eq!(result.attempts[0].outcome.mutation(), MutationState::None);
    assert!(available.exists());
    assert!(unavailable.exists());
    assert_eq!(std::fs::read(&existing).unwrap(), b"previous archive bytes");
    assert!(run.cleaned_up());
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
        probe: capacity(move |_| {
            if !flag.load(Ordering::SeqCst) && std::fs::rename(&link, &moved).is_ok() {
                junction(&link, &target);
                flag.store(true, Ordering::SeqCst);
            }
            Some(UNLIMITED)
        }),
        ..Finalizer::default()
    };
    let run = finalizer.run(std::slice::from_ref(&root));
    std::fs::remove_dir(&source_parent).unwrap();

    assert!(swapped.load(Ordering::SeqCst));
    let result = run.finalization();
    assert_eq!(result.attempts.len(), 1);
    assert_eq!(
        result.attempts[0]
            .outcome
            .failure()
            .map(|failure| failure.category()),
        Some(ArchiveFinalizationFailure::WriteFailed)
    );
    assert_eq!(result.attempts[0].outcome.mutation(), MutationState::None);
    assert!(!result.attempts[0].archive_path.exists());
    assert!(retained_parent.join("armor/asset.dds").exists());
    assert_eq!(
        std::fs::read(&outside_source).unwrap(),
        b"outside source bytes"
    );
    assert!(run.cleaned_up());
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
        probe: capacity(move |_| {
            if !competed.swap(true, Ordering::SeqCst) {
                write_file(&competitor, b"competing creator bytes");
            }
            Some(UNLIMITED)
        }),
        ..Finalizer::default()
    };
    let run = finalizer.run_observed(std::slice::from_ref(&root), Some(&sink));

    let result = run.finalization();
    assert_eq!(result.attempts.len(), 2);
    assert!(result.safe_to_continue);
    assert!(!result.cancelled);
    assert_eq!(result.attempts[0].archive_path, first_destination);
    assert_ne!(result.attempts[1].archive_path, first_destination);
    assert!(!result.attempts[0].succeeded());
    assert_eq!(result.attempts[0].outcome.mutation(), MutationState::None);
    assert!(
        result.attempts[1].succeeded(),
        "{:?}",
        result.attempts[1].outcome
    );
    assert_eq!(
        result.attempts[1].outcome.mutation(),
        MutationState::Committed
    );
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
    assert!(run.cleaned_up());
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
        probe: capacity(move |_| {
            if !occupied.swap(true, Ordering::SeqCst) {
                write_file(&competitor, b"competing plugin bytes");
            }
            Some(UNLIMITED)
        }),
        ..Finalizer::default()
    };
    let run = finalizer.run(std::slice::from_ref(&root));

    let result = run.finalization();
    assert_eq!(result.attempts.len(), 1);
    let attempt = &result.attempts[0];
    assert_eq!(
        attempt.outcome.failure().map(|failure| failure.category()),
        Some(ArchiveFinalizationFailure::PluginCreationFailed)
    );
    assert_eq!(attempt.outcome.mutation(), MutationState::Committed);
    assert!(!attempt.safe_to_continue());
    assert!(!result.safe_to_continue);
    assert!(result.mutations.is_empty());
    assert!(reads_back(&attempt.archive_path));
    assert!(source.exists());
    assert_eq!(
        std::fs::read(&planned_plugin).unwrap(),
        b"competing plugin bytes"
    );
    assert!(run.cleaned_up());
}

/// Rust-only, for #602: the phase loop starts the next output only on an
/// attempt's safe-to-continue verdict. A competitor at the first Mod Root's
/// planned Dummy Plugin name leaves its committed output attempt an unsafe
/// Operation Failure, so the later Mod Root's output is never attempted and
/// the run is Failed, with the failure's detail in the run's evidence.
#[test]
fn an_unsafe_committed_output_attempt_stops_later_outputs_and_fails_the_run() {
    let parent = scratch("unsafe-committed-stops");
    let root = parent.join("mod-a");
    let later_root = parent.join("mod-b");
    let source = root.join("textures/asset.dds");
    write_file(&source, b"source bytes");
    let later_source = later_root.join("textures/later.dds");
    write_file(&later_source, b"later source bytes");
    let competitor = root_plugin(&root);
    let occupied = AtomicBool::new(false);
    let finalizer = Finalizer {
        settings: ArchiveFinalizationSettings {
            compress: false,
            ..ArchiveFinalizationSettings::default()
        },
        // The first capacity sample comes after planning froze mod-a's
        // planned Dummy Plugin name and before any output attempt.
        probe: capacity(move |_| {
            if !occupied.swap(true, Ordering::SeqCst) {
                write_file(&competitor, b"competing plugin bytes");
            }
            Some(UNLIMITED)
        }),
        ..Finalizer::default()
    };
    let run = finalizer.run(&[root.clone(), later_root.clone()]);

    assert_eq!(run.result.outcome(), RunOutcome::Failed);
    let result = run.finalization();
    assert_eq!(run.progress().unwrap().total(), 2);
    let [attempt] = result.attempts.as_slice() else {
        panic!("only the first output is attempted: {result:?}");
    };
    assert!(!attempt.succeeded());
    assert!(!attempt.safe_to_continue());
    assert_eq!(attempt.outcome.mutation(), MutationState::Committed);
    let failure = attempt.outcome.failure().expect("an Operation Failure");
    assert_eq!(
        failure.category(),
        ArchiveFinalizationFailure::PluginCreationFailed
    );
    assert!(!failure.detail().is_empty());
    assert!(!result.safe_to_continue);
    assert!(reads_back(&attempt.archive_path));
    assert!(source.exists());
    assert!(later_source.exists());
    assert!(archives_in(&later_root).is_empty());
    assert!(run.cleaned_up());
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
        probe: capacity(move |_| {
            let mut recorded = recorded.lock().unwrap();
            if recorded.is_none() {
                write_file(&plugin, &fake_dummy_plugin());
                *recorded = Some(age(&plugin));
            }
            Some(UNLIMITED)
        }),
        ..Finalizer::default()
    };
    let run = finalizer.run(std::slice::from_ref(&root));

    let original = written.lock().unwrap().expect("the dummy appeared");
    let result = run.finalization();
    assert_eq!(result.attempts.len(), 1);
    assert!(
        result.attempts[0].succeeded(),
        "{:?}",
        result.attempts[0].outcome
    );
    assert_eq!(
        result.attempts[0].outcome.mutation(),
        MutationState::Committed
    );
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
    assert!(run.cleaned_up());
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
    let run = finalizer.run(std::slice::from_ref(&root));

    let result = run.finalization();
    assert_eq!(result.attempts.len(), 2);
    for attempt in &result.attempts {
        assert!(attempt.succeeded(), "{:?}", attempt.outcome);
        assert_eq!(attempt.outcome.mutation(), MutationState::Committed);
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
    assert!(run.cleaned_up());

    // The run's whole Run Evidence also summarizes its other phases' effects.
    let summaries = run.finalization_summaries();
    assert_eq!(summaries.len(), 1);
    let summary = summaries[0];
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
        probe: capacity(move |_| {
            if !flag.load(Ordering::SeqCst) {
                flag.store(std::fs::remove_file(&vanishing).is_ok(), Ordering::SeqCst);
            }
            Some(UNLIMITED)
        }),
        ..Finalizer::default()
    };
    let run = finalizer.run(std::slice::from_ref(&root));

    assert!(removed.load(Ordering::SeqCst));
    let result = run.finalization();
    assert_eq!(result.attempts.len(), 1);
    assert!(
        result.attempts[0].succeeded(),
        "{:?}",
        result.attempts[0].outcome
    );
    assert_eq!(
        result.attempts[0].outcome.mutation(),
        MutationState::Committed
    );
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
    assert!(run.cleaned_up());
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
        let run = finalizer.run(std::slice::from_ref(&root));

        let result = run.finalization();
        assert_eq!(result.attempts.len(), 1);
        assert!(
            result.attempts[0].succeeded(),
            "{:?}",
            result.attempts[0].outcome
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
        assert!(run.cleaned_up());
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
        probe: capacity(move |_| {
            if !flag.swap(true, Ordering::SeqCst) {
                for name in &names {
                    assert!(symlink(&link_target, name));
                }
            }
            Some(UNLIMITED)
        }),
        ..Finalizer::default()
    };
    let run = finalizer.run(std::slice::from_ref(&root));

    assert!(linked.load(Ordering::SeqCst));
    let result = run.finalization();
    assert_eq!(result.attempts.len(), 1);
    assert_eq!(
        result.attempts[0]
            .outcome
            .failure()
            .map(|failure| failure.category()),
        Some(ArchiveFinalizationFailure::PluginCreationFailed)
    );
    assert!(source.exists());
    assert!(reads_back(&result.attempts[0].archive_path));
    assert!(run.cleaned_up());
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
        let stop = AtomicBool::new(false);
        let (run, (saw_cleanup_gap, removed_plugin)) = std::thread::scope(|scope| {
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
            let run = finalizer.run(std::slice::from_ref(&root));
            stop.store(true, Ordering::SeqCst);
            (run, remover.join().unwrap())
        });

        let result = run.finalization();
        assert_eq!(result.attempts.len(), 1);
        assert!(
            result.attempts[0].succeeded(),
            "{:?}",
            result.attempts[0].outcome
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
        assert!(run.cleaned_up());
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
        let mut run = finalizer.run(&[root.clone(), later_root.clone()]);
        run.release_fakes();
        drop(locked);

        let result = run.finalization();
        assert_eq!(result.attempts.len(), if deny_reads { 1 } else { 2 });
        let first = &result.attempts[0];
        assert!(!first.succeeded());
        assert_eq!(
            first.outcome.failure().map(|failure| failure.category()),
            Some(ArchiveFinalizationFailure::SourceCleanupFailed)
        );
        assert_eq!(first.outcome.mutation(), MutationState::Committed);
        assert_eq!(result.safe_to_continue, !deny_reads);
        assert!(reads_back(&first.archive_path));
        assert_eq!(later_source.exists(), deny_reads);
        assert_eq!(archives_in(&later_root).len(), usize::from(!deny_reads));
        assert_eq!(empty_directory.exists(), deny_reads);
        if !deny_reads {
            let last = result.attempts.last().unwrap();
            assert!(last.succeeded(), "{:?}", last.outcome);
            assert!(reads_back(&last.archive_path));
        }
        assert_eq!(std::fs::read(&source).unwrap(), b"retained source bytes");
        assert!(run.cleaned_up());
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
        probe: capacity(move |_| {
            counted.fetch_add(1, Ordering::SeqCst);
            Some(UNLIMITED)
        }),
        ..Finalizer::default()
    };
    let run = finalizer.run(std::slice::from_ref(&root));

    let result = run.finalization();
    assert!(result.attempts.is_empty());
    assert_eq!(result.failure, None);
    assert!(result.safe_to_continue);
    assert_eq!(run.progress().unwrap().total(), 0);
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
    assert!(run.cleaned_up());
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
    let run = finalizer.run(std::slice::from_ref(&root));

    let result = run.finalization();
    assert!(std::fs::read_dir(&root).unwrap().next().is_none());
    assert_eq!(result.mutations.len(), 1);
    assert_eq!(result.mutations[0].count, 3);
    assert!(run.cleaned_up());
}

/// Split points: sources over the size limit split into several Archives of
/// one kind, in sorted order. The first takes the plugin-derived name, the
/// rest the next free counter names, and each gets its own Loading Plugin.
/// A test cannot cheaply reach a real game's 2 GiB limit, so this pins the
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
    let run = finalizer.run(std::slice::from_ref(&root));

    let result = run.finalization();
    let archives: Vec<_> = result
        .attempts
        .iter()
        .map(|attempt| {
            assert!(attempt.succeeded(), "{:?}", attempt.outcome);
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
    let names: Vec<_> = run
        .toolset
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
    assert!(run.cleaned_up());
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
        let run = finalizer.run(std::slice::from_ref(&root));

        let result = run.finalization();
        assert!(result.attempts.iter().all(|attempt| attempt.succeeded()));
        let kinds: Vec<_> = run
            .toolset
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
        assert!(run.cleaned_up());
    }
}

/// Origin: unexpectedExceptionIsRecordedOnce (both rows). A failure of the
/// phase's own work, in planning or after the output total, is recorded once
/// as an unsafe phase-level failure.
///
/// The after-plan failure is a packer whose name listing panics once
/// planning is done, in Loading Plugin maintenance: a panicking volume query
/// is an unknown volume, not a failure (#592). The root holds only an
/// existing Archive, so it plans no output and nothing is mutated before
/// maintenance fails.
///
/// The in-planning failure is the same packer panicking at planning's first
/// listing. C++ used a missing Mod Root, which Preparing now rejects before
/// any run reaches the phase (#597).
#[test]
fn an_unexpected_failure_is_recorded_once() {
    for after_plan in [false, true] {
        let parent = scratch(&format!("unexpected-{after_plan}"));
        let root = parent.join("mod");
        let source = root.join("existing.bsa");
        write_file(&source, b"retained archive");
        let finalizer = Finalizer {
            // Planning lists the root's Archives and then its Plugins;
            // maintenance's first listing panics. Before the plan, planning's
            // own first listing panics.
            packer: FakeArchivePacker::panicking_list_names_beyond(if after_plan { 2 } else { 0 }),
            ..Finalizer::default()
        };
        let run = finalizer.run(std::slice::from_ref(&root));

        let result = run.finalization();
        assert_eq!(
            result.failure,
            Some(ArchiveFinalizationFailure::UnexpectedException)
        );
        assert!(!result.safe_to_continue);
        assert!(!result.detail.is_empty());
        if after_plan {
            assert_eq!(result.detail, "the archive name listing panicked");
            assert_eq!(run.progress().unwrap().total(), 0);
        } else {
            assert_eq!(run.progress(), None);
        }
        assert!(result.attempts.is_empty());
        assert_eq!(std::fs::read(&source).unwrap(), b"retained archive");
        assert_eq!(archives_in(&root), std::slice::from_ref(&source));
        assert!(!root.join("existing.esp").exists(), "no Dummy Plugin");
        assert!(run.cleaned_up());
    }
}

/// Rust-only, for #588: Archive Finalization is the only layer that contains
/// a panic inside an output attempt. A packer panicking once it has written
/// the first Archive to staging becomes that attempt's Unexpected Exception,
/// which forbids continuation: nothing is published, the later Mod Root's
/// output is never attempted, and pruning does not run.
#[test]
fn a_packer_panicking_after_its_write_is_an_unexpected_exception_that_stops_later_outputs() {
    let parent = scratch("packer-panics");
    let root = parent.join("mod-a");
    let later_root = parent.join("mod-b");
    let source = root.join("textures/asset.dds");
    write_file(&source, b"source bytes");
    let later_source = later_root.join("textures/later.dds");
    write_file(&later_source, b"later source bytes");
    let empty_directory = root.join("empty/nested");
    std::fs::create_dir_all(&empty_directory).unwrap();
    let finalizer = Finalizer {
        settings: ArchiveFinalizationSettings {
            create_dummy_plugins: false,
            compress: false,
            ..ArchiveFinalizationSettings::default()
        },
        packer: FakeArchivePacker::panicking_after_write(),
        ..Finalizer::default()
    };
    let run = finalizer.run(&[root.clone(), later_root.clone()]);

    let result = run.finalization();
    assert_eq!(run.progress().unwrap().total(), 2);
    assert_eq!(
        result.attempts.len(),
        1,
        "the later output is not attempted"
    );
    let attempt = &result.attempts[0];
    assert_eq!(
        attempt.outcome.failure().map(|failure| failure.category()),
        Some(ArchiveFinalizationFailure::UnexpectedException)
    );
    assert_eq!(
        attempt.outcome.failure().map(|failure| failure.detail()),
        Some("the archive writer panicked")
    );
    assert!(!attempt.safe_to_continue());
    assert_eq!(attempt.outcome.mutation(), MutationState::None);
    assert!(!result.safe_to_continue);
    assert_eq!(run.toolset.packer.writes().len(), 1);
    assert!(source.exists());
    assert!(later_source.exists());
    assert!(archives_in(&root).is_empty());
    assert!(archives_in(&later_root).is_empty());
    assert!(empty_directory.exists(), "pruning does not run");
    assert!(run.cleaned_up());
}

/// Origin: cleanupExceptionKeepsRecordedAttempts. A failure in Loading
/// Plugin maintenance keeps the committed attempt before it.
///
/// C++ moved mod-b away after mod-a's output; a run pins every Mod Root, so
/// here the packer panics as maintenance lists mod-b's Archives once mod-a's
/// output is committed (#597).
#[test]
fn a_plugin_maintenance_failure_keeps_recorded_attempts() {
    let parent = scratch("cleanup-failure");
    let packed = parent.join("mod-a");
    let existing = parent.join("mod-b");
    write_file(&packed.join("textures/asset.dds"), b"source bytes");
    write_file(&existing.join("existing.bsa"), b"retained archive");
    let failed = Arc::new(AtomicBool::new(false));
    let (flag, packed_root, existing_root) =
        (Arc::clone(&failed), packed.clone(), existing.clone());
    let finalizer = Finalizer {
        settings: ArchiveFinalizationSettings {
            compress: false,
            ..ArchiveFinalizationSettings::default()
        },
        // Planning lists mod-b before any output exists, so only
        // maintenance's listing comes after mod-a's commit.
        packer: FakeArchivePacker {
            before_list: Some(Box::new(move |dir, kind| {
                if dir == existing_root
                    && kind == ArchiveNameKind::Archive
                    && !archives_in(&packed_root).is_empty()
                {
                    flag.store(true, Ordering::SeqCst);
                    panic!("the archive name listing panicked");
                }
            })),
            ..FakeArchivePacker::default()
        },
        ..Finalizer::default()
    };
    let run = finalizer.run(&[packed, existing]);

    assert!(failed.load(Ordering::SeqCst));
    let result = run.finalization();
    assert_eq!(
        result.failure,
        Some(ArchiveFinalizationFailure::UnexpectedException)
    );
    assert!(!result.safe_to_continue);
    assert_eq!(result.attempts.len(), 1);
    assert!(
        result.attempts[0].succeeded(),
        "{:?}",
        result.attempts[0].outcome
    );
    assert_eq!(
        result.attempts[0].outcome.mutation(),
        MutationState::Committed
    );
    assert_eq!(run.progress().unwrap().completed(), 1);
    assert!(reads_back(&result.attempts[0].archive_path));
    assert!(run.cleaned_up());
}
