//! The Archive Extractor against hand-built plans, as the C++ suite drove
//! `ArchiveExtractor::extract` directly, over a fake archive reader and real
//! Mod Root directories.
//!
//! Each scenario names its C++ origin in
//! `tests/ArchiveFirstAssetDiscoveryTests.cpp`. A plan built by hand skips the
//! preflight, which is exactly what lets these reach the extractor's own
//! rechecks: a manifest or tree that changed after preflight.
//!
//! Not ported, with reasons:
//! - The per-format rows of `stagedExtractionCommitsPayload`: TES3, TES4 and
//!   FO4 containers are the reader's concern, ported against real Archives in
//!   `cao-optimizers` (#497). One fake format exercises the staged writer.

mod common;

use std::path::Path;
use std::sync::atomic::Ordering;

use cao_core::execution::MutationState;
use cao_core::run::{
    ArchiveExtractionFailure, ArchiveExtractionPlan, ArchiveExtractor, TemporaryArtifactRegistry,
    create_run_id,
};
use common::{
    FakeArchiveReader, FakeCapacity, canonical, entry, junction, scratch_dir, write_archive,
};

/// A plan that publishes every one of `entries` from `archive`.
fn plan(archive: &Path, root: &Path, entries: &[&str]) -> ArchiveExtractionPlan {
    let entries: Vec<String> = entries.iter().map(|entry| (*entry).to_owned()).collect();
    ArchiveExtractionPlan {
        archive_path: archive.to_path_buf(),
        mod_root: root.to_path_buf(),
        merge_entries: entries.clone(),
        entries,
        estimated_capacity_bytes: 0,
    }
}

/// A fresh Temporary Ownership scope for one attempt.
fn registry() -> TemporaryArtifactRegistry {
    TemporaryArtifactRegistry::new(create_run_id())
}

/// Origin: extractionCapacityRecheck (shortage and unknown rows). Free space
/// is rechecked just before staging: a shortage leaves no staging and no
/// output, and unknown capacity proceeds.
#[test]
fn the_capacity_check_is_repeated_before_staging() {
    for unknown in [false, true] {
        let root = canonical(&scratch_dir(&format!("extractor-capacity-{unknown}")));
        let archive = root.join("source.bsa");
        write_archive(&archive, &[entry("textures/fixture.dds", b"fixture")]);
        let original = std::fs::read(&archive).unwrap();
        let reader = FakeArchiveReader::default();
        let capacity = FakeCapacity(Some(Box::new(move |_| (!unknown).then_some(0))));
        let mut artifacts = registry();

        let result = ArchiveExtractor::new(&reader, &capacity).extract(
            &plan(&archive, &root, &["textures/fixture.dds"]),
            &mut artifacts,
        );

        assert_eq!(result.succeeded(), unknown, "{}", result.detail);
        assert!(result.safe_to_continue);
        assert_eq!(std::fs::read(&archive).unwrap(), original);
        if !unknown {
            assert_eq!(
                result.failure,
                Some(ArchiveExtractionFailure::InsufficientCapacity)
            );
            assert_eq!(result.mutation, MutationState::None);
            assert!(!root.join(".cao-staging").exists());
            assert!(!root.join("textures").exists());
        }
        assert!(artifacts.cleanup().is_empty());
    }
}

/// Origin: extractionFailureBeforeMergeCommitsNothing.
#[test]
fn a_source_lost_after_preflight_commits_nothing() {
    let root = canonical(&scratch_dir("extractor-missing-source"));
    let reader = FakeArchiveReader::default();
    let mut artifacts = registry();

    let result = ArchiveExtractor::new(&reader, &FakeCapacity::default()).extract(
        &plan(&root.join("missing.bsa"), &root, &["textures/a.dds"]),
        &mut artifacts,
    );

    assert_eq!(
        result.failure,
        Some(ArchiveExtractionFailure::ExtractionFailed)
    );
    assert_eq!(result.mutation, MutationState::None);
    assert!(result.safe_to_continue);
    assert!(!root.join("textures").exists());
    assert!(artifacts.cleanup().is_empty());
}

