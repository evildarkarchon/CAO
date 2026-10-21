//! Whole Optimization Runs over the production Archive adapters (#596).
//!
//! The run's Toolset is built from a seams builder over [`ArchiveFileReader`],
//! [`GameArchivePacker`] and [`VolumeProbes`], so core's real Archive
//! extraction and Archive Finalization run over each game's real rules
//! through the only interface production uses. Each scenario asserts on the
//! run's Run Evidence and on the Archives it really wrote.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cao_archive::{Game, ReadArchive, Settings};
use cao_core::run::{ArchiveSeams, CancellationToken, RunOutcome, RunPhase, RunPhaseStatus};
use cao_core::test_support::{ArchiveRunRequest, CountingCleanup, execute_run};
use cao_optimizers::archives::{ArchiveFileReader, GameArchivePacker, VolumeProbes};
use common::{scratch_dir, write};

/// The canonical form of a directory, as Preparing resolves it.
fn canonical(path: &Path) -> PathBuf {
    cao_winfs::msvc_canonical(path).unwrap()
}

/// The lowercase, backslash-separated names stored in the real Archive at
/// `archive`, which must open as a known format.
fn archived_names(archive: &Path) -> Vec<String> {
    ReadArchive::open(archive)
        .unwrap()
        .expect("a known Archive format")
        .archived_assets()
        .unwrap()
        .into_iter()
        .map(|asset| asset.name.to_lowercase().replace('/', "\\"))
        .collect()
}

/// An Apply run whose Toolset is built over the production reader, the SSE
/// packer and the real volume probes packs a Loose Asset into an SSE Archive
/// that loads, and publishes its Dummy Plugin.
#[test]
fn a_run_over_the_production_adapters_packs_a_loose_asset_into_a_loadable_archive() {
    let root = canonical(&scratch_dir("archive-run-seams")).join("mod");
    write(&root, "meshes/asset.nif", b"mesh bytes");
    let archive_run = ArchiveRunRequest::new(&root);
    let configuration = archive_run.seams_configuration(|| ArchiveSeams {
        reader: Arc::new(ArchiveFileReader::default()),
        volume_probe: Arc::new(VolumeProbes),
        packer: Arc::new(GameArchivePacker::new(Settings::get(Game::Sse))),
    });
    let mut cleanup = CountingCleanup::default();

    let result = execute_run(
        &archive_run.request(),
        &mut cleanup,
        None,
        Some(&configuration),
        &CancellationToken::new(),
    );

    assert_eq!(
        result.outcome(),
        RunOutcome::Succeeded,
        "{:?}",
        result.failures()
    );
    assert_eq!(
        result
            .phase(RunPhase::ArchiveFinalization)
            .map(|record| record.status()),
        Some(RunPhaseStatus::Executed)
    );
    let finalization = result.archive_finalization().unwrap();
    assert_eq!(finalization.attempts.len(), 1, "{}", finalization.detail);
    let attempt = &finalization.attempts[0];
    assert!(attempt.succeeded(), "{}", attempt.detail);
    assert_eq!(attempt.archive_path, root.join("mod.bsa"));
    assert_eq!(archived_names(&attempt.archive_path), [r"meshes\asset.nif"]);
    assert!(!root.join("meshes/asset.nif").exists());
    assert_eq!(
        std::fs::read(root.join("mod.esp")).unwrap(),
        Settings::get(Game::Sse).dummy_plugin
    );
    assert_eq!(cleanup.passes, 1, "Safety Cleanup always runs once");
}
