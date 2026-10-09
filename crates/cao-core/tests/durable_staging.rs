//! The producer half of C++ `DurableStagingTests`, ported against
//! [`TemporaryArtifactRegistry`] and real temporary directories.
//!
//! The scenarios that recover leftover staging belong to #492. Where a C++
//! scenario ended by recovering, this port instead asserts what recovery
//! would find: the leftover bytes and the manifest that still owns them.
//! Left for #492 entirely: `abandonedArchiveEntryIsRecovered`,
//! `abandonedOutputIsRecovered`, `killedAfterPublicationKeepsDestination` and
//! `archivePublicationSurvivesProducerTermination` (their recovery halves),
//! `recoveryAndProductionShareTheOwnershipLock`,
//! `recoverySkipsMissingSiblingParent`,
//! `cancelledPreparationPreservesDurableSibling`,
//! `malformedSiblingOwnershipIsPreserved`,
//! `uppercaseDdsDestinationUsesRecoverableSibling` (its recovery half),
//! `meshSiblingRecoveryPreservesOriginal`,
//! `malformedMeshSiblingOwnershipIsPreserved`,
//! `animationSiblingRecoveryPreservesOriginal`,
//! `malformedAnimationSiblingOwnershipIsPreserved` and
//! `partialScratchIsRecoveredButCorruptOwnershipIsPreserved`.
//!
//! Two crash scenarios run a real producer in a child process (this test
//! binary, re-entered through `CAO_STAGING_CHILD`) and kill it between
//! publication steps.

mod common;

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use cao_core::execution::MutationState;
use cao_core::run::{
    CancellationToken, PublicationPolicy, PublicationReceipt, PublicationState, RunFailureCode,
    TemporaryArtifactRegistry, create_run_id,
};
use cao_winfs::{Access, Open, Share};
use common::{canonical, scratch_dir};

/// A fresh registry with its own Run ID.
fn registry() -> TemporaryArtifactRegistry {
    TemporaryArtifactRegistry::new(create_run_id())
}

/// A fresh, canonical Mod Root.
fn mod_root(name: &str) -> PathBuf {
    canonical(&scratch_dir(&format!("durable-staging/{name}")))
}

fn read(path: &Path) -> String {
    String::from_utf8(std::fs::read(path).unwrap()).unwrap()
}

fn staging(root: &Path) -> PathBuf {
    root.join(".cao-staging")
}

fn manifest(root: &Path) -> String {
    read(&staging(root).join("ownership.manifest"))
}

/// The receipt's staged path, while its registry lives.
fn staged(receipt: &PublicationReceipt) -> PathBuf {
    receipt.path().unwrap().to_path_buf()
}

/// Whether renaming `directory` is refused, restoring it if it was not.
fn renaming_denied(directory: &Path) -> bool {
    let shifted = directory.with_file_name(format!(
        "{}-shifted",
        directory.file_name().unwrap().to_string_lossy()
    ));
    match std::fs::rename(directory, &shifted) {
        Ok(()) => {
            std::fs::rename(&shifted, directory).unwrap();
            false
        }
        Err(_) => true,
    }
}

/// Safety Cleanup removes partial Archive entries and the run child, keeps a
/// published entry, and closes the registry.
#[test]
fn archive_cleanup_keeps_the_committed_entry() {
    let root = mod_root("archive-cleanup");
    let destination = root.join("entry.pex");
    let mut registry = registry();
    let committed = registry.stage_archive_file_for_publication(&root).unwrap();
    let abandoned = registry.stage_archive_file_for_publication(&root).unwrap();
    let (committed_path, abandoned_path) = (staged(&committed), staged(&abandoned));
    assert_ne!(committed_path, abandoned_path);
    assert_eq!(
        committed_path.parent().unwrap().parent().unwrap(),
        staging(&root)
    );
    std::fs::write(&committed_path, "complete script").unwrap();
    std::fs::write(&abandoned_path, "partial script").unwrap();

    let publication = committed.publish(&destination, PublicationPolicy::NoReplace);
    assert_eq!(
        publication.state,
        PublicationState::PublishedAndReleased,
        "{}",
        publication.error_detail
    );
    drop(abandoned);
    assert!(registry.cleanup().is_empty());

    assert!(!abandoned_path.exists());
    assert!(!committed_path.parent().unwrap().exists());
    assert!(registry.stage_archive_file(&root).is_err());
    assert_eq!(read(&destination), "complete script");
}