/// Origin: stagedExtractionCommitsPayload. The payload commits from
/// registered staging, the source Archive is kept, and Safety Cleanup leaves
/// the committed entry alone.
#[test]
fn a_staged_payload_commits_and_the_source_archive_is_kept() {
    let root = canonical(&scratch_dir("extractor-commits"));
    let archive = root.join("source.bsa");
    write_archive(&archive, &[entry("textures/a.dds", b"B")]);
    let original = std::fs::read(&archive).unwrap();
    let reader = FakeArchiveReader::default();
    let mut artifacts = registry();

    let result = ArchiveExtractor::new(&reader, &FakeCapacity::default())
        .extract(&plan(&archive, &root, &["textures/a.dds"]), &mut artifacts);

    assert!(result.succeeded(), "{}", result.detail);
    assert_eq!(result.mutation, MutationState::Committed);
    assert_eq!(std::fs::read(root.join("textures/a.dds")).unwrap(), b"B");
    assert_eq!(std::fs::read(&archive).unwrap(), original);
    assert!(artifacts.cleanup().is_empty());
    assert_eq!(std::fs::read(root.join("textures/a.dds")).unwrap(), b"B");
}

/// Origin: extractionUsesExistingParentCasing.
#[test]
fn extraction_publishes_beneath_the_existing_parent_casing() {
    let root = canonical(&scratch_dir("extractor-parent-casing"));
    let archive = root.join("source.bsa");
    write_archive(&archive, &[entry("textures/a.dds", b"B")]);
    std::fs::create_dir(root.join("Textures")).unwrap();
    let reader = FakeArchiveReader::default();
    let mut artifacts = registry();

    let result = ArchiveExtractor::new(&reader, &FakeCapacity::default())
        .extract(&plan(&archive, &root, &["textures/a.dds"]), &mut artifacts);

    assert!(result.succeeded(), "{}", result.detail);
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
    assert!(artifacts.cleanup().is_empty());
}

/// Origin: partialMergeRetainsCommittedOutput. A failure after the first
/// published entry keeps that output and reports unknown, unsafe mutation.
#[test]
fn a_failure_after_the_first_commit_is_partial_and_unsafe() {
    let root = canonical(&scratch_dir("extractor-partial"));
    let archive = root.join("source.bsa");
    write_archive(
        &archive,
        &[entry("a.dds", b"committed"), entry("z/b.dds", b"blocked")],
    );
    std::fs::write(root.join("z"), "obstruction").unwrap();
    let reader = FakeArchiveReader::default();
    let mut artifacts = registry();

    let result = ArchiveExtractor::new(&reader, &FakeCapacity::default()).extract(
        &plan(&archive, &root, &["a.dds", "z/b.dds"]),
        &mut artifacts,
    );

    assert_eq!(result.failure, Some(ArchiveExtractionFailure::MergeFailed));
    assert_eq!(result.mutation, MutationState::PartialOrUnknown);
    assert!(!result.safe_to_continue);
    assert!(artifacts.cleanup().is_empty());
    assert_eq!(std::fs::read(root.join("a.dds")).unwrap(), b"committed");
    assert_eq!(std::fs::read(root.join("z")).unwrap(), b"obstruction");
    assert!(archive.exists());
}

/// Origin: the `catch (...)` of C++ `ArchiveExtractor::extract`, which kept
/// a non-standard exception classified by how far the attempt got. A reader
/// that panics while staging has published nothing, so the attempt is a
/// safe failure rather than unknown mutation.
#[test]
fn a_reader_panicking_before_any_commit_is_a_safe_failure() {
    let root = canonical(&scratch_dir("extractor-reader-panics"));
    let archive = root.join("source.bsa");
    write_archive(&archive, &[entry("textures/a.dds", b"B")]);
    let reader = FakeArchiveReader {
        on_extract: Some(Box::new(|_, _| panic!("the archive library panicked"))),
        ..FakeArchiveReader::default()
    };
    let mut artifacts = registry();

    let result = ArchiveExtractor::new(&reader, &FakeCapacity::default())
        .extract(&plan(&archive, &root, &["textures/a.dds"]), &mut artifacts);

    assert_eq!(
        result.failure,
        Some(ArchiveExtractionFailure::ExtractionFailed)
    );
    assert_eq!(result.mutation, MutationState::None);
    assert!(result.safe_to_continue);
    assert!(result.detail.contains("the archive library panicked"));
    assert!(!root.join("textures").exists());
    assert!(
        artifacts.cleanup().is_empty(),
        "the staged entry is removed"
    );
}

