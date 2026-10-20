//! Archive discovery into the Effective Asset Tree, through the Run Executor
//! with fakes of the archive reader and the capacity and volume-identity
//! probes over real Mod Root directories.
//!
//! Each scenario names its C++ origin, mostly from
//! `tests/ArchiveFirstAssetDiscoveryTests.cpp`, plus the Archive scenarios of
//! `tests/AssetRunTests.cpp` and `tests/RunExecutorTests.cpp`. A fake Archive
//! is a small length-prefixed file the fake reader parses, so a manifest can
//! carry any raw name and a scenario can corrupt an Archive just by
//! rewriting it. File-symlink scenarios skip, as C++ did, on a host without
//! the symlink privilege.
//!
//! Each run's work goes through a scripted Run Toolset whose reader and
//! probes are those fakes. A scenario that acts at a Run Phase does so on
//! that phase's record through a recording Run Observation Sink, and cancels
//! through the run's cancellation token.
//!
//! Every run opts in to core's real Archive extraction and Archive
//! Finalization over those fakes, so it does what a production run does: an
//! extracted source Archive is backed up to `<name>.bak`, and Archive
//! Finalization prunes empty directories and maintains Loading Plugins. A
//! scenario observes that work through the run's Run Evidence, and through
//! the fakes' own records only for what Run Evidence does not hold. The only
//! runs that still script an Archive port are the panic and escaping-work
//! scenarios no real port can produce; they are deleted with the ports
//! (#588).
//!
//! Ported elsewhere: the scenarios that drove `ArchiveExtractor` with a
//! hand-built plan are in `archive_extraction.rs`.
//!
//! Not ported, with reasons:
//! - `removedExplicitArchiveRootDoesNotThrow`,
//!   `explicitArchiveRootDiscoversExtractedSiblings`, and the file-root rows
//!   of `collisionsAreReportedBeforeExtraction`: a Rust Mod Selection is
//!   always a directory, so no Archive can be selected as a root.
//! - `removedDirectoryRootDoesNotThrow`: Apply Preparing pins every Mod Root
//!   for the whole run, so extraction cannot remove one.
//! - The symlink rows of `directoryLinksAreExcluded`: a directory symlink and
//!   a junction are both reparse points, excluded by the same attribute; the
//!   junction rows are ported.
//! - The per-format rows of `rawManifestPaths`: container formats belong to
//!   the real reader in `cao-optimizers` (#497).

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use cao_core::execution::MutationState;
use cao_core::routing::{ExecutionMode, RequestedWork};
use cao_core::run::{
    ArchiveExtractionFailure, ArchiveExtractionResult, ArchiveFinalizationMutationKind,
    ArchivePrecedence, CancellationToken, ModSelection, MutationKind, OptimizationRunResult,
    RunDiagnosticCode, RunFailureCode, RunObservationSink, RunOutcome, RunPhase, RunRequest,
};
use cao_core::test_support::{
    CountingCleanup, FakeArchiveReader, FakeCapacity, FakeEntry, FakeVolumes, Observed,
    RecordingSink, ScriptedToolset, ToolsetConfiguration, entry, execute_run, write_archive,
};
use common::{canonical, junction, request, scratch_dir, snapshot_tree, write_tree};

/// The Apply request over native Textures and Archive extraction.
fn archives_and_textures(root: &Path) -> RunRequest {
    request(
        ExecutionMode::Apply,
        root,
        &[
            RequestedWork::NativeTextureOptimization,
            RequestedWork::ArchiveExtraction,
        ],
    )
}

/// Executes one request through the Run Executor over `toolset`, without an
/// observer or cancellation.
fn execute(request: &RunRequest, toolset: &Arc<ScriptedToolset>) -> OptimizationRunResult {
    execute_with(request, toolset, None, &CancellationToken::new())
}

/// Executes one request over a fresh Run Toolset from `toolset`, publishing
/// live facts to `observations` and observing cancellation through `stop`.
///
/// The SSE-like profile is the run's configuration. Every run makes exactly
/// one Safety Cleanup pass, which this asserts. It also asserts `toolset`
/// opted in to core's real Archive work, unless it scripts an Archive port
/// for a panic or escaping-work scenario.
fn execute_with(
    request: &RunRequest,
    toolset: &Arc<ScriptedToolset>,
    observations: Option<&dyn RunObservationSink>,
    stop: &CancellationToken,
) -> OptimizationRunResult {
    assert!(
        toolset.core_archive_work || toolset.extraction.is_some() || toolset.finalization.is_some(),
        "a run opts in to core's real Archive work unless it scripts a port's panic or escape"
    );
    let provider = ToolsetConfiguration::sse(Some(Arc::clone(toolset)));
    let mut cleanup = CountingCleanup::default();
    let result = execute_run(request, &mut cleanup, observations, Some(&provider), stop);
    assert_eq!(cleanup.passes, 1, "Safety Cleanup always runs once");
    result
}

/// A default Toolset over the fakes, opted in to core's real Archive
/// extraction and Archive Finalization with the default Archive work
/// settings: extracted sources are backed up.
fn real_archive_work() -> ScriptedToolset {
    ScriptedToolset::default().with_core_archive_work()
}

/// A Toolset doing real Archive work whose capacity probe answers `capacity`
/// for every Mod Root.
fn with_capacity(
    capacity: impl Fn(&Path) -> Option<u64> + Send + Sync + 'static,
) -> ScriptedToolset {
    ScriptedToolset {
        capacity: Arc::new(FakeCapacity(Some(Box::new(capacity)))),
        ..real_archive_work()
    }
}

/// A Toolset doing real Archive work whose fake reader runs `on_extract` as
/// it extracts each entry, before writing the payload.
fn on_extract(hook: impl Fn(&Path, &str) + Send + Sync + 'static) -> Arc<ScriptedToolset> {
    Arc::new(ScriptedToolset {
        reader: Arc::new(FakeArchiveReader {
            on_extract: Some(Box::new(hook)),
            ..FakeArchiveReader::default()
        }),
        ..real_archive_work()
    })
}

/// Whether the reader listed no Archive's manifest.
fn no_manifest_listed(toolset: &ScriptedToolset) -> bool {
    toolset.reader.listed.lock().unwrap().is_empty()
}

/// The Archive path of every extraction attempt in the run's Run Evidence,
/// in the order the run made them.
fn extracted_archives(result: &OptimizationRunResult) -> Vec<PathBuf> {
    result
        .archive_extraction_attempts()
        .iter()
        .map(|attempt| attempt.archive_path.clone())
        .collect()
}

/// The `<name>.bak` backup Backup source cleanup renames an extracted
/// source Archive at `archive` to.
fn backup_of(archive: &Path) -> PathBuf {
    let mut name = archive.file_name().unwrap().to_os_string();
    name.push(".bak");
    archive.with_file_name(name)
}

/// The progress of every phase record `sink` saw for `phase` after at least
/// one attempt completed, as `(completed, total)`, in order.
fn attempt_progress(sink: &RecordingSink, phase: RunPhase) -> Vec<(usize, usize)> {
    sink.observed()
        .into_iter()
        .filter_map(|observed| match observed {
            Observed::Phase(record) if record.phase() == phase => record.progress(),
            _ => None,
        })
        .filter(|progress| progress.completed() > 0)
        .map(|progress| (progress.completed(), progress.total()))
        .collect()
}

/// Origin: ArchiveFirstAssetDiscoveryTests::extractsEnabledArchivesBeforeDefinitiveDiscovery.
#[test]
fn an_enabled_archive_is_extracted_before_the_definitive_tree_is_built() {
    let root = canonical(&scratch_dir("archive-extracts-before-tree"));
    write_archive(
        &root.join("content.bsa"),
        &[entry("textures/extracted.dds", b"extracted")],
    );
    write_tree(&root, &["textures/loose.dds"]);
    let toolset = Arc::new(real_archive_work());

    let result = execute(&archives_and_textures(&root), &toolset);

    assert_eq!(
        result.outcome(),
        RunOutcome::Succeeded,
        "{:?}",
        result.failures()
    );
    assert_eq!(extracted_archives(&result), [root.join("content.bsa")]);
    assert_eq!(
        std::fs::read(root.join("textures/extracted.dds")).unwrap(),
        b"extracted"
    );
    let mut paths = toolset.attempted();
    paths.sort();
    assert_eq!(
        paths,
        [
            root.join("textures").join("extracted.dds"),
            root.join("textures").join("loose.dds"),
        ]
    );
    // Backup source cleanup keeps the extracted source under a new name.
    assert!(!root.join("content.bsa").exists());
    assert!(
        backup_of(&root.join("content.bsa")).exists(),
        "the source Archive is backed up"
    );
    // Archive Finalization ran, with no Archive creation requested and no
    // empty directory left to prune.
    let finalization = result.archive_finalization().unwrap();
    assert!(finalization.attempts.is_empty());
    assert!(finalization.mutations.is_empty());
}

/// The Apply request over native Textures and Archive extraction for every
/// child Mod Root of `base`, which Preparing orders by name.
fn several_archives_and_textures(base: &Path) -> RunRequest {
    RunRequest::new(
        "SkyrimSE",
        ExecutionMode::Apply,
        ModSelection::ChildModRoots(base.to_path_buf()),
        vec![
            RequestedWork::NativeTextureOptimization,
            RequestedWork::ArchiveExtraction,
        ],
    )
}

/// A small valid Archive holding one Texture.
fn fixture_archive(path: &Path) {
    write_archive(path, &[entry("textures/fixture.dds", b"fixture")]);
}

/// A valid Archive whose one entry declares `size` decompressed bytes, so
/// Capacity Checks can be set with ample slack for overhead allowances.
fn sized_archive(path: &Path, size: u64) {
    write_archive(
        path,
        &[FakeEntry {
            name: "textures/fixture.dds",
            payload: b"fixture",
            declared_size: size,
        }],
    );
}

/// Asserts that the preflight failed the run on `path` with `code` and that
/// no Archive was extracted, nothing was routed, and the tree is unchanged.
fn assert_blocked_before_extraction(
    result: &OptimizationRunResult,
    toolset: &ScriptedToolset,
    code: RunFailureCode,
    path: &Path,
) {
    assert_eq!(result.outcome(), RunOutcome::Failed);
    assert_eq!(result.final_phase(), RunPhase::DiscoveringArchives);
    assert_eq!(result.failures().len(), 1, "{:?}", result.failures());
    let failure = &result.failures()[0];
    assert_eq!(failure.code, code, "{}", failure.detail);
    assert_eq!(failure.phase, RunPhase::DiscoveringArchives);
    assert_eq!(failure.path, path);
    assert!(!failure.detail.is_empty());
    assert!(
        result.archive_extraction_attempts().is_empty(),
        "no extraction starts"
    );
    assert!(
        result.archive_finalization().is_none(),
        "nor does Archive Finalization"
    );
    assert!(result.archive_collisions().is_empty());
    assert!(result.routing_ledger().is_none());
    assert!(toolset.attempted().is_empty());
    assert!(result.mutation_summaries().is_empty());
}