/// Cleanup removes registered temporaries and the run child, but never a
/// published destination, and a second pass finds nothing.
#[test]
fn cleanup_removes_the_run_child_and_keeps_committed_output() {
    let root = mod_root("cleanup-run-child");
    let destination = root.join("texture.dds");
    let mut registry = registry();
    let receipt = registry
        .capture_and_stage_file(&root, &destination)
        .unwrap();
    let staged_path = staged(&receipt);
    std::fs::write(&staged_path, "committed").unwrap();
    let publication = receipt.publish(&destination, PublicationPolicy::Replace);
    assert_eq!(
        publication.state,
        PublicationState::PublishedAndReleased,
        "{}",
        publication.error_detail
    );
    let uncommitted = registry
        .capture_and_stage_file(&root, &root.join("other.dds"))
        .unwrap();
    let uncommitted_path = staged(&uncommitted);
    drop(uncommitted);

    assert!(registry.cleanup().is_empty());
    assert!(!staged_path.exists());
    assert!(!uncommitted_path.exists());
    assert_eq!(read(&destination), "committed");
    // The control files stay for the next run; the run child is gone.
    let left: Vec<String> = std::fs::read_dir(staging(&root))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(left, ["owner.lock", "ownership.manifest"]);
    assert!(registry.cleanup().is_empty());
}

/// The writer emits the documented v3 grammar: the canonical generic Mod
/// Root, the Run ID and its run child, then `D` and `S` records in
/// registration order.
#[test]
fn the_manifest_is_v3_with_the_run_child_first() {
    let root = mod_root("manifest-v3");
    std::fs::create_dir(root.join("textures")).unwrap();
    let run_id = create_run_id();
    let mut registry = TemporaryArtifactRegistry::new(run_id.clone());
    let sibling = registry
        .stage_file(&root, &root.join("textures").join("example.dds"))
        .unwrap();

    let text = manifest(&root);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 6, "{text}");
    assert_eq!(lines[0], "CAO-STAGING 3");
    let generic_root = root.to_str().unwrap().replace('\\', "/");
    assert_eq!(lines[1], format!("\"{generic_root}\""));
    let child = lines[2]
        .strip_prefix(&format!("\"{run_id}\" \"run-{run_id}-"))
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or_else(|| panic!("unexpected run line: {}", lines[2]));
    assert!(is_nonce(child), "{child}");
    assert_eq!(lines[3], "2");
    assert_eq!(lines[4], format!("D \"run-{run_id}-{child}\""));
    let name = sibling.path.file_name().unwrap().to_string_lossy();
    assert_eq!(lines[5], format!("S \"textures/{name}\""));
    let nonce = name
        .strip_prefix(&format!(".cao-staging-texture-{run_id}-"))
        .and_then(|rest| rest.strip_suffix(".dds"))
        .unwrap();
    assert!(is_nonce(nonce), "{name}");
    assert!(text.ends_with('\n'));
    assert!(registry.cleanup().is_empty());
}