/// Origin: changedManifestFailsBeforeMerge.
#[test]
fn a_manifest_changed_after_preflight_fails_before_any_merge() {
    let root = canonical(&scratch_dir("extractor-changed-manifest"));
    let archive = root.join("source.bsa");
    write_archive(&archive, &[entry("textures/a.dds", b"B")]);
    let reader = FakeArchiveReader::default();
    let mut artifacts = registry();

    let result = ArchiveExtractor::new(&reader, &FakeCapacity::default()).extract(
        &plan(&archive, &root, &["textures/expected.dds"]),
        &mut artifacts,
    );

    assert_eq!(
        result.failure,
        Some(ArchiveExtractionFailure::ExtractionFailed)
    );
    assert_eq!(result.mutation, MutationState::None);
    assert!(result.safe_to_continue);
    assert!(archive.exists());
    assert!(!root.join("textures").exists());
    assert!(artifacts.cleanup().is_empty());
}

/// Origin: mergeRejectsLinkedParent. A junction inserted after preflight
/// cannot redirect a staged commit outside the Mod Root.
#[test]
fn a_linked_parent_cannot_redirect_a_merge() {
    let base = canonical(&scratch_dir("extractor-linked-parent"));
    let root = base.join("mod");
    let outside = base.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let archive = root.join("source.bsa");
    write_archive(&archive, &[entry("textures/a.dds", b"B")]);
    let link = root.join("textures");
    junction(&link, &outside);
    let reader = FakeArchiveReader::default();
    let mut artifacts = registry();

    let result = ArchiveExtractor::new(&reader, &FakeCapacity::default())
        .extract(&plan(&archive, &root, &["textures/a.dds"]), &mut artifacts);
    // Remove only the link, never its independent target.
    std::fs::remove_dir(&link).unwrap();

    assert_eq!(result.failure, Some(ArchiveExtractionFailure::MergeFailed));
    assert_eq!(result.mutation, MutationState::None);
    assert!(result.safe_to_continue);
    assert!(!outside.join("a.dds").exists());
    assert!(archive.exists());
    assert!(artifacts.cleanup().is_empty());
}

/// Rust-only, for the #497 risk "drop a memory-mapped Archive before deleting
/// or renaming its source". The reader may keep its Archive open between
/// entries, so the extractor tells it to let go at the end of every attempt,
/// whether the attempt succeeded, failed or panicked; source cleanup comes
/// after.
#[test]
fn every_attempt_releases_the_reader_before_returning() {
    let root = canonical(&scratch_dir("extractor-releases"));
    let archive = root.join("source.bsa");
    write_archive(&archive, &[entry("textures/a.dds", b"B")]);
    let succeeded = FakeArchiveReader::default();
    let failed = FakeArchiveReader::default();
    let panicked = FakeArchiveReader {
        on_extract: Some(Box::new(|_, _| panic!("the archive library panicked"))),
        ..FakeArchiveReader::default()
    };
    let attempts = [
        (&succeeded, "textures/a.dds", true),
        (&failed, "textures/expected.dds", false),
        (&panicked, "textures/a.dds", false),
    ];

    for (reader, planned, succeeds) in attempts {
        let mut artifacts = registry();
        let result = ArchiveExtractor::new(reader, &FakeCapacity::default())
            .extract(&plan(&archive, &root, &[planned]), &mut artifacts);

        assert_eq!(result.succeeded(), succeeds, "{planned}: {}", result.detail);
        assert_eq!(reader.releases.load(Ordering::SeqCst), 1, "{planned}");
        assert!(artifacts.cleanup().is_empty());
        let _ = std::fs::remove_file(root.join("textures/a.dds"));
    }
}