/// Origin: ArchiveFirstAssetDiscoveryTests::insufficientCapacityBlocksEntireBatch.
#[test]
fn a_capacity_shortage_in_a_later_root_blocks_every_extraction() {
    let base = canonical(&scratch_dir("archive-capacity-batch"));
    sized_archive(&base.join("first/source.bsa"), 1_000_000);
    sized_archive(&base.join("last/source.bsa"), 1_000_000);
    let before = snapshot_tree(&base);
    let last = base.join("last");
    // Either root alone fits; both together, on unknown volumes, do not.
    let toolset = Arc::new(with_capacity(move |root| {
        Some(if root == last { 1_500_000 } else { u64::MAX })
    }));

    let result = execute(&several_archives_and_textures(&base), &toolset);

    assert_blocked_before_extraction(
        &result,
        &toolset,
        RunFailureCode::ArchiveInsufficientCapacity,
        &base.join("last"),
    );
    assert_eq!(snapshot_tree(&base), before);
}

/// Origin: ArchiveFirstAssetDiscoveryTests::capacityIsGroupedByVolume.
#[test]
fn independent_volumes_each_need_only_their_own_estimate() {
    let base = canonical(&scratch_dir("archive-capacity-volumes"));
    sized_archive(&base.join("first/source.bsa"), 1_000_000);
    sized_archive(&base.join("last/source.bsa"), 1_000_000);
    let first = base.join("first");
    let toolset = Arc::new(ScriptedToolset {
        volumes: Arc::new(FakeVolumes(Some(Box::new(move |root| {
            Some(
                if root == first {
                    "first-volume"
                } else {
                    "last-volume"
                }
                .to_owned(),
            )
        })))),
        ..with_capacity(|_| Some(1_500_000))
    });

    let result = execute(&several_archives_and_textures(&base), &toolset);

    assert!(result.failures().is_empty(), "{:?}", result.failures());
    assert_eq!(result.archive_extraction_attempts().len(), 2);
}

/// Origin: ArchiveFirstAssetDiscoveryTests::unknownVolumeWithWorkRetainsBatchCapacity.
#[test]
fn an_unidentified_volume_with_work_keeps_the_whole_batch_estimate() {
    let base = canonical(&scratch_dir("archive-capacity-unknown-volume"));
    sized_archive(&base.join("first/source.bsa"), 1_000_000);
    sized_archive(&base.join("last/source.bsa"), 1_000_000);
    let first = base.join("first");
    let toolset = Arc::new(ScriptedToolset {
        volumes: Arc::new(FakeVolumes(Some(Box::new(move |root| {
            (root == first).then(|| "known-volume".to_owned())
        })))),
        ..with_capacity(|_| Some(1_500_000))
    });

    let result = execute(&several_archives_and_textures(&base), &toolset);

    assert_eq!(result.outcome(), RunOutcome::Failed);
    assert_eq!(result.failures().len(), 1);
    assert_eq!(
        result.failures()[0].code,
        RunFailureCode::ArchiveInsufficientCapacity
    );
    assert!(result.archive_extraction_attempts().is_empty());
}

/// Origin: ArchiveFirstAssetDiscoveryTests::compressedShadowedEntryRequiresCapacity.
/// A compressed entry a Loose Asset shadows still needs its full decompressed
/// size staged.
#[test]
fn a_compressed_shadowed_entry_still_needs_its_decompressed_capacity() {
    let root = canonical(&scratch_dir("archive-capacity-shadowed"));
    write_archive(
        &root.join("source.bsa"),
        &[FakeEntry {
            name: "textures/large.dds",
            payload: b"small on disk",
            declared_size: 512 * 1024,
        }],
    );
    std::fs::create_dir_all(root.join("textures")).unwrap();
    std::fs::write(root.join("textures/large.dds"), "loose winner").unwrap();
    let toolset = Arc::new(with_capacity(|_| Some(400_000)));

    let result = execute(&archives_and_textures(&root), &toolset);

    assert_blocked_before_extraction(
        &result,
        &toolset,
        RunFailureCode::ArchiveInsufficientCapacity,
        &root,
    );
    assert_eq!(
        std::fs::read(root.join("textures/large.dds")).unwrap(),
        b"loose winner"
    );
    assert!(!root.join(".cao-staging").exists());
}

/// Origin: ArchiveFirstAssetDiscoveryTests::windowsDeviceNamesBlockBatch (all
/// five rows), windowsControlCharactersBlockBatch (both rows) and the
/// escaping row of rawManifestPaths. An Unsafe Game Path in a later root
/// stops every extraction, including the earlier root's valid Archive.
#[test]
fn an_unsafe_game_path_in_a_later_archive_blocks_the_whole_batch() {
    for (index, name) in [
        "textures/con.dds",
        "aux/file.dds",
        "textures/COM1.dds",
        "textures/COM\u{B9}.dds",
        "textures/conin$.dds",
        "textures/bad\u{1}.dds",
        "bad\u{1F}/file.dds",
        "..\\escaped.dds",
        "textures/trailing.dds.",
        ".cao-staging/smuggled.dds",
    ]
    .into_iter()
    .enumerate()
    {
        let base = canonical(&scratch_dir(&format!("archive-unsafe-{index}")));
        fixture_archive(&base.join("first/source.bsa"));
        write_archive(&base.join("last/source.bsa"), &[entry(name, b"B")]);
        let before = snapshot_tree(&base);
        let toolset = Arc::new(real_archive_work());

        let result = execute(&several_archives_and_textures(&base), &toolset);

        assert_blocked_before_extraction(
            &result,
            &toolset,
            RunFailureCode::ArchiveEntryInvalid,
            &base.join("last/source.bsa"),
        );
        assert_eq!(snapshot_tree(&base), before, "{name:?}");
        assert!(!base.join("escaped.dds").exists());
    }
}

/// Rust-only, deviation 15: device-name detection trims trailing spaces and
/// dots from the stem, so `NUL .txt` is an Unsafe Game Path. C++ compared the
/// untrimmed stem `NUL ` and let it through, though Windows opens the NUL
/// device for that name.
#[test]
fn deviation_15_a_device_stem_with_trailing_spaces_is_an_unsafe_game_path() {
    for (index, name) in ["textures/NUL .txt", "textures/com1 .dds", "Con  .dds"]
        .into_iter()
        .enumerate()
    {
        let root = canonical(&scratch_dir(&format!("archive-deviation-15-{index}")));
        write_archive(&root.join("source.bsa"), &[entry(name, b"B")]);
        let toolset = Arc::new(real_archive_work());

        let result = execute(&archives_and_textures(&root), &toolset);

        assert_blocked_before_extraction(
            &result,
            &toolset,
            RunFailureCode::ArchiveEntryInvalid,
            &root.join("source.bsa"),
        );
    }
}

/// Origin: ArchiveFirstAssetDiscoveryTests::sameArchiveAliasesBlockBatch
/// (exact and case-folded duplicates).
#[test]
fn aliased_entries_within_one_archive_block_the_batch() {
    for (index, alias) in ["textures/a.dds", "textures/A.dds"].into_iter().enumerate() {
        let base = canonical(&scratch_dir(&format!("archive-aliases-{index}")));
        fixture_archive(&base.join("first/source.bsa"));
        write_archive(
            &base.join("last/source.bsa"),
            &[entry("textures/a.dds", b"A"), entry(alias, b"B")],
        );
        let toolset = Arc::new(real_archive_work());

        let result = execute(&several_archives_and_textures(&base), &toolset);

        assert_blocked_before_extraction(
            &result,
            &toolset,
            RunFailureCode::ArchiveEntryInvalid,
            &base.join("last/source.bsa"),
        );
        assert!(!base.join("first/textures").exists());
    }
}

/// Origin: ArchiveFirstAssetDiscoveryTests::occupiedArchiveDestinationBlocksBatch
/// (a directory at the destination, and a regular file as its parent).
#[test]
fn a_destination_occupied_by_a_non_file_blocks_the_batch() {
    for parent_file in [false, true] {
        let base = canonical(&scratch_dir(&format!("archive-occupied-{parent_file}")));
        fixture_archive(&base.join("first/source.bsa"));
        write_archive(
            &base.join("last/source.bsa"),
            &[entry("textures/a.dds", b"B")],
        );
        if parent_file {
            std::fs::write(base.join("last/textures"), "occupied parent").unwrap();
        } else {
            std::fs::create_dir_all(base.join("last/textures/a.dds")).unwrap();
        }
        let toolset = Arc::new(real_archive_work());

        let result = execute(&several_archives_and_textures(&base), &toolset);

        assert_blocked_before_extraction(
            &result,
            &toolset,
            RunFailureCode::ArchiveEntryInvalid,
            &base.join("last/source.bsa"),
        );
    }
}

/// Origin: ArchiveFirstAssetDiscoveryTests::plannedArchiveParentConflictBlocksBatch
/// (file before child, and child before file).
#[test]
fn two_winners_cannot_plan_a_file_and_its_child() {
    for child_first in [false, true] {
        let root = canonical(&scratch_dir(&format!(
            "archive-parent-conflict-{child_first}"
        )));
        let (first, second) = if child_first {
            ("textures/a.dds", "textures")
        } else {
            ("textures", "textures/a.dds")
        };
        write_archive(&root.join("a.bsa"), &[entry(first, b"A")]);
        write_archive(&root.join("b.bsa"), &[entry(second, b"B")]);
        let toolset = Arc::new(real_archive_work());

        let result = execute(&archives_and_textures(&root), &toolset);

        assert_blocked_before_extraction(
            &result,
            &toolset,
            RunFailureCode::ArchiveEntryInvalid,
            &root.join("b.bsa"),
        );
        assert!(!root.join("textures").exists());
    }
}

/// Origin: ArchiveFirstAssetDiscoveryTests::junctionArchiveParentBlocksBatch.
#[test]
fn a_junction_destination_parent_blocks_the_batch() {
    let base = canonical(&scratch_dir("archive-junction-parent"));
    fixture_archive(&base.join("first/source.bsa"));
    write_archive(
        &base.join("last/source.bsa"),
        &[entry("textures/a.dds", b"B")],
    );
    let target = base.join("last").join("target");
    std::fs::create_dir_all(&target).unwrap();
    // `mklink` rejects `/`, so the paths are joined one component at a time.
    let link = base.join("last").join("textures");
    junction(&link, &target);
    let toolset = Arc::new(real_archive_work());

    let result = execute(&several_archives_and_textures(&base), &toolset);
    std::fs::remove_dir(&link).unwrap();

    assert_blocked_before_extraction(
        &result,
        &toolset,
        RunFailureCode::ArchiveEntryInvalid,
        &base.join("last/source.bsa"),
    );
    assert!(!base.join("last/target/a.dds").exists());
}

