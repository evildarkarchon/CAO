//! Archive extraction's own rechecks, through whole Optimization Runs over a
//! fake archive reader and real Mod Root directories (#597).
//!
//! Each scenario names its C++ origin in
//! `tests/ArchiveFirstAssetDiscoveryTests.cpp`, which drove
//! `ArchiveExtractor::extract` with hand-built plans. Here every plan comes
//! from the run's own preflight, as in production, and a scenario reaches the
//! extraction's rechecks by changing the Mod Root or the Archive after
//! preflight: on the run's first Archive extraction phase record, through a
//! recording Run Observation Sink, or from a fake the extraction calls. Each
//! asserts on the run's Run Evidence and the filesystem. Extracted sources
//! are backed up to `<name>.bak`, as the default source cleanup does.
//!
//! Not ported, with reasons:
//! - The per-format rows of `stagedExtractionCommitsPayload`: TES3, TES4 and
//!   FO4 containers are the reader's concern, ported against real Archives in
//!   `cao-optimizers` (#497). One fake format exercises the staged writer.
//!
//! Deleted as duplicates of `archive_discovery.rs` (#597):
//! - `partialMergeRetainsCommittedOutput`: `archive_failures_control_continuation`
//!   asserts a partial merge's committed entry, unknown mutation and unsafe
//!   continuation over the same obstruction written after preflight.
//! - `mergeRejectsLinkedParent`: `every_attempt_repeats_containment_after_a_safe_rejection`
//!   inserts the same junction after preflight and asserts the same safe
//!   rejection with nothing written outside.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use cao_core::execution::MutationState;
use cao_core::routing::RequestedWork;
use cao_core::run::{
    ArchiveExtractionFailure, ArchiveExtractionResult, CancellationToken, OptimizationRunResult,
    RunPhase,
};
use cao_core::test_support::{
    ArchiveRunRequest, FakeArchiveReader, FakeVolumeProbe, RecordingSink, ScriptedToolset, entry,
    write_archive,
};
use common::{canonical, scratch_dir};

/// More bytes than any scenario needs.
const UNLIMITED: u64 = u64::MAX;

/// An Apply run over `root` that requests only Archive extraction.
fn extraction_run(root: &Path) -> ArchiveRunRequest {
    ArchiveRunRequest::new(root).with_work([RequestedWork::ArchiveExtraction])
}

/// Executes an extraction run over `root` with `toolset`'s fakes, running
/// `after_preflight` on the run's first Archive extraction phase record:
/// after preflight planned every Archive, before the first attempt.
///
/// Returns the run's result and the toolset, whose fakes' records stay
/// readable.
fn run_extraction(
    root: &Path,
    toolset: ScriptedToolset,
    after_preflight: impl Fn() + 'static,
) -> (OptimizationRunResult, Arc<ScriptedToolset>) {
    let run = extraction_run(root);
    let configuration = run.configuration(toolset);
    let sink = RecordingSink::on_first_record_of(RunPhase::ExtractingArchives, after_preflight);
    let result = run.execute(&configuration, Some(&sink), &CancellationToken::new());
    (result, configuration.toolset)
}

/// The run's one Archive extraction attempt.
///
/// # Panics
///
/// When the run made no attempt or several, such as when its preflight
/// failed.
fn only_attempt(result: &OptimizationRunResult) -> &ArchiveExtractionResult {
    let attempts = result.archive_extraction_attempts();
    assert_eq!(attempts.len(), 1, "{:?}", result.failures());
    &attempts[0]
}

/// The `<name>.bak` backup Backup source cleanup renames an extracted
/// source Archive at `archive` to.
fn backup_of(archive: &Path) -> PathBuf {
    let mut name = archive.file_name().unwrap().to_os_string();
    name.push(".bak");
    archive.with_file_name(name)
}

/// A Toolset over the default fakes whose volume probe is `probe`.
fn probing(probe: FakeVolumeProbe) -> ScriptedToolset {
    ScriptedToolset {
        volume_probe: Arc::new(probe),
        ..ScriptedToolset::default()
    }
}

/// A Toolset over the default fakes whose reader panics as it extracts each
/// entry, before writing the payload.
fn panicking_reader() -> ScriptedToolset {
    ScriptedToolset {
        reader: Arc::new(FakeArchiveReader {
            on_extract: Some(Box::new(|_, _| panic!("the archive library panicked"))),
            ..FakeArchiveReader::default()
        }),
        ..ScriptedToolset::default()
    }
}

