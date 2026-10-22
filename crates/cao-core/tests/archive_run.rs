//! The test-support harness that drives Archive work through a whole
//! Optimization Run (#596).
//!
//! [`ArchiveRunRequest`] builds an Apply request over one Mod Root, carrying
//! its Archive choices, and the configuration that supplies its profile, so a
//! scenario reaches Archive Finalization as production does: the Archive
//! creation request and the Packing Exclusions come from the profile, and
//! Preparing makes the run's preparation. Each scenario asserts on the run's
//! Run Evidence and the filesystem, never on the Archive work directly.

mod common;

use cao_core::run::{
    ArchiveFinalizationChoices, CancellationToken, RunOutcome, RunPhase, RunPhaseStatus,
};
use cao_core::test_support::{ArchiveRunRequest, ScriptedToolset};
use common::{canonical, scratch_dir, write_tree};

/// An Apply run built by the request builder reaches Archive Finalization
/// with Archive creation requested, packs the Loose Asset its Packing
/// Exclusion does not match, and packs it with the Archive Finalization
/// choices its Run Request carries.
#[test]
fn an_archive_run_packs_loose_assets_with_its_profile_exclusions_and_settings() {
    let root = canonical(&scratch_dir("archive-run-request")).join("mod");
    write_tree(&root, &["meshes/packed.nif", "meshes/keep/kept.nif"]);
    let archive_run = ArchiveRunRequest::new(&root)
        .with_packing_exclusions(["MESHES/KEEP/"])
        .with_archive_finalization(ArchiveFinalizationChoices {
            compress: false,
            create_dummy_plugins: false,
            ..ArchiveFinalizationChoices::default()
        });
    let configuration = archive_run.configuration(ScriptedToolset::default());

    // `execute` also asserts the run made its one Safety Cleanup pass.
    let result = archive_run.execute(&configuration, None, &CancellationToken::new());

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
    assert!(attempt.succeeded(), "{:?}", attempt.outcome);
    assert_eq!(attempt.archive_path, root.join("mod.bsa"));
    // Only the Loose Asset the exclusion does not match is packed, and the
    // chosen settings turned compression off.
    let writes = configuration.toolset.packer.writes();
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0].names, ["meshes/packed.nif"]);
    assert!(!writes[0].compress);
    assert!(!root.join("meshes/packed.nif").exists());
    assert!(root.join("meshes/keep/kept.nif").exists());
    // The chosen settings create no Dummy Plugin for the new Archive.
    assert!(!root.join("mod.esp").exists());
}