/// Origin: ArchiveFirstAssetDiscoveryTests::unreadableArchiveStopsExtraction,
/// AssetRunTests::unreadableArchiveStopsRunBeforeMutation and the fatal
/// Archive preflight row of RunExecutorTests::productionWorkApplicability.
#[test]
fn an_unreadable_archive_fails_the_run_before_any_mutation() {
    let root = canonical(&scratch_dir("archive-unreadable"));
    write_tree(&root, &["textures/loose.dds"]);
    std::fs::write(root.join("broken.bsa"), "not an archive").unwrap();
    let before = snapshot_tree(&root);
    let toolset = Arc::new(real_archive_work());

    let result = execute(&archives_and_textures(&root), &toolset);

    assert_blocked_before_extraction(
        &result,
        &toolset,
        RunFailureCode::ArchiveUnreadable,
        &root.join("broken.bsa"),
    );
    assert!(result.archive_finalization().is_none());
    assert!(result.phase(RunPhase::ProcessingAssets).is_none());
    assert!(result.phase(RunPhase::ArchiveFinalization).is_none());
    assert!(!result.cancellation_observed());
    assert_eq!(snapshot_tree(&root), before);
}

/// Origin: ArchiveFirstAssetDiscoveryTests::lateUnreadableRootBlocksEveryExtraction.
#[test]
fn an_unreadable_archive_in_a_later_root_blocks_every_extraction() {
    let base = canonical(&scratch_dir("archive-late-unreadable"));
    write_archive(
        &base.join("first/content.bsa"),
        &[entry("textures/shared.dds", b"A")],
    );
    std::fs::create_dir_all(base.join("second")).unwrap();
    std::fs::write(base.join("second/broken.bsa"), "unreadable").unwrap();
    let toolset = Arc::new(real_archive_work());

    let result = execute(&several_archives_and_textures(&base), &toolset);

    assert_blocked_before_extraction(
        &result,
        &toolset,
        RunFailureCode::ArchiveUnreadable,
        &base.join("second/broken.bsa"),
    );
    assert!(!base.join("first/textures").exists());
}

/// Origin: ArchiveFirstAssetDiscoveryTests::invalidExplicitOrder (missing,
/// extra, duplicate and outside-root rows).
#[test]
fn invalid_explicit_archive_precedence_fails_before_extraction() {
    for (index, (order, code)) in [
        (vec![], RunFailureCode::ArchiveOrderMissing),
        (
            vec!["content.bsa", "other.bsa"],
            RunFailureCode::ArchiveOrderExtra,
        ),
        (
            vec!["content.bsa", "./content.bsa"],
            RunFailureCode::ArchiveOrderDuplicate,
        ),
        (
            vec!["../content.bsa"],
            RunFailureCode::ArchiveOrderOutsideRoot,
        ),
        // Normalizes to the root itself, which is contained but no Archive.
        (vec!["textures/.."], RunFailureCode::ArchiveOrderExtra),
    ]
    .into_iter()
    .enumerate()
    {
        let root = canonical(&scratch_dir(&format!("archive-invalid-order-{index}")));
        write_archive(
            &root.join("content.bsa"),
            &[entry("textures/shared.dds", b"archived")],
        );
        let toolset = Arc::new(real_archive_work());
        let precedence =
            ArchivePrecedence::ExplicitOrder(order.iter().map(PathBuf::from).collect());

        let result = execute(
            &archives_and_textures(&root).with_archive_precedence(precedence),
            &toolset,
        );

        assert_eq!(result.outcome(), RunOutcome::Failed);
        assert_eq!(result.failures().len(), 1);
        assert_eq!(result.failures()[0].code, code, "{order:?}");
        assert!(result.archive_extraction_attempts().is_empty());
        assert!(result.routing_ledger().is_none());
        assert!(
            no_manifest_listed(&toolset),
            "precedence is validated before any manifest is read"
        );
    }
}

/// Origin: ArchiveFirstAssetDiscoveryTests::collisionsDoNotCrossModRoots.
#[test]
fn collisions_never_cross_mod_roots() {
    let base = canonical(&scratch_dir("archive-collisions-per-root"));
    for root in ["first", "second"] {
        write_archive(
            &base.join(root).join("content.bsa"),
            &[entry("textures/shared.dds", root.as_bytes())],
        );
    }
    let toolset = Arc::new(real_archive_work());

    let result = execute(&several_archives_and_textures(&base), &toolset);

    assert!(result.failures().is_empty(), "{:?}", result.failures());
    assert!(result.archive_collisions().is_empty());
    assert_eq!(result.archive_extraction_attempts().len(), 2);
    assert_eq!(
        std::fs::read(base.join("second/textures/shared.dds")).unwrap(),
        b"second"
    );
}

/// The one extraction attempt a run made, by Archive file name.
fn attempt_for<'r>(
    result: &'r OptimizationRunResult,
    name: &str,
) -> &'r cao_core::run::ArchiveExtractionResult {
    result
        .archive_extraction_attempts()
        .iter()
        .find(|attempt| attempt.archive_path.file_name().unwrap() == name)
        .unwrap_or_else(|| panic!("no attempt for {name}"))
}

/// Origin: ArchiveFirstAssetDiscoveryTests::collisionsAreReportedBeforeExtraction
/// (deterministic, explicit, loose-over-deterministic and loose-over-explicit
/// rows). Every shadowed Archive is reported, high to low, before the first
/// extraction, and Loose Assets outrank every Archive. The plan is retained
/// before the Archive extraction phase is recorded, so no entry may be
/// extracted before an observer has seen that record.
#[test]
fn collisions_are_recorded_and_reported_before_the_first_extraction() {
    for (explicit, loose) in [(false, false), (true, false), (false, true), (true, true)] {
        let root = canonical(&scratch_dir(&format!(
            "archive-collisions-{explicit}-{loose}"
        )));
        for name in ["a", "b", "c"] {
            write_archive(
                &root.join(format!("{name}.bsa")),
                &[entry("textures/shared.dds", name.as_bytes())],
            );
        }
        if loose {
            std::fs::create_dir_all(root.join("textures")).unwrap();
            std::fs::write(root.join("textures/shared.dds"), "loose").unwrap();
        }
        let reported = Arc::new(AtomicBool::new(false));
        let extracted_before_report = Arc::new(AtomicBool::new(false));
        let (seen, violated) = (Arc::clone(&reported), Arc::clone(&extracted_before_report));
        let toolset = on_extract(move |_, _| {
            if !seen.load(Ordering::SeqCst) {
                violated.store(true, Ordering::SeqCst);
            }
        });
        let flag = Arc::clone(&reported);
        let sink = RecordingSink::on_first_record_of(RunPhase::ExtractingArchives, move || {
            flag.store(true, Ordering::SeqCst)
        });
        let precedence = if explicit {
            ArchivePrecedence::ExplicitOrder(vec!["c.bsa".into(), "a.bsa".into(), "b.bsa".into()])
        } else {
            ArchivePrecedence::DeterministicDiscovery
        };

        let result = execute_with(
            &archives_and_textures(&root).with_archive_precedence(precedence),
            &toolset,
            Some(&sink),
            &CancellationToken::new(),
        );

        assert!(result.failures().is_empty(), "{:?}", result.failures());
        assert!(reported.load(Ordering::SeqCst));
        assert!(!extracted_before_report.load(Ordering::SeqCst));
        let [collision] = result.archive_collisions() else {
            panic!("one collision expected: {:?}", result.archive_collisions());
        };
        let (winner, shadowed) = if explicit {
            ("c.bsa", ["a.bsa", "b.bsa"])
        } else {
            ("a.bsa", ["b.bsa", "c.bsa"])
        };
        assert_eq!(collision.mod_root, root);
        assert_eq!(collision.game_path, Path::new("textures/shared.dds"));
        assert_eq!(collision.winning_archive, root.join(winner));
        assert_eq!(
            collision.shadowed_archives,
            shadowed.map(|name| root.join(name))
        );
        assert_eq!(collision.loose_asset_wins, loose);
        let extracted: Vec<_> = extracted_archives(&result);
        assert_eq!(extracted[0], root.join(winner), "the winner extracts first");
        // A Loose Asset's precedence is planned, not left to a failed publish.
        assert!(
            result
                .archive_extraction_attempts()
                .iter()
                .all(|attempt| attempt.succeeded()),
            "{:?}",
            result.archive_extraction_attempts()
        );
        let expected = if loose { "loose" } else { &winner[..1] };
        assert_eq!(
            std::fs::read(root.join("textures/shared.dds")).unwrap(),
            expected.as_bytes()
        );
    }
}

/// Origin: ArchiveFirstAssetDiscoveryTests::rawManifestPaths (contained rows).
/// A collision keeps the winning Archive's spelling of the game path, after
/// normalizing separators, `.` and `..` in the loser's.
#[test]
fn a_collision_keeps_the_winning_archives_spelling() {
    let root = canonical(&scratch_dir("archive-collision-spelling"));
    write_archive(&root.join("a.bsa"), &[entry("Textures/Shared.dds", b"A")]);
    write_archive(
        &root.join("b.bsa"),
        &[entry("TEXTURES\\folder\\..\\.\\SHARED.DDS", b"B")],
    );
    let toolset = Arc::new(real_archive_work());

    let result = execute(&archives_and_textures(&root), &toolset);

    assert!(result.failures().is_empty(), "{:?}", result.failures());
    assert_eq!(result.archive_extraction_attempts().len(), 2);
    let [collision] = result.archive_collisions() else {
        panic!("one collision expected");
    };
    assert_eq!(collision.game_path, Path::new("Textures/Shared.dds"));
    assert_eq!(collision.winning_archive, root.join("a.bsa"));
    assert_eq!(collision.shadowed_archives, [root.join("b.bsa")]);
    assert_eq!(
        std::fs::read(root.join("Textures/Shared.dds")).unwrap(),
        b"A"
    );
}

/// Origin: ArchiveFirstAssetDiscoveryTests::archivesAreOrderedWithinEachModRoot.
/// Within each Mod Root, Archives order by case-folded relative name with the
/// original spelling breaking ties; the Mod Roots keep their own run order.
#[test]
fn archives_are_ordered_by_folded_name_within_each_mod_root() {
    let base = canonical(&scratch_dir("archive-ordering"));
    let names = [
        "alpha.bsa",
        "alpha/z.bsa",
        "Beta.bsa",
        "STRASSE.bsa",
        "Stra\u{DF}e.bsa",
        "z.bsa",
    ];
    for root in ["a-mod", "z-mod"] {
        for name in names.iter().rev() {
            fixture_archive(&base.join(root).join(name));
        }
    }
    let toolset = Arc::new(real_archive_work());

    let result = execute(&several_archives_and_textures(&base), &toolset);

    let observed: Vec<_> = extracted_archives(&result);
    let expected: Vec<_> = ["a-mod", "z-mod"]
        .into_iter()
        .flat_map(|root| names.map(|name| base.join(root).join(name)))
        .collect();
    assert_eq!(observed, expected);
    // Several Archives provide the same Texture, so only collisions are reported.
    assert!(result.failures().is_empty(), "{:?}", result.failures());
}