/// Origin: extractionCapacityRecheck (shortage and unknown rows). Free space
/// is rechecked just before staging: a shortage the preflight did not see
/// leaves no staging and no output, and unknown capacity proceeds.
#[test]
fn the_capacity_check_is_repeated_before_staging() {
    for unknown in [false, true] {
        let root = canonical(&scratch_dir(&format!("extractor-capacity-{unknown}")));
        let archive = root.join("source.bsa");
        write_archive(&archive, &[entry("textures/fixture.dds", b"fixture")]);
        let original = std::fs::read(&archive).unwrap();
        // Call 1 is the preflight's, call 2 the attempt's recheck: the volume
        // fills up between them.
        let probe = if unknown {
            FakeVolumeProbe::default()
        } else {
            FakeVolumeProbe::default()
                .with_counted_capacity(|call, _| Some(if call == 1 { UNLIMITED } else { 0 }))
        };

        let (result, _) = run_extraction(&root, probing(probe), || {});

        let attempt = only_attempt(&result);
        assert_eq!(attempt.succeeded(), unknown, "{:?}", attempt.outcome);
        assert!(attempt.safe_to_continue());
        let kept = if unknown {
            backup_of(&archive)
        } else {
            archive.clone()
        };
        assert_eq!(std::fs::read(&kept).unwrap(), original);
        if !unknown {
            assert_eq!(
                attempt.outcome.failure_category(),
                Some(ArchiveExtractionFailure::InsufficientCapacity)
            );
            assert_eq!(attempt.outcome.mutation(), MutationState::None);
            assert!(!root.join(".cao-staging").exists());
            assert!(!root.join("textures").exists());
        }
        assert!(result.cleanup_failures().is_empty());
    }
}

/// Rust-only, for #592: a free-space probe that panics in the recheck is
/// unknown capacity, so the attempt proceeds rather than failing as an
/// extraction failure. The preflight's own sample answers, so only the
/// recheck meets the panic.
#[test]
fn a_panicking_capacity_probe_in_the_recheck_is_unknown_capacity() {
    let root = canonical(&scratch_dir("extractor-capacity-panic"));
    let archive = root.join("source.bsa");
    write_archive(&archive, &[entry("textures/fixture.dds", b"fixture")]);
    let probe = FakeVolumeProbe::default().with_counted_capacity(|call, _| {
        // The panic is the probe failure under test: the preflight's sample
        // (call 1) answers, and every later query, the recheck's, panics as
        // a broken free-space query would.
        if call != 1 {
            panic!("the free-space query failed");
        }
        Some(UNLIMITED)
    });

    let (result, _) = run_extraction(&root, probing(probe), || {});

    let attempt = only_attempt(&result);
    assert!(attempt.succeeded(), "{:?}", attempt.outcome);
    assert_eq!(attempt.outcome.failure_category(), None);
    assert_eq!(attempt.outcome.mutation(), MutationState::Committed);
    assert!(attempt.safe_to_continue());
    assert_eq!(
        std::fs::read(root.join("textures/fixture.dds")).unwrap(),
        b"fixture"
    );
    assert!(result.cleanup_failures().is_empty());
}

/// Origin: extractionFailureBeforeMergeCommitsNothing. A source Archive that
/// vanishes after preflight fails its attempt before anything is staged.
#[test]
fn a_source_lost_after_preflight_commits_nothing() {
    let root = canonical(&scratch_dir("extractor-missing-source"));
    let archive = root.join("source.bsa");
    write_archive(&archive, &[entry("textures/a.dds", b"B")]);
    let lost = archive.clone();

    let (result, _) = run_extraction(&root, ScriptedToolset::default(), move || {
        std::fs::remove_file(&lost).unwrap();
    });

    let attempt = only_attempt(&result);
    assert_eq!(
        attempt.outcome.failure_category(),
        Some(ArchiveExtractionFailure::ExtractionFailed)
    );
    assert_eq!(attempt.outcome.mutation(), MutationState::None);
    assert!(attempt.safe_to_continue());
    assert!(!root.join("textures").exists());
    assert!(result.cleanup_failures().is_empty());
}

/// Origin: stagedExtractionCommitsPayload. The payload commits from
/// registered staging, the source Archive is kept byte for byte as its
/// backup, and Safety Cleanup leaves the committed entry alone.
#[test]
fn a_staged_payload_commits_and_the_source_archive_is_kept() {
    let root = canonical(&scratch_dir("extractor-commits"));
    let archive = root.join("source.bsa");
    write_archive(&archive, &[entry("textures/a.dds", b"B")]);
    let original = std::fs::read(&archive).unwrap();

    let (result, _) = run_extraction(&root, ScriptedToolset::default(), || {});

    let attempt = only_attempt(&result);
    assert!(attempt.succeeded(), "{:?}", attempt.outcome);
    assert_eq!(attempt.outcome.mutation(), MutationState::Committed);
    // The run's Safety Cleanup has already run.
    assert!(result.cleanup_failures().is_empty());
    assert_eq!(std::fs::read(root.join("textures/a.dds")).unwrap(), b"B");
    assert_eq!(std::fs::read(backup_of(&archive)).unwrap(), original);
}

