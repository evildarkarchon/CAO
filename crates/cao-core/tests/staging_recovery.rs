//! Recovery of leftover `CAO-STAGING` v1–v3 staging (#492), through
//! [`TemporaryArtifactRegistry::prepare_root`] over real directories.
//!
//! The manifests under `tests/fixtures/staging/` are reused byte for byte,
//! except for the Mod Root line, which must name the scratch Mod Root (see the
//! README there). Two were written by the C++ oracle, so a C++ run's leftover
//! recovers in the port. The C++ suites had no v1 or v2 fixture.
//!
//! `durable_staging.rs` ports the recovery scenarios of C++
//! `DurableStagingTests`; this file covers the format and its failure codes.

mod common;

use std::path::{Path, PathBuf};

use cao_core::run::{
    CancellationToken, RunFailure, RunFailureCode, RunPhase, TemporaryArtifactRegistry,
    create_run_id,
};
use common::{canonical, scratch_dir, snapshot_tree};

const V1: &[u8] = include_bytes!("fixtures/staging/v1.manifest");
const V2: &[u8] = include_bytes!("fixtures/staging/v2.manifest");
const ORACLE_COMPLETED: &[u8] = include_bytes!("fixtures/staging/oracle-v3-completed.manifest");
const ORACLE_KILLED: &[u8] = include_bytes!("fixtures/staging/oracle-v3-killed.manifest");

/// The run child every v2 fixture record lives under.
const V2_CHILD: &str = "run-4f1c2a9e-0123456789abcdef0123456789abcdef";
const V2_ENTRY: &str = "archive-entry-00112233445566778899aabbccddeeff";
const V2_NESTED: &str = "extracted/archive-entry-ffeeddccbbaa99887766554433221100";
/// The guidance on a failure that recovery could not verify.
const INSPECT: &str = "Leave the contents in place. Inspect ownership.manifest and move unrecognized material \
     outside .cao-staging before retrying.";

/// A fresh, canonical Mod Root.
fn mod_root(name: &str) -> PathBuf {
    canonical(&scratch_dir(&format!("staging-recovery/{name}")))
}

fn staging(root: &Path) -> PathBuf {
    root.join(".cao-staging")
}

/// `fixture` with its second line, the recorded Mod Root, naming `root`.
fn rehome(fixture: &[u8], root: &Path) -> Vec<u8> {
    let first = fixture.iter().position(|&byte| byte == b'\n').unwrap();
    let second = first
        + 1
        + fixture[first + 1..]
            .iter()
            .position(|&byte| byte == b'\n')
            .unwrap();
    let generic = root.to_str().unwrap().replace('\\', "/");
    let mut bytes = fixture[..=first].to_vec();
    bytes.extend_from_slice(format!("\"{generic}\"").as_bytes());
    bytes.extend_from_slice(&fixture[second..]);
    bytes
}

/// The name in `manifest`'s first record of `kind` (`D`, `F` or `S`), which
/// the fixtures write as `<kind> "<name>"` with nothing to unescape.
fn record(manifest: &str, kind: char) -> &str {
    let prefix = format!("{kind} \"");
    manifest
        .lines()
        .find_map(|line| line.strip_prefix(&prefix)?.strip_suffix('"'))
        .unwrap_or_else(|| panic!("no {kind} record in:\n{manifest}"))
}

/// Writes the control files: an empty `owner.lock` and `manifest`.
fn write_controls(root: &Path, manifest: &[u8]) {
    std::fs::create_dir_all(staging(root)).unwrap();
    std::fs::write(staging(root).join("owner.lock"), b"").unwrap();
    std::fs::write(staging(root).join("ownership.manifest"), manifest).unwrap();
}

/// A v2 leftover: the fixture's manifest, its run child and every entry it
/// registers, each holding bytes.
fn v2_leftover(name: &str) -> PathBuf {
    let root = mod_root(name);
    write_controls(&root, &rehome(V2, &root));
    let child = staging(&root).join(V2_CHILD);
    std::fs::create_dir_all(child.join("extracted")).unwrap();
    std::fs::write(child.join(V2_ENTRY), "extracted script").unwrap();
    std::fs::write(child.join(V2_NESTED), "extracted mesh").unwrap();
    std::fs::write(root.join("unrelated.esp"), "plugin").unwrap();
    root
}