/// Origin: ArchiveFirstAssetDiscoveryTests::dryRunIgnoresPrecedenceAndManifests.
#[test]
fn a_dry_run_ignores_precedence_and_never_reads_a_manifest() {
    let root = canonical(&scratch_dir("archive-dry-run"));
    std::fs::write(root.join("broken.bsa"), "invalid manifest").unwrap();
    let toolset = Arc::new(real_archive_work());
    let precedence =
        ArchivePrecedence::ExplicitOrder(vec!["../missing.bsa".into(), "../missing.bsa".into()]);

    let result = execute(
        &request(
            ExecutionMode::DryRun,
            &root,
            &[RequestedWork::ArchiveExtraction],
        )
        .with_archive_precedence(precedence),
        &toolset,
    );

    assert_eq!(
        result.outcome(),
        RunOutcome::Succeeded,
        "{:?}",
        result.failures()
    );
    assert!(
        result.archive_collisions().is_empty(),
        "Dry Run retains no collisions"
    );
    assert!(result.archive_extraction_attempts().is_empty());
    assert!(
        no_manifest_listed(&toolset),
        "Dry Run never reads a manifest"
    );
    assert_eq!(
        result.skipped_asset_count(cao_core::routing::SkipReason::DisabledPhase),
        1
    );
    assert_eq!(
        std::fs::read(root.join("broken.bsa")).unwrap(),
        b"invalid manifest"
    );
    assert!(!root.join(".cao-staging").exists());
}

/// Origin: ArchiveFirstAssetDiscoveryTests::collisionObserverCanCancelBeforeExtraction.
/// The collision plan is known once the Archive extraction phase is
/// recorded; an observer cancelling there stops the run before the first
/// extraction.
#[test]
fn a_collision_observer_can_cancel_before_any_extraction() {
    let root = canonical(&scratch_dir("archive-collision-cancels"));
    for name in ["a.bsa", "b.bsa"] {
        write_archive(&root.join(name), &[entry("textures/shared.dds", b"B")]);
    }
    let toolset = Arc::new(real_archive_work());
    let stop = CancellationToken::new();
    let raise = stop.clone();
    let sink =
        RecordingSink::on_first_record_of(RunPhase::ExtractingArchives, move || raise.cancel());

    let result = execute_with(&archives_and_textures(&root), &toolset, Some(&sink), &stop);

    assert_eq!(result.outcome(), RunOutcome::Cancelled);
    assert!(result.failures().is_empty());
    assert_eq!(
        result.archive_collisions().len(),
        1,
        "the plan is still retained"
    );
    assert!(result.archive_extraction_attempts().is_empty());
    assert!(result.routing_ledger().is_none());
    assert!(!root.join("textures").exists());
}

/// Origin: ArchiveFirstAssetDiscoveryTests::explicitArchiveAliasUsesResolvedScope.
/// A Rust Mod Selection is always a directory, so the alias here is the
/// selected Mod Root itself; explicit precedence still names the Archive
/// relative to the resolved root.
#[test]
fn explicit_precedence_resolves_against_the_mod_roots_resolved_target() {
    let base = canonical(&scratch_dir("archive-alias-scope"));
    let root = base.join("mod");
    write_archive(&root.join("a.bsa"), &[entry("textures/shared.dds", b"B")]);
    let alias = base.join("alias");
    junction(&alias, &root);
    let toolset = Arc::new(real_archive_work());

    let result = execute(
        &archives_and_textures(&alias)
            .with_archive_precedence(ArchivePrecedence::ExplicitOrder(vec!["a.bsa".into()])),
        &toolset,
    );
    std::fs::remove_dir(&alias).unwrap();

    assert!(result.failures().is_empty(), "{:?}", result.failures());
    let extracted: Vec<_> = extracted_archives(&result);
    assert_eq!(extracted, [root.join("a.bsa")]);
}