/// Origin: extractionUsesExistingParentCasing.
#[test]
fn extraction_publishes_beneath_the_existing_parent_casing() {
    let root = canonical(&scratch_dir("extractor-parent-casing"));
    let archive = root.join("source.bsa");
    write_archive(&archive, &[entry("textures/a.dds", b"B")]);
    std::fs::create_dir(root.join("Textures")).unwrap();

    let (result, _) = run_extraction(&root, ScriptedToolset::default(), || {});

    let attempt = only_attempt(&result);
    assert!(attempt.succeeded(), "{:?}", attempt.outcome);
    let parents: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .filter(|name| name.eq_ignore_ascii_case("textures"))
        .collect();
    assert_eq!(parents, ["Textures"]);
    assert_eq!(
        std::fs::read(root.join("Textures").join("a.dds")).unwrap(),
        b"B"
    );
    assert!(result.cleanup_failures().is_empty());
}

/// Origin: the `catch (...)` of C++ `ArchiveExtractor::extract`, which kept
/// a non-standard exception classified by how far the attempt got. A reader
/// that panics while staging has published nothing, so the attempt is a
/// safe failure rather than unknown mutation, and its source is not cleaned.
#[test]
fn a_reader_panicking_before_any_commit_is_a_safe_failure() {
    let root = canonical(&scratch_dir("extractor-reader-panics"));
    let archive = root.join("source.bsa");
    write_archive(&archive, &[entry("textures/a.dds", b"B")]);

    let (result, _) = run_extraction(&root, panicking_reader(), || {});

    let attempt = only_attempt(&result);
    assert_eq!(
        attempt.outcome.failure_category(),
        Some(ArchiveExtractionFailure::ExtractionFailed)
    );
    assert_eq!(attempt.outcome.mutation(), MutationState::None);
    assert!(attempt.safe_to_continue());
    assert!(
        attempt
            .outcome
            .failure()
            .expect("an Operation Failure")
            .detail()
            .contains("the archive library panicked")
    );
    assert!(!root.join("textures").exists());
    assert!(archive.exists());
    assert!(
        result.cleanup_failures().is_empty(),
        "the staged entry is removed"
    );
}

/// Rewrites the fake Archive at `archive` with a valid manifest that differs
/// from the one preflight planned.
fn rewrite_manifest(archive: &Path) {
    write_archive(archive, &[entry("textures/other.dds", b"O")]);
}

/// Origin: changedManifestFailsBeforeMerge. An Archive whose manifest changed
/// after preflight, though it still reads, fails before any merge.
#[test]
fn a_manifest_changed_after_preflight_fails_before_any_merge() {
    let root = canonical(&scratch_dir("extractor-changed-manifest"));
    let archive = root.join("source.bsa");
    write_archive(&archive, &[entry("textures/a.dds", b"B")]);
    let changed = archive.clone();

    let (result, _) = run_extraction(&root, ScriptedToolset::default(), move || {
        rewrite_manifest(&changed);
    });

    let attempt = only_attempt(&result);
    assert_eq!(
        attempt.outcome.failure_category(),
        Some(ArchiveExtractionFailure::ExtractionFailed)
    );
    assert_eq!(attempt.outcome.mutation(), MutationState::None);
    assert!(attempt.safe_to_continue());
    assert!(
        attempt
            .outcome
            .failure()
            .expect("an Operation Failure")
            .detail()
            .contains("changed after preflight")
    );
    assert!(archive.exists());
    assert!(!root.join("textures").exists());
    assert!(result.cleanup_failures().is_empty());
}

/// Rust-only, for the #497 risk "drop a memory-mapped Archive before deleting
/// or renaming its source". The reader may keep its Archive open between
/// entries, so the extraction tells it to let go at the end of every
/// attempt, whether the attempt succeeded, failed or panicked; source cleanup
/// comes after.
///
/// Each row is its own run, so each reader sees one attempt. The runs
/// request no Archive creation, whose read-back of each output would release
/// the reader again.
#[test]
fn every_attempt_releases_the_reader_before_returning() {
    /// How a row's one extraction attempt ends.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Ending {
        Succeeded,
        /// The manifest changes after preflight, so the attempt fails.
        Failed,
        /// The reader panics while staging.
        Panicked,
    }

    for row in [Ending::Succeeded, Ending::Failed, Ending::Panicked] {
        let root = canonical(&scratch_dir(&format!("extractor-releases-{row:?}")));
        let archive = root.join("source.bsa");
        write_archive(&archive, &[entry("textures/a.dds", b"B")]);
        let toolset = if row == Ending::Panicked {
            panicking_reader()
        } else {
            ScriptedToolset::default()
        };
        let changed = archive.clone();

        let (result, toolset) = run_extraction(&root, toolset, move || {
            if row == Ending::Failed {
                rewrite_manifest(&changed);
            }
        });

        let attempt = only_attempt(&result);
        assert_eq!(
            attempt.succeeded(),
            row == Ending::Succeeded,
            "{row:?}: {:?}",
            attempt.outcome
        );
        assert_eq!(toolset.reader.releases.load(Ordering::SeqCst), 1, "{row:?}");
        assert!(result.cleanup_failures().is_empty(), "{row:?}");
    }
}