/// Recovers `root` with a fresh registry, returning any failure. The
/// registry is returned too, because it holds the recovered lock and pins.
fn recover(root: &Path) -> (TemporaryArtifactRegistry, Option<RunFailure>) {
    let mut registry = TemporaryArtifactRegistry::new(create_run_id());
    let failure = registry
        .prepare_root(root, &CancellationToken::new())
        .unwrap();
    (registry, failure)
}

/// The names directly inside `directory`, sorted.
fn names(directory: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// Asserts a fail-closed `StagingOwnershipUnverified` failure naming `path`,
/// which tells the user what to do.
fn assert_unverified(failure: Option<RunFailure>, path: &Path) -> RunFailure {
    let failure = failure.expect("unverifiable staging fails closed");
    assert_eq!(
        failure.code,
        RunFailureCode::StagingOwnershipUnverified,
        "{failure:?}"
    );
    assert_eq!(failure.phase, RunPhase::Preparing);
    assert_eq!(failure.path, path, "{}", failure.detail);
    assert!(failure.detail.ends_with(INSPECT), "{}", failure.detail);
    failure
}

/// Spec (#492): a literal v2 manifest recovers. Every registered entry goes,
/// children before parents, and so does the interrupted scratch snapshot that
/// v2 authorizes; both control files stay byte for byte, and nothing outside
/// the registrations is touched.
#[test]
fn a_literal_v2_manifest_is_recovered_with_its_scratch() {
    let root = v2_leftover("v2");
    let scratch = staging(&root).join("ownership.manifest.next");
    std::fs::write(&scratch, "CAO-STAGING 3\n\"trunc").unwrap();
    let manifest = std::fs::read(staging(&root).join("ownership.manifest")).unwrap();

    let (registry, failure) = recover(&root);

    assert!(failure.is_none(), "{failure:?}");
    assert_eq!(names(&staging(&root)), ["owner.lock", "ownership.manifest"]);
    assert_eq!(
        std::fs::read(staging(&root).join("ownership.manifest")).unwrap(),
        manifest
    );
    assert_eq!(names(&root), [".cao-staging", "unrelated.esp"]);
    // The run keeps the recovered lock, exclusively, until it ends.
    let lock = staging(&root).join("owner.lock");
    let held = std::fs::read(&lock).unwrap_err();
    assert!(cao_winfs::is_sharing_violation(&held), "{held}");
    drop(registry);
    assert_eq!(std::fs::read(&lock).unwrap(), b"");
}

/// Spec (#492): a v1 manifest recovers too, but it never authorized the
/// scratch snapshot name, so scratch beside it is unverified and nothing is
/// deleted.
#[test]
fn a_literal_v1_manifest_is_recovered_but_cannot_own_scratch() {
    let child = "run-7d0e-fedcba9876543210fedcba9876543210";
    let entry = "archive-entry-0f1e2d3c4b5a69788796a5b4c3d2e1f0";
    let leftover = |name: &str| {
        let root = mod_root(name);
        write_controls(&root, &rehome(V1, &root));
        std::fs::create_dir(staging(&root).join(child)).unwrap();
        std::fs::write(staging(&root).join(child).join(entry), "bytes").unwrap();
        root
    };

    let root = leftover("v1");
    let (_registry, failure) = recover(&root);
    assert!(failure.is_none(), "{failure:?}");
    assert_eq!(names(&staging(&root)), ["owner.lock", "ownership.manifest"]);

    let root = leftover("v1-scratch");
    let scratch = staging(&root).join("ownership.manifest.next");
    std::fs::write(&scratch, "partial").unwrap();
    let before = snapshot_tree(&root);
    let (_registry, failure) = recover(&root);
    let failure = assert_unverified(failure, &scratch);
    assert!(
        failure
            .detail
            .starts_with("Staging contains an unregistered entry."),
        "{}",
        failure.detail
    );
    assert_eq!(snapshot_tree(&root), before);
}

/// Spec (#492): the leftover of a C++ Apply run that finished, captured from
/// the oracle, recovers. Its manifest still records the run child Safety
/// Cleanup removed. The recovered area then serves this run's staging.
#[test]
fn a_completed_cpp_leftover_from_the_oracle_is_recovered() {
    let root = mod_root("oracle-completed");
    let manifest = rehome(ORACLE_COMPLETED, &root);
    write_controls(&root, &manifest);
    std::fs::create_dir(root.join("textures")).unwrap();
    std::fs::write(root.join("textures/a.dds"), "optimized by C++").unwrap();

    let (mut registry, failure) = recover(&root);

    assert!(failure.is_none(), "{failure:?}");
    assert_eq!(
        std::fs::read(staging(&root).join("ownership.manifest")).unwrap(),
        manifest
    );
    // The recovered area is reused: its manifest now records this run.
    let sibling = registry
        .stage_file(&root, &root.join("textures/a.dds"))
        .unwrap();
    let text = std::fs::read_to_string(staging(&root).join("ownership.manifest")).unwrap();
    assert!(text.starts_with("CAO-STAGING 3\n"), "{text}");
    assert!(
        text.contains("S \"textures/.cao-staging-texture-"),
        "{text}"
    );
    assert!(registry.cleanup().is_empty());
    assert!(!sibling.path.exists());
    assert_eq!(
        std::fs::read_to_string(root.join("textures/a.dds")).unwrap(),
        "optimized by C++"
    );
}

/// Spec (#492): the leftover of a C++ Apply run killed mid-run, captured from
/// the oracle, recovers. Its unpublished Texture sibling and run child go,
/// and the Textures beside them stay.
#[test]
fn a_killed_cpp_leftover_from_the_oracle_is_recovered() {
    let root = mod_root("oracle-killed");
    let manifest = rehome(ORACLE_KILLED, &root);
    write_controls(&root, &manifest);
    let text = String::from_utf8(manifest.clone()).unwrap();
    let child = record(&text, 'D');
    let sibling = root.join(record(&text, 'S').replace('/', "\\"));
    std::fs::create_dir(staging(&root).join(child)).unwrap();
    std::fs::create_dir_all(sibling.parent().unwrap()).unwrap();
    std::fs::write(&sibling, "complete staged Texture").unwrap();
    let original = sibling.with_file_name("t002.dds");
    std::fs::write(&original, "original Texture").unwrap();

    let (_registry, failure) = recover(&root);

    assert!(failure.is_none(), "{failure:?}");
    assert!(!sibling.exists());
    assert_eq!(names(&staging(&root)), ["owner.lock", "ownership.manifest"]);
    assert_eq!(
        std::fs::read(staging(&root).join("ownership.manifest")).unwrap(),
        manifest
    );
    assert_eq!(
        std::fs::read_to_string(&original).unwrap(),
        "original Texture"
    );
}

/// `S` records belong to v3; a v2 manifest naming a sibling is unverified.
#[test]
fn a_sibling_record_needs_version_3() {
    let root = mod_root("v2-sibling");
    let text = String::from_utf8(rehome(ORACLE_KILLED, &root)).unwrap();
    let manifest = staging(&root).join("ownership.manifest");
    write_controls(
        &root,
        text.replacen("CAO-STAGING 3", "CAO-STAGING 2", 1)
            .as_bytes(),
    );

    let (_registry, failure) = recover(&root);

    assert_unverified(failure, &manifest);
}

/// Spec (#492): a manifest of exactly 8 MiB is read, and one byte more is
/// not trusted. Like C++, an untrusted manifest is `StagingOwnershipUnverified`
/// naming the manifest, and nothing is deleted.
#[test]
fn the_manifest_size_bound_is_8_mib() {
    for (name, size, recovered) in [
        ("size-at-bound", 8 * 1024 * 1024, true),
        ("size-over-bound", 8 * 1024 * 1024 + 1, false),
    ] {
        let root = mod_root(name);
        let mut manifest = rehome(V2, &root);
        // Trailing whitespace is valid, so only the size can be wrong.
        manifest.resize(size, b'\n');
        write_controls(&root, &manifest);
        let child = staging(&root).join(V2_CHILD);
        std::fs::create_dir(&child).unwrap();
        std::fs::write(child.join(V2_ENTRY), "owned").unwrap();

        let (_registry, failure) = recover(&root);

        if recovered {
            assert!(failure.is_none(), "{failure:?}");
            assert!(!child.exists());
        } else {
            assert_unverified(failure, &staging(&root).join("ownership.manifest"));
            assert!(child.join(V2_ENTRY).exists());
        }
    }
}

/// Spec (#492): 100,000 registrations are read, and 100,001 are not trusted.
#[test]
fn the_registration_bound_is_100000() {
    for (name, count, recovered) in [
        ("count-at-bound", 100_000, true),
        ("count-over-bound", 100_001, false),
    ] {
        let root = mod_root(name);
        let child = "run-r-0123456789abcdef0123456789abcdef";
        let mut text = format!("CAO-STAGING 3\n\"x\"\n\"r\" \"{child}\"\n{count}\nD \"{child}\"\n");
        for index in 1..count {
            text.push_str(&format!("F \"{child}/{index}\"\n"));
        }
        let manifest = rehome(text.as_bytes(), &root);
        assert!(manifest.len() < 8 * 1024 * 1024);
        write_controls(&root, &manifest);
        std::fs::create_dir(staging(&root).join(child)).unwrap();
        std::fs::write(staging(&root).join(child).join("1"), "owned").unwrap();

        let (_registry, failure) = recover(&root);

        if recovered {
            assert!(failure.is_none(), "{failure:?}");
            assert!(!staging(&root).join(child).exists());
        } else {
            let failure = assert_unverified(failure, &staging(&root).join("ownership.manifest"));
            assert!(
                failure.detail.contains("artifact count"),
                "{}",
                failure.detail
            );
            assert!(staging(&root).join(child).join("1").exists());
        }
    }
}

/// Spec (#492): staging recovery cannot prove it owns fails closed with
/// `StagingOwnershipUnverified`. The failure names the offending path and
/// says what to do, and recovery deletes nothing at all.
#[test]
fn unverifiable_ownership_names_the_path_and_deletes_nothing() {
    type Damage = fn(&Path) -> PathBuf;
    let cases: [(&str, Damage); 9] = [
        ("unregistered-entry", |root| {
            let path = staging(root).join(V2_CHILD).join("stray.txt");
            std::fs::write(&path, "not CAO's").unwrap();
            path
        }),
        ("conflict-control", |root| {
            let path = staging(root).join("ownership.conflict");
            std::fs::write(&path, "A staged name collided before creation.\n").unwrap();
            path
        }),
        ("type-mismatch", |root| {
            let path = staging(root).join(V2_CHILD).join(V2_ENTRY);
            std::fs::remove_file(&path).unwrap();
            std::fs::create_dir(&path).unwrap();
            path
        }),
        ("hard-link", |root| {
            let path = staging(root).join(V2_CHILD).join(V2_ENTRY);
            std::fs::hard_link(&path, root.join("evidence.bin")).unwrap();
            path
        }),
        ("junction", |root| {
            let path = staging(root).join(V2_CHILD).join("extracted");
            std::fs::remove_dir_all(&path).unwrap();
            let outside = root.with_file_name(format!(
                "{}-outside",
                root.file_name().unwrap().to_string_lossy()
            ));
            std::fs::create_dir_all(&outside).unwrap();
            common::junction(&path, &outside);
            path
        }),
        ("other-root", |root| {
            std::fs::write(staging(root).join("ownership.manifest"), V2).unwrap();
            staging(root).join("ownership.manifest")
        }),
        ("corrupt-manifest", |root| {
            std::fs::write(staging(root).join("ownership.manifest"), "corrupt").unwrap();
            staging(root).join("ownership.manifest")
        }),
        ("missing-manifest", |root| {
            std::fs::remove_file(staging(root).join("ownership.manifest")).unwrap();
            staging(root).join("ownership.manifest")
        }),
        ("missing-lock", |root| {
            std::fs::remove_file(staging(root).join("owner.lock")).unwrap();
            staging(root)
        }),
    ];
    for (name, damage) in cases {
        let root = v2_leftover(&format!("unverified-{name}"));
        let path = damage(&root);
        let before = snapshot_tree(&root);

        let (registry, failure) = recover(&root);

        assert_unverified(failure, &path);
        drop(registry);
        assert_eq!(snapshot_tree(&root), before, "{name}");
        assert!(staging(&root).join(V2_CHILD).exists(), "{name}");
        if name == "junction" {
            std::fs::remove_dir(&path).unwrap();
        }
    }
}

/// An owned entry that recovery cannot open, here because another process
/// holds it without sharing, is unverified ownership of that entry, as in
/// C++: the failure names the entry to inspect, and nothing is deleted.
#[test]
fn an_entry_recovery_cannot_open_is_named() {
    let root = v2_leftover("held-entry");
    let held = staging(&root).join(V2_CHILD).join(V2_ENTRY);
    let before = snapshot_tree(&root);
    let handle = cao_winfs::Open::new(cao_winfs::Access::READ, cao_winfs::Share::NONE)
        .open(&held)
        .unwrap();

    let (registry, failure) = recover(&root);

    let failure = assert_unverified(failure, &held);
    assert!(
        failure.detail.contains("could not be opened"),
        "{}",
        failure.detail
    );
    drop((registry, handle));
    assert_eq!(snapshot_tree(&root), before);
}

/// Spec (#492): a removal that fails after recovery began deleting is
/// `StagingRecoveryFailed`, naming the entry it could not remove. The
/// entries removed before it stay removed, the rest stay owned, and a later
/// run finishes the recovery.
#[test]
fn a_failed_removal_is_staging_recovery_failed() {
    let root = v2_leftover("recovery-failed");
    let child = staging(&root).join(V2_CHILD);
    let stuck = child.join(V2_ENTRY);
    let mut permissions = std::fs::metadata(&stuck).unwrap().permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(&stuck, permissions.clone()).unwrap();

    let (registry, failure) = recover(&root);

    let failure = failure.expect("the read-only entry cannot be removed");
    assert_eq!(
        failure.code,
        RunFailureCode::StagingRecoveryFailed,
        "{failure:?}"
    );
    assert_eq!(failure.path, stuck, "{}", failure.detail);
    assert!(
        failure.detail.ends_with(
            "Leave remaining staging in place; check permissions and ownership before \
             retrying recovery."
        ),
        "{}",
        failure.detail
    );
    // Reverse registration order: the nested entry and its directory went first.
    assert!(!child.join("extracted").exists());
    assert!(stuck.exists());
    drop(registry);

    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    std::fs::set_permissions(&stuck, permissions).unwrap();
    let (_registry, failure) = recover(&root);
    assert!(failure.is_none(), "{failure:?}");
    assert!(!child.exists());
}

/// The quoting grammar is C++ `std::quoted` over whitespace-separated fields:
/// any whitespace separates them, and a backslash escapes the next character.
#[test]
fn quoted_fields_and_any_whitespace_are_accepted() {
    let root = mod_root("grammar");
    let generic = root.to_str().unwrap().replace('\\', "/");
    let text = format!(
        "CAO-STAGING\t2\r\n\"{generic}\"\x0B\"4f1c\\2a9e\"\x0C\"run-4f1c2a9e-\\0123456789abcdef0123456789abcdef\"\r\n \
         2\r\nD\"{V2_CHILD}\"   F \"{V2_CHILD}/{V2_ENTRY}\"\r\n\r\n"
    );
    write_controls(&root, text.as_bytes());
    std::fs::create_dir(staging(&root).join(V2_CHILD)).unwrap();
    std::fs::write(staging(&root).join(V2_CHILD).join(V2_ENTRY), "owned").unwrap();

    let (_registry, failure) = recover(&root);

    assert!(failure.is_none(), "{failure:?}");
    assert!(!staging(&root).join(V2_CHILD).exists());
}

/// Every unquoted string, unsafe name, broken containment or stray byte makes
/// the whole manifest untrusted, so nothing it names is deleted.
#[test]
fn a_malformed_manifest_is_never_trusted() {
    let child = V2_CHILD;
    let records = |records: &str| format!("\"4f1c2a9e\" \"{child}\"\n{records}");
    let cases = [
        (
            "bare-field",
            format!("4f1c2a9e \"{child}\"\n1\nD \"{child}\"\n"),
        ),
        ("unterminated", records("1\nD \"run-4f1c2a9e")),
        ("truncated-escape", records("1\nD \"x\\")),
        ("too-few-records", records(&format!("2\nD \"{child}\"\n"))),
        (
            "trailing-data",
            records(&format!("1\nD \"{child}\"\nF \"{child}/x\"\n")),
        ),
        ("zero-count", records("0\n")),
        ("first-not-child", records(&format!("1\nD \"{child}/x\"\n"))),
        ("first-is-file", records(&format!("1\nF \"{child}\"\n"))),
        (
            "unknown-kind",
            records(&format!("2\nD \"{child}\"\nX \"{child}/x\"\n")),
        ),
        (
            "backslash",
            records(&format!("2\nD \"{child}\"\nF \"{child}\\\\x\"\n")),
        ),
        (
            "traversal",
            records(&format!("2\nD \"{child}\"\nF \"{child}/../x\"\n")),
        ),
        (
            "stream",
            records(&format!("2\nD \"{child}\"\nF \"{child}/x:s\"\n")),
        ),
        (
            "trailing-dot",
            records(&format!("2\nD \"{child}\"\nF \"{child}/x.\"\n")),
        ),
        (
            "outside-child",
            records(&format!("2\nD \"{child}\"\nF \"run-other/x\"\n")),
        ),
        (
            "parent-unregistered",
            records(&format!("2\nD \"{child}\"\nF \"{child}/a/x\"\n")),
        ),
        (
            "parent-is-file",
            records(&format!(
                "3\nD \"{child}\"\nF \"{child}/a\"\nF \"{child}/a/x\"\n"
            )),
        ),
        (
            "duplicate",
            records(&format!(
                "3\nD \"{child}\"\nF \"{child}/x\"\nF \"{child}/x\"\n"
            )),
        ),
        ("uppercase-nonce", {
            let child = "run-4f1c2a9e-0123456789ABCDEF0123456789abcdef";
            format!("\"4f1c2a9e\" \"{child}\"\n1\nD \"{child}\"\n")
        }),
        ("run-id-too-long", {
            let run_id = "a".repeat(129);
            let child = format!("run-{run_id}-0123456789abcdef0123456789abcdef");
            format!("\"{run_id}\" \"{child}\"\n1\nD \"{child}\"\n")
        }),
    ];
    for (name, tail) in cases {
        let root = mod_root(&format!("malformed-{name}"));
        let generic = root.to_str().unwrap().replace('\\', "/");
        let manifest = staging(&root).join("ownership.manifest");
        write_controls(
            &root,
            format!("CAO-STAGING 2\n\"{generic}\"\n{tail}").as_bytes(),
        );
        std::fs::create_dir(staging(&root).join(child)).unwrap();
        std::fs::write(staging(&root).join(child).join("x"), "kept").unwrap();

        let (_registry, failure) = recover(&root);

        let failure = failure.unwrap_or_else(|| panic!("{name} was trusted"));
        assert_eq!(
            failure.code,
            RunFailureCode::StagingOwnershipUnverified,
            "{name}: {failure:?}"
        );
        assert_eq!(failure.path, manifest, "{name}: {}", failure.detail);
        assert!(
            staging(&root).join(child).join("x").exists(),
            "{name} deleted owned bytes"
        );
    }
    // Only versions 1 to 3 exist.
    let root = mod_root("malformed-version");
    let text = String::from_utf8(rehome(V2, &root)).unwrap();
    write_controls(
        &root,
        text.replacen("CAO-STAGING 2", "CAO-STAGING 4", 1)
            .as_bytes(),
    );
    let (_registry, failure) = recover(&root);
    assert_unverified(failure, &staging(&root).join("ownership.manifest"));
}