/// Origin: ArchiveFirstAssetDiscoveryTests::unicodeEntrySpellingSurvivesExtraction.
#[test]
fn extraction_keeps_an_entrys_unicode_spelling() {
    let root = canonical(&scratch_dir("archive-unicode-spelling"));
    write_archive(
        &root.join("source.bsa"),
        &[entry("Textures/Stra\u{DF}e.DDS", b"B")],
    );
    let toolset = Arc::new(real_archive_work());

    let result = execute(&archives_and_textures(&root), &toolset);

    assert!(result.failures().is_empty(), "{:?}", result.failures());
    let published = root.join("Textures").join("Stra\u{DF}e.DDS");
    let names: Vec<_> = std::fs::read_dir(root.join("Textures"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names, ["Stra\u{DF}e.DDS"]);
    assert_eq!(std::fs::read(&published).unwrap(), b"B");
    assert_eq!(toolset.attempted(), [published]);
}

/// Origin: ArchiveFirstAssetDiscoveryTests::distinctWindowsNamesBothPublish.
/// `ß` and `ss` are distinct Windows names, so neither shadows the other.
#[test]
fn windows_distinct_sharp_s_and_ss_entries_both_publish() {
    let root = canonical(&scratch_dir("archive-sharp-s"));
    write_archive(
        &root.join("source.bsa"),
        &[
            entry("textures/strasse.dds", b"A"),
            entry("textures/stra\u{DF}e.dds", b"B"),
        ],
    );
    let toolset = Arc::new(real_archive_work());

    let result = execute(&archives_and_textures(&root), &toolset);

    assert!(result.failures().is_empty(), "{:?}", result.failures());
    assert!(result.archive_collisions().is_empty());
    assert_eq!(
        std::fs::read_dir(root.join("textures")).unwrap().count(),
        2,
        "both entries were merged"
    );
    assert_eq!(
        std::fs::read(root.join("textures/strasse.dds")).unwrap(),
        b"A"
    );
    assert_eq!(
        std::fs::read(root.join("textures").join("stra\u{DF}e.dds")).unwrap(),
        b"B"
    );
    assert_eq!(toolset.attempted().len(), 2);
}

/// Origin: ArchiveFirstAssetDiscoveryTests::realExtractionPreservesLooseAssetPrecedence
/// and AssetRunTests::realExtractionPreservesLooseAssetPrecedence.
#[test]
fn extraction_adds_archived_assets_without_replacing_a_loose_asset() {
    let root = canonical(&scratch_dir("archive-loose-precedence"));
    write_archive(
        &root.join("content.bsa"),
        &[
            entry("textures/collision.dds", b"archived collision"),
            entry("textures/archived-only.dds", b"archived only"),
        ],
    );
    std::fs::create_dir_all(root.join("textures")).unwrap();
    std::fs::write(root.join("textures/collision.dds"), "loose collision").unwrap();
    let toolset = Arc::new(real_archive_work());

    let result = execute(&archives_and_textures(&root), &toolset);

    assert_eq!(
        result.outcome(),
        RunOutcome::Succeeded,
        "{:?}",
        result.failures()
    );
    assert_eq!(
        std::fs::read(root.join("textures/collision.dds")).unwrap(),
        b"loose collision"
    );
    assert_eq!(
        std::fs::read(root.join("textures/archived-only.dds")).unwrap(),
        b"archived only"
    );
    // Only the unshadowed entry is merged: publishing the shadowed one over
    // the Loose Asset would have failed the attempt.
    let attempt = attempt_for(&result, "content.bsa");
    assert!(attempt.succeeded(), "{}", attempt.detail);
    assert_eq!(std::fs::read_dir(root.join("textures")).unwrap().count(), 2);
    assert_eq!(toolset.attempted().len(), 2);
    let summary = &result.mutation_summaries()[0];
    assert_eq!(summary.kind, cao_core::run::MutationKind::ArchiveExtraction);
    assert_eq!((summary.committed, summary.partial_or_unknown), (1, 0));
}

/// Origin: ArchiveFirstAssetDiscoveryTests::lateLooseAssetBlocksArchiveMerge.
/// A Loose Asset created after preflight wins the no-replace publication, so
/// the attempt fails without any durable change and the run continues.
#[test]
fn a_loose_asset_created_after_preflight_blocks_the_merge() {
    let root = canonical(&scratch_dir("archive-late-loose"));
    write_archive(&root.join("source.bsa"), &[entry("textures/a.dds", b"B")]);
    let loose = root.join("Textures").join("A.DDS");
    let late = loose.clone();
    let toolset = Arc::new(real_archive_work());
    let sink = RecordingSink::on_first_record_of(RunPhase::ExtractingArchives, move || {
        std::fs::create_dir_all(late.parent().unwrap()).unwrap();
        std::fs::write(&late, "late loose").unwrap();
    });

    let result = execute_with(
        &archives_and_textures(&root),
        &toolset,
        Some(&sink),
        &CancellationToken::new(),
    );

    assert_eq!(result.outcome(), RunOutcome::CompletedWithFailures);
    // The entry was planned for merging at preflight, so its no-replace
    // publication is what fails.
    let attempt = attempt_for(&result, "source.bsa");
    assert_eq!(
        attempt.failure,
        Some(cao_core::run::ArchiveExtractionFailure::MergeFailed)
    );
    assert_eq!(attempt.mutation, cao_core::execution::MutationState::None);
    assert!(attempt.safe_to_continue);
    assert_eq!(std::fs::read(&loose).unwrap(), b"late loose");
    assert!(root.join("source.bsa").exists());
    assert!(result.cleanup_failures().is_empty());
}

/// Origin: ArchiveFirstAssetDiscoveryTests::frozenPrecedenceSurvivesFailures
/// (failed winning Archive, and disappeared Loose Asset). Winners are frozen
/// at preflight: a failed winner never promotes the shadowed Archive, and a
/// Loose Asset removed later keeps its precedence.
#[test]
fn frozen_precedence_survives_a_failed_winner_or_a_vanished_loose_asset() {
    for loose in [false, true] {
        let root = canonical(&scratch_dir(&format!("archive-frozen-{loose}")));
        write_archive(&root.join("a.bsa"), &[entry("textures/a.dds", b"A")]);
        write_archive(&root.join("b.bsa"), &[entry("textures/a.dds", b"B")]);
        if loose {
            write_tree(&root, &["textures/a.dds"]);
        }
        let (archive, asset) = (root.join("a.bsa"), root.join("textures/a.dds"));
        let toolset = Arc::new(real_archive_work());
        let sink = RecordingSink::on_first_record_of(RunPhase::ExtractingArchives, move || {
            if loose {
                std::fs::remove_file(&asset).unwrap();
            } else {
                std::fs::write(&archive, "corrupted after preflight").unwrap();
            }
        });

        let result = execute_with(
            &archives_and_textures(&root),
            &toolset,
            Some(&sink),
            &CancellationToken::new(),
        );

        assert!(result.failures().is_empty(), "{:?}", result.failures());
        let attempts = result.archive_extraction_attempts();
        assert_eq!(attempts.len(), 2);
        assert_eq!(attempts[0].succeeded(), loose, "{}", attempts[0].detail);
        assert!(attempts[0].safe_to_continue);
        assert!(attempts[1].succeeded(), "{}", attempts[1].detail);
        // The shadowed Archive merges nothing, but its successful extraction
        // still backs up its source, which is its Committed Mutation.
        assert_eq!(
            attempts[1].mutation,
            cao_core::execution::MutationState::Committed
        );
        assert!(!root.join("textures/a.dds").exists());
        // Removing the Loose Asset left `textures` empty, so Archive
        // Finalization prunes it; without one, `textures` never existed.
        assert!(!root.join("textures").exists());
        let pruned: Vec<_> = result
            .archive_finalization()
            .unwrap()
            .mutations
            .iter()
            .map(|mutation| (mutation.kind, mutation.mod_root.clone(), mutation.count))
            .collect();
        let expected_pruning = if loose {
            vec![(
                ArchiveFinalizationMutationKind::EmptyDirectoryPruning,
                root.clone(),
                1,
            )]
        } else {
            Vec::new()
        };
        assert_eq!(pruned, expected_pruning);
        assert_eq!(
            result
                .mutation_summaries()
                .iter()
                .any(|summary| summary.kind == MutationKind::ArchiveFinalization),
            loose
        );
        assert!(backup_of(&root.join("b.bsa")).exists());
        // A failed winner keeps its source; a winner shadowed by the vanished
        // Loose Asset merged nothing and is backed up.
        assert_eq!(root.join("a.bsa").exists(), !loose);
        assert_eq!(backup_of(&root.join("a.bsa")).exists(), loose);
    }
}

/// Origin: ArchiveFirstAssetDiscoveryTests::laterArchivesRepeatContainmentAfterSafeRejection.
/// A junction inserted after preflight is rejected by each attempt's own
/// containment checks, safely, and an unaffected later Archive still merges.
#[test]
fn every_attempt_repeats_containment_after_a_safe_rejection() {
    let base = canonical(&scratch_dir("archive-repeat-containment"));
    let root = base.join("mod");
    let outside = base.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    write_archive(&root.join("a.bsa"), &[entry("textures/a.dds", b"A")]);
    write_archive(&root.join("b.bsa"), &[entry("textures/b.dds", b"B")]);
    write_archive(&root.join("c.bsa"), &[entry("meshes/c.dds", b"C")]);
    let link = root.join("textures");
    let (linked, target) = (link.clone(), outside.clone());
    let toolset = Arc::new(real_archive_work());
    let sink = RecordingSink::with_hook(move |observed| {
        let Observed::Phase(record) = observed else {
            return;
        };
        match record.phase() {
            RunPhase::ExtractingArchives if !linked.exists() => junction(&linked, &target),
            // Remove only the link, never its independent target.
            RunPhase::BuildingEffectiveAssetTree if linked.exists() => {
                std::fs::remove_dir(&linked).unwrap()
            }
            _ => {}
        }
    });

    let result = execute_with(
        &archives_and_textures(&root),
        &toolset,
        Some(&sink),
        &CancellationToken::new(),
    );

    assert!(!link.exists());
    assert!(result.failures().is_empty(), "{:?}", result.failures());
    assert_eq!(result.archive_extraction_attempts().len(), 3);
    for name in ["a.bsa", "b.bsa"] {
        let attempt = attempt_for(&result, name);
        assert_eq!(
            attempt.failure,
            Some(cao_core::run::ArchiveExtractionFailure::MergeFailed),
            "{name}"
        );
        assert_eq!(attempt.mutation, cao_core::execution::MutationState::None);
        assert!(attempt.safe_to_continue);
    }
    let merged = attempt_for(&result, "c.bsa");
    assert!(merged.succeeded(), "{}", merged.detail);
    assert_eq!(
        merged.mutation,
        cao_core::execution::MutationState::Committed
    );
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
    assert_eq!(std::fs::read(root.join("meshes/c.dds")).unwrap(), b"C");
    // The rejected Archives keep their sources; the merged one is backed up.
    for name in ["a.bsa", "b.bsa"] {
        assert!(root.join(name).exists());
    }
    assert!(backup_of(&root.join("c.bsa")).exists());
    assert_eq!(result.outcome(), RunOutcome::CompletedWithFailures);
}

/// Origin: ArchiveFirstAssetDiscoveryTests::excludedArchivesAreNotExtracted.
#[test]
fn an_archive_excluded_by_policy_is_never_read_or_extracted() {
    let root = canonical(&scratch_dir("archive-excluded"));
    std::fs::write(root.join("disabled.bsa"), "archive placeholder").unwrap();
    write_tree(&root, &["textures/loose.dds"]);
    let toolset = Arc::new(real_archive_work());

    let result = execute(
        &request(
            ExecutionMode::Apply,
            &root,
            &[RequestedWork::NativeTextureOptimization],
        ),
        &toolset,
    );

    assert_eq!(
        result.outcome(),
        RunOutcome::Succeeded,
        "{:?}",
        result.failures()
    );
    assert!(result.archive_extraction_attempts().is_empty());
    assert!(no_manifest_listed(&toolset));
    assert_eq!(
        toolset.attempted(),
        [root.join("textures").join("loose.dds")]
    );
    // Policy accounted for it as a Skip Reason, not as malformed nesting.
    let discovery = result.evidence().archive_discovery().unwrap();
    assert_eq!(discovery.nested_archive_count, 0);
}

/// Origin: ArchiveFirstAssetDiscoveryTests::archivesProducedByExtractionStayOutOfTheTree
/// and AssetRunTests::nestedArchivesAreReportedWithoutInflatingTheWorkTotal.
#[test]
fn an_archive_produced_by_extraction_is_counted_but_never_worked() {
    let root = canonical(&scratch_dir("archive-nested"));
    write_archive(
        &root.join("content.bsa"),
        &[
            entry("textures/nested.bsa", b"nested archive placeholder"),
            entry("textures/extracted.dds", b"extracted"),
        ],
    );
    let toolset = Arc::new(real_archive_work());

    let result = execute(&archives_and_textures(&root), &toolset);

    assert_eq!(
        result.outcome(),
        RunOutcome::Succeeded,
        "{:?}",
        result.failures()
    );
    assert_eq!(
        result.archive_extraction_attempts().len(),
        1,
        "nesting is never extracted"
    );
    assert!(root.join("textures/nested.bsa").exists());
    assert_eq!(
        toolset.attempted(),
        [root.join("textures").join("extracted.dds")]
    );
    assert_eq!(result.routing_ledger().unwrap().routed_assets().len(), 1);
    let discovery = result.evidence().archive_discovery().unwrap();
    assert_eq!(discovery.nested_archive_count, 1);
}

/// Origin: ArchiveFirstAssetDiscoveryTests::extractionProducedLinksAreExcluded.
/// A junction stands in for C++'s file symlink: both are links the
/// definitive pass must not follow, and a junction needs no privilege.
#[test]
fn a_link_produced_by_extraction_is_excluded_from_the_definitive_tree() {
    let base = canonical(&scratch_dir("archive-produced-link"));
    let root = base.join("mod");
    let outside = base.join("outside");
    write_tree(&outside, &["outside.dds"]);
    fixture_archive(&root.join("content.bsa"));
    let link = root.join("extracted");
    let (linked, target) = (link.clone(), outside.clone());
    let toolset = on_extract(move |_, _| {
        if !linked.exists() {
            junction(&linked, &target);
        }
    });

    let result = execute(&archives_and_textures(&root), &toolset);
    std::fs::remove_dir(&link).unwrap();

    assert!(result.failures().is_empty(), "{:?}", result.failures());
    assert_eq!(
        toolset.attempted(),
        [root.join("textures").join("fixture.dds")]
    );
    let [diagnostic] = result.diagnostics() else {
        panic!("one diagnostic expected: {:?}", result.diagnostics());
    };
    assert_eq!(diagnostic.phase, RunPhase::BuildingEffectiveAssetTree);
    assert_eq!(diagnostic.path, link);
}

/// Origin: ArchiveFirstAssetDiscoveryTests::selectedDirectoryAliasKeepsItsOriginalTarget.
/// The selected alias is resolved once, in Preparing, so retargeting it
/// during extraction cannot change the definitive pass's scope.
#[test]
fn a_selected_alias_keeps_its_original_target_through_extraction() {
    let base = canonical(&scratch_dir("archive-alias-retarget"));
    let original = base.join("original");
    let replacement = base.join("replacement");
    fixture_archive(&original.join("content.bsa"));
    write_tree(&original, &["original.dds"]);
    write_tree(&replacement, &["replacement.dds"]);
    let alias = base.join("selected");
    junction(&alias, &original);
    let (retargeted, target) = (alias.clone(), replacement.clone());
    let done = AtomicBool::new(false);
    let toolset = on_extract(move |_, _| {
        if !done.swap(true, Ordering::SeqCst) {
            std::fs::remove_dir(&retargeted).unwrap();
            junction(&retargeted, &target);
        }
    });

    let result = execute(&archives_and_textures(&alias), &toolset);
    std::fs::remove_dir(&alias).unwrap();

    assert!(result.failures().is_empty(), "{:?}", result.failures());
    let mut paths = toolset.attempted();
    paths.sort();
    assert_eq!(
        paths,
        [
            original.join("original.dds"),
            original.join("textures").join("fixture.dds"),
        ]
    );
}

/// Origin: ArchiveFirstAssetDiscoveryTests::cancelledExtractionSkipsDefinitiveTraversal
/// and AssetRunTests::finalArchiveCancellationSkipsDefinitiveDiscovery.
/// Cancellation during the final Archive lets it finish, then skips the
/// definitive pass, so no partial tree becomes work.
#[test]
fn cancellation_during_the_final_archive_skips_the_definitive_tree() {
    let root = canonical(&scratch_dir("archive-cancel-final"));
    fixture_archive(&root.join("content.bsa"));
    write_tree(&root, &["textures/loose.dds"]);
    let stop = CancellationToken::new();
    let raise = stop.clone();
    let toolset = on_extract(move |_, _| raise.cancel());

    let result = execute_with(&archives_and_textures(&root), &toolset, None, &stop);

    assert_eq!(result.outcome(), RunOutcome::Cancelled);
    assert!(result.cancellation_observed());
    let attempt = attempt_for(&result, "content.bsa");
    assert!(
        attempt.succeeded(),
        "the in-flight Archive finished: {}",
        attempt.detail
    );
    assert_eq!(
        attempt.mutation,
        cao_core::execution::MutationState::Committed
    );
    assert!(root.join("textures/fixture.dds").exists());
    assert!(result.routing_ledger().is_none());
    assert!(toolset.attempted().is_empty());
    assert!(result.phase(RunPhase::BuildingEffectiveAssetTree).is_none());
}

/// Origin: AssetRunTests::archiveCancellationSkipsDefinitiveDiscovery.
/// Cancellation after the first of two Archives stops before the second.
#[test]
fn cancellation_between_archives_stops_before_the_next_attempt() {
    let root = canonical(&scratch_dir("archive-cancel-between"));
    fixture_archive(&root.join("a.bsa"));
    write_archive(&root.join("b.bsa"), &[entry("meshes/b.dds", b"B")]);
    let token = CancellationToken::new();
    let raise = token.clone();
    let toolset = on_extract(move |_, _| raise.cancel());

    let result = execute_with(&archives_and_textures(&root), &toolset, None, &token);

    assert_eq!(result.outcome(), RunOutcome::Cancelled);
    assert_eq!(extracted_archives(&result), [root.join("a.bsa")]);
    assert!(attempt_for(&result, "a.bsa").succeeded());
    assert!(root.join("b.bsa").exists(), "the next Archive is untouched");
    assert!(!root.join("meshes").exists());
    let progress = result
        .phase(RunPhase::ExtractingArchives)
        .unwrap()
        .progress()
        .unwrap();
    assert_eq!((progress.completed(), progress.total()), (1, 2));
    assert!(result.routing_ledger().is_none());
}

/// Creates the file symlink `link` to `target`, or returns `false` when this
/// host lacks the privilege, as C++ skipped with `QSKIP`.
fn file_symlink(target: &Path, link: &Path) -> bool {
    match std::os::windows::fs::symlink_file(target, link) {
        Ok(()) => true,
        Err(error) => {
            eprintln!("skipped: file symlink creation is unavailable on this host: {error}");
            false
        }
    }
}

/// Origin: ArchiveFirstAssetDiscoveryTests::containedLooseFileLinkKeepsPrecedence.
#[test]
fn a_contained_linked_loose_asset_still_shadows_its_archive_entry() {
    let root = canonical(&scratch_dir("archive-linked-loose"));
    write_archive(&root.join("source.bsa"), &[entry("textures/a.dds", b"B")]);
    std::fs::create_dir_all(root.join("textures")).unwrap();
    let target = root.join("textures").join("target.dds");
    std::fs::write(&target, "loose asset").unwrap();
    let loose = root.join("textures").join("a.dds");
    if !file_symlink(&target, &loose) {
        return;
    }
    let toolset = Arc::new(real_archive_work());

    let result = execute(&archives_and_textures(&root), &toolset);
    let still_linked = std::fs::symlink_metadata(&loose)
        .unwrap()
        .file_type()
        .is_symlink();
    std::fs::remove_file(&loose).unwrap();

    assert!(result.failures().is_empty(), "{:?}", result.failures());
    // The linked Loose Asset wins, so the entry is never merged: publishing
    // it at the link's name would have failed the attempt.
    let [attempt] = result.archive_extraction_attempts() else {
        panic!("one attempt expected");
    };
    assert!(attempt.succeeded(), "{}", attempt.detail);
    assert!(still_linked, "the linked Loose Asset wins");
    assert_eq!(std::fs::read(&target).unwrap(), b"loose asset");
    assert!(backup_of(&root.join("source.bsa")).exists());
}

/// Origin: ArchiveFirstAssetDiscoveryTests::escapingFileLinksAreExcluded.
/// An escaping linked Archive is never read or extracted, and each escaping
/// link is diagnosed once.
#[test]
fn escaping_file_links_are_neither_extracted_nor_worked() {
    let base = canonical(&scratch_dir("archive-escaping-links"));
    let root = base.join("mod");
    let outside = base.join("outside");
    write_tree(&root, &["inside.dds"]);
    write_tree(&outside, &["outside.dds"]);
    fixture_archive(&outside.join("outside.bsa"));
    if !file_symlink(&outside.join("outside.dds"), &root.join("linked.dds")) {
        return;
    }
    assert!(file_symlink(
        &outside.join("outside.bsa"),
        &root.join("linked.bsa")
    ));
    let toolset = Arc::new(real_archive_work());

    let result = execute(&archives_and_textures(&root), &toolset);
    std::fs::remove_file(root.join("linked.dds")).unwrap();
    std::fs::remove_file(root.join("linked.bsa")).unwrap();

    assert!(result.failures().is_empty(), "{:?}", result.failures());
    assert!(result.archive_extraction_attempts().is_empty());
    assert!(no_manifest_listed(&toolset));
    assert_eq!(toolset.attempted(), [root.join("inside.dds")]);
    let mut diagnosed: Vec<_> = result
        .diagnostics()
        .iter()
        .map(|diagnostic| {
            assert_eq!(diagnostic.code, RunDiagnosticCode::LinkedEntryExcluded);
            assert_eq!(diagnostic.phase, RunPhase::DiscoveringArchives);
            diagnostic.path.clone()
        })
        .collect();
    diagnosed.sort();
    assert_eq!(
        diagnosed,
        [root.join("linked.bsa"), root.join("linked.dds")]
    );
}

/// Origin: ArchiveFirstAssetDiscoveryTests::stagingIsExcludedFromDiscovery
/// and linksIntoStagingAreExcluded. A Dry Run stands in for C++'s bare
/// discovery: an Apply run refuses unverifiable staging in Preparing before
/// discovery could see it. No staging content, nor a contained link into it,
/// is recognized as an Archive or an Asset.
#[test]
fn staging_and_links_into_it_never_enter_discovery() {
    let root = canonical(&scratch_dir("archive-staging-excluded"));
    write_tree(&root, &["loose.dds"]);
    for name in [".cao-staging", ".CAO-Staging-abandoned"] {
        write_tree(&root.join(name), &["temporary.dds"]);
        fixture_archive(&root.join(name).join("archive.bsa"));
    }
    let staged = root.join(".cao-staging");
    let linked = file_symlink(&staged.join("archive.bsa"), &root.join("linked.bsa"))
        && file_symlink(&staged.join("temporary.dds"), &root.join("linked.dds"));
    let toolset = Arc::new(real_archive_work());

    let result = execute(
        &request(
            ExecutionMode::DryRun,
            &root,
            &[
                RequestedWork::NativeTextureOptimization,
                RequestedWork::ArchiveExtraction,
            ],
        ),
        &toolset,
    );
    if linked {
        std::fs::remove_file(root.join("linked.bsa")).unwrap();
        std::fs::remove_file(root.join("linked.dds")).unwrap();
    }

    assert_eq!(
        result.outcome(),
        RunOutcome::Succeeded,
        "{:?}",
        result.failures()
    );
    assert_eq!(toolset.attempted(), [root.join("loose.dds")]);
    assert_eq!(
        result.skipped_asset_count(cao_core::routing::SkipReason::DisabledPhase),
        0,
        "no staged Archive is recognized"
    );
    assert_eq!(
        result.diagnostics().len(),
        if linked { 2 } else { 0 },
        "{:?}",
        result.diagnostics()
    );
    assert_eq!(
        std::fs::read(staged.join("temporary.dds")).unwrap(),
        b"temporary.dds"
    );
}

/// The request C++ called the Archive-disabled policy: Archives are
/// recognized but never extracted.
fn textures_only(root: &Path) -> RunRequest {
    request(
        ExecutionMode::Apply,
        root,
        &[RequestedWork::NativeTextureOptimization],
    )
}

/// Origin: ArchiveFirstAssetDiscoveryTests::directoryLinksAreExcluded (the
/// contained and escaping junction rows; the symlink rows add nothing a
/// junction does not, since both are reparse points).
#[test]
fn directory_junctions_are_never_traversed() {
    for contained in [false, true] {
        let base = canonical(&scratch_dir(&format!(
            "archive-directory-links-{contained}"
        )));
        let root = base.join("mod");
        let target = if contained { &root } else { &base }.join("target");
        write_tree(&root, &["inside.dds"]);
        write_tree(&target, &["target.dds"]);
        let link = root.join("linked");
        junction(&link, &target);
        let toolset = Arc::new(real_archive_work());

        let result = execute(&textures_only(&root), &toolset);
        std::fs::remove_dir(&link).unwrap();

        let mut paths = toolset.attempted();
        paths.sort();
        let mut expected = vec![root.join("inside.dds")];
        if contained {
            expected.push(target.join("target.dds"));
        }
        assert_eq!(paths, expected, "the link itself is never traversed");
        let [diagnostic] = result.diagnostics() else {
            panic!("one diagnostic expected: {:?}", result.diagnostics());
        };
        assert_eq!(diagnostic.code, RunDiagnosticCode::LinkedEntryExcluded);
        assert_eq!(diagnostic.path, link);
    }
}

/// Origin: ArchiveFirstAssetDiscoveryTests::containedFileLinkIsDiscovered and
/// danglingFileLinkIsDiagnosed.
#[test]
fn a_contained_file_link_is_work_and_a_dangling_one_is_diagnosed() {
    let root = canonical(&scratch_dir("archive-file-links"));
    write_tree(&root, &["original.dds"]);
    let contained = root.join("linked.dds");
    let dangling = root.join("dangling.dds");
    if !file_symlink(&root.join("original.dds"), &contained) {
        return;
    }
    assert!(file_symlink(&root.join("missing.dds"), &dangling));
    let toolset = Arc::new(real_archive_work());

    let result = execute(&textures_only(&root), &toolset);
    std::fs::remove_file(&contained).unwrap();
    std::fs::remove_file(&dangling).unwrap();

    let mut paths = toolset.attempted();
    paths.sort();
    assert_eq!(paths, [contained.clone(), root.join("original.dds")]);
    let [diagnostic] = result.diagnostics() else {
        panic!("one diagnostic expected: {:?}", result.diagnostics());
    };
    assert_eq!(diagnostic.code, RunDiagnosticCode::LinkedEntryExcluded);
    assert_eq!(diagnostic.path, dangling);
}

/// Origin: RunExecutorTests::discoveryDiagnosticsSurviveInterruption (the
/// Archive half). A link exclusion retained before extraction survives a
/// later orchestration failure, alongside the committed extraction.
///
/// Every port called between extraction and routing contains its own
/// panics, so the later failure is the one that can still escape the run's
/// work from outside it: an Archive Finalization port failing with an error
/// that is no Run Evidence invariant violation. A panic escaping the
/// executor's own work code is covered by its in-crate containment tests.
#[test]
fn discovery_diagnostics_survive_an_interruption_after_extraction() {
    let base = canonical(&scratch_dir("archive-diagnostics-interrupted"));
    let root = base.join("mod");
    write_tree(&base, &["source/asset.dds"]);
    fixture_archive(&root.join("source.bsa"));
    let link = root.join("linked.dds");
    if !file_symlink(&base.join("source").join("asset.dds"), &link) {
        return;
    }
    let output = root.join("asset.dds");
    let toolset = Arc::new(
        ScriptedToolset::default()
            .with_extraction(move |plan, _| {
                std::fs::write(&output, "committed").unwrap();
                ArchiveExtractionResult {
                    mutation: MutationState::Committed,
                    ..ArchiveExtractionResult::new(plan)
                }
            })
            .with_escaping_finalization_work("interrupted after exclusion"),
    );

    let result = execute(
        &request(
            ExecutionMode::Apply,
            &root,
            &[RequestedWork::ArchiveExtraction],
        ),
        &toolset,
    );
    std::fs::remove_file(&link).unwrap();

    assert_eq!(result.outcome(), RunOutcome::Failed);
    assert_eq!(result.archive_extraction_attempts().len(), 1);
    assert_eq!(result.failures().len(), 1);
    assert_eq!(result.failures()[0].code, RunFailureCode::RunWorkEscaped);
    assert_eq!(result.failures()[0].phase, RunPhase::ArchiveFinalization);
    let [diagnostic] = result.diagnostics() else {
        panic!("one diagnostic expected: {:?}", result.diagnostics());
    };
    assert_eq!(diagnostic.code, RunDiagnosticCode::LinkedEntryExcluded);
    assert_eq!(diagnostic.phase, RunPhase::DiscoveringArchives);
    assert_eq!(diagnostic.path, link);
    let summary = &result.mutation_summaries()[0];
    assert_eq!(summary.kind, MutationKind::ArchiveExtraction);
    assert_eq!(summary.committed, 1);
    assert_eq!(std::fs::read(root.join("asset.dds")).unwrap(), b"committed");
}

/// Origin: AssetRunTests::filesystemTraversalPollsCancellation (the
/// definitive-mod-tree row). Cancellation raised once the definitive pass
/// has begun, after extraction, is observed before any entry becomes work,
/// even when every entry is unsupported and offers no other seam.
///
/// The token is cancelled on the pass's own phase record, the last point a
/// scenario can reach before the pass walks the tree; whether the walk then
/// stops before or between entries leaves the same evidence.
#[test]
fn cancellation_once_the_definitive_pass_begins_leaves_no_tree() {
    let root = canonical(&scratch_dir("archive-definitive-polls"));
    fixture_archive(&root.join("content.bsa"));
    for index in 0..100 {
        std::fs::write(root.join(format!("{index}.txt")), "unsupported").unwrap();
    }
    let toolset = Arc::new(real_archive_work());
    let stop = CancellationToken::new();
    let raise = stop.clone();
    let sink = RecordingSink::on_first_record_of(RunPhase::BuildingEffectiveAssetTree, move || {
        raise.cancel()
    });

    let result = execute_with(&archives_and_textures(&root), &toolset, Some(&sink), &stop);

    assert_eq!(result.outcome(), RunOutcome::Cancelled);
    assert!(result.cancellation_observed());
    assert_eq!(result.archive_extraction_attempts().len(), 1);
    assert!(result.phase(RunPhase::BuildingEffectiveAssetTree).is_some());
    assert!(result.routing_ledger().is_none());
    assert!(toolset.attempted().is_empty());
    assert!(result.archive_finalization().is_none());
}

/// Origin: AssetRunTests::archiveFailuresControlContinuation (safe failure,
/// unsafe first and unsafe last). A safe failure lets the run go on; an
/// unsafe one, with partial mutation, stops all later Archives, Assets and
/// finalization.
///
/// The safe failure is an Archive corrupted after preflight, which fails
/// before anything is published. The unsafe one is an obstruction placed
/// after preflight where the Archive's second entry needs a directory, so
/// its first entry is already published when the merge fails.
///
/// Not covered: C++'s row of partial mutation overriding a safe flag, and an
/// attempt naming a Mod Root other than its plan's. The real extractor
/// reports partial mutation as unsafe and keeps its plan's Mod Root, so only
/// a scripted extraction port could produce either.
#[test]
fn archive_failures_control_continuation() {
    for (failed_attempt, partial) in [(1, false), (1, true), (2, true)] {
        let can_continue = !partial;
        let root = canonical(&scratch_dir(&format!(
            "archive-continuation-{failed_attempt}-{partial}"
        )));
        let name = if failed_attempt == 1 {
            "a.bsa"
        } else {
            "b.bsa"
        };
        fixture_archive(&root.join("a.bsa"));
        write_archive(&root.join("b.bsa"), &[entry("meshes/b.dds", b"B")]);
        let failing = root.join(name);
        if partial {
            write_archive(
                &failing,
                &[
                    entry("a-committed.dds", b"committed"),
                    entry("z/blocked.dds", b"blocked"),
                ],
            );
        }
        write_tree(&root, &["loose.dds"]);
        let toolset = Arc::new(real_archive_work());
        let (directory, corrupted) = (root.clone(), failing.clone());
        let sink = RecordingSink::on_first_record_of(RunPhase::ExtractingArchives, move || {
            if partial {
                std::fs::write(directory.join("z"), "obstruction").unwrap();
            } else {
                std::fs::write(&corrupted, "corrupted after preflight").unwrap();
            }
        });

        let result = execute_with(
            &archives_and_textures(&root),
            &toolset,
            Some(&sink),
            &CancellationToken::new(),
        );

        let attempted = if can_continue { 2 } else { failed_attempt };
        assert_eq!(result.archive_extraction_attempts().len(), attempted);
        // After a safe failure the later Archive's entry is worked too.
        let mut worked = toolset.attempted();
        worked.sort();
        let expected_work = if can_continue {
            vec![root.join("loose.dds"), root.join("meshes").join("b.dds")]
        } else {
            Vec::new()
        };
        assert_eq!(worked, expected_work);
        assert_eq!(result.archive_finalization().is_some(), can_continue);
        assert_eq!(result.routing_ledger().is_some(), can_continue);
        assert!(!result.cancellation_observed());
        let failure = &result.archive_extraction_attempts()[failed_attempt - 1];
        assert_eq!(failure.mod_root, root);
        assert!(!failure.succeeded());
        assert_eq!(failure.safe_to_continue, !partial);
        if partial {
            assert_eq!(failure.failure, Some(ArchiveExtractionFailure::MergeFailed));
            assert_eq!(failure.mutation, MutationState::PartialOrUnknown);
            assert_eq!(
                std::fs::read(root.join("a-committed.dds")).unwrap(),
                b"committed"
            );
        } else {
            assert_eq!(failure.mutation, MutationState::None);
        }
        assert!(!failure.detail.is_empty());
        assert!(failing.exists(), "a failed attempt keeps its source");
        assert_eq!(failure.archive_path, failing);
        let last = attempt_progress(&sink, RunPhase::ExtractingArchives)
            .pop()
            .unwrap();
        assert_eq!(last, (attempted, 2));
        let expected = if can_continue {
            RunOutcome::CompletedWithFailures
        } else {
            RunOutcome::Failed
        };
        assert_eq!(result.outcome(), expected);
    }
}

/// Origin: AssetRunTests::reportsCollisionsBeforeOrderedExtraction and
/// RunExecutorTests::productionWorkRetainsArchiveCollisions. A failing
/// collision observer is contained as a diagnostic; the evidence already
/// holds the plan, and extraction follows explicit precedence. The observer
/// fails on the Archive extraction phase record, the first it sees once the
/// plan is known.
#[test]
fn a_failing_collision_observer_cannot_lose_the_retained_plan() {
    let root = canonical(&scratch_dir("archive-collision-observer-fails"));
    // Each Archive also holds an entry of its own, so each extraction reads
    // an entry whatever the shadowed one does.
    for (name, own) in [("a.bsa", "meshes/a.nif"), ("z.bsa", "meshes/z.nif")] {
        write_archive(
            &root.join(name),
            &[
                entry("textures/shared.dds", b"archived"),
                entry(own, b"own"),
            ],
        );
    }
    write_tree(&root, &["textures/shared.dds"]);
    let reported = Arc::new(AtomicBool::new(false));
    let (flag, seen) = (Arc::clone(&reported), Arc::clone(&reported));
    let extracted_before_report = Arc::new(AtomicBool::new(false));
    let violated = Arc::clone(&extracted_before_report);
    let sink = RecordingSink::on_first_record_of(RunPhase::ExtractingArchives, move || {
        flag.store(true, Ordering::SeqCst);
        panic!("collision observer");
    });
    let toolset = on_extract(move |_, _| {
        if !seen.load(Ordering::SeqCst) {
            violated.store(true, Ordering::SeqCst);
        }
    });
    let precedence = ArchivePrecedence::ExplicitOrder(vec!["z.bsa".into(), "a.bsa".into()]);

    let result = execute_with(
        &request(
            ExecutionMode::Apply,
            &root,
            &[RequestedWork::ArchiveExtraction],
        )
        .with_archive_precedence(precedence),
        &toolset,
        Some(&sink),
        &CancellationToken::new(),
    );

    assert_eq!(
        result.outcome(),
        RunOutcome::Succeeded,
        "{:?}",
        result.failures()
    );
    assert!(reported.load(Ordering::SeqCst));
    assert!(
        !extracted_before_report.load(Ordering::SeqCst),
        "reported before extraction"
    );
    assert_eq!(
        extracted_archives(&result),
        [root.join("z.bsa"), root.join("a.bsa")]
    );
    assert_eq!(std::fs::read(root.join("meshes/a.nif")).unwrap(), b"own");
    assert_eq!(std::fs::read(root.join("meshes/z.nif")).unwrap(), b"own");
    let [collision] = result.archive_collisions() else {
        panic!("one collision expected");
    };
    assert_eq!(collision.winning_archive, root.join("z.bsa"));
    assert_eq!(collision.shadowed_archives, [root.join("a.bsa")]);
    assert!(collision.loose_asset_wins);
    assert_eq!(result.archive_extraction_attempts().len(), 2);
    let observer_failures = result
        .diagnostics()
        .iter()
        .filter(|diagnostic| diagnostic.code == RunDiagnosticCode::ObserverFailed)
        .count();
    assert_eq!(observer_failures, 1);
}

/// Origin: AssetRunTests::archiveExtractionPrecedesDefinitiveRoutedExecution.
#[test]
fn extraction_progress_precedes_routed_asset_progress() {
    let root = canonical(&scratch_dir("archive-progress-order"));
    write_archive(
        &root.join("content.bsa"),
        &[entry("textures/extracted.dds", b"extracted")],
    );
    write_tree(&root, &["textures/loose.dds"]);
    let toolset = Arc::new(real_archive_work());
    let sink = RecordingSink::default();

    let result = execute_with(
        &archives_and_textures(&root),
        &toolset,
        Some(&sink),
        &CancellationToken::new(),
    );

    assert_eq!(
        result.outcome(),
        RunOutcome::Succeeded,
        "{:?}",
        result.failures()
    );
    assert_eq!(result.routing_ledger().unwrap().routed_assets().len(), 2);
    // Each attempt's progress, in the order the run published it.
    let progress: Vec<_> = sink
        .observed()
        .into_iter()
        .filter_map(|observed| match observed {
            Observed::Phase(record) => {
                let progress = record.progress()?;
                (progress.completed() > 0)
                    .then(|| (record.phase(), progress.completed(), progress.total()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        progress,
        [
            (RunPhase::ExtractingArchives, 1, 1),
            (RunPhase::ProcessingAssets, 1, 2),
            (RunPhase::ProcessingAssets, 2, 2),
        ]
    );
}

/// Origin: RunExecutorTests::mixedExtractionAttemptsAdvanceProgress. Mixed
/// attempts advance one determinate account; a failing progress observer is
/// contained; and an Archive left behind by extraction is counted as nesting
/// rather than worked.
///
/// The failed attempt is a safe one, which publishes nothing, so the nested
/// Archive comes from the committed attempt instead. C++'s failed attempt
/// leaving an Archive behind is not covered: the real extractor publishes
/// nothing on a safe failure, so only a scripted extraction port could.
#[test]
fn mixed_extraction_attempts_advance_one_determinate_account() {
    let root = canonical(&scratch_dir("archive-mixed-attempts"));
    write_archive(
        &root.join("a.bsa"),
        &[
            entry("a.dds", b"extracted"),
            entry("nested.bsa", b"nested archive"),
        ],
    );
    write_archive(&root.join("b.bsa"), &[entry("meshes/b.dds", b"B")]);
    let later = root.join("b.bsa");
    // b.bsa is corrupted after preflight, while a.bsa is extracting, so its
    // own attempt fails safely before publishing anything.
    let toolset = on_extract(move |archive, _| {
        if archive.ends_with("a.bsa") {
            std::fs::write(&later, "corrupted after preflight").unwrap();
        }
    });
    let threw = std::cell::Cell::new(false);
    let sink = RecordingSink::with_hook(move |observed| {
        if let Observed::Phase(record) = observed
            && record.phase() == RunPhase::ExtractingArchives
            && record
                .progress()
                .is_some_and(|progress| progress.completed() == 1)
            && !threw.replace(true)
        {
            panic!("Archive progress observer failed");
        }
    });

    let result = execute_with(
        &archives_and_textures(&root),
        &toolset,
        Some(&sink),
        &CancellationToken::new(),
    );

    assert_eq!(result.outcome(), RunOutcome::CompletedWithFailures);
    let progress = result
        .phase(RunPhase::ExtractingArchives)
        .unwrap()
        .progress()
        .unwrap();
    assert_eq!(
        (progress.total(), progress.succeeded(), progress.failed()),
        (2, 1, 1)
    );
    assert!(attempt_for(&result, "a.bsa").succeeded());
    assert!(!attempt_for(&result, "b.bsa").succeeded());
    assert_eq!(toolset.attempted(), [root.join("a.dds")]);
    let discovery = result.evidence().archive_discovery().unwrap();
    assert_eq!(discovery.nested_archive_count, 1);
    assert!(root.join("nested.bsa").exists());
    let summary = &result.mutation_summaries()[0];
    assert_eq!(summary.kind, MutationKind::ArchiveExtraction);
    assert_eq!(summary.committed, 1);
    assert!(
        result
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code == RunDiagnosticCode::ObserverFailed)
    );
}

/// Origin: RunExecutorTests::throwingPreflightFailureObserverRetainsEvidence.
/// A failure observer that panics is called once, the failure is retained,
/// and no extraction or finalization follows.
#[test]
fn a_failing_preflight_observer_cannot_lose_the_failure() {
    let root = canonical(&scratch_dir("archive-preflight-observer"));
    std::fs::write(root.join("broken.bsa"), "invalid archive").unwrap();
    let toolset = Arc::new(real_archive_work());
    let sink = RecordingSink::with_hook(|observed| {
        if let Observed::Failure(_) = observed {
            panic!("preflight failure observer failed");
        }
    });

    let result = execute_with(
        &request(
            ExecutionMode::Apply,
            &root,
            &[RequestedWork::ArchiveExtraction],
        ),
        &toolset,
        Some(&sink),
        &CancellationToken::new(),
    );

    assert_eq!(result.outcome(), RunOutcome::Failed);
    let observed_failures: Vec<_> = sink
        .observed()
        .into_iter()
        .filter_map(|observed| match observed {
            Observed::Failure(failure) => Some(failure.code),
            _ => None,
        })
        .collect();
    assert_eq!(observed_failures, [RunFailureCode::ArchiveUnreadable]);
    assert_eq!(result.failures().len(), 1);
    assert_eq!(result.failures()[0].code, RunFailureCode::ArchiveUnreadable);
    let observer_failures = result
        .diagnostics()
        .iter()
        .filter(|diagnostic| diagnostic.code == RunDiagnosticCode::ObserverFailed)
        .count();
    assert_eq!(observer_failures, 1);
    assert!(result.archive_extraction_attempts().is_empty());
    assert!(result.archive_extraction_attempts().is_empty());
    assert!(result.archive_finalization().is_none());
    assert!(result.phase(RunPhase::ArchiveFinalization).is_none());
    assert_eq!(
        std::fs::read(root.join("broken.bsa")).unwrap(),
        b"invalid archive"
    );
}

/// Rust-only, from the spec's panic rule (#476): a panic in the extraction
/// adapter is contained as unknown mutation, unsafe to continue, as C++
/// contained an adapter exception.
#[test]
fn a_panicking_extraction_adapter_stops_the_run_with_unknown_mutation() {
    let root = canonical(&scratch_dir("archive-adapter-panics"));
    fixture_archive(&root.join("a.bsa"));
    write_archive(&root.join("b.bsa"), &[entry("meshes/b.dds", b"B")]);
    write_tree(&root, &["loose.dds"]);
    let toolset = Arc::new(
        ScriptedToolset::default()
            .with_extraction(|_, _| panic!("the extraction backend panicked")),
    );

    let result = execute(&archives_and_textures(&root), &toolset);

    assert_eq!(result.outcome(), RunOutcome::Failed);
    let [attempt] = result.archive_extraction_attempts() else {
        panic!("one attempt expected");
    };
    assert_eq!(attempt.mutation, MutationState::PartialOrUnknown);
    assert!(!attempt.safe_to_continue);
    assert_eq!(
        attempt.failure,
        Some(ArchiveExtractionFailure::ExtractionFailed)
    );
    assert!(attempt.detail.contains("the extraction backend panicked"));
    assert!(result.routing_ledger().is_none());
    assert!(toolset.attempted().is_empty());
    let summary = &result.mutation_summaries()[0];
    assert_eq!(summary.kind, MutationKind::ArchiveExtraction);
    assert_eq!((summary.committed, summary.partial_or_unknown), (0, 1));
}

/// Origin: RunExecutorTests::committedExtractionSurvivesDiscoveryInterruption
/// (cancel row). An extraction committed before discovery is interrupted by
/// cancellation stays a Committed Mutation in the evidence, with its source
/// backed up.
#[test]
fn a_committed_extraction_survives_a_later_cancellation() {
    let root = canonical(&scratch_dir("archive-interrupted-cancelled"));
    fixture_archive(&root.join("source.bsa"));
    let stop = CancellationToken::new();
    let raise = stop.clone();
    // Raised while the attempt is in flight, so the run samples it only once
    // the contained extraction has returned.
    let toolset = on_extract(move |_, _| raise.cancel());

    let result = execute_with(
        &request(
            ExecutionMode::Apply,
            &root,
            &[RequestedWork::ArchiveExtraction],
        ),
        &toolset,
        None,
        &stop,
    );

    assert_eq!(result.outcome(), RunOutcome::Cancelled);
    assert!(result.failures().is_empty());
    assert_eq!(result.archive_extraction_attempts().len(), 1);
    assert!(result.archive_finalization().is_none());
    let summary = &result.mutation_summaries()[0];
    assert_eq!(summary.kind, MutationKind::ArchiveExtraction);
    assert_eq!((summary.committed, summary.partial_or_unknown), (1, 0));
    assert_eq!(
        std::fs::read(root.join("textures/fixture.dds")).unwrap(),
        b"fixture"
    );
    assert!(!root.join("source.bsa").exists());
    assert!(backup_of(&root.join("source.bsa")).exists());
}

/// Origin: RunExecutorTests::committedExtractionSurvivesDiscoveryInterruption
/// (throw row). An extraction committed before discovery is interrupted by a
/// later orchestration failure stays a Committed Mutation in the evidence.
///
/// The later failure is an Archive Finalization port failing with an error
/// that is no Run Evidence invariant violation, the one failure after
/// extraction that still escapes the run's work from outside it; see
/// `discovery_diagnostics_survive_an_interruption_after_extraction`. The
/// cancel row is `a_committed_extraction_survives_a_later_cancellation`.
#[test]
fn a_committed_extraction_survives_a_later_interruption() {
    let root = canonical(&scratch_dir("archive-interrupted-true"));
    fixture_archive(&root.join("source.bsa"));
    let output = root.join("asset.dds");
    let toolset = Arc::new(
        ScriptedToolset::default()
            .with_extraction(move |plan, _| {
                std::fs::write(&output, "committed extraction").unwrap();
                ArchiveExtractionResult {
                    mutation: MutationState::Committed,
                    ..ArchiveExtractionResult::new(plan)
                }
            })
            .with_escaping_finalization_work("interrupted after extraction"),
    );

    let result = execute(
        &request(
            ExecutionMode::Apply,
            &root,
            &[RequestedWork::ArchiveExtraction],
        ),
        &toolset,
    );

    assert_eq!(result.outcome(), RunOutcome::Failed);
    assert_eq!(result.failures()[0].code, RunFailureCode::RunWorkEscaped);
    assert_eq!(result.archive_extraction_attempts().len(), 1);
    let summary = &result.mutation_summaries()[0];
    assert_eq!(summary.kind, MutationKind::ArchiveExtraction);
    assert_eq!((summary.committed, summary.partial_or_unknown), (1, 0));
    assert_eq!(
        std::fs::read(root.join("asset.dds")).unwrap(),
        b"committed extraction"
    );
    assert!(root.join("source.bsa").exists());
}