fn is_nonce(text: &str) -> bool {
    text.len() == 32
        && text
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// Uppercase native destinations still stage a sibling with the canonical
/// lowercase extension, beside the destination, in the Asset's namespace.
#[test]
fn siblings_use_their_asset_namespace_and_a_lowercase_extension() {
    let root = mod_root("sibling-names");
    for (relative, prefix, extension) in [
        ("Texture.DDS", ".cao-staging-texture-", "dds"),
        ("meshes/Mesh.NIF", ".cao-staging-mesh-", "nif"),
        ("meshes/Terrain.BTR", ".cao-staging-mesh-", "btr"),
        ("meshes/Terrain.BTO", ".cao-staging-mesh-", "bto"),
        ("animations/Walk.HKX", ".cao-staging-animation-", "hkx"),
    ] {
        let destination = root.join(relative);
        std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
        std::fs::write(&destination, "original").unwrap();
        let mut registry = registry();
        let sibling = registry.stage_file(&root, &destination).unwrap().path;
        assert_eq!(sibling.parent(), destination.parent());
        assert_eq!(sibling.extension().unwrap(), extension);
        let name = sibling.file_name().unwrap().to_string_lossy();
        assert!(name.starts_with(prefix), "{name}");
        assert!(registry.cleanup().is_empty());
        assert!(!sibling.exists());
        assert_eq!(read(&destination), "original");
        // Each registry's leftover staging blocks the next until #492; clear
        // it once the registry has released its pins and lock.
        drop(registry);
        std::fs::remove_dir_all(staging(&root)).unwrap();
    }
}

/// A Run ID outside the manifest grammar never names staging.
#[test]
fn a_malformed_run_id_creates_no_staging() {
    let root = mod_root("malformed-run-id");
    for run_id in ["", "has space", "slash/run", &"x".repeat(129)] {
        let mut registry = TemporaryArtifactRegistry::new(run_id.to_owned());
        assert!(registry.stage_archive_file(&root).is_err(), "{run_id:?}");
        assert!(!staging(&root).exists(), "{run_id:?}");
    }
}

/// Only DDS, NIF, BTR, BTO and HKX outputs can be staged as siblings.
#[test]
fn other_extensions_cannot_be_staged_as_siblings() {
    let root = mod_root("sibling-extension");
    let mut registry = registry();
    assert!(
        registry
            .stage_file(&root, &root.join("plugin.esp"))
            .is_err()
    );
    assert!(!staging(&root).exists(), "nothing is created for a refusal");
}

/// A reserved name without proven ownership is never adopted by a producer.
#[test]
fn an_unowned_reserved_directory_is_rejected() {
    let root = mod_root("unowned-bootstrap");
    std::fs::create_dir(staging(&root)).unwrap();
    let mut registry = registry();
    assert!(
        registry
            .stage_file(&root, &root.join("texture.dds"))
            .is_err()
    );
    assert!(std::fs::read_dir(staging(&root)).unwrap().next().is_none());
}

/// One damaged entry does not stop cleanup of other registered temporaries,
/// and unregistered contents of the damaged one survive.
#[test]
fn cleanup_continues_after_a_damaged_temporary() {
    let root = mod_root("damaged-temporary");
    let mut registry = registry();
    let intact = registry.stage_file(&root, &root.join("first.dds")).unwrap();
    let damaged = registry
        .stage_file(&root, &root.join("second.dds"))
        .unwrap();
    std::fs::remove_file(&damaged.path).unwrap();
    std::fs::create_dir(&damaged.path).unwrap();
    std::fs::write(damaged.path.join("unregistered"), "keep").unwrap();

    let failures = registry.cleanup();

    assert_eq!(failures.len(), 1, "{failures:?}");
    assert_eq!(
        failures[0].code,
        RunFailureCode::TemporaryArtifactCleanupFailed
    );
    assert_eq!(failures[0].path, damaged.path);
    assert!(!intact.path.exists());
    assert!(damaged.path.join("unregistered").exists());
}

/// Every ancestor of a staged Texture, Mesh or Animation sibling stays
/// pinned against rename from staging through Safety Cleanup, so none can be
/// swapped for a junction.
#[test]
fn nested_staged_parents_stay_pinned_through_safety_cleanup() {
    for extension in ["dds", "nif", "hkx"] {
        let root = mod_root(&format!("nested-pins-{extension}"));
        let ancestor = root.join("assets");
        let parent = ancestor.join("nested");
        std::fs::create_dir_all(&parent).unwrap();
        let destination = parent.join(format!("source.{extension}"));
        {
            let mut registry = registry();
            let receipt = registry
                .capture_and_stage_file(&root, &destination)
                .unwrap();
            let staged_path = staged(&receipt);
            std::fs::write(&staged_path, "unpublished").unwrap();
            drop(receipt);

            // A pin on the immediate parent alone would still let a higher
            // ancestor be swapped.
            assert!(
                renaming_denied(&parent),
                "parent replaceable before cleanup"
            );
            assert!(
                renaming_denied(&ancestor),
                "ancestor replaceable before cleanup"
            );
            assert!(registry.cleanup().is_empty());
            assert!(!staged_path.exists());
            assert!(renaming_denied(&parent), "parent replaceable after cleanup");
            assert!(
                renaming_denied(&ancestor),
                "ancestor replaceable after cleanup"
            );
        }
        std::fs::rename(&parent, ancestor.join("shifted")).expect("pins end with the registry");
    }
}

/// A clean Apply root is pinned against rename by Preparing's check, before
/// any staging exists, and released with the registry.
#[test]
fn a_prepared_clean_root_cannot_be_renamed() {
    let root = mod_root("clean-root-pin");
    {
        let mut registry = registry();
        let failure = registry
            .prepare_root(&root, &CancellationToken::new())
            .unwrap();
        assert!(failure.is_none(), "{failure:?}");
        assert!(!staging(&root).exists(), "preparing never creates staging");
        assert!(renaming_denied(&root));
    }
    assert!(!renaming_denied(&root));
}

/// A generic commit cannot release durable staging, before or after a
/// separate move; only a receipt can.
#[test]
fn a_generic_commit_cannot_release_a_durable_stage() {
    let root = mod_root("generic-commit");
    let destination = root.join("texture.dds");
    std::fs::write(&destination, "original").unwrap();
    let mut registry = registry();
    let sibling = registry.stage_file(&root, &destination).unwrap();
    assert!(registry.commit(sibling.registration).is_err());
    assert!(sibling.path.exists());
    let archive = registry.stage_archive_file(&root).unwrap();
    assert!(registry.commit(archive.registration).is_err());
    let moved = root.join("separately-moved.dds");
    std::fs::rename(&sibling.path, &moved).unwrap();
    assert!(registry.commit(sibling.registration).is_err());
    // Restoring the registered name makes retained ownership observable.
    std::fs::rename(&moved, &sibling.path).unwrap();

    assert!(registry.cleanup().is_empty());
    assert!(!sibling.path.exists());
    assert!(!archive.path.exists());
    assert_eq!(read(&destination), "original");
}

/// A non-durable registration is removed by cleanup unless committed.
#[test]
fn registered_artifacts_are_removed_unless_committed() {
    let root = mod_root("registered-artifacts");
    let mut registry = registry();
    let removed = root.join("scratch.tmp");
    let retained = root.join("retained.tmp");
    let first = registry.register_artifact(&removed).unwrap();
    let second = registry.register_artifact(&retained).unwrap();
    assert_ne!(first, second);
    std::fs::write(&removed, "scratch").unwrap();
    std::fs::write(&retained, "kept").unwrap();
    registry.commit(second).unwrap();
    assert!(registry.commit(second).is_err(), "a commit is one-use");
    // An existing entry, or one already registered, is never adopted.
    assert!(registry.register_artifact(&retained).is_err());
    assert!(
        registry
            .register_artifact(&root.join("SCRATCH.tmp"))
            .is_err()
    );

    assert!(registry.cleanup().is_empty());
    assert!(!removed.exists());
    assert_eq!(read(&retained), "kept");
}

/// Both publication policies commit staged bytes, report a safe Committed
/// Mutation, and release only the temporary path.
#[test]
fn both_publication_policies_preserve_destination_bytes() {
    let root = mod_root("policies");
    let asset_destination = root.join("texture.dds");
    let archive_destination = root.join("entry.pex");
    std::fs::write(&asset_destination, "old asset").unwrap();
    let mut registry = registry();

    let asset = registry
        .capture_and_stage_file(&root, &asset_destination)
        .unwrap();
    let asset_temporary = staged(&asset);
    std::fs::write(&asset_temporary, "new asset").unwrap();
    let asset_result = asset.publish(&asset_destination, PublicationPolicy::Replace);
    assert_eq!(asset_result.state, PublicationState::PublishedAndReleased);
    assert!(asset_result.error_detail.is_empty());
    assert_eq!(asset_result.mutation(), MutationState::Committed);
    assert!(asset_result.safe_to_continue());

    let archive = registry.stage_archive_file_for_publication(&root).unwrap();
    let archive_temporary = staged(&archive);
    std::fs::write(&archive_temporary, "new archive entry").unwrap();
    let archive_result = archive.publish(&archive_destination, PublicationPolicy::NoReplace);
    assert_eq!(archive_result.state, PublicationState::PublishedAndReleased);
    assert_eq!(archive_result.mutation(), MutationState::Committed);
    assert!(archive_result.safe_to_continue());

    // Both temporaries are released: only the run child remains registered.
    assert_eq!(manifest(&root).lines().nth(3), Some("1"));
    assert!(registry.cleanup().is_empty());
    assert!(!asset_temporary.exists());
    assert!(!archive_temporary.exists());
    assert_eq!(read(&asset_destination), "new asset");
    assert_eq!(read(&archive_destination), "new archive entry");
}

/// No-replace publication refuses a destination occupied after staging, and
/// the competing file survives.
#[test]
fn an_occupied_destination_is_not_published() {
    let root = mod_root("occupied");
    let destination = root.join("entry.pex");
    let mut registry = registry();
    let receipt = registry.stage_archive_file_for_publication(&root).unwrap();
    let temporary = staged(&receipt);
    std::fs::write(&temporary, "new entry").unwrap();
    // Occupied after staging: only the final native rename can arbitrate it.
    std::fs::write(&destination, "competing entry").unwrap();

    let result = receipt.publish(&destination, PublicationPolicy::NoReplace);

    assert_eq!(result.state, PublicationState::NotPublished);
    assert!(!result.error_detail.is_empty());
    assert_eq!(result.mutation(), MutationState::None);
    assert!(result.safe_to_continue());
    assert!(temporary.exists());
    assert!(registry.cleanup().is_empty());
    assert!(!temporary.exists());
    assert_eq!(read(&destination), "competing entry");
}

/// A missing staged file fails before any destination mutation, and a file
/// recreated at the owned name afterwards is still cleaned up.
#[test]
fn an_unavailable_staged_file_is_not_published() {
    let root = mod_root("unavailable-staged");
    let destination = root.join("texture.dds");
    std::fs::write(&destination, "original").unwrap();
    let mut registry = registry();
    let receipt = registry
        .capture_and_stage_file(&root, &destination)
        .unwrap();
    let temporary = staged(&receipt);
    std::fs::remove_file(&temporary).unwrap();

    let result = receipt.publish(&destination, PublicationPolicy::Replace);

    assert_eq!(result.state, PublicationState::NotPublished);
    assert!(!result.error_detail.is_empty());
    // The receipt is consumed; the type system rules out a second publish.
    std::fs::write(&temporary, "late bytes").unwrap();
    assert!(registry.cleanup().is_empty());
    assert!(!temporary.exists());
    assert_eq!(read(&destination), "original");
}

/// A receipt moves, and publishing consumes it.
#[test]
fn a_receipt_moves_and_publishes_once() {
    let root = mod_root("receipt-moves");
    let destination = root.join("texture.dds");
    let mut registry = registry();
    let original = registry
        .capture_and_stage_file(&root, &destination)
        .unwrap();
    let receipt = original;
    std::fs::write(staged(&receipt), "once").unwrap();
    let result = receipt.publish(&destination, PublicationPolicy::Replace);
    assert_eq!(result.state, PublicationState::PublishedAndReleased);
    assert!(registry.cleanup().is_empty());
    assert_eq!(read(&destination), "once");
}

/// A receipt cannot act for a registry that has been dropped; its staged
/// file stays owned by the manifest for recovery.
#[test]
fn an_ended_scope_cannot_publish() {
    let root = mod_root("ended-scope");
    let destination = root.join("entry.pex");
    let (receipt, temporary) = {
        let mut registry = registry();
        let receipt = registry.stage_archive_file_for_publication(&root).unwrap();
        let temporary = staged(&receipt);
        std::fs::write(&temporary, "unpublished").unwrap();
        (receipt, temporary)
    };
    assert!(receipt.path().is_err());

    let result = receipt.publish(&destination, PublicationPolicy::NoReplace);

    assert_eq!(result.state, PublicationState::NotPublished);
    assert!(!result.error_detail.is_empty());
    assert!(!destination.exists());
    assert!(temporary.exists());
    let relative = temporary.strip_prefix(staging(&root)).unwrap();
    let record = format!("F \"{}\"", relative.to_str().unwrap().replace('\\', "/"));
    assert!(manifest(&root).contains(&record), "{}", manifest(&root));
}

/// Asset publication cannot redirect its staged bytes to another destination.
#[test]
fn asset_publication_rejects_a_changed_destination() {
    let root = mod_root("changed-destination");
    let intended = root.join("intended.dds");
    let redirected = root.join("redirected.dds");
    let mut registry = registry();
    let receipt = registry.capture_and_stage_file(&root, &intended).unwrap();
    let temporary = staged(&receipt);
    std::fs::write(&temporary, "staged").unwrap();

    let result = receipt.publish(&redirected, PublicationPolicy::Replace);

    assert_eq!(result.state, PublicationState::NotPublished);
    // A preflight rejection leaves every destination untouched.
    assert_eq!(result.mutation(), MutationState::None);
    assert!(result.safe_to_continue());
    assert!(!intended.exists());
    assert!(!redirected.exists());
    assert!(temporary.exists());
    assert!(registry.cleanup().is_empty());
    assert!(!temporary.exists());
}

/// Archive publication rejects destinations outside the Mod Root, inside the
/// reserved namespace, relative, or naming a DOS device (deviation 15 trims
/// trailing spaces from the device stem).
#[test]
fn archive_publication_rejects_unsafe_destinations() {
    let root = mod_root("unsafe-destinations");
    let outside = mod_root("unsafe-destinations-outside");
    let mut registry = registry();
    let unsafe_destinations = [
        outside.join("escape.pex"),
        staging(&root).join("reserved.pex"),
        PathBuf::from("relative.pex"),
        root.join("NUL.pex"),
        root.join("NUL .pex"),
        root.join("entry.pex."),
        root.join("entry.pex:stream"),
    ];
    let mut temporaries = Vec::new();
    for destination in &unsafe_destinations {
        let receipt = registry.stage_archive_file_for_publication(&root).unwrap();
        let temporary = staged(&receipt);
        std::fs::write(&temporary, "staged").unwrap();
        let result = receipt.publish(destination, PublicationPolicy::NoReplace);
        assert_eq!(
            result.state,
            PublicationState::NotPublished,
            "{}",
            destination.display()
        );
        assert!(!result.error_detail.is_empty());
        assert_eq!(result.mutation(), MutationState::None);
        assert!(result.safe_to_continue());
        temporaries.push(temporary);
    }
    assert!(!outside.join("escape.pex").exists());
    assert!(registry.cleanup().is_empty());
    assert!(temporaries.iter().all(|temporary| !temporary.exists()));
}

/// A junctioned destination parent cannot redirect committed bytes outside
/// the Mod Root.
#[test]
fn archive_publication_rejects_a_linked_parent() {
    let root = mod_root("linked-parent");
    let outside = mod_root("linked-parent-outside");
    let linked = root.join("linked");
    common::junction(&linked, &outside);
    let mut registry = registry();
    let receipt = registry.stage_archive_file_for_publication(&root).unwrap();
    let temporary = staged(&receipt);
    std::fs::write(&temporary, "staged").unwrap();

    let result = receipt.publish(&linked.join("entry.pex"), PublicationPolicy::NoReplace);

    assert_eq!(result.state, PublicationState::NotPublished);
    assert!(!outside.join("entry.pex").exists());
    assert!(registry.cleanup().is_empty());
    assert!(!temporary.exists());
    std::fs::remove_dir(&linked).unwrap();
}

/// A staged Asset's parent stays pinned, so it cannot be replaced before
/// publication; C++ skipped its rejection branch on such file systems too.
#[test]
fn an_asset_parent_cannot_be_replaced_after_staging() {
    let root = mod_root("replaced-parent");
    let parent = root.join("assets");
    std::fs::create_dir(&parent).unwrap();
    let destination = parent.join("texture.dds");
    let mut registry = registry();
    let receipt = registry
        .capture_and_stage_file(&root, &destination)
        .unwrap();
    std::fs::write(staged(&receipt), "staged").unwrap();

    assert!(std::fs::rename(&parent, root.join("shifted-assets")).is_err());

    let result = receipt.publish(&destination, PublicationPolicy::Replace);
    assert_eq!(result.state, PublicationState::PublishedAndReleased);
    assert!(registry.cleanup().is_empty());
}

/// Replacement refuses a different leaf at the same name and parent.
#[test]
fn asset_publication_rejects_a_replaced_destination() {
    let root = mod_root("replaced-destination");
    let destination = root.join("texture.dds");
    let old = root.join("old-texture.dds");
    std::fs::write(&destination, "original").unwrap();
    let mut registry = registry();
    let receipt = registry
        .capture_and_stage_file(&root, &destination)
        .unwrap();
    let temporary = staged(&receipt);
    std::fs::write(&temporary, "optimized original").unwrap();
    std::fs::rename(&destination, &old).unwrap();
    std::fs::write(&destination, "newcomer").unwrap();

    let result = receipt.publish(&destination, PublicationPolicy::Replace);

    assert_eq!(result.state, PublicationState::NotPublished);
    assert!(!result.error_detail.is_empty());
    assert_eq!(read(&destination), "newcomer");
    assert_eq!(read(&old), "original");
    assert!(registry.cleanup().is_empty());
    assert!(!temporary.exists());
}

/// A snapshot denies delete sharing, so it conflicts with an open handle
/// that could rename the destination while its bytes are read.
#[test]
fn a_destination_snapshot_rejects_an_open_rename_handle() {
    let root = mod_root("rename-handle");
    let destination = root.join("texture.dds");
    std::fs::write(&destination, "original").unwrap();
    let rename_handle = Open::new(Access::DELETE, Share::READ | Share::WRITE | Share::DELETE)
        .open(&destination)
        .unwrap();
    let registry = registry();

    let error = registry
        .capture_publication_target(&root, &destination)
        .unwrap_err();

    assert!(error.to_string().contains("os error 32"), "{error}");
    drop(rename_handle);
    let _recaptured = registry
        .capture_publication_target(&root, &destination)
        .unwrap();
}

/// A same-size in-place edit with the write time restored cannot be replaced
/// by output made from the stale bytes: the content fingerprint differs.
#[test]
fn asset_publication_rejects_an_in_place_destination_edit() {
    let root = mod_root("in-place-edit");
    let destination = root.join("texture.dds");
    std::fs::write(&destination, "original").unwrap();
    let modified = std::fs::metadata(&destination).unwrap().modified().unwrap();
    let mut registry = registry();
    let target = registry
        .capture_publication_target(&root, &destination)
        .unwrap();
    assert_eq!(read(&destination), "original");
    let receipt = registry.stage_file_for_publication(target).unwrap();
    let temporary = staged(&receipt);
    std::fs::write(&temporary, "optimized original").unwrap();
    std::fs::write(&destination, "revised!").unwrap();
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(&destination)
        .unwrap();
    file.set_modified(modified).unwrap();
    drop(file);

    let result = receipt.publish(&destination, PublicationPolicy::Replace);

    assert_eq!(result.state, PublicationState::NotPublished);
    assert!(
        result
            .error_detail
            .contains("Publication destination changed after input capture"),
        "{}",
        result.error_detail
    );
    assert!(registry.cleanup().is_empty());
    assert!(!temporary.exists());
    assert_eq!(read(&destination), "revised!");
}

/// A failed release after publication keeps the committed destination,
/// reports it as Committed but unsafe, and leaves ownership with the manifest
/// while the run still holds the lock.
#[test]
fn a_release_failure_preserves_the_committed_destination() {
    let root = mod_root("release-failure");
    let destination = root.join("entry.pex");
    let scratch = staging(&root).join("ownership.manifest.next");
    let mut registry = registry();
    let receipt = registry.stage_archive_file_for_publication(&root).unwrap();
    let temporary = staged(&receipt);
    std::fs::write(&temporary, "committed entry").unwrap();
    // The fixed scratch name blocks only the post-publication release.
    std::fs::write(&scratch, "occupied scratch").unwrap();

    let result = receipt.publish(&destination, PublicationPolicy::NoReplace);

    assert_eq!(
        result.state,
        PublicationState::PublishedStillOwned,
        "{}",
        result.error_detail
    );
    assert!(!result.error_detail.is_empty());
    assert_eq!(result.mutation(), MutationState::Committed);
    assert!(!result.safe_to_continue());
    assert!(!temporary.exists());
    assert_eq!(read(&destination), "committed entry");
    assert!(registry.cleanup().is_empty());

    let mut contender = TemporaryArtifactRegistry::new(create_run_id());
    let active = contender
        .prepare_root(&root, &CancellationToken::new())
        .unwrap()
        .expect("the owning run still holds the lock");
    assert_eq!(active.code, RunFailureCode::StagingActive);
    assert_eq!(active.path, staging(&root).join("owner.lock"));
}

/// Preparing fails closed on staging it cannot yet recover (#492), and on
/// unknown names in the reserved namespace, without touching either.
#[test]
fn preparing_fails_closed_on_existing_or_unknown_staging() {
    let root = mod_root("existing-staging");
    {
        let mut producer = registry();
        let sibling = producer
            .stage_file(&root, &root.join("texture.dds"))
            .unwrap();
        std::fs::write(&sibling.path, "partial").unwrap();
    }
    let before = common::snapshot_tree(&root);
    let mut next = registry();
    let failure = next
        .prepare_root(&root, &CancellationToken::new())
        .unwrap()
        .expect("existing staging is not adopted");
    assert_eq!(failure.code, RunFailureCode::StagingOwnershipUnverified);
    assert!(
        failure.detail.contains("Inspect ownership.manifest"),
        "{}",
        failure.detail
    );
    assert_eq!(common::snapshot_tree(&root), before);

    let unknown_root = mod_root("unknown-staging");
    std::fs::create_dir(unknown_root.join(".CAO-STAGING-old")).unwrap();
    let failure = registry()
        .prepare_root(&unknown_root, &CancellationToken::new())
        .unwrap()
        .expect("an unknown staging-like name fails closed");
    assert_eq!(failure.code, RunFailureCode::StagingOwnershipUnverified);
    assert_eq!(failure.path, unknown_root.join(".CAO-STAGING-old"));
}

/// The child-process producer: what it does is chosen by
/// `CAO_STAGING_CHILD`, and it waits to be killed after reporting.
#[test]
fn staging_crash_child() {
    let (Ok(mode), Ok(root), Ok(report)) = (
        std::env::var("CAO_STAGING_CHILD"),
        std::env::var("CAO_STAGING_ROOT"),
        std::env::var("CAO_STAGING_REPORT"),
    ) else {
        return;
    };
    let root = PathBuf::from(root);
    let destination = root.join("texture.dds");
    let mut registry = registry();
    let receipt = registry
        .capture_and_stage_file(&root, &destination)
        .unwrap();
    let message = match mode.as_str() {
        // Killed with half-written output in its staged sibling.
        "before-publication" => {
            std::fs::write(staged(&receipt), "half-writ").unwrap();
            staged(&receipt).display().to_string()
        }
        // Killed after the destination was published but before the
        // manifest released the temporary name.
        "after-publication" => {
            std::fs::write(staged(&receipt), "converted").unwrap();
            let scratch = staging(&root).join("ownership.manifest.next");
            std::fs::write(scratch, "occupied scratch").unwrap();
            let result = receipt.publish(&destination, PublicationPolicy::Replace);
            assert_eq!(result.state, PublicationState::PublishedStillOwned);
            "published-still-owned".to_owned()
        }
        other => panic!("unknown child mode {other}"),
    };
    // A report file, renamed into place so the parent never reads it half
    // written. libtest's stdout is not a reliable channel for a child here.
    let report = PathBuf::from(report);
    let pending = report.with_extension("pending");
    std::fs::write(&pending, message).unwrap();
    std::fs::rename(&pending, &report).unwrap();
    // Bounded, so a parent that failed before killing it cannot leave it
    // running for long.
    std::thread::sleep(Duration::from_secs(60));
}

/// A producer process, killed when dropped so a failing parent never leaks it.
struct CrashChild(Child);

impl CrashChild {
    /// Kills the producer, as a crash would, and waits for it to exit.
    fn kill(&mut self) {
        // It may already have exited; either way it is gone afterwards.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Drop for CrashChild {
    fn drop(&mut self) {
        self.kill();
    }
}

/// Starts this test binary as a producer and waits, at most 30 seconds, for
/// its report. The report and the child's output sit beside the Mod Root,
/// never inside it.
fn crash_child(mode: &str, root: &Path) -> (CrashChild, String) {
    let report = root.with_extension("report");
    let output = root.with_extension("child-output");
    // Leftovers from an earlier run would satisfy the wait below; absence is fine.
    let _ = std::fs::remove_file(&report);
    let log = File::create(&output).unwrap();
    let mut child = CrashChild(
        Command::new(std::env::current_exe().unwrap())
            .args(["staging_crash_child", "--exact", "--test-threads=1"])
            .env("CAO_STAGING_CHILD", mode)
            .env("CAO_STAGING_ROOT", root)
            .env("CAO_STAGING_REPORT", &report)
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(text) = std::fs::read_to_string(&report) {
            return (child, text);
        }
        let exited = child.0.try_wait().unwrap();
        if exited.is_some() || Instant::now() > deadline {
            child.kill();
            panic!(
                "the producer did not report ({exited:?}):\n{}",
                std::fs::read_to_string(&output).unwrap_or_default()
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Spec (#491): a producer killed after writing its staged output but before
/// publication leaves the original Texture whole; the partial bytes stay in
/// a sibling the manifest owns.
#[test]
fn a_crash_before_publication_leaves_the_texture_untouched() {
    let root = mod_root("crash-before-publication");
    let destination = root.join("texture.dds");
    std::fs::write(&destination, "original").unwrap();

    let (mut child, report) = crash_child("before-publication", &root);
    let sibling = PathBuf::from(report);
    let active = registry()
        .prepare_root(&root, &CancellationToken::new())
        .unwrap()
        .expect("the live producer owns staging");
    assert_eq!(active.code, RunFailureCode::StagingActive);
    child.kill();

    assert_eq!(read(&destination), "original");
    assert_eq!(read(&sibling), "half-writ");
    let name = sibling.file_name().unwrap().to_string_lossy();
    assert!(manifest(&root).contains(&format!("S \"{name}\"")));
}

/// Spec (#491): a producer killed between publication and the ownership
/// release leaves the complete Texture in place, never a half-written one;
/// the manifest still names the moved temporary for recovery to skip.
#[test]
fn a_crash_after_publication_keeps_the_complete_texture() {
    let root = mod_root("crash-after-publication");
    let destination = root.join("texture.dds");
    std::fs::write(&destination, "original").unwrap();

    let (mut child, _) = crash_child("after-publication", &root);
    let active = registry()
        .prepare_root(&root, &CancellationToken::new())
        .unwrap()
        .expect("the live producer owns staging");
    assert_eq!(active.code, RunFailureCode::StagingActive);
    child.kill();

    assert_eq!(read(&destination), "converted");
    let siblings: Vec<String> = std::fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(".cao-staging-"))
        .collect();
    assert!(siblings.is_empty(), "{siblings:?}");
    assert!(manifest(&root).contains("S \".cao-staging-texture-"));
}
