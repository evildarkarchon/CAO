//! Admission Rules: which of the user's choices a run's Run Request may
//! carry, given its execution mode, its Mod Selection and, optionally, its
//! Profile Capabilities.

use cao_core::admission::Admission;
use cao_core::routing::{ExecutionMode, ProfileCapabilities, ProfileCapability, RequestedWork};
use cao_core::run::{
    ArchiveFinalizationChoices, MeshChoices, MeshLevel, SourceCleanup, TextureChoices,
    TextureResizeChoice, WorkChoices,
};
use cao_core::test_support::sse_capabilities;

/// Every piece of requested work, in enumeration order.
const ALL_WORK: [RequestedWork; 6] = [
    RequestedWork::NativeTextureOptimization,
    RequestedWork::StandardMeshOptimization,
    RequestedWork::TerrainMeshOptimization,
    RequestedWork::AnimationOptimization,
    RequestedWork::ArchiveExtraction,
    RequestedWork::ArchiveCreation,
];

/// The work `admission` holds back, in enumeration order.
fn held_back(admission: &Admission) -> Vec<RequestedWork> {
    ALL_WORK
        .into_iter()
        .filter(|work| !admission.admits(*work))
        .collect()
}

#[test]
fn a_dry_run_holds_back_archive_extraction_and_creation_and_nothing_else() {
    let admission = Admission::new(ExecutionMode::DryRun, false, None);

    assert_eq!(
        held_back(&admission),
        [
            RequestedWork::ArchiveExtraction,
            RequestedWork::ArchiveCreation
        ]
    );
}

#[test]
fn an_apply_run_admits_every_piece_of_work() {
    let admission = Admission::new(ExecutionMode::Apply, false, None);

    assert_eq!(held_back(&admission), []);
}

#[test]
fn several_mods_admits_mesh_levels_up_to_necessary() {
    for mode in [ExecutionMode::Apply, ExecutionMode::DryRun] {
        let admission = Admission::new(mode, true, None);

        assert_eq!(admission.highest_mesh_level().get(), 1, "{mode:?}");
    }
}

#[test]
fn one_mod_admits_every_mesh_level() {
    let capabilities = partial_capabilities();
    for capabilities in [None, Some(&capabilities)] {
        let admission = Admission::new(ExecutionMode::Apply, false, capabilities);

        assert_eq!(admission.highest_mesh_level().get(), 3);
    }
}

/// Work Choices away from every default, at Mesh level `level`.
fn choices_at(level: i32) -> WorkChoices {
    WorkChoices {
        textures: TextureChoices {
            necessary: false,
            compress: true,
            mipmaps: true,
            resize: TextureResizeChoice::None,
        },
        meshes: MeshChoices {
            level: MeshLevel::new(level).unwrap(),
            headparts: false,
            resave: true,
        },
        archive_extraction: SourceCleanup::Remove,
        archive_finalization: ArchiveFinalizationChoices {
            compress: false,
            delete_sources: false,
            create_dummy_plugins: false,
            merge_incompressible: false,
            merge_textures: true,
        },
    }
}

#[test]
fn several_mods_caps_the_chosen_mesh_level_at_necessary_and_keeps_the_rest() {
    let admission = Admission::new(ExecutionMode::Apply, true, None);

    for (chosen, admitted) in [(0, 0), (1, 1), (2, 1), (3, 1)] {
        let mut expected = choices_at(chosen);
        expected.meshes.level = MeshLevel::new(admitted).unwrap();

        assert_eq!(
            admission.admitted_work_choices(choices_at(chosen)),
            expected,
            "level {chosen}"
        );
    }
}

#[test]
fn one_mod_keeps_the_chosen_mesh_level() {
    let admission = Admission::new(ExecutionMode::Apply, false, None);

    assert_eq!(
        admission.admitted_work_choices(choices_at(3)),
        choices_at(3)
    );
}

#[test]
fn a_dry_run_keeps_the_archive_choices_as_chosen() {
    // Choices for work that is not requested have no effect, so Delete
    // backups and the Archive Finalization choices are not held back.
    let admission = Admission::new(ExecutionMode::DryRun, false, None);

    assert_eq!(
        admission.admitted_work_choices(choices_at(3)),
        choices_at(3)
    );
}

#[test]
fn the_admitted_work_is_the_chosen_work_less_what_is_held_back() {
    let capabilities = partial_capabilities();
    let admission = Admission::new(ExecutionMode::DryRun, false, Some(&capabilities));

    assert_eq!(
        admission.admitted_work(&ALL_WORK),
        [
            RequestedWork::NativeTextureOptimization,
            RequestedWork::TerrainMeshOptimization,
        ]
    );
    assert_eq!(
        admission.admitted_work(&[RequestedWork::AnimationOptimization]),
        []
    );
}

/// A profile supporting native Textures, terrain Meshes and Archive creation
/// only. Conversion and Mesh Reference Maintenance are never requested work,
/// so supporting them admits nothing more.
fn partial_capabilities() -> ProfileCapabilities {
    ProfileCapabilities::define(
        ".bsa",
        &[
            ProfileCapability::NativeTextureOptimization,
            ProfileCapability::ConvertibleTextureConversion,
            ProfileCapability::TerrainMeshOptimization,
            ProfileCapability::MeshReferenceMaintenance,
            ProfileCapability::ArchiveCreation,
        ],
    )
}

#[test]
fn profile_capabilities_hold_back_exactly_the_work_they_do_not_support() {
    let capabilities = partial_capabilities();

    let admission = Admission::new(ExecutionMode::Apply, false, Some(&capabilities));

    assert_eq!(
        held_back(&admission),
        [
            RequestedWork::StandardMeshOptimization,
            RequestedWork::AnimationOptimization,
            RequestedWork::ArchiveExtraction,
        ]
    );
}

#[test]
fn a_profile_supporting_every_capability_holds_back_nothing() {
    let capabilities = sse_capabilities(".bsa");

    let admission = Admission::new(ExecutionMode::Apply, true, Some(&capabilities));

    assert_eq!(held_back(&admission), []);
}

#[test]
fn a_dry_run_and_the_profile_capabilities_each_hold_back_their_own_work() {
    let capabilities = partial_capabilities();

    let admission = Admission::new(ExecutionMode::DryRun, false, Some(&capabilities));

    assert_eq!(
        held_back(&admission),
        [
            RequestedWork::StandardMeshOptimization,
            RequestedWork::AnimationOptimization,
            RequestedWork::ArchiveExtraction,
            RequestedWork::ArchiveCreation,
        ]
    );
}
